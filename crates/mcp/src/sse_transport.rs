//! HTTP/SSE transport for remote MCP servers (Streamable HTTP transport).
//!
//! Uses HTTP POST for JSON-RPC requests and GET for server-initiated SSE events.
//! Supports optional OAuth Bearer token injection and automatic 401 retry.

use std::{
    sync::{
        Arc, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use {
    futures::StreamExt,
    reqwest::{Client, header::HeaderMap},
    secrecy::{ExposeSecret, Secret},
    tokio::sync::{RwLock, mpsc},
    tracing::{debug, info, warn},
};

use crate::{
    auth::SharedAuthProvider,
    error::{Context, Error, Result},
    incoming::{IncomingDispatcher, IncomingMessage, IncomingRouter},
    remote::{ResolvedRemoteConfig, sanitize_reqwest_error},
    sse_stream::SseDecoder,
    traits::McpTransport,
    types::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, McpTransportError},
};

/// Reconnect attempts for the server-to-client GET stream before giving up.
const MAX_LISTENER_RECONNECTS: u32 = 5;

const MCP_PROTOCOL_VERSION_HEADER: &str = "MCP-Protocol-Version";
const MCP_SESSION_ID_HEADER: &str = "Mcp-Session-Id";
const STREAMABLE_ACCEPT_HEADER: &str = "application/json, text/event-stream";

/// HTTP/SSE-based transport for a remote MCP server.
pub struct SseTransport {
    client: Client,
    request_url: Secret<String>,
    display_url: String,
    default_headers: HeaderMap,
    next_id: AtomicU64,
    /// Optional auth provider for Bearer token injection.
    auth: Option<SharedAuthProvider>,
    /// Session identifier used by streamable HTTP servers.
    session_id: RwLock<Option<String>>,
    request_timeout: Duration,
    /// Negotiated protocol version; the header is omitted until known.
    protocol_version: std::sync::RwLock<Option<String>>,
    incoming: Arc<IncomingRouter>,
    listener: std::sync::Mutex<Option<tokio::task::AbortHandle>>,
    weak_self: Weak<Self>,
}

impl SseTransport {
    /// Create a new SSE transport pointing at the given MCP server URL.
    pub fn new(url: &str) -> Result<Arc<Self>> {
        Self::new_with_timeout(url, Duration::from_secs(60))
    }

    /// Create a new SSE transport with a custom request timeout.
    pub fn new_with_timeout(url: &str, request_timeout: Duration) -> Result<Arc<Self>> {
        let remote = ResolvedRemoteConfig::from_server_config(
            &crate::registry::McpServerConfig {
                transport: crate::registry::TransportType::Sse,
                url: Some(Secret::new(url.to_string())),
                ..Default::default()
            },
            &std::collections::HashMap::new(),
        )?;
        Self::new_with_remote(remote, request_timeout)
    }

    pub fn new_with_remote(
        remote: ResolvedRemoteConfig,
        request_timeout: Duration,
    ) -> Result<Arc<Self>> {
        Self::build(remote, None, request_timeout)
    }

    /// Create a new SSE transport with an OAuth auth provider.
    pub fn with_auth(url: &str, auth: SharedAuthProvider) -> Result<Arc<Self>> {
        Self::with_auth_and_timeout(url, auth, Duration::from_secs(60))
    }

    /// Create a new SSE transport with an OAuth auth provider and custom request timeout.
    pub fn with_auth_and_timeout(
        url: &str,
        auth: SharedAuthProvider,
        request_timeout: Duration,
    ) -> Result<Arc<Self>> {
        let remote = ResolvedRemoteConfig::from_server_config(
            &crate::registry::McpServerConfig {
                transport: crate::registry::TransportType::Sse,
                url: Some(Secret::new(url.to_string())),
                ..Default::default()
            },
            &std::collections::HashMap::new(),
        )?;
        Self::with_auth_remote(remote, auth, request_timeout)
    }

    pub fn with_auth_remote(
        remote: ResolvedRemoteConfig,
        auth: SharedAuthProvider,
        request_timeout: Duration,
    ) -> Result<Arc<Self>> {
        Self::build(remote, Some(auth), request_timeout)
    }

    fn build(
        remote: ResolvedRemoteConfig,
        auth: Option<SharedAuthProvider>,
        request_timeout: Duration,
    ) -> Result<Arc<Self>> {
        // No total timeout: response streams stay open while the client
        // answers server requests. `request_timeout` is enforced per step.
        let client = Client::builder()
            .connect_timeout(request_timeout)
            .build()
            .context("failed to build HTTP client for SSE transport")?;

        Ok(Arc::new_cyclic(|weak_self| Self {
            client,
            request_url: Secret::new(remote.request_url().to_string()),
            display_url: remote.display_url().to_string(),
            default_headers: remote.headers().clone(),
            next_id: AtomicU64::new(1),
            auth,
            session_id: RwLock::new(None),
            request_timeout,
            protocol_version: std::sync::RwLock::new(None),
            incoming: Arc::new(IncomingRouter::default()),
            listener: std::sync::Mutex::new(None),
            weak_self: weak_self.clone(),
        }))
    }

    fn apply_protocol_version(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let version = self
            .protocol_version
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        match version {
            Some(version) => req.header(MCP_PROTOCOL_VERSION_HEADER, version),
            None => req,
        }
    }

    async fn apply_session_and_auth(
        &self,
        mut req: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder> {
        if let Some(session_id) = self.session_id.read().await.clone() {
            req = req.header(MCP_SESSION_ID_HEADER, session_id);
        }
        if let Some(token) = match &self.auth {
            Some(auth) => auth.access_token().await?,
            None => None,
        } {
            req = req.header("Authorization", format!("Bearer {}", token.expose_secret()));
        }
        Ok(req)
    }

    async fn timed_send(&self, req: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        match tokio::time::timeout(self.request_timeout, req.send()).await {
            Ok(result) => Ok(result.map_err(sanitize_reqwest_error)?),
            Err(_) => Err(Error::message(format!(
                "no response within {}s",
                self.request_timeout.as_secs()
            ))),
        }
    }

    /// Build a request builder with optional Bearer token.
    async fn build_post(&self) -> Result<reqwest::RequestBuilder> {
        let mut req = self
            .client
            .post(self.request_url.expose_secret())
            .header("Content-Type", "application/json")
            .header("Accept", STREAMABLE_ACCEPT_HEADER);
        req = self.apply_protocol_version(req);
        req = self.apply_default_headers(req);
        self.apply_session_and_auth(req).await
    }

    async fn store_session_id_from_response(&self, response: &reqwest::Response) {
        let Some(raw) = response.headers().get(MCP_SESSION_ID_HEADER) else {
            return;
        };
        let Ok(session_id) = raw.to_str() else {
            return;
        };
        if session_id.trim().is_empty() {
            return;
        }

        let mut slot = self.session_id.write().await;
        let session_id = session_id.to_string();
        if slot.as_ref() != Some(&session_id) {
            debug!(
                url = %self.display_url,
                session_id = %session_id,
                "updated MCP streamable HTTP session id"
            );
            *slot = Some(session_id);
        }
    }

    fn apply_default_headers(&self, mut req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        for (name, value) in &self.default_headers {
            if self.auth.is_some() && name == reqwest::header::AUTHORIZATION {
                continue;
            }
            req = req.header(name, value);
        }
        req
    }

    fn response_is_event_stream(resp: &reqwest::Response) -> bool {
        resp.headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|ct| {
                ct.split(';')
                    .next()
                    .is_some_and(|base| base.trim() == "text/event-stream")
            })
            .unwrap_or(false)
    }

    /// Send a POST request and handle 401 with auth retry.
    /// Returns the HTTP response or a typed `McpTransportError`.
    async fn send_with_auth_retry(
        &self,
        method: &str,
        body: &impl serde::Serialize,
    ) -> Result<reqwest::Response> {
        // First attempt
        let req = self.build_post().await?;
        let http_resp = self
            .timed_send(req.json(body))
            .await
            .with_context(|| format!("SSE POST to '{}' for '{method}' failed", self.display_url))?;

        if http_resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            let has_session_header = http_resp.headers().contains_key(MCP_SESSION_ID_HEADER);
            self.store_session_id_from_response(&http_resp).await;

            if let Some(auth) = &self.auth {
                let first_www_auth = http_resp
                    .headers()
                    .get("www-authenticate")
                    .and_then(|v| v.to_str().ok())
                    .map(String::from);

                // Some streamable HTTP servers issue a session ID alongside 401
                // and expect subsequent requests to include it. Retry once with
                // current auth/session before forcing interactive re-auth.
                if has_session_header {
                    info!(
                        method = %method,
                        url = %self.display_url,
                        www_authenticate = ?first_www_auth,
                        "received 401 with session header, replaying request before OAuth re-auth"
                    );

                    let req = self.build_post().await?;
                    let replay_resp = self.timed_send(req.json(body)).await.with_context(|| {
                        format!(
                            "SSE POST session replay to '{}' for '{method}' failed",
                            self.display_url
                        )
                    })?;

                    if replay_resp.status() != reqwest::StatusCode::UNAUTHORIZED {
                        self.store_session_id_from_response(&replay_resp).await;
                        return Ok(replay_resp);
                    }

                    self.store_session_id_from_response(&replay_resp).await;
                    let replay_www_auth = replay_resp
                        .headers()
                        .get("www-authenticate")
                        .and_then(|v| v.to_str().ok())
                        .map(String::from);

                    info!(
                        method = %method,
                        url = %self.display_url,
                        "received 401 after session replay, attempting OAuth re-auth"
                    );

                    if auth.handle_unauthorized(replay_www_auth.as_deref()).await? {
                        // Retry with new token
                        let req = self.build_post().await?;
                        let retry_resp =
                            self.timed_send(req.json(body)).await.with_context(|| {
                                format!(
                                    "SSE POST retry to '{}' for '{method}' failed",
                                    self.display_url
                                )
                            })?;

                        if retry_resp.status() == reqwest::StatusCode::UNAUTHORIZED {
                            return Err(McpTransportError::Unauthorized {
                                www_authenticate: retry_resp
                                    .headers()
                                    .get("www-authenticate")
                                    .and_then(|v| v.to_str().ok())
                                    .map(String::from),
                            }
                            .into());
                        }

                        self.store_session_id_from_response(&retry_resp).await;
                        return Ok(retry_resp);
                    }

                    return Err(McpTransportError::Unauthorized {
                        www_authenticate: replay_www_auth,
                    }
                    .into());
                }

                info!(
                    method = %method,
                    url = %self.display_url,
                    www_authenticate = ?first_www_auth,
                    "received 401, attempting OAuth re-auth"
                );

                if auth.handle_unauthorized(first_www_auth.as_deref()).await? {
                    // Retry with new token
                    let req = self.build_post().await?;
                    let retry_resp = self.timed_send(req.json(body)).await.with_context(|| {
                        format!(
                            "SSE POST retry to '{}' for '{method}' failed",
                            self.display_url
                        )
                    })?;

                    if retry_resp.status() == reqwest::StatusCode::UNAUTHORIZED {
                        return Err(McpTransportError::Unauthorized {
                            www_authenticate: retry_resp
                                .headers()
                                .get("www-authenticate")
                                .and_then(|v| v.to_str().ok())
                                .map(String::from),
                        }
                        .into());
                    }

                    self.store_session_id_from_response(&retry_resp).await;
                    return Ok(retry_resp);
                }
            }

            // No auth provider or auth failed
            return Err(McpTransportError::Unauthorized {
                www_authenticate: http_resp
                    .headers()
                    .get("www-authenticate")
                    .and_then(|v| v.to_str().ok())
                    .map(String::from),
            }
            .into());
        }

        self.store_session_id_from_response(&http_resp).await;
        Ok(http_resp)
    }
}

impl SseTransport {
    /// Read an SSE response until the response to `id` arrives, answering
    /// server requests and forwarding notifications sent on the way.
    async fn read_response_stream(
        &self,
        http_resp: reqwest::Response,
        id: &serde_json::Value,
        method: &str,
    ) -> Result<JsonRpcResponse> {
        self.incoming.touch();
        let read = self.pump_stream(http_resp, Some(id));
        match self.incoming.await_active(read, self.request_timeout).await {
            Some(Ok(Some(resp))) => Ok(resp),
            Some(Ok(None)) => Err(Error::message(format!(
                "event stream for '{method}' ended without a response"
            ))),
            Some(Err(error)) => Err(error),
            None => Err(Error::message(format!(
                "MCP request '{method}' timed out after {}s (no response from server)",
                self.request_timeout.as_secs()
            ))),
        }
    }

    /// Process one SSE body. Returns the response matching `wanted`, or
    /// `None` when the stream ends first (always `None` for the GET stream).
    async fn pump_stream(
        &self,
        http_resp: reqwest::Response,
        wanted: Option<&serde_json::Value>,
    ) -> Result<Option<JsonRpcResponse>> {
        let mut stream = http_resp.bytes_stream();
        let mut decoder = SseDecoder::default();
        let (reply_tx, mut reply_rx) = mpsc::unbounded_channel::<JsonRpcResponse>();
        loop {
            tokio::select! {
                chunk = stream.next() => {
                    let Some(chunk) = chunk else {
                        let last = decoder.finish();
                        return Ok(last.and_then(|data| self.handle_event(&data, wanted, &reply_tx)));
                    };
                    let chunk = chunk.map_err(sanitize_reqwest_error)?;
                    // Keepalives on the GET stream must not extend unrelated requests.
                    if wanted.is_some() {
                        self.incoming.touch();
                    }
                    for data in decoder.push(&chunk)? {
                        if let Some(resp) = self.handle_event(&data, wanted, &reply_tx) {
                            return Ok(Some(resp));
                        }
                    }
                },
                Some(reply) = reply_rx.recv() => self.post_reply(reply).await,
            }
        }
    }

    fn handle_event(
        &self,
        data: &str,
        wanted: Option<&serde_json::Value>,
        reply_tx: &mpsc::UnboundedSender<JsonRpcResponse>,
    ) -> Option<JsonRpcResponse> {
        match IncomingMessage::parse(data) {
            Ok(IncomingMessage::Response(resp)) if Some(&resp.id) == wanted => Some(resp),
            Ok(IncomingMessage::Response(resp)) => {
                warn!(id = %resp.id, "received SSE response for unknown request id");
                None
            },
            Ok(IncomingMessage::Request(request)) => {
                let reply_tx = reply_tx.clone();
                self.incoming
                    .route_request(request, move |response| async move {
                        let _ = reply_tx.send(response);
                    });
                None
            },
            Ok(IncomingMessage::Notification(notification)) => {
                self.incoming.route_notification(notification);
                None
            },
            Err(error) => {
                debug!(%error, bytes = data.len(), "ignoring non-JSON-RPC SSE event");
                None
            },
        }
    }

    /// POST our answer to a server request.
    async fn post_reply(&self, reply: JsonRpcResponse) {
        match self.send_with_auth_retry("response", &reply).await {
            Ok(resp) if resp.status().is_success() => {},
            Ok(resp) => warn!(status = %resp.status(), "MCP server rejected our response"),
            Err(error) => warn!(%error, "failed to send response to MCP server"),
        }
    }

    async fn open_listener(&self) -> Result<Option<reqwest::Response>> {
        let mut req = self
            .client
            .get(self.request_url.expose_secret())
            .header("Accept", "text/event-stream");
        req = self.apply_protocol_version(req);
        req = self.apply_default_headers(req);
        req = self.apply_session_and_auth(req).await?;
        let resp = self.timed_send(req).await?;
        if !resp.status().is_success() || !Self::response_is_event_stream(&resp) {
            // 405 means the server offers no GET stream; that is allowed.
            debug!(status = %resp.status(), url = %self.display_url, "no MCP server-to-client stream");
            return Ok(None);
        }
        Ok(Some(resp))
    }

    /// Keep the GET stream open for notifications and requests the server
    /// sends outside any client request.
    async fn run_listener(self: Arc<Self>) {
        let mut failures = 0u32;
        while failures <= MAX_LISTENER_RECONNECTS {
            match self.open_listener().await {
                Ok(Some(resp)) => {
                    failures = 0;
                    if let Err(error) = self.pump_stream(resp, None).await {
                        debug!(%error, url = %self.display_url, "MCP server stream closed");
                    }
                },
                Ok(None) => return,
                Err(error) => {
                    debug!(%error, url = %self.display_url, "MCP server stream failed");
                },
            }
            failures += 1;
            tokio::time::sleep(Duration::from_secs(1 << failures.min(5))).await;
        }
        warn!(url = %self.display_url, "giving up on MCP server-to-client stream");
    }
}

#[async_trait::async_trait]
impl McpTransport for SseTransport {
    async fn request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<JsonRpcResponse> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let req = JsonRpcRequest::new(id, method, params);

        debug!(method = %method, id = %id, url = %self.display_url, "SSE client -> server");

        let http_resp = self.send_with_auth_retry(method, &req).await?;

        if !http_resp.status().is_success() {
            let status = http_resp.status();
            let body = http_resp.text().await.unwrap_or_default();
            return Err(Error::message(format!(
                "MCP SSE server returned HTTP {status} for '{method}': {body}"
            )));
        }

        let resp = if Self::response_is_event_stream(&http_resp) {
            self.read_response_stream(http_resp, &req.id, method)
                .await?
        } else {
            match tokio::time::timeout(self.request_timeout, http_resp.json::<JsonRpcResponse>())
                .await
            {
                Ok(parsed) => parsed
                    .with_context(|| format!("failed to parse JSON-RPC response for '{method}'"))?,
                Err(_) => {
                    return Err(Error::message(format!(
                        "MCP request '{method}' timed out reading the response"
                    )));
                },
            }
        };

        if let Some(ref err) = resp.error {
            return Err(Error::message(format!(
                "MCP SSE error on '{method}': code={} message={}",
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

        debug!(
            method = %method,
            url = %self.display_url,
            "SSE client -> server (notification)"
        );

        let http_resp = self.send_with_auth_retry(method, &notif).await?;

        if !http_resp.status().is_success() {
            let status = http_resp.status();
            warn!(method = %method, %status, "SSE notification returned non-success");
        }

        Ok(())
    }

    async fn is_alive(&self) -> bool {
        // Try a lightweight GET request to check connectivity and session continuity.
        let mut req = self
            .client
            .get(self.request_url.expose_secret())
            .timeout(std::time::Duration::from_secs(5))
            .header("Accept", STREAMABLE_ACCEPT_HEADER);
        req = self.apply_protocol_version(req);

        req = self.apply_default_headers(req);

        if let Some(session_id) = self.session_id.read().await.clone() {
            req = req.header(MCP_SESSION_ID_HEADER, session_id);
        }

        // Include auth header in health checks too
        if let Some(token) = match &self.auth {
            Some(auth) => auth.access_token().await.ok().flatten(),
            None => None,
        } {
            req = req.header("Authorization", format!("Bearer {}", token.expose_secret()));
        }

        match req.send().await {
            Ok(resp) => {
                self.store_session_id_from_response(&resp).await;
                // Any HTTP response proves the server is reachable.
                // Non-success statuses (401, 403, 405) are common for
                // Streamable HTTP servers where GET is optional (MCP spec)
                // or auth headers differ between GET and POST. Only true
                // network errors (connection refused, timeout, DNS) mean dead.
                true
            },
            Err(_) => false,
        }
    }

    async fn kill(&self) {
        if let Some(listener) = self
            .listener
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            listener.abort();
        }
        let session_id = {
            let mut slot = self.session_id.write().await;
            slot.take()
        };

        let Some(session_id) = session_id else {
            return;
        };

        let mut req = self
            .client
            .delete(self.request_url.expose_secret())
            .timeout(std::time::Duration::from_secs(5))
            .header(MCP_SESSION_ID_HEADER, session_id);
        req = self.apply_protocol_version(req);

        req = self.apply_default_headers(req);

        if let Some(token) = match &self.auth {
            Some(auth) => auth.access_token().await.ok().flatten(),
            None => None,
        } {
            req = req.header("Authorization", format!("Bearer {}", token.expose_secret()));
        }

        if let Err(e) = req.send().await {
            warn!(
                url = %self.display_url,
                error = %sanitize_reqwest_error(e),
                "failed to close MCP streamable HTTP session"
            );
        }
    }

    fn set_dispatcher(&self, dispatcher: Arc<dyn IncomingDispatcher>) {
        self.incoming.set_dispatcher(dispatcher);
    }

    fn accepts_server_requests(&self) -> bool {
        true
    }

    fn set_protocol_version(&self, version: &str) {
        *self
            .protocol_version
            .write()
            .unwrap_or_else(|e| e.into_inner()) = Some(version.to_string());
    }

    async fn start_server_stream(&self) {
        let Some(transport) = self.weak_self.upgrade() else {
            return;
        };
        let mut listener = self.listener.lock().unwrap_or_else(|e| e.into_inner());
        if listener.is_some() {
            return;
        }
        let task = tokio::spawn(async move { transport.run_listener().await });
        *listener = Some(task.abort_handle());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unused_local_url() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        format!("http://{addr}/mcp")
    }

    #[test]
    fn test_sse_transport_creation() {
        let transport = SseTransport::new("http://localhost:8080/mcp");
        assert!(transport.is_ok());
    }

    #[test]
    fn test_sse_transport_invalid_url_fails_creation() {
        let transport = SseTransport::new("not-a-url");
        assert!(transport.is_err());
    }

    #[tokio::test]
    async fn test_sse_transport_is_alive_unreachable() {
        let transport = SseTransport::new(&unused_local_url()).unwrap();
        assert!(!transport.is_alive().await);
    }

    #[tokio::test]
    async fn test_sse_transport_request_unreachable() {
        let transport = SseTransport::new(&unused_local_url()).unwrap();
        let result = transport.request("test", None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_sse_transport_kill() {
        let transport = SseTransport::new("http://localhost:8080/mcp").unwrap();
        transport.kill().await;
        // Should not panic
    }

    #[test]
    fn test_sse_transport_with_auth_creation() {
        let auth: SharedAuthProvider = Arc::new(crate::auth::NoAuthProvider);
        let transport = SseTransport::with_auth("http://localhost:8080/mcp", auth);
        assert!(transport.is_ok());
    }

    #[tokio::test]
    async fn test_sse_transport_401_without_auth_returns_unauthorized() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(401)
            .with_header("www-authenticate", r#"Bearer realm="test""#)
            .create_async()
            .await;

        let transport = SseTransport::new(&server.url()).unwrap();
        let result = transport.request("test", None).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            err,
            crate::Error::Transport(McpTransportError::Unauthorized { .. })
        ));
    }

    #[tokio::test]
    async fn test_sse_transport_200_no_auth() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#)
            .create_async()
            .await;

        let transport = SseTransport::new(&server.url()).unwrap();
        let resp = transport.request("test", None).await.unwrap();
        assert!(resp.result.is_some());
    }

    fn remote_with_headers(
        url: &str,
        headers: &[(&str, &str)],
    ) -> crate::remote::ResolvedRemoteConfig {
        let config = crate::registry::McpServerConfig {
            transport: crate::registry::TransportType::Sse,
            url: Some(Secret::new(url.to_string())),
            headers: headers
                .iter()
                .map(|(name, value)| ((*name).to_string(), Secret::new((*value).to_string())))
                .collect(),
            ..Default::default()
        };
        crate::remote::ResolvedRemoteConfig::from_server_config(
            &config,
            &std::collections::HashMap::new(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn test_sse_transport_custom_headers_injected() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/")
            .match_header("x-api-key", "secret-header")
            .match_header("authorization", "ApiKey raw-secret")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#)
            .create_async()
            .await;

        let remote = remote_with_headers(&server.url(), &[
            ("x-api-key", "secret-header"),
            ("authorization", "ApiKey raw-secret"),
        ]);
        let transport = SseTransport::new_with_remote(remote, Duration::from_secs(60)).unwrap();
        let resp = transport.request("test", None).await.unwrap();
        assert!(resp.result.is_some());
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_sse_transport_bearer_header_injected() {
        use crate::auth::{McpAuthProvider, McpAuthState};

        /// Test auth provider that always returns a fixed token.
        struct FixedTokenProvider;

        #[async_trait::async_trait]
        impl McpAuthProvider for FixedTokenProvider {
            async fn access_token(&self) -> Result<Option<Secret<String>>> {
                Ok(Some(Secret::new("test-token-123".to_string())))
            }

            async fn handle_unauthorized(&self, _: Option<&str>) -> Result<bool> {
                Ok(false)
            }

            async fn start_oauth(
                &self,
                _redirect_uri: &str,
                _www_authenticate: Option<&str>,
            ) -> Result<Option<String>> {
                Ok(None)
            }

            async fn complete_oauth(&self, _state: &str, _code: &str) -> Result<bool> {
                Ok(false)
            }

            fn pending_auth_url(&self) -> Option<String> {
                None
            }

            fn auth_state(&self) -> McpAuthState {
                McpAuthState::Authenticated
            }
        }

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/")
            .match_header("authorization", "Bearer test-token-123")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#)
            .create_async()
            .await;

        let auth: SharedAuthProvider = Arc::new(FixedTokenProvider);
        let transport = SseTransport::with_auth(&server.url(), auth).unwrap();
        let resp = transport.request("test", None).await.unwrap();
        assert!(resp.result.is_some());
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_sse_transport_oauth_authorization_overrides_custom_header() {
        use crate::auth::{McpAuthProvider, McpAuthState};

        struct FixedTokenProvider;

        #[async_trait::async_trait]
        impl McpAuthProvider for FixedTokenProvider {
            async fn access_token(&self) -> Result<Option<Secret<String>>> {
                Ok(Some(Secret::new("oauth-token-123".to_string())))
            }

            async fn handle_unauthorized(&self, _: Option<&str>) -> Result<bool> {
                Ok(false)
            }

            async fn start_oauth(
                &self,
                _redirect_uri: &str,
                _www_authenticate: Option<&str>,
            ) -> Result<Option<String>> {
                Ok(None)
            }

            async fn complete_oauth(&self, _state: &str, _code: &str) -> Result<bool> {
                Ok(false)
            }

            fn pending_auth_url(&self) -> Option<String> {
                None
            }

            fn auth_state(&self) -> McpAuthState {
                McpAuthState::Authenticated
            }
        }

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/")
            .match_header("x-api-key", "secret-header")
            .match_header("authorization", "Bearer oauth-token-123")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#)
            .create_async()
            .await;

        let remote = remote_with_headers(&server.url(), &[
            ("x-api-key", "secret-header"),
            ("authorization", "ApiKey raw-secret"),
        ]);
        let auth: SharedAuthProvider = Arc::new(FixedTokenProvider);
        let transport =
            SseTransport::with_auth_remote(remote, auth, Duration::from_secs(60)).unwrap();
        let resp = transport.request("test", None).await.unwrap();
        assert!(resp.result.is_some());
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_sse_transport_propagates_session_id() {
        let mut server = mockito::Server::new_async().await;

        let first = server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_header("mcp-session-id", "session-123")
            .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#)
            .create_async()
            .await;

        let second = server
            .mock("POST", "/")
            .match_header("mcp-session-id", "session-123")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"jsonrpc":"2.0","id":2,"result":{"ok":true}}"#)
            .create_async()
            .await;

        let transport = SseTransport::new(&server.url()).unwrap();
        transport.request("initialize", None).await.unwrap();
        transport.request("tools/list", None).await.unwrap();

        first.assert_async().await;
        second.assert_async().await;
    }

    #[tokio::test]
    async fn test_sse_transport_401_with_session_replays_before_reauth() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        use secrecy::Secret;

        use crate::auth::{McpAuthProvider, McpAuthState};

        struct CountingAuthProvider {
            reauth_calls: AtomicUsize,
        }

        #[async_trait::async_trait]
        impl McpAuthProvider for CountingAuthProvider {
            async fn access_token(&self) -> Result<Option<Secret<String>>> {
                Ok(Some(Secret::new("token-123".to_string())))
            }

            async fn handle_unauthorized(&self, _www_authenticate: Option<&str>) -> Result<bool> {
                self.reauth_calls.fetch_add(1, Ordering::SeqCst);
                Ok(false)
            }

            async fn start_oauth(
                &self,
                _redirect_uri: &str,
                _www_authenticate: Option<&str>,
            ) -> Result<Option<String>> {
                Ok(None)
            }

            async fn complete_oauth(&self, _state: &str, _code: &str) -> Result<bool> {
                Ok(false)
            }

            fn pending_auth_url(&self) -> Option<String> {
                None
            }

            fn auth_state(&self) -> McpAuthState {
                McpAuthState::Authenticated
            }
        }

        let mut server = mockito::Server::new_async().await;

        let first = server
            .mock("POST", "/")
            .match_header("authorization", "Bearer token-123")
            .with_status(401)
            .with_header("mcp-session-id", "session-reauth-1")
            .with_header("www-authenticate", r#"Bearer realm="test""#)
            .create_async()
            .await;

        let second = server
            .mock("POST", "/")
            .match_header("authorization", "Bearer token-123")
            .match_header("mcp-session-id", "session-reauth-1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#)
            .create_async()
            .await;

        let auth = Arc::new(CountingAuthProvider {
            reauth_calls: AtomicUsize::new(0),
        });
        let auth_shared: SharedAuthProvider = auth.clone();

        let transport = SseTransport::with_auth(&server.url(), auth_shared).unwrap();
        let resp = transport.request("initialize", None).await.unwrap();
        assert!(resp.result.is_some());
        assert_eq!(auth.reauth_calls.load(Ordering::SeqCst), 0);

        first.assert_async().await;
        second.assert_async().await;
    }

    #[tokio::test]
    async fn test_sse_transport_parses_event_stream_response() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(
                "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}\n\n",
            )
            .create_async()
            .await;

        let transport = SseTransport::new(&server.url()).unwrap();
        let resp = transport.request("initialize", None).await.unwrap();
        assert!(resp.result.is_some());
    }

    // ── is_alive health-check tests (issue #732) ──────────────────────

    #[tokio::test]
    async fn test_is_alive_200_returns_true() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":true}"#)
            .create_async()
            .await;

        let transport = SseTransport::new(&server.url()).unwrap();
        assert!(transport.is_alive().await);
    }

    #[tokio::test]
    async fn test_is_alive_401_returns_true() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", "/")
            .with_status(401)
            .with_header("www-authenticate", r#"Bearer realm="test""#)
            .create_async()
            .await;

        let transport = SseTransport::new(&server.url()).unwrap();
        assert!(transport.is_alive().await);
    }

    /// Streamable HTTP servers may not support GET (it's optional in the MCP
    /// spec). A 405 response proves the server is reachable and alive.
    #[tokio::test]
    async fn test_is_alive_405_method_not_allowed_returns_true() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", "/")
            .with_status(405)
            .with_header("allow", "POST, DELETE")
            .create_async()
            .await;

        let transport = SseTransport::new(&server.url()).unwrap();
        assert!(transport.is_alive().await);
    }

    /// A 403 Forbidden from the health check still proves the server is
    /// reachable — it just refused the request. The server is alive.
    #[tokio::test]
    async fn test_is_alive_403_forbidden_returns_true() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", "/")
            .with_status(403)
            .create_async()
            .await;

        let transport = SseTransport::new(&server.url()).unwrap();
        assert!(transport.is_alive().await);
    }

    /// A 400 Bad Request still proves the server endpoint is alive.
    #[tokio::test]
    async fn test_is_alive_400_bad_request_returns_true() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", "/")
            .with_status(400)
            .with_body(r#"{"error":"bad request"}"#)
            .create_async()
            .await;

        let transport = SseTransport::new(&server.url()).unwrap();
        assert!(transport.is_alive().await);
    }

    #[tokio::test]
    async fn test_sse_transport_kill_sends_delete_with_session_id() {
        let mut server = mockito::Server::new_async().await;
        let init = server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_header("mcp-session-id", "session-to-close")
            .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#)
            .create_async()
            .await;
        let delete = server
            .mock("DELETE", "/")
            .match_header("mcp-session-id", "session-to-close")
            .with_status(204)
            .create_async()
            .await;

        let transport = SseTransport::new(&server.url()).unwrap();
        transport.request("initialize", None).await.unwrap();
        transport.kill().await;

        init.assert_async().await;
        delete.assert_async().await;
    }
}
