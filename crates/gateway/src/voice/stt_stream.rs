//! Live transcription relay between a browser WebSocket and a streaming STT
//! provider. The protocol is documented in `docs/src/voice.md`.

use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use {
    bytes::Bytes,
    serde::{Deserialize, Serialize},
    tokio::{
        sync::mpsc,
        time::{Instant, sleep_until, timeout},
    },
    tracing::{debug, info, warn},
};

use moltis_voice::{
    GeminiLiveStt, OpenAiRealtimeStt, StreamingSttEvent, StreamingSttInput, StreamingSttProvider,
    StreamingSttRequest, SttProviderId,
};

use super::{
    load_voice_config, resolve_gemini_key, resolve_openai_key, resolve_openai_whisper_base_url,
    stt_service::LiveSttService,
};

/// Largest binary audio frame accepted from the browser (about 1.3 s of
/// 24 kHz PCM16).
pub const MAX_AUDIO_FRAME_BYTES: usize = 64 * 1024;
/// Concurrent live transcription streams allowed per client IP.
pub const MAX_STREAMS_PER_IP: usize = 2;

/// Frame received from the browser socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SttStreamFrame {
    Text(String),
    Binary(Bytes),
}

/// Control message sent by the browser.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SttStreamClientMessage {
    Start {
        #[serde(default)]
        provider: Option<SttProviderId>,
        #[serde(default)]
        language: Option<String>,
        #[serde(default)]
        prompt: Option<String>,
    },
    Stop,
    Cancel,
}

/// Why a stream failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SttStreamErrorCode {
    /// The browser broke the protocol (bad message, oversized frame, ...).
    Protocol,
    /// No streaming provider, or the provider session could not be opened.
    ProviderUnavailable,
    /// The provider failed mid-stream.
    Provider,
}

impl SttStreamErrorCode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Protocol => "protocol",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::Provider => "provider",
        }
    }
}

/// Message sent to the browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SttStreamServerMessage {
    Ready {
        provider: SttProviderId,
        model: String,
        sample_rate: u32,
    },
    SpeechStarted,
    Partial {
        segment: String,
        text: String,
    },
    Final {
        segment: String,
        text: String,
    },
    Done {
        text: String,
    },
    Error {
        code: SttStreamErrorCode,
        message: String,
        /// Whether the browser should transcribe its recorded clip instead.
        fallback: bool,
    },
}

impl SttStreamServerMessage {
    fn error(code: SttStreamErrorCode, message: impl Into<String>) -> Self {
        Self::Error {
            code,
            message: message.into(),
            fallback: code != SttStreamErrorCode::Protocol,
        }
    }
}

/// Timing limits of one stream.
#[derive(Debug, Clone, Copy)]
pub struct SttStreamLimits {
    /// Wait for the `start` message.
    pub start_timeout: Duration,
    /// Hard cap on the stream; the stream is finished gracefully after it.
    pub max_duration: Duration,
    /// Finish the stream when no audio arrives for this long.
    pub idle_timeout: Duration,
    /// After `stop`, give up waiting for the provider after this long.
    pub stop_timeout: Duration,
}

impl SttStreamLimits {
    #[must_use]
    pub fn with_max_seconds(max_stream_seconds: u64) -> Self {
        Self {
            start_timeout: Duration::from_secs(10),
            max_duration: Duration::from_secs(max_stream_seconds),
            idle_timeout: Duration::from_secs(30),
            stop_timeout: Duration::from_secs(20),
        }
    }
}

/// Live transcription availability, reported by `stt.status`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamingStatus {
    pub available: bool,
    pub provider: Option<SttProviderId>,
    pub model: Option<String>,
}

/// Build the streaming provider for `id` from the current config, if that
/// provider supports live transcription and is configured.
#[must_use]
pub fn create_streaming_provider(
    cfg: &moltis_config::MoltisConfig,
    id: SttProviderId,
) -> Option<Arc<dyn StreamingSttProvider>> {
    match id {
        SttProviderId::Whisper => {
            let whisper = &cfg.voice.stt.whisper;
            if !whisper.enabled {
                return None;
            }
            let provider = OpenAiRealtimeStt::new(
                resolve_openai_key(whisper.api_key.as_ref(), cfg),
                resolve_openai_whisper_base_url(cfg),
                whisper.realtime_model.clone(),
                whisper.language.clone(),
            );
            provider
                .is_configured()
                .then(|| Arc::new(provider) as Arc<dyn StreamingSttProvider>)
        },
        SttProviderId::Gemini => {
            let gemini = &cfg.voice.stt.gemini;
            if !gemini.enabled {
                return None;
            }
            let provider =
                GeminiLiveStt::new(resolve_gemini_key(cfg), gemini.realtime_model.clone());
            provider
                .is_configured()
                .then(|| Arc::new(provider) as Arc<dyn StreamingSttProvider>)
        },
        SttProviderId::Groq
        | SttProviderId::Deepgram
        | SttProviderId::Google
        | SttProviderId::Mistral
        | SttProviderId::ElevenLabs
        | SttProviderId::VoxtralLocal
        | SttProviderId::WhisperLocal
        | SttProviderId::WhisperCli
        | SttProviderId::SherpaOnnx => None,
    }
}

/// Resolve the streaming provider for a stream request from the current
/// config. `requested` overrides the configured STT provider.
pub fn resolve_live_streaming_provider(
    requested: Option<SttProviderId>,
) -> Result<Arc<dyn StreamingSttProvider>, String> {
    let cfg = load_voice_config();
    if !cfg.voice.stt.enabled || !cfg.voice.stt.streaming {
        return Err("live transcription is disabled".into());
    }
    let id = requested
        .or_else(|| LiveSttService::resolve_provider(cfg.voice.stt.provider))
        .ok_or_else(|| "no STT provider configured".to_string())?;
    create_streaming_provider(&cfg, id)
        .ok_or_else(|| format!("STT provider '{id}' does not support live transcription"))
}

/// Live transcription availability for the active STT provider.
#[must_use]
pub fn streaming_status() -> StreamingStatus {
    let cfg = load_voice_config();
    if !cfg.voice.stt.enabled || !cfg.voice.stt.streaming {
        return StreamingStatus::default();
    }
    let Some(id) = LiveSttService::resolve_provider(cfg.voice.stt.provider) else {
        return StreamingStatus::default();
    };
    match create_streaming_provider(&cfg, id) {
        Some(provider) => StreamingStatus {
            available: true,
            provider: Some(id),
            model: Some(provider.model().to_string()),
        },
        None => StreamingStatus::default(),
    }
}

/// Configured hard cap per stream.
#[must_use]
pub fn configured_limits() -> SttStreamLimits {
    SttStreamLimits::with_max_seconds(load_voice_config().voice.stt.max_stream_seconds)
}

/// Per-IP counter of open streams, bounding provider spend.
#[derive(Debug, Default)]
pub struct SttStreamSlots {
    open: Mutex<HashMap<IpAddr, usize>>,
    max_per_ip: usize,
}

/// A held stream slot, released on drop.
#[derive(Debug)]
pub struct SttStreamSlot {
    slots: Arc<SttStreamSlots>,
    ip: IpAddr,
}

impl SttStreamSlots {
    #[must_use]
    pub fn new(max_per_ip: usize) -> Arc<Self> {
        Arc::new(Self {
            open: Mutex::new(HashMap::new()),
            max_per_ip,
        })
    }

    /// Take a slot for `ip`, or `None` when the IP already holds the maximum.
    #[must_use]
    pub fn try_acquire(self: &Arc<Self>, ip: IpAddr) -> Option<SttStreamSlot> {
        let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
        let count = open.entry(ip).or_default();
        if *count >= self.max_per_ip {
            return None;
        }
        *count += 1;
        Some(SttStreamSlot {
            slots: Arc::clone(self),
            ip,
        })
    }
}

impl Drop for SttStreamSlot {
    fn drop(&mut self) {
        let mut open = self.slots.open.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(count) = open.get_mut(&self.ip) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                open.remove(&self.ip);
            }
        }
    }
}

/// How a stream ended, for logs and metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamEnd {
    Done,
    Cancelled,
    Failed(SttStreamErrorCode),
}

impl StreamEnd {
    fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Cancelled => "cancelled",
            Self::Failed(code) => code.as_str(),
        }
    }
}

async fn send(outgoing: &mpsc::Sender<SttStreamServerMessage>, message: SttStreamServerMessage) {
    // A closed socket ends the relay on the next incoming read.
    let _ = outgoing.send(message).await;
}

async fn wait_start(
    incoming: &mut mpsc::Receiver<SttStreamFrame>,
    start_timeout: Duration,
) -> Result<(Option<SttProviderId>, StreamingSttRequest), Option<String>> {
    let frame = match timeout(start_timeout, incoming.recv()).await {
        Ok(Some(frame)) => frame,
        Ok(None) => return Err(None),
        Err(_) => return Err(Some("no start message received".into())),
    };
    let SttStreamFrame::Text(text) = frame else {
        return Err(Some("audio received before start".into()));
    };
    match serde_json::from_str::<SttStreamClientMessage>(&text) {
        Ok(SttStreamClientMessage::Start {
            provider,
            language,
            prompt,
        }) => Ok((provider, StreamingSttRequest { language, prompt })),
        Ok(SttStreamClientMessage::Cancel | SttStreamClientMessage::Stop) => Err(None),
        Err(e) => Err(Some(format!("invalid start message: {e}"))),
    }
}

/// Run one live transcription stream.
///
/// `incoming` yields browser frames and closes when the socket closes;
/// `outgoing` carries messages to the browser. `resolve` picks the provider
/// for the `start` message.
#[tracing::instrument(skip_all)]
pub async fn run_stt_stream<F>(
    mut incoming: mpsc::Receiver<SttStreamFrame>,
    outgoing: mpsc::Sender<SttStreamServerMessage>,
    resolve: F,
    limits: SttStreamLimits,
) where
    F: FnOnce(Option<SttProviderId>) -> Result<Arc<dyn StreamingSttProvider>, String>,
{
    let (requested, request) = match wait_start(&mut incoming, limits.start_timeout).await {
        Ok(start) => start,
        Err(Some(message)) => {
            send(
                &outgoing,
                SttStreamServerMessage::error(SttStreamErrorCode::Protocol, message),
            )
            .await;
            return;
        },
        Err(None) => return,
    };

    let provider = match resolve(requested) {
        Ok(provider) => provider,
        Err(message) => {
            send(
                &outgoing,
                SttStreamServerMessage::error(SttStreamErrorCode::ProviderUnavailable, message),
            )
            .await;
            return;
        },
    };
    let provider_id = provider.id();
    let model = provider.model().to_string();

    let session = match provider.open(request).await {
        Ok(session) => session,
        Err(e) => {
            warn!(provider = %provider_id, model, error = %e, "live transcription failed to open");
            record_end(
                provider_id,
                StreamEnd::Failed(SttStreamErrorCode::ProviderUnavailable),
                Duration::ZERO,
                0,
            );
            send(
                &outgoing,
                SttStreamServerMessage::error(
                    SttStreamErrorCode::ProviderUnavailable,
                    e.to_string(),
                ),
            )
            .await;
            return;
        },
    };
    send(&outgoing, SttStreamServerMessage::Ready {
        provider: provider_id,
        model: model.clone(),
        sample_rate: provider.input_sample_rate(),
    })
    .await;
    info!(provider = %provider_id, model, "live transcription started");

    let started = Instant::now();
    let (end, audio_bytes) = relay(&mut incoming, &outgoing, session, limits, started).await;
    let elapsed = started.elapsed();
    info!(
        provider = %provider_id,
        model,
        outcome = end.as_str(),
        audio_bytes,
        duration_ms = elapsed.as_millis() as u64,
        "live transcription ended"
    );
    record_end(provider_id, end, elapsed, audio_bytes);
}

async fn relay(
    incoming: &mut mpsc::Receiver<SttStreamFrame>,
    outgoing: &mpsc::Sender<SttStreamServerMessage>,
    session: moltis_voice::StreamingSttSession,
    limits: SttStreamLimits,
    started: Instant,
) -> (StreamEnd, u64) {
    let moltis_voice::StreamingSttSession { input, mut events } = session;
    let mut input = Some(input);
    let mut finals: Vec<String> = Vec::new();
    let mut audio_bytes = 0u64;
    let max_deadline = started + limits.max_duration;
    let mut idle_deadline = Instant::now() + limits.idle_timeout;
    let mut stop_deadline: Option<Instant> = None;

    let end = loop {
        let deadline = stop_deadline.unwrap_or_else(|| max_deadline.min(idle_deadline));
        tokio::select! {
            frame = incoming.recv() => match frame {
                None => break StreamEnd::Cancelled,
                Some(SttStreamFrame::Binary(audio)) => {
                    // The provider stops reading audio after `Finish`.
                    let Some(sender) = input.as_ref().filter(|_| stop_deadline.is_none()) else {
                        continue;
                    };
                    if audio.len() > MAX_AUDIO_FRAME_BYTES {
                        send(outgoing, SttStreamServerMessage::error(
                            SttStreamErrorCode::Protocol,
                            format!("audio frame larger than {MAX_AUDIO_FRAME_BYTES} bytes"),
                        )).await;
                        break StreamEnd::Failed(SttStreamErrorCode::Protocol);
                    }
                    audio_bytes += audio.len() as u64;
                    idle_deadline = Instant::now() + limits.idle_timeout;
                    if sender.send(StreamingSttInput::Audio(audio)).await.is_err() {
                        input = None;
                    }
                },
                Some(SttStreamFrame::Text(text)) => match serde_json::from_str(&text) {
                    Ok(SttStreamClientMessage::Stop) => {
                        finish(&mut input, &mut stop_deadline, limits).await;
                    },
                    Ok(SttStreamClientMessage::Cancel) => break StreamEnd::Cancelled,
                    Ok(SttStreamClientMessage::Start { .. }) | Err(_) => {
                        send(outgoing, SttStreamServerMessage::error(
                            SttStreamErrorCode::Protocol,
                            "unexpected message during a stream",
                        )).await;
                        break StreamEnd::Failed(SttStreamErrorCode::Protocol);
                    },
                },
            },
            event = events.recv() => match event {
                Some(StreamingSttEvent::SpeechStarted) => {
                    send(outgoing, SttStreamServerMessage::SpeechStarted).await;
                },
                Some(StreamingSttEvent::Partial { segment, text }) => {
                    send(outgoing, SttStreamServerMessage::Partial { segment, text }).await;
                },
                Some(StreamingSttEvent::Final { segment, text }) => {
                    if !text.trim().is_empty() {
                        finals.push(text.trim().to_string());
                    }
                    send(outgoing, SttStreamServerMessage::Final { segment, text }).await;
                },
                Some(StreamingSttEvent::Error { message }) => {
                    warn!(error = %message, "live transcription provider error");
                    send(outgoing, SttStreamServerMessage::error(SttStreamErrorCode::Provider, message)).await;
                    break StreamEnd::Failed(SttStreamErrorCode::Provider);
                },
                Some(StreamingSttEvent::Closed) | None => {
                    send(outgoing, SttStreamServerMessage::Done { text: finals.join(" ") }).await;
                    break StreamEnd::Done;
                },
            },
            () = sleep_until(deadline) => {
                if stop_deadline.is_some() {
                    warn!("live transcription provider did not close after stop");
                    send(outgoing, SttStreamServerMessage::Done { text: finals.join(" ") }).await;
                    break StreamEnd::Done;
                }
                debug!("live transcription reached its duration or idle limit");
                finish(&mut input, &mut stop_deadline, limits).await;
            },
        }
    };
    (end, audio_bytes)
}

async fn finish(
    input: &mut Option<mpsc::Sender<StreamingSttInput>>,
    stop_deadline: &mut Option<Instant>,
    limits: SttStreamLimits,
) {
    if stop_deadline.is_some() {
        return;
    }
    *stop_deadline = Some(Instant::now() + limits.stop_timeout);
    // Keep the sender: dropping it would cancel the provider stream.
    if let Some(sender) = input.as_ref() {
        let _ = sender.send(StreamingSttInput::Finish).await;
    }
}

#[cfg_attr(not(feature = "metrics"), allow(unused_variables))]
fn record_end(provider: SttProviderId, end: StreamEnd, elapsed: Duration, audio_bytes: u64) {
    #[cfg(feature = "metrics")]
    {
        use moltis_metrics::{counter, histogram, labels};
        counter!(
            moltis_metrics::voice::STT_STREAMS_TOTAL,
            labels::PROVIDER => provider.as_str(),
            labels::STATUS => end.as_str()
        )
        .increment(1);
        histogram!(
            moltis_metrics::voice::STT_STREAM_DURATION_SECONDS,
            labels::PROVIDER => provider.as_str()
        )
        .record(elapsed.as_secs_f64());
        counter!(
            moltis_metrics::voice::STT_STREAM_AUDIO_BYTES_TOTAL,
            labels::PROVIDER => provider.as_str()
        )
        .increment(audio_bytes);
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
#[path = "stt_stream/tests.rs"]
mod tests;
