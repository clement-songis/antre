use std::{collections::HashMap, sync::Mutex};

use {
    antre_channels::{
        ChannelEvent, ChannelEventSink, ChannelMessageMeta, ChannelReplyTarget, ChannelType, Error,
        InboundMediaDownloader, InboundMediaSource, Result, SavedChannelFile,
    },
    async_trait::async_trait,
    serde_json::json,
};

use super::*;

#[derive(Default)]
struct MockSink {
    refuse_saves: bool,
    saved: Mutex<Vec<(String, Vec<u8>)>>,
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

    async fn save_channel_attachment(
        &self,
        file_data: &[u8],
        filename: &str,
        _reply_to: &ChannelReplyTarget,
    ) -> Option<SavedChannelFile> {
        if self.refuse_saves {
            return None;
        }
        self.saved
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((filename.to_string(), file_data.to_vec()));
        Some(SavedChannelFile {
            filename: filename.to_string(),
            media_ref: format!("media/main/{filename}"),
            absolute_path: format!("/tmp/media/{filename}"),
        })
    }
}

/// Serves attachment bytes by ID; unknown IDs fail like an unreachable daemon.
#[derive(Default)]
struct MockDownloader {
    files: HashMap<&'static str, Vec<u8>>,
    requested: Mutex<Vec<(String, usize)>>,
}

impl MockDownloader {
    fn with(files: &[(&'static str, &[u8])]) -> Self {
        Self {
            files: files
                .iter()
                .map(|(id, data)| (*id, data.to_vec()))
                .collect(),
            requested: Mutex::new(Vec::new()),
        }
    }

    fn requested(&self) -> Vec<(String, usize)> {
        self.requested
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

#[async_trait]
impl InboundMediaDownloader for MockDownloader {
    async fn download_media(
        &self,
        source: &InboundMediaSource,
        max_bytes: usize,
    ) -> Result<Vec<u8>> {
        let InboundMediaSource::AttachmentId { id } = source else {
            return Err(Error::invalid_input("unexpected source"));
        };
        self.requested
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((id.clone(), max_bytes));
        self.files
            .get(id.as_str())
            .cloned()
            .ok_or_else(|| Error::unavailable("daemon unreachable"))
    }
}

fn reply_to() -> ChannelReplyTarget {
    ChannelReplyTarget {
        direct_chat: false,
        ack_message_id: None,
        channel_type: ChannelType::Signal,
        account_id: "acct".to_string(),
        chat_id: "+15550001111".to_string(),
        message_id: Some("1".to_string()),
        thread_id: None,
    }
}

fn attachment(id: &str, content_type: &str, filename: Option<&str>) -> SignalAttachment {
    SignalAttachment {
        id: Some(id.to_string()),
        content_type: Some(content_type.to_string()),
        filename: filename.map(ToString::to_string),
        size: None,
    }
}

async fn ingest(
    attachments: &[SignalAttachment],
    downloader: &MockDownloader,
    sink: &MockSink,
) -> IngestedMedia {
    let refs: Vec<_> = attachments.iter().collect();
    ingest_attachments(&refs, downloader, sink, &reply_to(), "acct").await
}

#[test]
fn parse_attachments_reads_signal_cli_fields() {
    let parsed = parse_attachments(&json!({
        "attachments": [{
            "contentType": "application/pdf",
            "filename": "report.pdf",
            "id": "abc",
            "size": 12
        }]
    }));
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].id.as_deref(), Some("abc"));
    assert_eq!(parsed[0].size, Some(12));
    assert!(parse_attachments(&json!({ "attachments": "nope" })).is_empty());
}

#[test]
fn media_type_normalizes_and_falls_back_to_extension() {
    assert_eq!(
        attachment("a", "Text/Plain; charset=utf-8", None).media_type(),
        "text/plain"
    );
    assert_eq!(
        attachment("a", "application/octet-stream", Some("notes.md")).media_type(),
        "text/markdown"
    );
    let unknown = SignalAttachment {
        id: Some("a".to_string()),
        ..Default::default()
    };
    assert_eq!(unknown.media_type(), "application/octet-stream");
}

#[tokio::test]
async fn images_become_multimodal_attachments() {
    let downloader = MockDownloader::with(&[("img", b"not-really-a-jpeg")]);
    let sink = MockSink::default();
    let media = ingest(
        &[attachment("img", "image/jpeg", Some("photo.jpg"))],
        &downloader,
        &sink,
    )
    .await;

    assert_eq!(media.images.len(), 1);
    assert_eq!(media.images[0].media_type, "image/jpeg");
    assert_eq!(media.images[0].data, b"not-really-a-jpeg");
    assert!(media.sections.is_empty());
    assert!(media.documents.is_empty());
    assert_eq!(downloader.requested(), vec![(
        "img".to_string(),
        MAX_IMAGE_BYTES
    )]);
}

#[tokio::test]
async fn text_documents_are_saved_and_inlined() {
    let downloader = MockDownloader::with(&[("doc", b"hello from a file")]);
    let sink = MockSink::default();
    let media = ingest(
        &[attachment("doc", "text/plain", Some("notes.txt"))],
        &downloader,
        &sink,
    )
    .await;

    assert_eq!(media.sections, vec![
        "[Document: notes.txt (text/plain)]\n\nhello from a file".to_string()
    ]);
    assert_eq!(media.documents.len(), 1);
    assert_eq!(media.documents[0].display_name, "notes.txt");
    assert_eq!(media.documents[0].stored_filename, "doc_notes.txt");
    assert_eq!(media.documents[0].mime_type, "text/plain");
    assert_eq!(media.documents[0].size_bytes, Some(17));
    let saved = sink.saved.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(*saved, vec![(
        "doc_notes.txt".to_string(),
        b"hello from a file".to_vec()
    )]);
    assert_eq!(downloader.requested(), vec![(
        "doc".to_string(),
        MAX_DOCUMENT_BYTES
    )]);
}

#[tokio::test]
async fn binary_and_non_llm_image_attachments_are_saved_as_documents() {
    let downloader = MockDownloader::with(&[("zip", b"PK\x03\x04"), ("svg", b"<svg/>")]);
    let sink = MockSink::default();
    let media = ingest(
        &[
            attachment("zip", "application/zip", Some("archive.zip")),
            attachment("svg", "image/svg+xml", Some("logo.svg")),
        ],
        &downloader,
        &sink,
    )
    .await;

    assert!(media.images.is_empty());
    assert_eq!(media.sections, vec![
        "[Document: archive.zip (application/zip)]".to_string(),
        "[Document: logo.svg (image/svg+xml)]\n\n<svg/>".to_string(),
    ]);
    assert_eq!(media.documents.len(), 2);
}

#[tokio::test]
async fn oversized_attachments_are_not_downloaded() {
    let downloader = MockDownloader::with(&[("big", b"x"), ("huge", b"x")]);
    let sink = MockSink::default();
    let media = ingest(
        &[
            SignalAttachment {
                size: Some(MAX_DOCUMENT_BYTES as u64 + 1),
                ..attachment("big", "application/pdf", Some("big.pdf"))
            },
            SignalAttachment {
                size: Some(MAX_IMAGE_BYTES as u64 + 1),
                ..attachment("huge", "image/png", Some("huge.png"))
            },
        ],
        &downloader,
        &sink,
    )
    .await;

    assert!(downloader.requested().is_empty());
    assert_eq!(media.sections, vec![
        "[Document: big.pdf (application/pdf)]\n[Document - too large to download]".to_string(),
        "[Image: huge.png - too large to download]".to_string(),
    ]);
    assert!(media.images.is_empty());
    assert!(media.documents.is_empty());
}

#[tokio::test]
async fn download_failures_become_notes() {
    let downloader = MockDownloader::default();
    let sink = MockSink::default();
    let media = ingest(
        &[
            attachment("missing-doc", "application/pdf", None),
            attachment("missing-img", "image/webp", None),
            SignalAttachment {
                id: None,
                ..attachment("", "text/plain", Some("no-id.txt"))
            },
        ],
        &downloader,
        &sink,
    )
    .await;

    assert_eq!(media.sections, vec![
        "[Document: application/pdf]\n[Document - download failed]".to_string(),
        "[Image: image/webp - download failed]".to_string(),
        "[Document: no-id.txt (text/plain)]\n[Document - download failed]".to_string(),
    ]);
    assert_eq!(downloader.requested().len(), 2);
}

#[tokio::test]
async fn unsaved_documents_are_flagged() {
    let downloader = MockDownloader::with(&[("doc", b"{}")]);
    let sink = MockSink {
        refuse_saves: true,
        ..Default::default()
    };
    let media = ingest(
        &[attachment("doc", "application/json", Some("data.json"))],
        &downloader,
        &sink,
    )
    .await;

    assert!(media.documents.is_empty());
    assert_eq!(media.sections, vec![
        "[Document: data.json (application/json)]\n\n{}\n\n[Document - could not be saved]"
            .to_string()
    ]);
}
