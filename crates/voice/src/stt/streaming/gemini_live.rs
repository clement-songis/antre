//! Gemini Live API live transcription.
//!
//! Opens a `BidiGenerateContent` session on a transcription model with input
//! audio transcription enabled. The server segments speech into utterances
//! with its own voice activity detection.

use std::time::Duration;

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
            http::{HeaderName, HeaderValue},
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

const LIVE_URL: &str = "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent";

/// Default Gemini live transcription model.
pub const DEFAULT_REALTIME_MODEL: &str = "gemini-3.5-transcribe-live";

const SAMPLE_RATE: u32 = 16_000;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// After `audioStreamEnd` the server closes the utterance in about 0.5 s.
const FINAL_TIMEOUT: Duration = Duration::from_millis(2500);

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Gemini Live API live transcription provider.
pub struct GeminiLiveStt {
    api_key: Option<Secret<String>>,
    url: String,
    model: String,
    connect_timeout: Duration,
    final_timeout: Duration,
}

impl std::fmt::Debug for GeminiLiveStt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeminiLiveStt")
            .field("api_key", &"[REDACTED]")
            .field("url", &self.url)
            .field("model", &self.model)
            .finish()
    }
}

impl GeminiLiveStt {
    #[must_use]
    pub fn new(api_key: Option<Secret<String>>, model: Option<String>) -> Self {
        Self {
            api_key,
            url: LIVE_URL.into(),
            model: model
                .filter(|m| !m.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_REALTIME_MODEL.into()),
            connect_timeout: CONNECT_TIMEOUT,
            final_timeout: FINAL_TIMEOUT,
        }
    }

    /// Override the Live API WebSocket URL (tests, proxies).
    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// Override how long to wait for the session to open and for the last
    /// utterance after [`StreamingSttInput::Finish`].
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

    fn setup_message(&self) -> Value {
        json!({
            "setup": {
                "model": format!("models/{}", self.model),
                "inputAudioTranscription": {},
            }
        })
    }

    async fn connect(&self) -> Result<WsStream> {
        let key = self
            .api_key
            .as_ref()
            .ok_or_else(|| anyhow!("Gemini API key not configured"))?;
        let mut request = self.url.as_str().into_client_request()?;
        // Header rather than `?key=` so the key stays out of URLs and logs.
        request.headers_mut().insert(
            HeaderName::from_static("x-goog-api-key"),
            HeaderValue::from_str(key.expose_secret())?,
        );
        let (mut ws, _) = connect_async(request)
            .await
            .context("Gemini Live connection failed")?;
        ws.send(Message::text(self.setup_message().to_string()))
            .await?;
        wait_setup_complete(&mut ws).await?;
        Ok(ws)
    }
}

#[async_trait]
impl StreamingSttProvider for GeminiLiveStt {
    fn id(&self) -> SttProviderId {
        SttProviderId::Gemini
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn input_sample_rate(&self) -> u32 {
        SAMPLE_RATE
    }

    async fn open(&self, _request: StreamingSttRequest) -> Result<StreamingSttSession> {
        // The transcription model detects the language and takes no prompt.
        let ws = timeout(self.connect_timeout, self.connect())
            .await
            .context("timed out opening the Gemini Live session")??;
        debug!(model = %self.model, "Gemini Live transcription session ready");
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
struct Transcription {
    #[serde(default)]
    text: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerContent {
    #[serde(default)]
    interim_input_transcription: Option<Transcription>,
    #[serde(default)]
    input_transcription: Option<Transcription>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum ActivityType {
    ActivityStart,
    ActivityEnd,
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct VoiceActivity {
    #[serde(rename = "type")]
    kind: ActivityType,
}

#[derive(Debug, Default, Deserialize)]
struct ApiError {
    #[serde(default)]
    message: String,
}

/// One Live API server message; Gemini sets one of these fields per message.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerMessage {
    #[serde(default)]
    setup_complete: Option<Value>,
    #[serde(default)]
    server_content: Option<ServerContent>,
    #[serde(default)]
    voice_activity: Option<VoiceActivity>,
    #[serde(default)]
    error: Option<ApiError>,
}

fn parse_message(message: &Message) -> Option<ServerMessage> {
    // Gemini sends JSON in binary frames.
    let text = match message {
        Message::Text(text) => text.as_str(),
        Message::Binary(bytes) => std::str::from_utf8(bytes).ok()?,
        _ => return None,
    };
    match serde_json::from_str(text) {
        Ok(message) => Some(message),
        Err(e) => {
            debug!(error = %e, "ignoring unparsable Gemini Live message");
            None
        },
    }
}

async fn wait_setup_complete(ws: &mut WsStream) -> Result<()> {
    while let Some(message) = ws.next().await {
        let message = message?;
        if let Message::Close(frame) = &message {
            let reason = frame
                .as_ref()
                .map(|f| f.reason.to_string())
                .unwrap_or_default();
            bail!("Gemini Live closed the session: {reason}");
        }
        let Some(parsed) = parse_message(&message) else {
            continue;
        };
        if let Some(error) = parsed.error {
            bail!("Gemini Live session rejected: {}", error.message);
        }
        if parsed.setup_complete.is_some() {
            return Ok(());
        }
    }
    bail!("Gemini Live connection closed before setup completed")
}

/// Utterance being transcribed. `interim` is the utterance so far and
/// replaces itself; `settled` accumulates settled chunks.
#[derive(Default)]
struct Utterance {
    index: usize,
    settled: String,
    interim: String,
}

impl Utterance {
    fn segment(&self) -> String {
        format!("u{}", self.index)
    }

    fn text(&self) -> &str {
        let settled = self.settled.trim();
        if settled.is_empty() {
            self.interim.trim()
        } else {
            settled
        }
    }

    fn partial(&self) -> Option<StreamingSttEvent> {
        let text = self.text();
        (!text.is_empty()).then(|| StreamingSttEvent::Partial {
            segment: self.segment(),
            text: text.to_string(),
        })
    }

    /// Settle the utterance and move to the next one.
    fn flush(&mut self) -> Option<StreamingSttEvent> {
        let text = self.text().to_string();
        let event = (!text.is_empty()).then(|| StreamingSttEvent::Final {
            segment: self.segment(),
            text,
        });
        self.index += 1;
        self.settled.clear();
        self.interim.clear();
        event
    }
}

/// Events to forward for a server message, and whether the stream must stop.
fn apply(
    utterance: &mut Utterance,
    message: ServerMessage,
    finishing: bool,
) -> (Vec<StreamingSttEvent>, bool) {
    if let Some(error) = message.error {
        return (
            vec![StreamingSttEvent::Error {
                message: error.message,
            }],
            true,
        );
    }
    let mut events = Vec::new();
    if let Some(content) = message.server_content {
        let mut changed = false;
        if let Some(interim) = content.interim_input_transcription
            && !interim.text.is_empty()
        {
            utterance.interim = interim.text;
            changed = true;
        }
        if let Some(settled) = content.input_transcription
            && !settled.text.is_empty()
        {
            utterance.settled.push_str(&settled.text);
            changed = true;
        }
        if changed {
            events.extend(utterance.partial());
        }
    }
    let mut stop = false;
    match message.voice_activity.map(|activity| activity.kind) {
        Some(ActivityType::ActivityStart) => events.push(StreamingSttEvent::SpeechStarted),
        Some(ActivityType::ActivityEnd) => {
            events.extend(utterance.flush());
            stop = finishing;
        },
        Some(ActivityType::Other) | None => {},
    }
    (events, stop)
}

async fn run_stream(
    mut ws: WsStream,
    mut input: mpsc::Receiver<StreamingSttInput>,
    events: mpsc::Sender<StreamingSttEvent>,
    final_timeout: Duration,
) {
    let mut utterance = Utterance {
        index: 1,
        ..Utterance::default()
    };
    let mut finishing = false;
    let mut deadline = Instant::now();
    let mut tail = Vec::new();

    'stream: loop {
        tokio::select! {
            next = input.recv(), if !finishing => match next {
                Some(StreamingSttInput::Audio(audio)) => {
                    let payload = json!({
                        "realtimeInput": {
                            "audio": {
                                "mimeType": format!("audio/pcm;rate={SAMPLE_RATE}"),
                                "data": base64::engine::general_purpose::STANDARD.encode(&audio),
                            }
                        }
                    });
                    if let Err(e) = ws.send(Message::text(payload.to_string())).await {
                        tail.push(StreamingSttEvent::Error { message: e.to_string() });
                        break;
                    }
                },
                Some(StreamingSttInput::Finish) => {
                    finishing = true;
                    deadline = Instant::now() + final_timeout;
                    let end = json!({ "realtimeInput": { "audioStreamEnd": true } });
                    if let Err(e) = ws.send(Message::text(end.to_string())).await {
                        tail.extend(utterance.flush());
                        tail.push(StreamingSttEvent::Error { message: e.to_string() });
                        break;
                    }
                },
                None => {
                    debug!("Gemini Live stream cancelled by caller");
                    break;
                },
            },
            message = ws.next() => match message {
                Some(Ok(Message::Close(_))) | None => {
                    tail.extend(utterance.flush());
                    break;
                },
                Some(Ok(message)) => {
                    let Some(parsed) = parse_message(&message) else { continue };
                    let (forward, stop) = apply(&mut utterance, parsed, finishing);
                    for event in forward {
                        if events.send(event).await.is_err() {
                            break 'stream;
                        }
                    }
                    if stop {
                        break;
                    }
                },
                Some(Err(e)) => {
                    tail.push(StreamingSttEvent::Error { message: e.to_string() });
                    break;
                },
            },
            () = sleep_until(deadline), if finishing => {
                warn!("Gemini Live did not end the last utterance in time");
                tail.extend(utterance.flush());
                break;
            },
        }
    }

    let _ = ws.close(None).await;
    tail.push(StreamingSttEvent::Closed);
    for event in tail {
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

    /// Accept one WebSocket connection, record its API key header, then run
    /// `script` on it.
    // tungstenite's handshake callback fixes the large `Err` type.
    #[allow(clippy::result_large_err)]
    async fn mock_server<F, Fut>(script: F) -> (String, Arc<Mutex<Option<String>>>)
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
                *seen_in_server.lock().unwrap() = req
                    .headers()
                    .get("x-goog-api-key")
                    .and_then(|v| v.to_str().ok())
                    .map(String::from);
                Ok(resp)
            })
            .await
            .unwrap();
            script(ws).await;
        });
        (format!("ws://{addr}/live"), seen)
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

    /// Gemini answers with JSON in binary frames.
    async fn send(ws: &mut ServerWs, value: Value) {
        ws.send(Message::binary(value.to_string().into_bytes()))
            .await
            .unwrap();
    }

    async fn drain_until_close(ws: &mut ServerWs) {
        while let Some(Ok(message)) = ws.next().await {
            if matches!(message, Message::Close(_)) {
                break;
            }
        }
    }

    fn provider(url: String) -> GeminiLiveStt {
        GeminiLiveStt::new(Some(Secret::new("gm-test".into())), None)
            .with_url(url)
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

    fn partial(segment: &str, text: &str) -> StreamingSttEvent {
        StreamingSttEvent::Partial {
            segment: segment.into(),
            text: text.into(),
        }
    }

    fn final_text(segment: &str, text: &str) -> StreamingSttEvent {
        StreamingSttEvent::Final {
            segment: segment.into(),
            text: text.into(),
        }
    }

    #[test]
    fn defaults_and_configuration() {
        let p = GeminiLiveStt::new(None, Some(" ".into()));
        assert_eq!(p.model(), DEFAULT_REALTIME_MODEL);
        assert_eq!(p.input_sample_rate(), 16_000);
        assert_eq!(p.id(), SttProviderId::Gemini);
        assert!(!p.is_configured());
        assert_eq!(
            p.setup_message(),
            json!({"setup": {"model": "models/gemini-3.5-transcribe-live", "inputAudioTranscription": {}}})
        );
        let debug = format!(
            "{:?}",
            GeminiLiveStt::new(Some(Secret::new("gm-secret".into())), None)
        );
        assert!(!debug.contains("gm-secret"));
    }

    #[test]
    fn utterances_prefer_settled_text_and_advance() {
        let mut utterance = Utterance {
            index: 1,
            ..Utterance::default()
        };
        let interim = ServerMessage {
            server_content: Some(ServerContent {
                interim_input_transcription: Some(Transcription {
                    text: "bonjour tout".into(),
                }),
                input_transcription: None,
            }),
            ..ServerMessage::default()
        };
        assert_eq!(
            apply(&mut utterance, interim, false),
            (vec![partial("u1", "bonjour tout")], false)
        );
        let settled = ServerMessage {
            server_content: Some(ServerContent {
                interim_input_transcription: None,
                input_transcription: Some(Transcription {
                    text: "Bonjour tout le monde.".into(),
                }),
            }),
            ..ServerMessage::default()
        };
        assert_eq!(
            apply(&mut utterance, settled, false),
            (vec![partial("u1", "Bonjour tout le monde.")], false)
        );
        let end = ServerMessage {
            voice_activity: Some(VoiceActivity {
                kind: ActivityType::ActivityEnd,
            }),
            ..ServerMessage::default()
        };
        assert_eq!(
            apply(&mut utterance, end, true),
            (vec![final_text("u1", "Bonjour tout le monde.")], true)
        );
        assert_eq!(utterance.segment(), "u2");
        assert!(utterance.flush().is_none());
    }

    #[tokio::test]
    async fn open_fails_without_api_key() {
        let err = GeminiLiveStt::new(None, None)
            .open(StreamingSttRequest::default())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("API key"), "{err}");
    }

    #[tokio::test]
    async fn streams_utterances_and_flushes_on_finish() {
        let (url, seen) = mock_server(|mut ws| async move {
            let setup = recv_json(&mut ws).await;
            assert_eq!(setup["setup"]["model"], "models/gemini-3.5-transcribe-live");
            send(&mut ws, json!({"setupComplete": {}})).await;

            let audio = recv_json(&mut ws).await;
            assert_eq!(
                audio["realtimeInput"]["audio"]["mimeType"],
                "audio/pcm;rate=16000"
            );
            let data = base64::engine::general_purpose::STANDARD
                .decode(audio["realtimeInput"]["audio"]["data"].as_str().unwrap())
                .unwrap();
            assert_eq!(data, vec![1, 2, 3, 4]);

            send(
                &mut ws,
                json!({"voiceActivity": {"type": "ACTIVITY_START"}}),
            )
            .await;
            send(
                &mut ws,
                json!({"serverContent": {"interimInputTranscription": {"text": "first"}}}),
            )
            .await;
            send(
                &mut ws,
                json!({"serverContent": {"inputTranscription": {"text": "First one."}}}),
            )
            .await;
            send(&mut ws, json!({"voiceActivity": {"type": "ACTIVITY_END"}})).await;
            send(
                &mut ws,
                json!({"serverContent": {"interimInputTranscription": {"text": "second"}}}),
            )
            .await;

            let end = recv_json(&mut ws).await;
            assert_eq!(end, json!({"realtimeInput": {"audioStreamEnd": true}}));
            send(
                &mut ws,
                json!({"serverContent": {"inputTranscription": {"text": "Second one."}}}),
            )
            .await;
            send(&mut ws, json!({"voiceActivity": {"type": "ACTIVITY_END"}})).await;
            drain_until_close(&mut ws).await;
        })
        .await;

        let mut session = provider(url)
            .open(StreamingSttRequest::default())
            .await
            .unwrap();
        assert_eq!(seen.lock().unwrap().as_deref(), Some("gm-test"));
        session
            .input
            .send(StreamingSttInput::Audio(Bytes::from_static(&[1, 2, 3, 4])))
            .await
            .unwrap();
        let mut events = Vec::new();
        for _ in 0..5 {
            events.push(session.events.recv().await.unwrap());
        }
        assert_eq!(events, vec![
            StreamingSttEvent::SpeechStarted,
            partial("u1", "first"),
            partial("u1", "First one."),
            final_text("u1", "First one."),
            partial("u2", "second"),
        ]);
        session.input.send(StreamingSttInput::Finish).await.unwrap();
        assert_eq!(collect_events(&mut session).await, vec![
            partial("u2", "Second one."),
            final_text("u2", "Second one."),
            StreamingSttEvent::Closed,
        ]);
    }

    #[tokio::test]
    async fn open_fails_when_setup_is_rejected() {
        let (url, _) = mock_server(|mut ws| async move {
            let _ = recv_json(&mut ws).await;
            send(&mut ws, json!({"error": {"message": "model not found"}})).await;
        })
        .await;
        let err = provider(url)
            .open(StreamingSttRequest::default())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("model not found"), "{err}");
    }

    #[tokio::test]
    async fn open_times_out_without_setup_complete() {
        let (url, _) = mock_server(|mut ws| async move {
            let _ = recv_json(&mut ws).await;
            tokio::time::sleep(Duration::from_secs(5)).await;
        })
        .await;
        let err = provider(url)
            .with_timeouts(Duration::from_millis(200), Duration::from_secs(1))
            .open(StreamingSttRequest::default())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("timed out"), "{err}");
    }

    #[tokio::test]
    async fn finish_timeout_settles_the_open_utterance() {
        let (url, _) = mock_server(|mut ws| async move {
            let _ = recv_json(&mut ws).await;
            send(&mut ws, json!({"setupComplete": {}})).await;
            send(
                &mut ws,
                json!({"serverContent": {"interimInputTranscription": {"text": "cut short"}}}),
            )
            .await;
            drain_until_close(&mut ws).await;
        })
        .await;
        let mut session = provider(url)
            .with_timeouts(Duration::from_secs(5), Duration::from_millis(200))
            .open(StreamingSttRequest::default())
            .await
            .unwrap();
        assert_eq!(
            session.events.recv().await,
            Some(partial("u1", "cut short"))
        );
        session.input.send(StreamingSttInput::Finish).await.unwrap();
        assert_eq!(collect_events(&mut session).await, vec![
            final_text("u1", "cut short"),
            StreamingSttEvent::Closed,
        ]);
    }

    #[tokio::test]
    async fn provider_error_mid_stream_closes() {
        let (url, _) = mock_server(|mut ws| async move {
            let _ = recv_json(&mut ws).await;
            send(&mut ws, json!({"setupComplete": {}})).await;
            send(&mut ws, json!({"error": {"message": "quota exceeded"}})).await;
            drain_until_close(&mut ws).await;
        })
        .await;
        let mut session = provider(url)
            .open(StreamingSttRequest::default())
            .await
            .unwrap();
        assert_eq!(collect_events(&mut session).await, vec![
            StreamingSttEvent::Error {
                message: "quota exceeded".into()
            },
            StreamingSttEvent::Closed,
        ]);
    }

    #[tokio::test]
    async fn server_close_settles_the_open_utterance() {
        let (url, _) = mock_server(|mut ws| async move {
            let _ = recv_json(&mut ws).await;
            send(&mut ws, json!({"setupComplete": {}})).await;
            send(
                &mut ws,
                json!({"serverContent": {"inputTranscription": {"text": "kept"}}}),
            )
            .await;
            ws.close(None).await.unwrap();
        })
        .await;
        let mut session = provider(url)
            .open(StreamingSttRequest::default())
            .await
            .unwrap();
        assert_eq!(collect_events(&mut session).await, vec![
            partial("u1", "kept"),
            final_text("u1", "kept"),
            StreamingSttEvent::Closed,
        ]);
    }
}
