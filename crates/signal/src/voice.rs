//! Inbound Signal voice-note transcription.

use moltis_channels::{ChannelEventSink, InboundMediaDownloader, InboundMediaSource};

use crate::attachments::SignalAttachment;

/// Matches the upload cap of the common hosted STT APIs (OpenAI Whisper).
pub(crate) const MAX_VOICE_NOTE_BYTES: usize = 25 * 1024 * 1024;

pub(crate) const VOICE_REPLY_EMPTY_TRANSCRIPTION: &str =
    "I couldn't hear anything in that voice message. Could you try again or type it out?";
pub(crate) const VOICE_REPLY_TRANSCRIPTION_FAILED: &str =
    "I couldn't transcribe your voice message. Could you try again or type it out?";
pub(crate) const VOICE_REPLY_DOWNLOAD_FAILED: &str =
    "I couldn't download your voice message. Please try again.";
pub(crate) const VOICE_REPLY_TOO_LARGE: &str = "Your voice message is too long for me to transcribe. Could you send a shorter one or type it out?";
pub(crate) const VOICE_REPLY_STT_SETUP_HINT: &str =
    "I can't understand voice, you did not configure it, please visit Settings -> Voice";

impl SignalAttachment {
    /// Short STT format name for audio attachments, `None` for anything else.
    pub(crate) fn audio_format(&self) -> Option<&'static str> {
        let content_type = self.content_type.as_deref().map(|ct| {
            ct.split(';')
                .next()
                .unwrap_or(ct)
                .trim()
                .to_ascii_lowercase()
        });
        let from_content_type = content_type.as_deref().and_then(|ct| match ct {
            "audio/aac" => Some("aac"),
            "audio/mp4" | "audio/m4a" | "audio/x-m4a" => Some("m4a"),
            "audio/ogg" | "audio/opus" => Some("ogg"),
            "audio/mpeg" | "audio/mp3" => Some("mp3"),
            "audio/wav" | "audio/x-wav" | "audio/wave" => Some("wav"),
            "audio/webm" => Some("webm"),
            _ => None,
        });
        if from_content_type.is_some() {
            return from_content_type;
        }
        // Unknown or missing content type: only trust the extension when the
        // content type does not claim something non-audio.
        if content_type
            .as_deref()
            .is_some_and(|ct| !ct.starts_with("audio/") && ct != "application/octet-stream")
        {
            return None;
        }
        let extension = self
            .filename
            .as_deref()
            .and_then(|name| name.rsplit_once('.'))
            .map(|(_, ext)| ext.to_ascii_lowercase())?;
        match extension.as_str() {
            "aac" => Some("aac"),
            "m4a" | "mp4" => Some("m4a"),
            "ogg" | "oga" | "opus" => Some("ogg"),
            "mp3" => Some("mp3"),
            "wav" => Some("wav"),
            "webm" => Some("webm"),
            _ => None,
        }
    }
}

/// A voice note ready to be downloaded and transcribed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VoiceNote {
    pub attachment_id: String,
    pub format: &'static str,
    pub size: Option<u64>,
}

pub(crate) fn find_voice_note(attachments: &[SignalAttachment]) -> Option<VoiceNote> {
    attachments.iter().find_map(|attachment| {
        let format = attachment.audio_format()?;
        let attachment_id = attachment.id.clone().filter(|id| !id.trim().is_empty())?;
        Some(VoiceNote {
            attachment_id,
            format,
            size: attachment.size,
        })
    })
}

/// Raw audio kept so the gateway can store it in the session media directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VoiceAudio {
    pub data: Vec<u8>,
    pub format: &'static str,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum VoiceOutcome {
    /// Send `body` to the agent.
    Dispatch {
        body: String,
        audio: Option<VoiceAudio>,
    },
    /// Do not reach the agent; send this text straight back to the sender.
    Reply(&'static str),
}

/// Downloads, size-checks and transcribes a voice note.
///
/// `caption` is the text sent alongside the note. When transcription is not
/// possible and a caption exists, the caption alone goes to the agent;
/// otherwise the sender gets a direct explanation.
#[tracing::instrument(skip_all, fields(account_id = %account_id, format = voice.format))]
pub(crate) async fn transcribe_voice_note(
    voice: &VoiceNote,
    caption: &str,
    downloader: &dyn InboundMediaDownloader,
    sink: &dyn ChannelEventSink,
    account_id: &str,
) -> VoiceOutcome {
    let caption = caption.trim();
    let fallback = |reply: &'static str, audio: Option<VoiceAudio>| {
        if caption.is_empty() {
            VoiceOutcome::Reply(reply)
        } else {
            VoiceOutcome::Dispatch {
                body: caption.to_string(),
                audio,
            }
        }
    };

    if !sink.voice_stt_available().await {
        return fallback(VOICE_REPLY_STT_SETUP_HINT, None);
    }

    if voice
        .size
        .is_some_and(|size| size > MAX_VOICE_NOTE_BYTES as u64)
    {
        tracing::warn!(account_id, size = voice.size, "Signal voice note too large");
        return fallback(VOICE_REPLY_TOO_LARGE, None);
    }

    let source = InboundMediaSource::AttachmentId {
        id: voice.attachment_id.clone(),
    };
    let data = match downloader
        .download_media(&source, MAX_VOICE_NOTE_BYTES)
        .await
    {
        Ok(data) => data,
        Err(e) => {
            tracing::warn!(account_id, error = %e, "failed to download Signal voice note");
            return fallback(VOICE_REPLY_DOWNLOAD_FAILED, None);
        },
    };

    tracing::debug!(
        account_id,
        size = data.len(),
        "transcribing Signal voice note"
    );
    let transcription = sink.transcribe_voice(&data, voice.format).await;
    let audio = Some(VoiceAudio {
        data,
        format: voice.format,
    });
    match transcription {
        Ok(text) if text.trim().is_empty() => {
            tracing::warn!(account_id, "Signal voice transcription returned empty text");
            fallback(VOICE_REPLY_EMPTY_TRANSCRIPTION, audio)
        },
        Ok(text) => {
            let text = text.trim();
            let body = if caption.is_empty() {
                text.to_string()
            } else {
                format!("{caption}\n\n[Voice message]: {text}")
            };
            VoiceOutcome::Dispatch { body, audio }
        },
        Err(e) => {
            tracing::warn!(account_id, error = %e, "Signal voice transcription failed");
            fallback(VOICE_REPLY_TRANSCRIPTION_FAILED, audio)
        },
    }
}

#[cfg(test)]
#[path = "voice_tests.rs"]
mod tests;
