//! Provider-agnostic types for live (streaming) speech-to-text.

use {anyhow::Result, async_trait::async_trait, bytes::Bytes, serde::Serialize, tokio::sync::mpsc};

use crate::config::SttProviderId;

/// Capacity of the audio and event channels of a streaming session.
pub const STREAM_CHANNEL_CAPACITY: usize = 64;

/// Options for opening a live transcription stream.
#[derive(Debug, Clone, Default)]
pub struct StreamingSttRequest {
    /// Language hint (ISO 639-1 code, e.g. "en", "fr").
    pub language: Option<String>,
    /// Optional prompt to guide transcription (context, terminology).
    pub prompt: Option<String>,
}

/// Input sent by the caller to an open stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamingSttInput {
    /// PCM16 little-endian mono audio at [`StreamingSttProvider::input_sample_rate`].
    Audio(Bytes),
    /// End of audio: flush the final transcript, then close.
    Finish,
}

/// Event emitted by an open stream.
///
/// Serialized with a `type` tag so the gateway can forward it to the browser
/// as is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum StreamingSttEvent {
    /// The provider detected the start of speech.
    SpeechStarted,
    /// Current text of an open segment. Replaces the previous partial of the
    /// same segment.
    Partial { segment: String, text: String },
    /// Settled text of a segment.
    Final { segment: String, text: String },
    /// The provider reported an error. The stream closes after it.
    Error { message: String },
    /// The stream is closed. Always the last event.
    Closed,
}

/// Handles to an open live transcription stream.
///
/// Dropping `input` without sending [`StreamingSttInput::Finish`] cancels the
/// stream without waiting for a final transcript.
#[derive(Debug)]
pub struct StreamingSttSession {
    pub input: mpsc::Sender<StreamingSttInput>,
    pub events: mpsc::Receiver<StreamingSttEvent>,
}

/// Live speech-to-text provider.
///
/// Complements the clip-based [`super::super::SttProvider`]: a provider that
/// supports both implements both traits on separate types sharing a config.
#[async_trait]
pub trait StreamingSttProvider: Send + Sync {
    /// Provider identifier, shared with the clip provider of the same service.
    fn id(&self) -> SttProviderId;

    /// Model used for live transcription.
    fn model(&self) -> &str;

    /// Sample rate (Hz) of the PCM16 mono audio the provider expects.
    fn input_sample_rate(&self) -> u32;

    /// Open a stream. Returns once the provider session is ready to receive
    /// audio, or fails if it cannot be set up.
    async fn open(&self, request: StreamingSttRequest) -> Result<StreamingSttSession>;
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_serialize_with_type_tag() {
        let partial = StreamingSttEvent::Partial {
            segment: "s1".into(),
            text: "hello".into(),
        };
        assert_eq!(
            serde_json::to_value(&partial).unwrap(),
            serde_json::json!({"type": "partial", "segment": "s1", "text": "hello"})
        );
        assert_eq!(
            serde_json::to_value(StreamingSttEvent::SpeechStarted).unwrap(),
            serde_json::json!({"type": "speechStarted"})
        );
        assert_eq!(
            serde_json::to_value(StreamingSttEvent::Closed).unwrap(),
            serde_json::json!({"type": "closed"})
        );
    }
}
