use {
    super::*,
    antre_voice::{MockStreamingStt, StreamingSttSession},
    anyhow::anyhow,
    async_trait::async_trait,
};

const START: &str = r#"{"type":"start","language":"fr"}"#;
const STOP: &str = r#"{"type":"stop"}"#;

fn limits() -> SttStreamLimits {
    SttStreamLimits {
        start_timeout: Duration::from_secs(5),
        max_duration: Duration::from_secs(60),
        idle_timeout: Duration::from_secs(30),
        stop_timeout: Duration::from_secs(5),
    }
}

fn mock_provider(
    _requested: Option<SttProviderId>,
) -> Result<Arc<dyn StreamingSttProvider>, String> {
    Ok(Arc::new(MockStreamingStt::new("bonjour à tous")))
}

struct Harness {
    frames: mpsc::Sender<SttStreamFrame>,
    messages: mpsc::Receiver<SttStreamServerMessage>,
    relay: tokio::task::JoinHandle<()>,
}

fn spawn_relay<F>(resolve: F, limits: SttStreamLimits) -> Harness
where
    F: FnOnce(Option<SttProviderId>) -> Result<Arc<dyn StreamingSttProvider>, String>
        + Send
        + 'static,
{
    let (frames, incoming) = mpsc::channel(16);
    let (outgoing, messages) = mpsc::channel(16);
    let relay = tokio::spawn(run_stt_stream(incoming, outgoing, resolve, limits));
    Harness {
        frames,
        messages,
        relay,
    }
}

impl Harness {
    async fn text(&self, text: &str) {
        self.frames
            .send(SttStreamFrame::Text(text.into()))
            .await
            .unwrap();
    }

    async fn audio(&self, len: usize) {
        self.frames
            .send(SttStreamFrame::Binary(Bytes::from(vec![0u8; len])))
            .await
            .unwrap();
    }

    async fn next(&mut self) -> SttStreamServerMessage {
        self.messages.recv().await.unwrap()
    }

    async fn finished(self) -> Vec<SttStreamServerMessage> {
        let Self {
            frames,
            mut messages,
            relay,
        } = self;
        relay.await.unwrap();
        drop(frames);
        let mut rest = Vec::new();
        while let Some(message) = messages.recv().await {
            rest.push(message);
        }
        rest
    }
}

fn error_code(message: &SttStreamServerMessage) -> Option<(SttStreamErrorCode, bool)> {
    match message {
        SttStreamServerMessage::Error { code, fallback, .. } => Some((*code, *fallback)),
        _ => None,
    }
}

#[tokio::test]
async fn relays_partials_and_final_transcript() {
    let mut h = spawn_relay(mock_provider, limits());
    h.text(START).await;
    assert_eq!(h.next().await, SttStreamServerMessage::Ready {
        provider: SttProviderId::Whisper,
        model: "mock-live".into(),
        sample_rate: 16_000,
    });
    h.audio(3200).await;
    assert_eq!(h.next().await, SttStreamServerMessage::Partial {
        segment: "mock".into(),
        text: "chunk 1".into(),
    });
    h.text(STOP).await;
    assert_eq!(h.finished().await, vec![
        SttStreamServerMessage::Final {
            segment: "mock".into(),
            text: "bonjour à tous".into(),
        },
        SttStreamServerMessage::Done {
            text: "bonjour à tous".into(),
        },
    ]);
}

#[test]
fn server_messages_use_camel_case_wire_format() {
    let ready = serde_json::to_value(SttStreamServerMessage::Ready {
        provider: SttProviderId::Whisper,
        model: "gpt-live-transcribe".into(),
        sample_rate: 24_000,
    })
    .unwrap();
    assert_eq!(
        ready,
        serde_json::json!({
            "type": "ready",
            "provider": "whisper",
            "model": "gpt-live-transcribe",
            "sampleRate": 24000,
        })
    );
    let error = serde_json::to_value(SttStreamServerMessage::error(
        SttStreamErrorCode::ProviderUnavailable,
        "down",
    ))
    .unwrap();
    assert_eq!(
        error,
        serde_json::json!({
            "type": "error",
            "code": "provider_unavailable",
            "message": "down",
            "fallback": true,
        })
    );
}

#[tokio::test]
async fn audio_before_start_is_a_protocol_error() {
    let h = spawn_relay(mock_provider, limits());
    h.audio(10).await;
    let messages = h.finished().await;
    assert_eq!(messages.len(), 1);
    assert_eq!(
        error_code(&messages[0]),
        Some((SttStreamErrorCode::Protocol, false))
    );
}

#[tokio::test]
async fn invalid_start_message_is_a_protocol_error() {
    let h = spawn_relay(mock_provider, limits());
    h.text(r#"{"type":"start","provider":"nope"}"#).await;
    let messages = h.finished().await;
    assert_eq!(
        error_code(&messages[0]),
        Some((SttStreamErrorCode::Protocol, false))
    );
}

#[tokio::test(start_paused = true)]
async fn missing_start_times_out() {
    let h = spawn_relay(mock_provider, limits());
    let messages = h.finished().await;
    assert_eq!(
        error_code(&messages[0]),
        Some((SttStreamErrorCode::Protocol, false))
    );
}

#[tokio::test]
async fn unavailable_provider_asks_for_fallback() {
    let h = spawn_relay(|_| Err("no streaming provider".to_string()), limits());
    h.text(START).await;
    let messages = h.finished().await;
    assert_eq!(
        error_code(&messages[0]),
        Some((SttStreamErrorCode::ProviderUnavailable, true))
    );
}

struct FailingOpen;

#[async_trait]
impl StreamingSttProvider for FailingOpen {
    fn id(&self) -> SttProviderId {
        SttProviderId::Whisper
    }

    fn model(&self) -> &str {
        "broken"
    }

    fn input_sample_rate(&self) -> u32 {
        24_000
    }

    async fn open(&self, _request: StreamingSttRequest) -> anyhow::Result<StreamingSttSession> {
        Err(anyhow!("connection refused"))
    }
}

#[tokio::test]
async fn provider_open_failure_asks_for_fallback() {
    let h = spawn_relay(|_| Ok(Arc::new(FailingOpen) as Arc<_>), limits());
    h.text(START).await;
    let messages = h.finished().await;
    assert_eq!(
        error_code(&messages[0]),
        Some((SttStreamErrorCode::ProviderUnavailable, true))
    );
}

/// Emits one scripted event per audio chunk, then never closes on its own.
struct Scripted(Vec<StreamingSttEvent>);

#[async_trait]
impl StreamingSttProvider for Scripted {
    fn id(&self) -> SttProviderId {
        SttProviderId::Whisper
    }

    fn model(&self) -> &str {
        "scripted"
    }

    fn input_sample_rate(&self) -> u32 {
        24_000
    }

    async fn open(&self, _request: StreamingSttRequest) -> anyhow::Result<StreamingSttSession> {
        let (input, mut input_rx) = mpsc::channel(8);
        let (event_tx, events) = mpsc::channel(8);
        let mut script = self.0.clone().into_iter();
        tokio::spawn(async move {
            while let Some(StreamingSttInput::Audio(_)) = input_rx.recv().await {
                if let Some(event) = script.next() {
                    let _ = event_tx.send(event).await;
                }
            }
            // Hold the event sender until the relay gives up.
            tokio::time::sleep(Duration::from_secs(3600)).await;
            drop(event_tx);
        });
        Ok(StreamingSttSession { input, events })
    }
}

#[tokio::test]
async fn provider_error_mid_stream_asks_for_fallback() {
    let provider = Scripted(vec![StreamingSttEvent::Error {
        message: "quota exceeded".into(),
    }]);
    let mut h = spawn_relay(move |_| Ok(Arc::new(provider) as Arc<_>), limits());
    h.text(START).await;
    assert!(matches!(
        h.next().await,
        SttStreamServerMessage::Ready { .. }
    ));
    h.audio(10).await;
    let messages = h.finished().await;
    assert_eq!(
        error_code(&messages[0]),
        Some((SttStreamErrorCode::Provider, true))
    );
}

#[tokio::test(start_paused = true)]
async fn stop_timeout_sends_done_with_finals_so_far() {
    let provider = Scripted(vec![StreamingSttEvent::Final {
        segment: "a".into(),
        text: " first ".into(),
    }]);
    let mut h = spawn_relay(move |_| Ok(Arc::new(provider) as Arc<_>), limits());
    h.text(START).await;
    assert!(matches!(
        h.next().await,
        SttStreamServerMessage::Ready { .. }
    ));
    h.audio(10).await;
    assert!(matches!(
        h.next().await,
        SttStreamServerMessage::Final { .. }
    ));
    h.text(STOP).await;
    assert_eq!(h.finished().await, vec![SttStreamServerMessage::Done {
        text: "first".into()
    }]);
}

#[tokio::test(start_paused = true)]
async fn max_duration_finishes_the_stream() {
    let mut h = spawn_relay(mock_provider, SttStreamLimits {
        max_duration: Duration::from_secs(1),
        ..limits()
    });
    h.text(START).await;
    assert!(matches!(
        h.next().await,
        SttStreamServerMessage::Ready { .. }
    ));
    let messages = h.finished().await;
    assert_eq!(
        messages.last(),
        Some(&SttStreamServerMessage::Done {
            text: "bonjour à tous".into()
        })
    );
}

#[tokio::test]
async fn oversized_frame_is_rejected() {
    let mut h = spawn_relay(mock_provider, limits());
    h.text(START).await;
    assert!(matches!(
        h.next().await,
        SttStreamServerMessage::Ready { .. }
    ));
    h.audio(MAX_AUDIO_FRAME_BYTES + 1).await;
    let messages = h.finished().await;
    assert_eq!(
        error_code(&messages[0]),
        Some((SttStreamErrorCode::Protocol, false))
    );
}

#[tokio::test]
async fn cancel_ends_without_done() {
    let mut h = spawn_relay(mock_provider, limits());
    h.text(START).await;
    assert!(matches!(
        h.next().await,
        SttStreamServerMessage::Ready { .. }
    ));
    h.text(r#"{"type":"cancel"}"#).await;
    assert!(h.finished().await.is_empty());
}

#[test]
fn slots_cap_streams_per_ip() {
    let slots = SttStreamSlots::new(2);
    let ip: IpAddr = "10.0.0.1".parse().unwrap();
    let other: IpAddr = "10.0.0.2".parse().unwrap();
    let first = slots.try_acquire(ip).unwrap();
    let _second = slots.try_acquire(ip).unwrap();
    assert!(slots.try_acquire(ip).is_none());
    assert!(slots.try_acquire(other).is_some());
    drop(first);
    assert!(slots.try_acquire(ip).is_some());
}

#[test]
fn openai_and_gemini_support_streaming() {
    let mut cfg = antre_config::AntreConfig::default();
    cfg.voice.stt.whisper.api_key = Some(secrecy::Secret::new("sk-test".into()));
    let provider = create_streaming_provider(&cfg, SttProviderId::Whisper).unwrap();
    assert_eq!(provider.model(), antre_voice::OPENAI_DEFAULT_REALTIME_MODEL);
    assert_eq!(provider.input_sample_rate(), 24_000);

    cfg.voice.stt.whisper.realtime_model = Some("gpt-realtime-whisper".into());
    let provider = create_streaming_provider(&cfg, SttProviderId::Whisper).unwrap();
    assert_eq!(provider.model(), "gpt-realtime-whisper");

    cfg.voice.stt.whisper.enabled = false;
    assert!(create_streaming_provider(&cfg, SttProviderId::Whisper).is_none());

    cfg.voice.stt.gemini.api_key = Some(secrecy::Secret::new("gm-test".into()));
    let provider = create_streaming_provider(&cfg, SttProviderId::Gemini).unwrap();
    assert_eq!(provider.model(), antre_voice::GEMINI_DEFAULT_REALTIME_MODEL);
    assert_eq!(provider.input_sample_rate(), 16_000);
    cfg.voice.stt.gemini.enabled = false;
    assert!(create_streaming_provider(&cfg, SttProviderId::Gemini).is_none());
    assert!(create_streaming_provider(&cfg, SttProviderId::Groq).is_none());
}

#[tokio::test]
async fn audio_after_stop_is_ignored() {
    let mut h = spawn_relay(mock_provider, limits());
    h.text(START).await;
    assert!(matches!(
        h.next().await,
        SttStreamServerMessage::Ready { .. }
    ));
    h.text(STOP).await;
    h.audio(10).await;
    let messages = h.finished().await;
    assert!(
        messages
            .iter()
            .all(|m| !matches!(m, SttStreamServerMessage::Partial { .. })),
        "{messages:?}"
    );
    assert_eq!(
        messages.last(),
        Some(&SttStreamServerMessage::Done {
            text: "bonjour à tous".into()
        })
    );
}
