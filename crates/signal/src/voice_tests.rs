use std::sync::Mutex;

use {
    antre_channels::{
        ChannelEvent, ChannelEventSink, ChannelMessageMeta, ChannelReplyTarget, Error,
        InboundMediaDownloader, InboundMediaSource, Result,
    },
    async_trait::async_trait,
    serde_json::json,
};

use {
    super::*,
    crate::attachments::{SignalAttachment, parse_attachments},
};

enum Transcription {
    Text(&'static str),
    Failure,
}

struct MockSink {
    stt_available: bool,
    transcription: Transcription,
    transcribed_formats: Mutex<Vec<String>>,
}

impl MockSink {
    fn new(stt_available: bool, transcription: Transcription) -> Self {
        Self {
            stt_available,
            transcription,
            transcribed_formats: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl ChannelEventSink for MockSink {
    async fn emit(&self, _event: ChannelEvent) {}

    async fn dispatch_to_chat(
        &self,
        _text: &str,
        _reply_to: ChannelReplyTarget,
        _meta: ChannelMessageMeta,
    ) {
    }

    async fn dispatch_command(
        &self,
        _command: &str,
        _reply_to: ChannelReplyTarget,
        _sender_id: Option<&str>,
    ) -> Result<String> {
        Ok(String::new())
    }

    async fn request_disable_account(&self, _channel_type: &str, _account_id: &str, _reason: &str) {
    }

    async fn transcribe_voice(&self, _audio_data: &[u8], format: &str) -> Result<String> {
        self.transcribed_formats
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(format.to_string());
        match self.transcription {
            Transcription::Text(text) => Ok(text.to_string()),
            Transcription::Failure => Err(Error::unavailable("stt down")),
        }
    }

    async fn voice_stt_available(&self) -> bool {
        self.stt_available
    }
}

struct MockDownloader {
    result: std::result::Result<Vec<u8>, &'static str>,
    requested: Mutex<Vec<(InboundMediaSource, usize)>>,
}

impl MockDownloader {
    fn ok(data: &[u8]) -> Self {
        Self {
            result: Ok(data.to_vec()),
            requested: Mutex::new(Vec::new()),
        }
    }

    fn failing() -> Self {
        Self {
            result: Err("daemon unreachable"),
            requested: Mutex::new(Vec::new()),
        }
    }

    fn request_count(&self) -> usize {
        self.requested
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }
}

#[async_trait]
impl InboundMediaDownloader for MockDownloader {
    async fn download_media(
        &self,
        source: &InboundMediaSource,
        max_bytes: usize,
    ) -> Result<Vec<u8>> {
        self.requested
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((source.clone(), max_bytes));
        self.result.clone().map_err(Error::unavailable)
    }
}

fn voice_note() -> VoiceNote {
    VoiceNote {
        attachment_id: "att-1".to_string(),
        format: "aac",
        size: Some(1024),
    }
}

fn attachment(content_type: Option<&str>, filename: Option<&str>) -> SignalAttachment {
    SignalAttachment {
        id: Some("att".to_string()),
        content_type: content_type.map(ToString::to_string),
        filename: filename.map(ToString::to_string),
        size: None,
    }
}

#[test]
fn audio_format_maps_content_types() {
    assert_eq!(
        attachment(Some("audio/aac"), None).audio_format(),
        Some("aac")
    );
    assert_eq!(
        attachment(Some("audio/mp4"), None).audio_format(),
        Some("m4a")
    );
    assert_eq!(
        attachment(Some("audio/ogg; codecs=opus"), None).audio_format(),
        Some("ogg")
    );
    assert_eq!(
        attachment(Some("AUDIO/MPEG"), None).audio_format(),
        Some("mp3")
    );
    assert_eq!(attachment(Some("image/jpeg"), None).audio_format(), None);
}

#[test]
fn audio_format_falls_back_to_extension_only_for_audio_or_unknown_types() {
    assert_eq!(
        attachment(None, Some("note.M4A")).audio_format(),
        Some("m4a")
    );
    assert_eq!(
        attachment(Some("application/octet-stream"), Some("note.ogg")).audio_format(),
        Some("ogg")
    );
    assert_eq!(
        attachment(Some("audio/x-unknown"), Some("note.wav")).audio_format(),
        Some("wav")
    );
    assert_eq!(
        attachment(Some("application/pdf"), Some("trick.mp3")).audio_format(),
        None
    );
    assert_eq!(attachment(None, Some("photo.jpg")).audio_format(), None);
    assert_eq!(attachment(None, None).audio_format(), None);
}

#[test]
fn parse_and_find_voice_note_skips_non_audio_and_missing_ids() {
    let data_message = json!({
        "attachments": [
            {"contentType": "image/png", "id": "img", "size": 10},
            {"contentType": "audio/aac", "size": 20},
            {"contentType": "audio/aac", "id": "voice", "size": 30, "filename": null},
        ]
    });
    let attachments = parse_attachments(&data_message);
    assert_eq!(attachments.len(), 3);
    assert_eq!(
        find_voice_note(&attachments),
        Some(VoiceNote {
            attachment_id: "voice".to_string(),
            format: "aac",
            size: Some(30),
        })
    );
    assert!(parse_attachments(&json!({})).is_empty());
    assert_eq!(find_voice_note(&attachments[..2]), None);
}

#[tokio::test]
async fn transcript_becomes_the_message_body() {
    let sink = MockSink::new(true, Transcription::Text("  bonjour  "));
    let downloader = MockDownloader::ok(b"audio");
    let outcome = transcribe_voice_note(&voice_note(), "", &downloader, &sink, "acct").await;
    assert_eq!(outcome, VoiceOutcome::Dispatch {
        body: "bonjour".to_string(),
        audio: Some(VoiceAudio {
            data: b"audio".to_vec(),
            format: "aac",
        }),
    });
    let requested = downloader
        .requested
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    assert_eq!(requested, vec![(
        InboundMediaSource::AttachmentId {
            id: "att-1".to_string()
        },
        MAX_VOICE_NOTE_BYTES
    )]);
    assert_eq!(
        *sink
            .transcribed_formats
            .lock()
            .unwrap_or_else(|e| e.into_inner()),
        vec!["aac".to_string()]
    );
}

#[tokio::test]
async fn caption_is_kept_alongside_transcript() {
    let sink = MockSink::new(true, Transcription::Text("hello"));
    let downloader = MockDownloader::ok(b"audio");
    let outcome = transcribe_voice_note(&voice_note(), "note", &downloader, &sink, "acct").await;
    let VoiceOutcome::Dispatch { body, .. } = outcome else {
        panic!("expected dispatch");
    };
    assert_eq!(body, "note\n\n[Voice message]: hello");
}

#[tokio::test]
async fn stt_unavailable_sends_setup_hint_without_downloading() {
    let sink = MockSink::new(false, Transcription::Text("unused"));
    let downloader = MockDownloader::ok(b"audio");
    let outcome = transcribe_voice_note(&voice_note(), "", &downloader, &sink, "acct").await;
    assert_eq!(outcome, VoiceOutcome::Reply(VOICE_REPLY_STT_SETUP_HINT));
    assert_eq!(downloader.request_count(), 0);
}

#[tokio::test]
async fn stt_unavailable_with_caption_dispatches_caption() {
    let sink = MockSink::new(false, Transcription::Text("unused"));
    let downloader = MockDownloader::ok(b"audio");
    let outcome =
        transcribe_voice_note(&voice_note(), " caption ", &downloader, &sink, "acct").await;
    assert_eq!(outcome, VoiceOutcome::Dispatch {
        body: "caption".to_string(),
        audio: None,
    });
}

#[tokio::test]
async fn oversized_voice_note_is_rejected_before_download() {
    let sink = MockSink::new(true, Transcription::Text("unused"));
    let downloader = MockDownloader::ok(b"audio");
    let voice = VoiceNote {
        size: Some(MAX_VOICE_NOTE_BYTES as u64 + 1),
        ..voice_note()
    };
    let outcome = transcribe_voice_note(&voice, "", &downloader, &sink, "acct").await;
    assert_eq!(outcome, VoiceOutcome::Reply(VOICE_REPLY_TOO_LARGE));
    assert_eq!(downloader.request_count(), 0);
}

#[tokio::test]
async fn download_failure_replies_to_sender() {
    let sink = MockSink::new(true, Transcription::Text("unused"));
    let downloader = MockDownloader::failing();
    let outcome = transcribe_voice_note(&voice_note(), "", &downloader, &sink, "acct").await;
    assert_eq!(outcome, VoiceOutcome::Reply(VOICE_REPLY_DOWNLOAD_FAILED));
}

#[tokio::test]
async fn transcription_failure_replies_to_sender() {
    let sink = MockSink::new(true, Transcription::Failure);
    let downloader = MockDownloader::ok(b"audio");
    let outcome = transcribe_voice_note(&voice_note(), "", &downloader, &sink, "acct").await;
    assert_eq!(
        outcome,
        VoiceOutcome::Reply(VOICE_REPLY_TRANSCRIPTION_FAILED)
    );
}

#[tokio::test]
async fn transcription_failure_with_caption_keeps_audio() {
    let sink = MockSink::new(true, Transcription::Failure);
    let downloader = MockDownloader::ok(b"audio");
    let outcome = transcribe_voice_note(&voice_note(), "caption", &downloader, &sink, "acct").await;
    assert_eq!(outcome, VoiceOutcome::Dispatch {
        body: "caption".to_string(),
        audio: Some(VoiceAudio {
            data: b"audio".to_vec(),
            format: "aac",
        }),
    });
}

#[tokio::test]
async fn empty_transcription_replies_to_sender() {
    let sink = MockSink::new(true, Transcription::Text("   "));
    let downloader = MockDownloader::ok(b"audio");
    let outcome = transcribe_voice_note(&voice_note(), "", &downloader, &sink, "acct").await;
    assert_eq!(
        outcome,
        VoiceOutcome::Reply(VOICE_REPLY_EMPTY_TRANSCRIPTION)
    );
}
