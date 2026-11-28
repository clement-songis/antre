//! OpenAI Realtime API live transcription.
//!
//! Opens a transcription session (`/v1/realtime?intent=transcription`) with
//! turn detection disabled: the caller decides when the turn ends, matching
//! push-to-talk and the browser-side VAD.

use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use {
    anyhow::{Context, Result, anyhow, bail},
    async_trait::async_trait,
    base64::Engine,
    futures::{SinkExt, StreamExt},
    secrecy::{ExposeSecret, Secret},
    serde::Deserialize,
    serde_json::{Value, json},
    tokio::{
        net::TcpStream,
        sync::mpsc,
        time::{Instant, sleep_until, timeout},
    },
    tokio_tungstenite::{
        MaybeTlsStream, WebSocketStream, connect_async,
        tungstenite::{
            Message,
            client::IntoClientRequest,
            http::{HeaderValue, header::AUTHORIZATION},
        },
    },
    tracing::{debug, warn},
};

use {
    super::types::{
        STREAM_CHANNEL_CAPACITY, StreamingSttEvent, StreamingSttInput, StreamingSttProvider,
        StreamingSttRequest, StreamingSttSession,
    },
    crate::config::SttProviderId,
};

const API_BASE: &str = "https://api.openai.com/v1";

/// Default Realtime transcription model.
pub const DEFAULT_REALTIME_MODEL: &str = "gpt-live-transcribe";

const SAMPLE_RATE: u32 = 24_000;
/// OpenAI rejects a commit holding less than 100 ms of audio.
const MIN_COMMIT_BYTES: usize = (SAMPLE_RATE as usize / 10) * 2;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const FINAL_TIMEOUT: Duration = Duration::from_secs(10);

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// OpenAI Realtime API live transcription provider.
pub struct OpenAiRealtimeStt {
    api_key: Option<Secret<String>>,
    base_url: String,
    model: String,
    language: Option<String>,
    connect_timeout: Duration,
    final_timeout: Duration,
}

impl std::fmt::Debug for OpenAiRealtimeStt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiRealtimeStt")
            .field("api_key", &"[REDACTED]")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("language", &self.language)
            .finish()
    }
}

impl OpenAiRealtimeStt {
    /// Create a provider. `base_url` is the HTTP API base
    /// (default `https://api.openai.com/v1`).
    #[must_use]
    pub fn new(
        api_key: Option<Secret<String>>,
        base_url: Option<String>,
        model: Option<String>,
        language: Option<String>,
    ) -> Self {
        Self {
            api_key,
            base_url: base_url
                .map(|url| url.trim_end_matches('/').to_string())
                .unwrap_or_else(|| API_BASE.into()),
            model: model
                .filter(|m| !m.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_REALTIME_MODEL.into()),
            language,
            connect_timeout: CONNECT_TIMEOUT,
            final_timeout: FINAL_TIMEOUT,
        }
    }

    /// Override how long to wait for the session to open and for the final
    /// transcript after [`StreamingSttInput::Finish`].
    #[must_use]
    pub fn with_timeouts(mut self, connect: Duration, final_transcript: Duration) -> Self {
        self.connect_timeout = connect;
        self.final_timeout = final_transcript;
        self
    }

    #[must_use]
    pub fn is_configured(&self) -> bool {
        self.api_key.is_some()
    }

    fn websocket_url(&self) -> Result<String> {
        let ws_base = if let Some(rest) = self.base_url.strip_prefix("https://") {
            format!("wss://{rest}")
        } else if let Some(rest) = self.base_url.strip_prefix("http://") {
            format!("ws://{rest}")
        } else {
            bail!("unsupported OpenAI base URL scheme: {}", self.base_url);
        };
        Ok(format!("{ws_base}/realtime?intent=transcription"))
    }

    fn session_update(&self, request: &StreamingSttRequest) -> Value {
        let mut transcription = json!({ "model": self.model });
        let language = request
            .language
            .as_deref()
            .or(self.language.as_deref())
            .filter(|l| !l.is_empty());
        if let Some(language) = language {
            // gpt-live-transcribe takes a `languages` list and rejects `language`.
            if self.model.starts_with("gpt-live-transcribe") {
                transcription["languages"] = json!([language]);
            } else {
                transcription["language"] = json!(language);
            }
        }
        // gpt-realtime-whisper rejects prompts in GA Realtime sessions.
        if let Some(prompt) = request.prompt.as_deref().filter(|p| !p.is_empty())
            && !self.model.starts_with("gpt-realtime-whisper")
        {
            transcription["prompt"] = json!(prompt);
        }
        json!({
            "type": "session.update",
            "session": {
                "type": "transcription",
                "audio": {
                    "input": {
                        "format": { "type": "audio/pcm", "rate": SAMPLE_RATE },
                        "transcription": transcription,
                        "turn_detection": null,
                    }
                }
            }
        })
    }

    async fn connect(&self, request: &StreamingSttRequest) -> Result<WsStream> {
        let key = self
            .api_key
            .as_ref()
            .ok_or_else(|| anyhow!("OpenAI API key not configured"))?;
        let mut ws_request = self.websocket_url()?.into_client_request()?;
        ws_request.headers_mut().insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", key.expose_secret()))?,
        );
        let (mut ws, _) = connect_async(ws_request)
            .await
            .context("OpenAI Realtime connection failed")?;
        ws.send(Message::text(self.session_update(request).to_string()))
            .await?;
        wait_session_ready(&mut ws).await?;
        Ok(ws)
    }
}

#[async_trait]
impl StreamingSttProvider for OpenAiRealtimeStt {
    fn id(&self) -> SttProviderId {
        SttProviderId::Whisper
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn input_sample_rate(&self) -> u32 {
        SAMPLE_RATE
    }

    async fn open(&self, request: StreamingSttRequest) -> Result<StreamingSttSession> {
        let ws = timeout(self.connect_timeout, self.connect(&request))
            .await
            .context("timed out opening the OpenAI Realtime session")??;
        debug!(model = %self.model, "OpenAI Realtime transcription session ready");
        let (input_tx, input_rx) = mpsc::channel(STREAM_CHANNEL_CAPACITY);
        let (event_tx, event_rx) = mpsc::channel(STREAM_CHANNEL_CAPACITY);
        tokio::spawn(run_stream(ws, input_rx, event_tx, self.final_timeout));
        Ok(StreamingSttSession {
            input: input_tx,
            events: event_rx,
        })
    }
}

#[derive(Debug, Default, Deserialize)]
struct ApiError {
    #[serde(default)]
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ServerEvent {
    #[serde(rename = "session.updated")]
    SessionUpdated,
    #[serde(rename = "input_audio_buffer.committed")]
    Committed { item_id: String },
    #[serde(rename = "input_audio_buffer.speech_started")]
    SpeechStarted,
    #[serde(rename = "conversation.item.input_audio_transcription.delta")]
    Delta {
        item_id: String,
        #[serde(default)]
        delta: String,
    },
    #[serde(rename = "conversation.item.input_audio_transcription.completed")]
    Completed {
        item_id: String,
        #[serde(default)]
        transcript: String,
    },
    #[serde(rename = "conversation.item.input_audio_transcription.failed")]
    Failed {
        #[serde(default)]
        error: ApiError,
    },
    #[serde(rename = "error")]
    Error {
        #[serde(default)]
        error: ApiError,
    },
    #[serde(other)]
    Other,
}

fn parse_event(message: &Message) -> Option<ServerEvent> {
    let text = match message {
        Message::Text(text) => text.as_str(),
        Message::Binary(bytes) => std::str::from_utf8(bytes).ok()?,
        _ => return None,
    };
    match serde_json::from_str(text) {
        Ok(event) => Some(event),
        Err(e) => {
            debug!(error = %e, "ignoring unparsable OpenAI Realtime event");
            None
        },
    }
}

async fn wait_session_ready(ws: &mut WsStream) -> Result<()> {
    while let Some(message) = ws.next().await {
        let message = message?;
        if matches!(message, Message::Close(_)) {
            break;
        }
        match parse_event(&message) {
            Some(ServerEvent::SessionUpdated) => return Ok(()),
            Some(ServerEvent::Error { error }) => {
                bail!("OpenAI Realtime session rejected: {}", error.message)
            },
            _ => {},
        }
    }
    bail!("OpenAI Realtime connection closed before the session was ready")
}

#[derive(Default)]
struct StreamState {
    partials: HashMap<String, String>,
    open_items: HashSet<String>,
    uncommitted_bytes: usize,
    commit_pending: bool,
    finishing: bool,
}

impl StreamState {
    fn is_done(&self) -> bool {
        self.finishing && !self.commit_pending && self.open_items.is_empty()
    }

    /// Returns the event to forward, and whether the stream must stop.
    fn apply(&mut self, event: ServerEvent) -> (Option<StreamingSttEvent>, bool) {
        match event {
            ServerEvent::Committed { item_id } => {
                self.commit_pending = false;
                self.open_items.insert(item_id);
                (None, false)
            },
            ServerEvent::SpeechStarted => (Some(StreamingSttEvent::SpeechStarted), false),
            ServerEvent::Delta { item_id, delta } => {
                self.open_items.insert(item_id.clone());
                let text = self.partials.entry(item_id.clone()).or_default();
                text.push_str(&delta);
                (
                    Some(StreamingSttEvent::Partial {
                        segment: item_id,
                        text: text.clone(),
                    }),
                    false,
                )
            },
            ServerEvent::Completed {
                item_id,
                transcript,
            } => {
                self.open_items.remove(&item_id);
                self.partials.remove(&item_id);
                (
                    Some(StreamingSttEvent::Final {
                        segment: item_id,
                        text: transcript,
                    }),
                    false,
                )
            },
            ServerEvent::Failed { error } | ServerEvent::Error { error } => (
                Some(StreamingSttEvent::Error {
                    message: error.message,
                }),
                true,
            ),
            ServerEvent::SessionUpdated | ServerEvent::Other => (None, false),
        }
    }

    /// Settle open segments that will never complete (no commit was sent).
    fn drain_partials(&mut self) -> Vec<StreamingSttEvent> {
        self.open_items.clear();
        self.partials
            .drain()
            .map(|(segment, text)| StreamingSttEvent::Final { segment, text })
            .collect()
    }
}

async fn send_json(ws: &mut WsStream, value: Value) -> Result<()> {
    ws.send(Message::text(value.to_string())).await?;
    Ok(())
}

async fn run_stream(
    mut ws: WsStream,
    mut input: mpsc::Receiver<StreamingSttInput>,
    events: mpsc::Sender<StreamingSttEvent>,
    final_timeout: Duration,
) {
    let mut state = StreamState::default();
    let mut deadline = Instant::now();
    let mut emit = Vec::new();

    loop {
        tokio::select! {
            next = input.recv(), if !state.finishing => match next {
                Some(StreamingSttInput::Audio(audio)) => {
                    state.uncommitted_bytes += audio.len();
                    let payload = base64::engine::general_purpose::STANDARD.encode(&audio);
                    let sent = send_json(
                        &mut ws,
                        json!({ "type": "input_audio_buffer.append", "audio": payload }),
                    )
                    .await;
                    if let Err(e) = sent {
                        emit.push(StreamingSttEvent::Error { message: e.to_string() });
                        break;
                    }
                },
                Some(StreamingSttInput::Finish) => {
                    state.finishing = true;
                    deadline = Instant::now() + final_timeout;
                    if state.uncommitted_bytes >= MIN_COMMIT_BYTES {
                        if let Err(e) =
                            send_json(&mut ws, json!({ "type": "input_audio_buffer.commit" })).await
                        {
                            emit.push(StreamingSttEvent::Error { message: e.to_string() });
                            break;
                        }
                        state.commit_pending = true;
                        state.uncommitted_bytes = 0;
                    } else {
                        emit.extend(state.drain_partials());
                    }
                },
                None => {
                    debug!("OpenAI Realtime stream cancelled by caller");
                    break;
                },
            },
            message = ws.next() => match message {
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(message)) => {
                    let Some(event) = parse_event(&message) else { continue };
                    let (forward, stop) = state.apply(event);
                    if let Some(forward) = forward
                        && events.send(forward).await.is_err()
                    {
                        break;
                    }
                    if stop {
                        break;
                    }
                },
                Some(Err(e)) => {
                    emit.push(StreamingSttEvent::Error { message: e.to_string() });
                    break;
                },
            },
            () = sleep_until(deadline), if state.finishing => {
                warn!("OpenAI Realtime final transcript timed out");
                emit.extend(state.drain_partials());
                break;
            },
        }
        if state.is_done() {
            break;
        }
    }

    let _ = ws.close(None).await;
    emit.push(StreamingSttEvent::Closed);
    for event in emit {
        if events.send(event).await.is_err() {
            break;
        }
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use {
        super::*,
        bytes::Bytes,
        std::sync::{Arc, Mutex},
        tokio::net::TcpListener,
        tokio_tungstenite::{
            accept_hdr_async,
            tungstenite::handshake::server::{Request, Response},
        },
    };

    type ServerWs = WebSocketStream<TcpStream>;

    /// Accept one WebSocket connection, record its `Authorization` header and
    /// URI, then run `script` on it.
    // tungstenite's handshake callback fixes the large `Err` type.
    #[allow(clippy::result_large_err)]
    async fn mock_server<F, Fut>(script: F) -> (String, Arc<Mutex<Option<(String, String)>>>)
    where
        F: FnOnce(ServerWs) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(None));
        let seen_in_server = Arc::clone(&seen);
        tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let ws = accept_hdr_async(tcp, move |req: &Request, resp: Response| {
                let auth = req
                    .headers()
                    .get(AUTHORIZATION)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_string();
                *seen_in_server.lock().unwrap() = Some((auth, req.uri().to_string()));
                Ok(resp)
            })
            .await
            .unwrap();
            script(ws).await;
        });
        (format!("http://{addr}/v1"), seen)
    }

    async fn recv_json(ws: &mut ServerWs) -> Value {
        loop {
            match ws.next().await.unwrap().unwrap() {
                Message::Text(text) => return serde_json::from_str(text.as_str()).unwrap(),
                Message::Close(_) => panic!("client closed"),
                _ => {},
            }
        }
    }

    async fn send(ws: &mut ServerWs, value: Value) {
        ws.send(Message::text(value.to_string())).await.unwrap();
    }

    fn provider(base_url: String, model: Option<&str>) -> OpenAiRealtimeStt {
        OpenAiRealtimeStt::new(
            Some(Secret::new("sk-test".into())),
            Some(base_url),
            model.map(String::from),
            None,
        )
        .with_timeouts(Duration::from_secs(5), Duration::from_secs(5))
    }

    async fn collect_events(session: &mut StreamingSttSession) -> Vec<StreamingSttEvent> {
        let mut out = Vec::new();
        while let Some(event) = session.events.recv().await {
            let closed = event == StreamingSttEvent::Closed;
            out.push(event);
            if closed {
                break;
            }
        }
        out
    }

    #[test]
    fn websocket_url_maps_scheme() {
        let p = OpenAiRealtimeStt::new(None, None, None, None);
        assert_eq!(
            p.websocket_url().unwrap(),
            "wss://api.openai.com/v1/realtime?intent=transcription"
        );
        let p = OpenAiRealtimeStt::new(None, Some("http://10.0.0.2:8000/v1/".into()), None, None);
        assert_eq!(
            p.websocket_url().unwrap(),
            "ws://10.0.0.2:8000/v1/realtime?intent=transcription"
        );
        let p = OpenAiRealtimeStt::new(None, Some("ftp://x".into()), None, None);
        assert!(p.websocket_url().is_err());
    }

    #[test]
    fn defaults_and_configuration() {
        let p = OpenAiRealtimeStt::new(None, None, Some("  ".into()), None);
        assert_eq!(p.model(), DEFAULT_REALTIME_MODEL);
        assert_eq!(p.input_sample_rate(), 24_000);
        assert_eq!(p.id(), SttProviderId::Whisper);
        assert!(!p.is_configured());
        let debug = format!(
            "{:?}",
            OpenAiRealtimeStt::new(Some(Secret::new("sk-secret".into())), None, None, None)
        );
        assert!(!debug.contains("sk-secret"));
    }

    #[test]
    fn session_update_shapes_language_and_prompt_per_model() {
        let request = StreamingSttRequest {
            language: Some("fr".into()),
            prompt: Some("Antre, Rust".into()),
        };
        let live = OpenAiRealtimeStt::new(None, None, None, None).session_update(&request);
        let input = &live["session"]["audio"]["input"];
        assert_eq!(live["session"]["type"], "transcription");
        assert_eq!(input["format"], json!({"type": "audio/pcm", "rate": 24000}));
        assert!(input["turn_detection"].is_null());
        assert_eq!(input["transcription"]["languages"], json!(["fr"]));
        assert!(input["transcription"].get("language").is_none());
        assert_eq!(input["transcription"]["prompt"], "Antre, Rust");

        let whisper = OpenAiRealtimeStt::new(None, None, Some("gpt-realtime-whisper".into()), None)
            .session_update(&request);
        let transcription = &whisper["session"]["audio"]["input"]["transcription"];
        assert_eq!(transcription["language"], "fr");
        assert!(transcription.get("prompt").is_none());

        let configured_language = OpenAiRealtimeStt::new(
            None,
            None,
            Some("gpt-4o-transcribe".into()),
            Some("de".into()),
        )
        .session_update(&StreamingSttRequest::default());
        assert_eq!(
            configured_language["session"]["audio"]["input"]["transcription"]["language"],
            "de"
        );
    }

    #[tokio::test]
    async fn open_fails_without_api_key() {
        let p = OpenAiRealtimeStt::new(None, None, None, None);
        let err = p.open(StreamingSttRequest::default()).await.unwrap_err();
        assert!(err.to_string().contains("API key"), "{err}");
    }

    #[tokio::test]
    async fn streams_partials_and_final_transcript() {
        let (base_url, seen) = mock_server(|mut ws| async move {
            let update = recv_json(&mut ws).await;
            assert_eq!(update["type"], "session.update");
            assert_eq!(
                update["session"]["audio"]["input"]["transcription"]["model"],
                "gpt-live-transcribe"
            );
            send(&mut ws, json!({"type": "session.created"})).await;
            send(&mut ws, json!({"type": "session.updated"})).await;

            let append = recv_json(&mut ws).await;
            assert_eq!(append["type"], "input_audio_buffer.append");
            let audio = base64::engine::general_purpose::STANDARD
                .decode(append["audio"].as_str().unwrap())
                .unwrap();
            assert_eq!(audio.len(), MIN_COMMIT_BYTES);
            for delta in ["Hello", ", world"] {
                send(
                    &mut ws,
                    json!({
                        "type": "conversation.item.input_audio_transcription.delta",
                        "item_id": "item_1",
                        "delta": delta,
                    }),
                )
                .await;
            }

            let commit = recv_json(&mut ws).await;
            assert_eq!(commit["type"], "input_audio_buffer.commit");
            send(
                &mut ws,
                json!({"type": "input_audio_buffer.committed", "item_id": "item_1"}),
            )
            .await;
            send(
                &mut ws,
                json!({
                    "type": "conversation.item.input_audio_transcription.completed",
                    "item_id": "item_1",
                    "transcript": "Hello, world.",
                }),
            )
            .await;
            while let Some(Ok(message)) = ws.next().await {
                if matches!(message, Message::Close(_)) {
                    break;
                }
            }
        })
        .await;

        let mut session = provider(base_url, None)
            .open(StreamingSttRequest::default())
            .await
            .unwrap();
        let (auth, uri) = seen.lock().unwrap().clone().unwrap();
        assert_eq!(auth, "Bearer sk-test");
        assert_eq!(uri, "/v1/realtime?intent=transcription");

        session
            .input
            .send(StreamingSttInput::Audio(Bytes::from(vec![
                0u8;
                MIN_COMMIT_BYTES
            ])))
            .await
            .unwrap();
        let first = session.events.recv().await.unwrap();
        assert_eq!(first, StreamingSttEvent::Partial {
            segment: "item_1".into(),
            text: "Hello".into()
        });
        let second = session.events.recv().await.unwrap();
        assert_eq!(second, StreamingSttEvent::Partial {
            segment: "item_1".into(),
            text: "Hello, world".into()
        });
        session.input.send(StreamingSttInput::Finish).await.unwrap();
        assert_eq!(collect_events(&mut session).await, vec![
            StreamingSttEvent::Final {
                segment: "item_1".into(),
                text: "Hello, world.".into()
            },
            StreamingSttEvent::Closed,
        ]);
    }

    #[tokio::test]
    async fn open_fails_when_session_is_rejected() {
        let (base_url, _) = mock_server(|mut ws| async move {
            let _ = recv_json(&mut ws).await;
            send(
                &mut ws,
                json!({"type": "error", "error": {"message": "unknown model"}}),
            )
            .await;
        })
        .await;
        let err = provider(base_url, Some("nope"))
            .open(StreamingSttRequest::default())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("unknown model"), "{err}");
    }

    #[tokio::test]
    async fn open_times_out_without_session_update() {
        let (base_url, _) = mock_server(|mut ws| async move {
            let _ = recv_json(&mut ws).await;
            tokio::time::sleep(Duration::from_secs(5)).await;
        })
        .await;
        let err = provider(base_url, None)
            .with_timeouts(Duration::from_millis(200), Duration::from_secs(1))
            .open(StreamingSttRequest::default())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("timed out"), "{err}");
    }

    #[tokio::test]
    async fn short_audio_skips_commit_and_settles_partials() {
        let (base_url, _) = mock_server(|mut ws| async move {
            let _ = recv_json(&mut ws).await;
            send(&mut ws, json!({"type": "session.updated"})).await;
            let _ = recv_json(&mut ws).await;
            send(
                &mut ws,
                json!({
                    "type": "conversation.item.input_audio_transcription.delta",
                    "item_id": "item_9",
                    "delta": "hi",
                }),
            )
            .await;
            // Any further client message other than close would be a commit.
            match ws.next().await {
                Some(Ok(Message::Close(_))) | None => {},
                other => panic!("unexpected client message: {other:?}"),
            }
        })
        .await;
        let mut session = provider(base_url, None)
            .open(StreamingSttRequest::default())
            .await
            .unwrap();
        session
            .input
            .send(StreamingSttInput::Audio(Bytes::from_static(&[0, 0])))
            .await
            .unwrap();
        assert!(matches!(
            session.events.recv().await.unwrap(),
            StreamingSttEvent::Partial { .. }
        ));
        session.input.send(StreamingSttInput::Finish).await.unwrap();
        assert_eq!(collect_events(&mut session).await, vec![
            StreamingSttEvent::Final {
                segment: "item_9".into(),
                text: "hi".into()
            },
            StreamingSttEvent::Closed,
        ]);
    }

    #[tokio::test]
    async fn provider_error_mid_stream_closes() {
        let (base_url, _) = mock_server(|mut ws| async move {
            let _ = recv_json(&mut ws).await;
            send(&mut ws, json!({"type": "session.updated"})).await;
            send(
                &mut ws,
                json!({
                    "type": "conversation.item.input_audio_transcription.failed",
                    "item_id": "item_2",
                    "error": {"message": "audio unreadable"},
                }),
            )
            .await;
            let _ = ws.next().await;
        })
        .await;
        let mut session = provider(base_url, None)
            .open(StreamingSttRequest::default())
            .await
            .unwrap();
        assert_eq!(collect_events(&mut session).await, vec![
            StreamingSttEvent::Error {
                message: "audio unreadable".into()
            },
            StreamingSttEvent::Closed,
        ]);
    }

    #[tokio::test]
    async fn final_transcript_timeout_closes_stream() {
        let (base_url, _) = mock_server(|mut ws| async move {
            let _ = recv_json(&mut ws).await;
            send(&mut ws, json!({"type": "session.updated"})).await;
            // Swallow audio and the commit, never answer.
            while let Some(Ok(message)) = ws.next().await {
                if matches!(message, Message::Close(_)) {
                    break;
                }
            }
        })
        .await;
        let mut session = provider(base_url, None)
            .with_timeouts(Duration::from_secs(5), Duration::from_millis(200))
            .open(StreamingSttRequest::default())
            .await
            .unwrap();
        session
            .input
            .send(StreamingSttInput::Audio(Bytes::from(vec![
                0u8;
                MIN_COMMIT_BYTES
            ])))
            .await
            .unwrap();
        session.input.send(StreamingSttInput::Finish).await.unwrap();
        assert_eq!(collect_events(&mut session).await, vec![
            StreamingSttEvent::Closed
        ]);
    }

    #[tokio::test]
    async fn dropping_input_cancels_stream() {
        let (base_url, _) = mock_server(|mut ws| async move {
            let _ = recv_json(&mut ws).await;
            send(&mut ws, json!({"type": "session.updated"})).await;
            let _ = ws.next().await;
        })
        .await;
        let StreamingSttSession { input, mut events } = provider(base_url, None)
            .open(StreamingSttRequest::default())
            .await
            .unwrap();
        drop(input);
        assert_eq!(events.recv().await, Some(StreamingSttEvent::Closed));
    }
}
