//! Injection of the preview bridge script into HTML served for previews.
//!
//! The bridge (`vendor/canvas-bridge.js`) lets the web UI pick elements, show
//! comment pins and receive size and error reports through `postMessage`.
//! It is added at serve time only: design files, versions and exports never
//! contain it.

use crate::{csp::VENDOR_PREFIX, vendor::BRIDGE_SCRIPT};

fn bridge_tag() -> String {
    format!("<script src=\"{VENDOR_PREFIX}{BRIDGE_SCRIPT}\"></script>")
}

/// Byte offset just after the opening tag `name` (`<head ...>`), if any.
fn after_open_tag(lower: &str, name: &str) -> Option<usize> {
    let needle = format!("<{name}");
    let mut from = 0;
    while let Some(found) = lower[from..].find(&needle) {
        let start = from + found;
        let next = lower.as_bytes().get(start + needle.len()).copied();
        if matches!(next, Some(b'>' | b' ' | b'\t' | b'\n' | b'\r' | b'/')) {
            return lower[start..].find('>').map(|end| start + end + 1);
        }
        from = start + needle.len();
    }
    None
}

/// `html` with the bridge script as the first element of `<head>` (or of the
/// document when there is no `<head>`).
#[must_use]
pub fn inject_bridge(html: &str) -> String {
    let tag = bridge_tag();
    let lower = html.to_ascii_lowercase();
    let at = after_open_tag(&lower, "head").or_else(|| after_open_tag(&lower, "html"));
    match at {
        Some(at) => format!("{}{tag}{}", &html[..at], &html[at..]),
        None => {
            // Keep a leading doctype first so the page stays in standards mode.
            let doctype_end = lower
                .trim_start()
                .starts_with("<!doctype")
                .then(|| lower.find('>').map(|end| end + 1))
                .flatten();
            match doctype_end {
                Some(end) => format!("{}{tag}{}", &html[..end], &html[end..]),
                None => format!("{tag}{html}"),
            }
        },
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const TAG: &str = "<script src=\"/_vendor/canvas-bridge.js\"></script>";

    #[test]
    fn goes_first_in_head() {
        assert_eq!(
            inject_bridge("<!doctype html><html><head lang=x><title>t</title></head></html>"),
            format!("<!doctype html><html><head lang=x>{TAG}<title>t</title></head></html>")
        );
        assert_eq!(
            inject_bridge("<HEAD>x</HEAD>"),
            format!("<HEAD>{TAG}x</HEAD>")
        );
    }

    #[test]
    fn header_is_not_head() {
        assert_eq!(
            inject_bridge("<html><body><header>h</header></body></html>"),
            format!("<html>{TAG}<body><header>h</header></body></html>")
        );
    }

    #[test]
    fn fragments_keep_the_doctype_first() {
        assert_eq!(
            inject_bridge("<!DOCTYPE html><p>hi</p>"),
            format!("<!DOCTYPE html>{TAG}<p>hi</p>")
        );
        assert_eq!(inject_bridge("<p>hi</p>"), format!("{TAG}<p>hi</p>"));
        assert_eq!(inject_bridge(""), TAG);
    }

    #[test]
    fn multibyte_content_is_preserved() {
        let html = "<html><head><title>été 🌞</title></head></html>";
        let injected = inject_bridge(html);
        assert!(injected.contains("été 🌞"));
        assert_eq!(injected.len(), html.len() + TAG.len());
    }
}
