//! Inbound Signal attachments: parsing, download through signal-cli, and
//! ingestion of images and documents for the agent.

use {
    async_trait::async_trait,
    futures::future::join_all,
    moltis_channels::{
        ChannelAttachment, ChannelDocumentFile, ChannelEventSink, ChannelReplyTarget,
        InboundMediaDownloader, InboundMediaSource,
    },
    moltis_media::documents::{
        build_document_body, build_saved_document_filename, extract_pdf_document_content,
        extract_text_document_content, format_document_label, is_pdf_document_type,
        normalize_media_type,
    },
    serde::Deserialize,
    serde_json::Value,
};

use crate::{client::SignalClient, config::SignalAccountConfig};

pub(crate) const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;
pub(crate) const MAX_DOCUMENT_BYTES: usize = 25 * 1024 * 1024;

const FALLBACK_MEDIA_TYPE: &str = "application/octet-stream";

/// Attachment entry from a signal-cli `dataMessage.attachments` array.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SignalAttachment {
    pub id: Option<String>,
    pub content_type: Option<String>,
    pub filename: Option<String>,
    pub size: Option<u64>,
}

pub(crate) fn parse_attachments(data_message: &Value) -> Vec<SignalAttachment> {
    data_message
        .get("attachments")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| serde_json::from_value(item.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Fetches attachments for one conversation through the signal-cli daemon.
pub(crate) struct SignalAttachmentDownloader<'a> {
    pub client: &'a SignalClient,
    pub config: &'a SignalAccountConfig,
    pub chat_id: &'a str,
}

#[async_trait]
impl InboundMediaDownloader for SignalAttachmentDownloader<'_> {
    async fn download_media(
        &self,
        source: &InboundMediaSource,
        max_bytes: usize,
    ) -> moltis_channels::Result<Vec<u8>> {
        match source {
            InboundMediaSource::AttachmentId { id } => {
                self.client
                    .get_attachment(
                        &self.config.http_url,
                        self.config.account(),
                        id,
                        self.chat_id,
                        max_bytes,
                    )
                    .await
            },
            _ => Err(moltis_channels::Error::invalid_input(
                "Signal downloader received unsupported media source",
            )),
        }
    }
}

impl SignalAttachment {
    /// Normalized MIME type, guessed from the filename when signal-cli does
    /// not report a usable one.
    pub(crate) fn media_type(&self) -> String {
        let reported = self
            .content_type
            .as_deref()
            .map(normalize_media_type)
            .filter(|mt| mt.contains('/') && mt != FALLBACK_MEDIA_TYPE);
        reported
            .or_else(|| {
                let (_, ext) = self.filename.as_deref()?.rsplit_once('.')?;
                moltis_media::mime::mime_from_extension(&ext.to_ascii_lowercase())
                    .map(ToString::to_string)
            })
            .unwrap_or_else(|| FALLBACK_MEDIA_TYPE.to_string())
    }

    fn display_name(&self) -> Option<&str> {
        self.filename
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
    }

    fn attachment_id(&self) -> Option<&str> {
        self.id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
    }
}

/// Image types every multimodal provider accepts; other images are handled
/// as documents.
fn is_llm_image(media_type: &str) -> bool {
    matches!(
        media_type,
        "image/jpeg" | "image/png" | "image/gif" | "image/webp"
    )
}

/// What the agent receives for a message's images and documents.
#[derive(Debug, Default)]
pub(crate) struct IngestedMedia {
    /// Document labels, extracted text and failure notes, in message order.
    pub sections: Vec<String>,
    pub images: Vec<ChannelAttachment>,
    pub documents: Vec<ChannelDocumentFile>,
}

enum Ingested {
    Image(ChannelAttachment),
    Document {
        section: String,
        file: Option<ChannelDocumentFile>,
    },
    Note(String),
}

/// Downloads every attachment concurrently. Failures never drop an
/// attachment silently: they become a note the agent can relay to the sender.
pub(crate) async fn ingest_attachments(
    attachments: &[&SignalAttachment],
    downloader: &dyn InboundMediaDownloader,
    sink: &dyn ChannelEventSink,
    reply_to: &ChannelReplyTarget,
    account_id: &str,
) -> IngestedMedia {
    let results = join_all(
        attachments
            .iter()
            .map(|attachment| ingest_one(attachment, downloader, sink, reply_to, account_id)),
    )
    .await;

    results
        .into_iter()
        .fold(IngestedMedia::default(), |mut media, ingested| {
            match ingested {
                Ingested::Image(image) => media.images.push(image),
                Ingested::Document { section, file } => {
                    media.sections.push(section);
                    media.documents.extend(file);
                },
                Ingested::Note(note) => media.sections.push(note),
            }
            media
        })
}

async fn download(
    attachment: &SignalAttachment,
    downloader: &dyn InboundMediaDownloader,
    max_bytes: usize,
    account_id: &str,
) -> Result<Vec<u8>, &'static str> {
    if attachment.size.is_some_and(|size| size > max_bytes as u64) {
        return Err("too large to download");
    }
    let id = attachment.attachment_id().ok_or("download failed")?;
    let source = InboundMediaSource::AttachmentId { id: id.to_string() };
    downloader
        .download_media(&source, max_bytes)
        .await
        .map_err(|e| {
            tracing::warn!(account_id, error = %e, "failed to download Signal attachment");
            "download failed"
        })
}

async fn ingest_one(
    attachment: &SignalAttachment,
    downloader: &dyn InboundMediaDownloader,
    sink: &dyn ChannelEventSink,
    reply_to: &ChannelReplyTarget,
    account_id: &str,
) -> Ingested {
    let media_type = attachment.media_type();
    if is_llm_image(&media_type) {
        return match download(attachment, downloader, MAX_IMAGE_BYTES, account_id).await {
            Ok(data) => Ingested::Image(prepare_image(data, media_type, account_id)),
            Err(reason) => {
                let name = attachment.display_name().unwrap_or(&media_type);
                Ingested::Note(format!("[Image: {name} - {reason}]"))
            },
        };
    }

    let label = format_document_label(attachment.display_name(), &media_type);
    let data = match download(attachment, downloader, MAX_DOCUMENT_BYTES, account_id).await {
        Ok(data) => data,
        Err(reason) => return Ingested::Note(format!("{label}\n[Document - {reason}]")),
    };

    let filename = build_saved_document_filename(
        attachment.display_name(),
        &media_type,
        attachment.attachment_id().unwrap_or_default(),
    );
    let saved = sink
        .save_channel_attachment(&data, &filename, reply_to)
        .await;
    let file = saved.as_ref().map(|saved| ChannelDocumentFile {
        size_bytes: Some(data.len() as u64),
        ..ChannelDocumentFile::from_saved(saved, attachment.display_name(), &media_type)
    });
    let extracted = if is_pdf_document_type(&media_type) {
        extract_pdf_document_content(data).await
    } else {
        extract_text_document_content(&data, &media_type)
    };
    let mut section = build_document_body("", &label, extracted.as_deref());
    if file.is_none() {
        tracing::warn!(account_id, filename, "failed to save Signal document");
        section.push_str("\n\n[Document - could not be saved]");
    }
    Ingested::Document { section, file }
}

fn prepare_image(data: Vec<u8>, media_type: String, account_id: &str) -> ChannelAttachment {
    match moltis_media::image_ops::optimize_for_llm(&data, None) {
        Ok(optimized) => ChannelAttachment {
            media_type: optimized.media_type,
            data: optimized.data,
        },
        Err(e) => {
            tracing::warn!(account_id, error = %e, "failed to optimize Signal image, using original");
            ChannelAttachment { media_type, data }
        },
    }
}

#[cfg(test)]
#[path = "attachments_tests.rs"]
mod tests;
