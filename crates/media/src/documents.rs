//! Helpers for inbound channel documents: labels, stored filenames and inline text extraction.

pub const MAX_INLINE_DOCUMENT_BYTES: usize = 64 * 1024;
pub const MAX_INLINE_DOCUMENT_CHARS: usize = 24_000;

pub fn format_document_label(file_name: Option<&str>, media_type: &str) -> String {
    match file_name {
        Some(name) if !name.trim().is_empty() => format!("[Document: {name} ({media_type})]"),
        _ => format!("[Document: {media_type}]"),
    }
}

pub fn sanitize_document_filename(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
        .collect();
    sanitized.trim_start_matches('.').to_string()
}

pub fn build_saved_document_filename(
    file_name: Option<&str>,
    media_type: &str,
    file_id: &str,
) -> String {
    let ext = crate::mime::extension_for_mime(media_type);
    let file_id_prefix: String = sanitize_document_filename(file_id)
        .chars()
        .take(16)
        .collect();

    let base_name = file_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(sanitize_document_filename)
        .filter(|name| !name.is_empty())
        .map(|name| {
            if name.contains('.') {
                name
            } else {
                format!("{name}.{ext}")
            }
        })
        .unwrap_or_else(|| format!("document.{ext}"));

    if file_id_prefix.is_empty() {
        base_name
    } else {
        format!("{file_id_prefix}_{base_name}")
    }
}

pub fn build_document_body(caption: &str, doc_label: &str, extracted_text: Option<&str>) -> String {
    let mut sections = Vec::new();
    if !caption.is_empty() {
        sections.push(caption.to_string());
    }
    sections.push(doc_label.to_string());
    if let Some(text) = extracted_text
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        sections.push(text.to_string());
    }
    sections.join("\n\n")
}

pub fn normalize_media_type(media_type: &str) -> String {
    media_type
        .split(';')
        .next()
        .unwrap_or(media_type)
        .trim()
        .to_ascii_lowercase()
}

pub fn should_inline_document_text(media_type: &str) -> bool {
    matches!(
        media_type,
        "text/html"
            | "text/plain"
            | "text/markdown"
            | "text/x-markdown"
            | "text/xml"
            | "text/csv"
            | "text/x-toml"
            | "text/x-yaml"
            | "application/json"
            | "application/xml"
            // mime_guess returns text/x-toml and text/x-yaml for the inference
            // path; the application/* variants cover cases where Telegram (or
            // another source) sends these MIME types directly.
            | "application/toml"
            | "application/yaml"
            | "application/x-yaml"
    ) || media_type.ends_with("+json")
        || media_type.ends_with("+xml")
}

pub fn is_pdf_document_type(media_type: &str) -> bool {
    media_type == "application/pdf"
}

pub fn is_supported_document_type(media_type: &str) -> bool {
    media_type.starts_with("image/")
        || should_inline_document_text(media_type)
        || is_pdf_document_type(media_type)
}

pub fn truncate_inline_document_text(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut truncated = false;
    let mut text =
        if let Some((byte_idx, _)) = trimmed.char_indices().nth(MAX_INLINE_DOCUMENT_CHARS) {
            truncated = true;
            trimmed[..byte_idx].to_string()
        } else {
            trimmed.to_string()
        };

    if truncated {
        text.push_str("\n\n[Document content truncated]");
    }

    Some(text)
}

pub fn extract_text_document_content(data: &[u8], media_type: &str) -> Option<String> {
    if data.is_empty() || !should_inline_document_text(media_type) {
        return None;
    }

    let byte_truncated = data.len() > MAX_INLINE_DOCUMENT_BYTES;
    let bounded = if byte_truncated {
        let slice = &data[..MAX_INLINE_DOCUMENT_BYTES];
        match std::str::from_utf8(slice) {
            Ok(_) => slice,
            Err(e) => &slice[..e.valid_up_to()],
        }
    } else {
        data
    };

    let lossy = String::from_utf8_lossy(bounded);
    let mut text = truncate_inline_document_text(&lossy)?;
    if byte_truncated && !text.contains("[Document content truncated]") {
        text.push_str("\n\n[Document content truncated]");
    }
    Some(text)
}

pub async fn extract_pdf_document_content(data: Vec<u8>) -> Option<String> {
    let extracted = tokio::task::spawn_blocking(move || {
        use std::io::Write as _;

        let mut file = tempfile::Builder::new().suffix(".pdf").tempfile().ok()?;
        file.write_all(&data).ok()?;
        pdf_extract::extract_text(file.path()).ok()
    })
    .await
    .ok()??;
    truncate_inline_document_text(&extracted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_and_bodies() {
        assert_eq!(
            format_document_label(Some("a.pdf"), "application/pdf"),
            "[Document: a.pdf (application/pdf)]"
        );
        assert_eq!(
            format_document_label(Some(" "), "text/plain"),
            "[Document: text/plain]"
        );
        assert_eq!(
            build_document_body("", "[Doc]", Some(" body ")),
            "[Doc]\n\nbody"
        );
        assert_eq!(build_document_body("cap", "[Doc]", None), "cap\n\n[Doc]");
    }

    #[test]
    fn saved_filenames_are_sanitized() {
        assert_eq!(sanitize_document_filename("../../etc/passwd"), "etcpasswd");
        let name = build_saved_document_filename(Some("my report.pdf"), "application/pdf", "id/1");
        assert_eq!(name, "id1_myreport.pdf");
        let fallback = build_saved_document_filename(None, "application/pdf", "");
        assert_eq!(fallback, "document.pdf");
    }

    #[test]
    fn media_type_classification() {
        assert_eq!(
            normalize_media_type("Text/Plain; charset=utf-8"),
            "text/plain"
        );
        assert!(is_supported_document_type("application/pdf"));
        assert!(is_supported_document_type("image/png"));
        assert!(is_supported_document_type("application/ld+json"));
        assert!(!is_supported_document_type("application/zip"));
    }

    #[test]
    fn text_extraction_is_bounded() {
        assert_eq!(
            extract_text_document_content(b"hello", "text/plain").as_deref(),
            Some("hello")
        );
        assert_eq!(
            extract_text_document_content(b"hello", "application/zip"),
            None
        );
        let big = vec![b'a'; MAX_INLINE_DOCUMENT_BYTES + 10];
        let text = extract_text_document_content(&big, "text/plain").unwrap_or_default();
        assert!(text.ends_with("[Document content truncated]"));
    }
}
