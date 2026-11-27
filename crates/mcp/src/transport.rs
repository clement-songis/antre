//! Stdio transport: spawn a child process and communicate via JSON-RPC over stdin/stdout.

use std::{
    collections::HashMap,
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

/// Process isolation options for a stdio MCP server launch.
#[derive(Clone, Debug)]
pub struct StdioLaunchOptions {
    pub current_dir: Option<PathBuf>,
    pub inherit_parent_env: bool,
}

impl Default for StdioLaunchOptions {
    fn default() -> Self {
        Self {
            current_dir: None,
            inherit_parent_env: true,
        }
    }
}

use {
    futures::StreamExt,
    secrecy::{ExposeSecret, Secret},
    tokio::{
        io::{AsyncRead, AsyncWrite, AsyncWriteExt},
        process::Command,
        sync::{Mutex, oneshot},
    },
    tokio_util::codec::{FramedRead, LinesCodec},
    tracing::{debug, info, trace, warn},
};

use {
    crate::{
        error::{Context, Error, Result},
        incoming::{IncomingDispatcher, IncomingMessage, IncomingRouter},
        traits::McpTransport,
        types::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse},
    },
    moltis_common::process_tree::OwnedProcessTree,
};

const MAX_MCP_STDOUT_LINE_BYTES: usize = 4 * 1024 * 1024;
const MAX_MCP_STDERR_LINE_BYTES: usize = 64 * 1024;

type SharedWriter = Arc<Mutex<Box<dyn AsyncWrite + Send + Unpin>>>;
type PendingMap = Arc<Mutex<HashMap<String, oneshot::Sender<JsonRpcResponse>>>>;

/// Stdio-based transport for an MCP server process.
pub struct StdioTransport {
    /// `None` when the transport runs over in-memory streams (tests).
    child: Mutex<Option<OwnedProcessTree>>,
    stdin: SharedWriter,
    pending: PendingMap,
    next_id: AtomicU64,
    request_timeout: Duration,
    reader_closed: Arc<AtomicBool>,
    incoming: Arc<IncomingRouter>,
    /// Handle to the reader task so we can abort on drop.
    reader_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

struct PendingRequestGuard {
    id_key: Option<String>,
    request_id: serde_json::Value,
    pending: PendingMap,
    stdin: SharedWriter,
}

impl PendingRequestGuard {
    fn disarm(&mut self) {
        self.id_key = None;
    }

    async fn cancel(&mut self) {
        let Some(id_key) = self.id_key.as_ref() else {
            return;
        };
        self.pending.lock().await.remove(id_key);
        self.id_key = None;
        tokio::spawn(send_cancellation_notification(
            self.request_id.clone(),
            Arc::clone(&self.stdin),
        ));
    }
}

impl Drop for PendingRequestGuard {
    fn drop(&mut self) {
        let Some(id_key) = self.id_key.take() else {
            return;
        };
        let request_id = self.request_id.clone();
        let pending = Arc::clone(&self.pending);
        let stdin = Arc::clone(&self.stdin);
        tokio::spawn(async move {
            pending.lock().await.remove(&id_key);
            send_cancellation_notification(request_id, stdin).await;
        });
    }
}

async fn write_line(writer: &SharedWriter, message: &impl serde::Serialize) -> std::io::Result<()> {
    let mut payload = serde_json::to_string(message).map_err(std::io::Error::other)?;
    payload.push('\n');
    let mut writer = writer.lock().await;
    writer.write_all(payload.as_bytes()).await?;
    writer.flush().await
}

async fn send_cancellation_notification(request_id: serde_json::Value, stdin: SharedWriter) {
    let notification = JsonRpcNotification {
        jsonrpc: "2.0".into(),
        method: "notifications/cancelled".into(),
        params: Some(serde_json::json!({
            "requestId": request_id,
            "reason": "request cancelled by client",
        })),
    };
    let _ = write_line(&stdin, &notification).await;
}

impl StdioTransport {
    /// Spawn the server process and start the reader loop.
    pub async fn spawn(
        command: &str,
        args: &[String],
        env: &HashMap<String, Secret<String>>,
    ) -> Result<Arc<Self>> {
        Self::spawn_with_timeout(command, args, env, Duration::from_secs(30)).await
    }

    /// Spawn the server process with a custom request timeout and start the reader loop.
    pub async fn spawn_with_timeout(
        command: &str,
        args: &[String],
        env: &HashMap<String, Secret<String>>,
        request_timeout: Duration,
    ) -> Result<Arc<Self>> {
        Self::spawn_with_options(
            command,
            args,
            env,
            request_timeout,
            &StdioLaunchOptions::default(),
        )
        .await
    }

    /// Spawn with explicit process isolation and working-directory behavior.
    pub async fn spawn_with_options(
        command: &str,
        args: &[String],
        env: &HashMap<String, Secret<String>>,
        request_timeout: Duration,
        options: &StdioLaunchOptions,
    ) -> Result<Arc<Self>> {
        info!(command = %command, arg_count = args.len(), "spawning MCP server process");

        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if !options.inherit_parent_env {
            cmd.env_clear();
        }
        if let Some(current_dir) = options.current_dir.as_ref() {
            cmd.current_dir(current_dir);
        }
        for (name, value) in env {
            cmd.env(name, value.expose_secret());
        }

        let mut child = OwnedProcessTree::spawn(cmd)
            .with_context(|| format!("failed to spawn MCP server: {command}"))?;

        let stdin = child.take_stdin().context("failed to capture stdin")?;
        let stdout = child.take_stdout().context("failed to capture stdout")?;
        let stderr = child.take_stderr();

        // Start stderr reader task (log server errors).
        if let Some(stderr) = stderr {
            tokio::spawn(async move {
                let mut lines = FramedRead::new(
                    stderr,
                    LinesCodec::new_with_max_length(MAX_MCP_STDERR_LINE_BYTES),
                );
                while let Some(line) = lines.next().await {
                    match line {
                        Ok(line) => {
                            let trimmed = line.trim();
                            if !trimmed.is_empty() {
                                debug!(bytes = trimmed.len(), "MCP server wrote to stderr");
                            }
                        },
                        Err(error) => {
                            warn!(%error, "MCP server stderr line exceeded transport limit");
                        },
                    }
                }
            });
        }

        Ok(Self::start(Some(child), Box::new(stdin), stdout, request_timeout).await)
    }

    /// Run the transport over arbitrary streams: `reader` carries server
    /// output, `writer` carries client input. Used with in-process servers.
    pub async fn from_streams<R, W>(reader: R, writer: W, request_timeout: Duration) -> Arc<Self>
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        Self::start(None, Box::new(writer), reader, request_timeout).await
    }

    async fn start<R>(
        child: Option<OwnedProcessTree>,
        writer: Box<dyn AsyncWrite + Send + Unpin>,
        reader: R,
        request_timeout: Duration,
    ) -> Arc<Self>
    where
        R: AsyncRead + Send + Unpin + 'static,
    {
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let reader_closed = Arc::new(AtomicBool::new(false));
        let stdin: SharedWriter = Arc::new(Mutex::new(writer));
        let incoming = Arc::new(IncomingRouter::default());

        let transport = Arc::new(Self {
            child: Mutex::new(child),
            stdin: Arc::clone(&stdin),
            pending: Arc::clone(&pending),
            next_id: AtomicU64::new(1),
            request_timeout,
            reader_closed: Arc::clone(&reader_closed),
            incoming: Arc::clone(&incoming),
            reader_handle: Mutex::new(None),
        });

        let handle = tokio::spawn(async move {
            let mut lines = FramedRead::new(
                reader,
                LinesCodec::new_with_max_length(MAX_MCP_STDOUT_LINE_BYTES),
            );
            while let Some(line) = lines.next().await {
                let line = match line {
                    Ok(line) => line,
                    Err(error) => {
                        warn!(%error, "MCP server stdout line exceeded transport limit");
                        break;
                    },
                };
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match IncomingMessage::parse(trimmed) {
                    Ok(IncomingMessage::Response(resp)) => {
                        let key = resp.id.to_string();
                        let mut map = pending.lock().await;
                        if let Some(tx) = map.remove(&key) {
                            let _ = tx.send(resp);
                        } else {
                            warn!(id = %key, "received response for unknown request id");
                        }
                    },
                    Ok(IncomingMessage::Request(request)) => {
                        let writer = Arc::clone(&stdin);
                        incoming.route_request(request, move |response| async move {
                            if let Err(error) = write_line(&writer, &response).await {
                                warn!(%error, "failed to answer MCP server request");
                            }
                        });
                    },
                    Ok(IncomingMessage::Notification(notification)) => {
                        incoming.route_notification(notification);
                    },
                    Err(e) => {
                        debug!(error = %e, bytes = trimmed.len(), "MCP server sent non-JSON-RPC line");
                    },
                }
            }
            reader_closed.store(true, Ordering::Release);
            pending.lock().await.clear();
            debug!("MCP server stdout closed");
        });

        *transport.reader_handle.lock().await = Some(handle);
        transport
    }
}

#[async_trait::async_trait]
impl McpTransport for StdioTransport {
    async fn request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<JsonRpcResponse> {
        if self.reader_closed.load(Ordering::Acquire) {
            return Err(Error::message("MCP server stdout is closed"));
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let req = JsonRpcRequest::new(id, method, params);
        let id_key = req.id.to_string();

        let (tx, rx) = oneshot::channel();
        {
            let mut map = self.pending.lock().await;
            if self.reader_closed.load(Ordering::Acquire) {
                return Err(Error::message("MCP server stdout is closed"));
            }
            map.insert(id_key.clone(), tx);
        }
        let mut pending_guard = PendingRequestGuard {
            id_key: Some(id_key.clone()),
            request_id: req.id.clone(),
            pending: Arc::clone(&self.pending),
            stdin: Arc::clone(&self.stdin),
        };

        debug!(method = %method, id = %id, "client -> MCP server");

        if let Err(error) = write_line(&self.stdin, &req).await {
            pending_guard.cancel().await;
            return Err(error.into());
        }

        self.incoming.touch();
        let Some(received) = self.incoming.await_active(rx, self.request_timeout).await else {
            pending_guard.cancel().await;
            return Err(Error::message(format!(
                "MCP request '{method}' timed out after {}s (no response from server)",
                self.request_timeout.as_secs()
            )));
        };
        let resp = received.with_context(|| {
            format!("MCP reader task dropped while waiting for '{method}' response")
        })?;
        pending_guard.disarm();

        if let Some(ref err) = resp.error {
            return Err(Error::message(format!(
                "MCP error on '{method}': code={} message={}",
                err.code, err.message
            )));
        }

        Ok(resp)
    }

    async fn notify(&self, method: &str, params: Option<serde_json::Value>) -> Result<()> {
        let notif = JsonRpcNotification {
            jsonrpc: "2.0".into(),
            method: method.into(),
            params,
        };
        trace!(method = %method, "client -> MCP server (notification)");
        write_line(&self.stdin, &notif).await?;
        Ok(())
    }

    async fn is_alive(&self) -> bool {
        if self.reader_closed.load(Ordering::Acquire) {
            return false;
        }
        match self.child.lock().await.as_mut() {
            Some(child) => matches!(child.try_wait(), Ok(None)),
            None => true,
        }
    }

    async fn kill(&self) {
        self.reader_closed.store(true, Ordering::Release);
        self.pending.lock().await.clear();
        if let Some(handle) = self.reader_handle.lock().await.take() {
            handle.abort();
        }
        if let Some(child) = self.child.lock().await.as_mut() {
            let _ = child.kill().await;
        }
    }

    fn set_dispatcher(&self, dispatcher: Arc<dyn IncomingDispatcher>) {
        self.incoming.set_dispatcher(dispatcher);
    }

    fn accepts_server_requests(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_spawn_and_kill() {
        // Spawn a simple process that reads stdin (cat will echo back).
        let transport = StdioTransport::spawn("cat", &[], &HashMap::new())
            .await
            .unwrap();
        assert!(transport.is_alive().await);
        transport.kill().await;
        // After kill, process should be dead.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!transport.is_alive().await);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_kill_terminates_descendants() {
        let temp_dir = tempfile::tempdir().unwrap();
        let started = temp_dir.path().join("started");
        let marker = temp_dir.path().join("should-not-exist");
        // Touch `started` only after the background job is forked, so the
        // descendant exists by the time the test kills the tree.
        let script = format!(
            "(sleep 1; touch '{}') & touch '{}'; cat",
            marker.display(),
            started.display()
        );
        let args = vec!["-c".to_string(), script];
        let transport = StdioTransport::spawn("sh", &args, &HashMap::new())
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while !started.exists() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        transport.kill().await;
        tokio::time::sleep(Duration::from_millis(1100)).await;
        assert!(!marker.exists(), "MCP descendant survived transport kill");
    }

    #[tokio::test]
    async fn test_spawn_nonexistent_command() {
        let result =
            StdioTransport::spawn("nonexistent_command_xyz_42", &[], &HashMap::new()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_request_uses_configured_timeout() {
        let args = vec!["-c".to_string(), "while read line; do :; done".to_string()];
        let transport = StdioTransport::spawn_with_timeout(
            "sh",
            &args,
            &HashMap::new(),
            Duration::from_secs(1),
        )
        .await
        .unwrap();

        let err = transport.request("tools/list", None).await.unwrap_err();
        assert!(err.to_string().contains("timed out after 1s"));
        assert!(transport.pending.lock().await.is_empty());

        transport.kill().await;
    }

    #[tokio::test]
    async fn cancelled_request_removes_pending_entry() {
        let temp_dir = tempfile::tempdir().unwrap();
        let messages = temp_dir.path().join("messages.jsonl");
        let script = format!(
            "while IFS= read -r line; do printf '%s\\n' \"$line\" >> '{}'; done",
            messages.display()
        );
        let args = vec!["-c".to_string(), script];
        let transport = StdioTransport::spawn_with_timeout(
            "sh",
            &args,
            &HashMap::new(),
            Duration::from_secs(30),
        )
        .await
        .unwrap();
        let request_transport = Arc::clone(&transport);
        let request =
            tokio::spawn(async move { request_transport.request("tools/call", None).await });

        tokio::time::timeout(Duration::from_secs(1), async {
            while transport.pending.lock().await.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        request.abort();
        let _ = request.await;
        tokio::time::timeout(Duration::from_secs(1), async {
            while !transport.pending.lock().await.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let content = tokio::fs::read_to_string(&messages)
                    .await
                    .unwrap_or_default();
                if content.contains("notifications/cancelled") {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        transport.kill().await;
    }

    #[tokio::test]
    async fn cancellation_during_timeout_cleanup_keeps_guard_armed() {
        let args = vec!["-c".to_string(), "while read line; do :; done".to_string()];
        let transport = StdioTransport::spawn_with_timeout(
            "sh",
            &args,
            &HashMap::new(),
            Duration::from_millis(50),
        )
        .await
        .unwrap();
        let request_transport = Arc::clone(&transport);
        let request =
            tokio::spawn(async move { request_transport.request("tools/call", None).await });

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if !transport.pending.lock().await.is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let pending_guard = transport.pending.lock().await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        request.abort();
        let _ = request.await;
        drop(pending_guard);
        tokio::time::timeout(Duration::from_secs(1), async {
            while !transport.pending.lock().await.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        transport.kill().await;
    }

    #[tokio::test]
    async fn test_stdout_closure_fails_pending_request_without_timeout() {
        let args = vec!["-c".to_string(), "read line; exit 0".to_string()];
        let transport = StdioTransport::spawn_with_timeout(
            "sh",
            &args,
            &HashMap::new(),
            Duration::from_secs(30),
        )
        .await
        .unwrap();

        let result = tokio::time::timeout(
            Duration::from_secs(1),
            transport.request("tools/list", None),
        )
        .await
        .expect("stdout closure should wake the request");
        assert!(result.is_err());
        assert!(transport.pending.lock().await.is_empty());
        assert!(!transport.is_alive().await);
    }
}
