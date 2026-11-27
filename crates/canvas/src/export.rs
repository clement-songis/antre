//! Design exports: single-file HTML (assets inlined) and ZIP.
//!
//! The inliner is a targeted rewriter, not a full HTML parser: it handles
//! `<script src>`, `<link rel=stylesheet href>`, `src`/`href`/`poster`
//! attributes of media and icon tags, and CSS `url(...)`. References it
//! cannot resolve locally (remote URLs, absolute paths) are left untouched;
//! the render-time CSP blocks them anyway.

use std::{
    borrow::Cow,
    io::{Cursor, Write},
    path::Path,
    sync::LazyLock,
};

use {
    base64::{Engine, engine::general_purpose::STANDARD},
    regex::{Captures, Regex},
};

use crate::{
    Error, Result,
    csp::{OFFLINE_META_POLICY, VENDOR_PREFIX},
    paths::{list_files, resolve_in_dir},
    vendor::Vendor,
};

/// Entry document of every design.
pub const ENTRY_FILE: &str = "index.html";

/// Upper bound of a single-file export.
pub const MAX_SINGLE_FILE_BYTES: usize = 64 * 1024 * 1024;

static SCRIPT_SRC: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r#"(?is)<script\b([^>]*?)\bsrc\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+))([^>]*)>\s*</script\s*>"#)
        .ok()
});
static STYLESHEET_LINK: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(?is)<link\b[^>]*>").ok());
static MEDIA_TAG: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r"(?is)<(?:img|source|video|audio|image|track|input|link)\b[^>]*>").ok()
});
static URL_ATTR: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r#"(?is)\b(src|href|poster|xlink:href)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+))"#)
        .ok()
});
static REL_STYLESHEET: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r#"(?is)\brel\s*=\s*(?:"[^"]*\bstylesheet\b[^"]*"|'[^']*\bstylesheet\b[^']*'|stylesheet\b)"#)
        .ok()
});
static CSS_URL: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r#"(?i)url\(\s*(?:"([^"]*)"|'([^']*)'|([^'")\s]+))\s*\)"#).ok());
static STYLE_BLOCK: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(?is)(<style\b[^>]*>)(.*?)(</style\s*>)").ok());
static HEAD_OPEN: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(?is)<head\b[^>]*>").ok());

fn regex(cell: &'static LazyLock<Option<Regex>>) -> Result<&'static Regex> {
    cell.as_ref()
        .ok_or_else(|| Error::message("export pattern failed to compile"))
}

/// Resolves references found in a design to bytes.
struct Assets<'a> {
    dir: &'a Path,
    vendor: &'a Vendor,
    max_file_bytes: u64,
}

struct Asset {
    bytes: Vec<u8>,
    mime: String,
}

/// Local path of a reference, or `None` for remote, absolute or fragment
/// references that the exporter leaves alone.
fn local_reference(reference: &str) -> Option<LocalRef<'_>> {
    let reference = reference.trim();
    let without_suffix = reference
        .split(['?', '#'])
        .next()
        .filter(|path| !path.is_empty())?;
    if let Some(name) = without_suffix.strip_prefix(VENDOR_PREFIX) {
        return Some(LocalRef::Vendor(name));
    }
    let has_scheme = without_suffix
        .split_once(':')
        .is_some_and(|(scheme, _)| !scheme.contains('/'));
    if has_scheme || without_suffix.starts_with('/') {
        return None;
    }
    Some(LocalRef::File(
        without_suffix.strip_prefix("./").unwrap_or(without_suffix),
    ))
}

enum LocalRef<'a> {
    Vendor(&'a str),
    File(&'a str),
}

impl Assets<'_> {
    fn load(&self, reference: &str) -> Option<Asset> {
        match local_reference(reference)? {
            LocalRef::Vendor(name) => self.vendor.get_blocking(name).map(|file| Asset {
                bytes: file.bytes,
                mime: file.content_type.to_string(),
            }),
            LocalRef::File(path) => {
                let resolved = resolve_in_dir(self.dir, path).ok()?;
                let size = std::fs::metadata(&resolved).ok()?.len();
                if size > self.max_file_bytes {
                    return None;
                }
                let bytes = std::fs::read(&resolved).ok()?;
                let mime = mime_guess::from_path(&resolved)
                    .first_or_octet_stream()
                    .essence_str()
                    .to_string();
                Some(Asset { bytes, mime })
            },
        }
    }

    fn data_uri(&self, reference: &str) -> Option<String> {
        let asset = self.load(reference)?;
        Some(format!(
            "data:{};base64,{}",
            asset.mime,
            STANDARD.encode(asset.bytes)
        ))
    }

    fn text(&self, reference: &str) -> Option<String> {
        String::from_utf8(self.load(reference)?.bytes).ok()
    }
}

fn first_group<'h>(caps: &Captures<'h>, groups: &[usize]) -> Option<&'h str> {
    groups
        .iter()
        .find_map(|index| caps.get(*index))
        .map(|m| m.as_str())
}

/// Inline CSS `url(...)` references.
fn inline_css(css: &str, assets: &Assets<'_>) -> Result<String> {
    let pattern = regex(&CSS_URL)?;
    Ok(pattern
        .replace_all(css, |caps: &Captures<'_>| {
            let original = caps.get(0).map_or("", |m| m.as_str()).to_string();
            first_group(caps, &[1, 2, 3])
                .and_then(|reference| assets.data_uri(reference))
                .map_or(original, |uri| format!("url(\"{uri}\")"))
        })
        .into_owned())
}

/// `</script` and `</style` inside inlined content would end the element.
fn escape_raw_text(content: &str, tag: &str) -> String {
    let pattern = format!("</{tag}");
    let mut escaped = String::with_capacity(content.len());
    let lower = content.to_ascii_lowercase();
    let mut last = 0;
    for (index, _) in lower.match_indices(&pattern) {
        escaped.push_str(&content[last..index]);
        escaped.push_str("<\\/");
        escaped.push_str(&content[index + 2..index + pattern.len()]);
        last = index + pattern.len();
    }
    escaped.push_str(&content[last..]);
    escaped
}

fn inline_html(html: &str, assets: &Assets<'_>) -> Result<String> {
    let scripts = regex(&SCRIPT_SRC)?.replace_all(html, |caps: &Captures<'_>| {
        let original = caps.get(0).map_or("", |m| m.as_str()).to_string();
        let Some(source) = first_group(caps, &[2, 3, 4]).and_then(|r| assets.text(r)) else {
            return original;
        };
        let before = caps.get(1).map_or("", |m| m.as_str());
        let after = caps.get(5).map_or("", |m| m.as_str());
        let joined = format!("{before}{after}");
        let without_integrity = strip_attr(&joined, "integrity");
        let attrs = strip_attr(&without_integrity, "crossorigin");
        format!(
            "<script{}>{}</script>",
            attrs.trim_end(),
            escape_raw_text(&source, "script")
        )
    });

    let stylesheet = regex(&REL_STYLESHEET)?;
    let url_attr = regex(&URL_ATTR)?;
    let styles = regex(&STYLESHEET_LINK)?.replace_all(&scripts, |caps: &Captures<'_>| {
        let tag = caps.get(0).map_or("", |m| m.as_str());
        if !stylesheet.is_match(tag) {
            return tag.to_string();
        }
        let href = url_attr.captures_iter(tag).find_map(|attr| {
            let name = attr.get(1)?.as_str();
            name.eq_ignore_ascii_case("href")
                .then(|| first_group(&attr, &[2, 3, 4]))
                .flatten()
        });
        let Some(css) = href.and_then(|href| assets.text(href)) else {
            return tag.to_string();
        };
        match inline_css(&css, assets) {
            Ok(css) => format!("<style>{}</style>", escape_raw_text(&css, "style")),
            Err(_) => tag.to_string(),
        }
    });

    let media = regex(&MEDIA_TAG)?.replace_all(&styles, |caps: &Captures<'_>| {
        let tag = caps.get(0).map_or("", |m| m.as_str());
        url_attr
            .replace_all(tag, |attr: &Captures<'_>| {
                let original = attr.get(0).map_or("", |m| m.as_str()).to_string();
                let name = attr.get(1).map_or("", |m| m.as_str());
                first_group(attr, &[2, 3, 4])
                    .and_then(|reference| assets.data_uri(reference))
                    .map_or(original, |uri| format!("{name}=\"{uri}\""))
            })
            .into_owned()
    });

    let mut style_error = None;
    let blocks = regex(&STYLE_BLOCK)?.replace_all(&media, |caps: &Captures<'_>| {
        let open = caps.get(1).map_or("", |m| m.as_str());
        let body = caps.get(2).map_or("", |m| m.as_str());
        let close = caps.get(3).map_or("", |m| m.as_str());
        match inline_css(body, assets) {
            Ok(css) => format!("{open}{css}{close}"),
            Err(error) => {
                style_error = Some(error);
                caps.get(0).map_or("", |m| m.as_str()).to_string()
            },
        }
    });
    if let Some(error) = style_error {
        return Err(error);
    }
    Ok(blocks.into_owned())
}

fn strip_attr<'a>(attrs: &'a str, name: &str) -> Cow<'a, str> {
    let pattern = format!(r#"(?i)\s{name}(?:\s*=\s*(?:"[^"]*"|'[^']*'|[^\s>]+))?"#);
    match Regex::new(&pattern) {
        Ok(re) => re.replace_all(attrs, ""),
        Err(_) => Cow::Borrowed(attrs),
    }
}

/// Build a self-contained HTML document from a design directory.
pub fn single_file_html(dir: &Path, vendor: &Vendor, max_file_bytes: u64) -> Result<String> {
    let entry = resolve_in_dir(dir, ENTRY_FILE)?;
    let size = std::fs::metadata(&entry)?.len();
    if size > max_file_bytes {
        return Err(Error::TooLarge {
            path: ENTRY_FILE.into(),
            bytes: size,
            limit: max_file_bytes,
        });
    }
    let html = std::fs::read_to_string(entry)?;
    let assets = Assets {
        dir,
        vendor,
        max_file_bytes,
    };
    let inlined = inline_html(&html, &assets)?;
    if inlined.len() > MAX_SINGLE_FILE_BYTES {
        return Err(Error::TooLarge {
            path: "single-file export".into(),
            bytes: inlined.len() as u64,
            limit: MAX_SINGLE_FILE_BYTES as u64,
        });
    }
    Ok(inlined)
}

/// Insert a CSP `<meta>` blocking all network access as the first element of
/// `<head>`, so it applies before any inlined script runs.
pub fn with_offline_csp(html: &str) -> Result<String> {
    let meta =
        format!("<meta http-equiv=\"Content-Security-Policy\" content=\"{OFFLINE_META_POLICY}\">");
    let head = regex(&HEAD_OPEN)?;
    if let Some(found) = head.find(html) {
        let mut out = String::with_capacity(html.len() + meta.len());
        out.push_str(&html[..found.end()]);
        out.push_str(&meta);
        out.push_str(&html[found.end()..]);
        return Ok(out);
    }
    Ok(format!("<!doctype html><head>{meta}</head>{html}"))
}

/// Zip every served file of a design under a `<folder>/` prefix.
pub fn zip_design(dir: &Path, folder: &str, max_file_bytes: u64) -> Result<Vec<u8>> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);
    for file in list_files(dir)? {
        if file.bytes > max_file_bytes {
            return Err(Error::TooLarge {
                path: file.path,
                bytes: file.bytes,
                limit: max_file_bytes,
            });
        }
        let path = resolve_in_dir(dir, &file.path)?;
        writer.start_file(format!("{folder}/{}", file.path), options)?;
        writer.write_all(&std::fs::read(path)?)?;
    }
    Ok(writer.finish()?.into_inner())
}

/// File-name-safe slug of a title, for download names and ZIP folders.
#[must_use]
pub fn slug(title: &str) -> String {
    let mut slug = String::new();
    for ch in title.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
        if slug.len() >= 60 {
            break;
        }
    }
    let slug = slug.trim_end_matches('-').to_string();
    if slug.is_empty() {
        "design".into()
    } else {
        slug
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use {super::*, std::io::Read};

    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0, 0, 0, 0];

    fn design() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(
            dir.join("index.html"),
            r#"<!doctype html><html><head><title>T</title>
<link rel="stylesheet" href="./tokens.css">
<link rel="icon" href="assets/logo.png">
<style>.hero{background:url('assets/logo.png')}</style>
<script src="/_vendor/preact.js" integrity="sha384-x" crossorigin="anonymous"></script>
<script src=app.js></script>
<script src="https://cdn.example.com/lib.js"></script>
</head><body><img src="assets/logo.png" alt="x"><img src="../secret.png"><a href="page2.html">next</a></body></html>"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("tokens.css"),
            ":root{--c:red}.bg{background:url(assets/logo.png)}",
        )
        .unwrap();
        std::fs::write(dir.join("app.js"), "document.title='</script><b>';").unwrap();
        std::fs::write(dir.join("assets/logo.png"), PNG).unwrap();
        tmp
    }

    #[test]
    fn single_file_inlines_local_assets_only() {
        let tmp = design();
        let html = single_file_html(tmp.path(), &Vendor::new(None), 1 << 20).unwrap();
        assert!(!html.contains("tokens.css"), "{html}");
        assert!(html.contains(":root{--c:red}"));
        assert!(html.contains("<link rel=\"icon\" href=\"data:image/png;base64,"));
        assert!(html.contains("<img src=\"data:image/png;base64,"));
        assert!(html.contains("url(\"data:image/png;base64,"));
        assert!(!html.contains("src=\"app.js\""));
        assert!(html.contains("document.title='<\\/script><b>';"));
        assert!(html.contains("self.preact="), "vendored script inlined");
        assert!(!html.contains("integrity="), "stale SRI attributes removed");
        assert!(
            html.contains("https://cdn.example.com/lib.js"),
            "remote left alone"
        );
        assert!(
            html.contains("src=\"../secret.png\""),
            "traversal not followed"
        );
        assert!(html.contains("href=\"page2.html\""), "links left alone");
    }

    #[test]
    fn oversized_assets_are_not_inlined() {
        let tmp = design();
        std::fs::write(tmp.path().join("assets/logo.png"), vec![0_u8; 4096]).unwrap();
        let html = single_file_html(tmp.path(), &Vendor::new(None), 2048).unwrap();
        assert!(html.contains("<img src=\"assets/logo.png\""));
        let err = single_file_html(tmp.path(), &Vendor::new(None), 64).unwrap_err();
        assert!(matches!(err, Error::TooLarge { .. }));
    }

    #[test]
    fn missing_entry_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(single_file_html(tmp.path(), &Vendor::new(None), 1 << 20).is_err());
    }

    #[test]
    fn offline_csp_is_first_in_head() {
        let html = with_offline_csp("<html><HEAD lang=en><title>x</title></head></html>").unwrap();
        assert!(html.starts_with(
            "<html><HEAD lang=en><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none';"
        ));
        let bare = with_offline_csp("<p>no head</p>").unwrap();
        assert!(bare.starts_with("<!doctype html><head><meta http-equiv"));
        assert!(bare.ends_with("<p>no head</p>"));
    }

    #[test]
    fn zip_contains_served_files_under_folder() {
        let tmp = design();
        std::fs::write(tmp.path().join(".env"), "SECRET=1").unwrap();
        let bytes = zip_design(tmp.path(), "my-design", 1 << 20).unwrap();
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut names: Vec<_> = archive.file_names().map(str::to_string).collect();
        names.sort();
        assert_eq!(names, vec![
            "my-design/app.js",
            "my-design/assets/logo.png",
            "my-design/index.html",
            "my-design/tokens.css",
        ]);
        let mut content = String::new();
        archive
            .by_name("my-design/tokens.css")
            .unwrap()
            .read_to_string(&mut content)
            .unwrap();
        assert!(content.starts_with(":root"));
        assert!(matches!(
            zip_design(tmp.path(), "x", 4).unwrap_err(),
            Error::TooLarge { .. }
        ));
    }

    #[test]
    fn slug_is_file_name_safe() {
        assert_eq!(slug("Landing Page — v2!"), "landing-page-v2");
        assert_eq!(slug("../../etc"), "etc");
        assert_eq!(slug("日本"), "design");
        assert!(slug(&"a".repeat(200)).len() <= 60);
    }

    #[test]
    fn local_reference_classification() {
        assert!(matches!(
            local_reference("a.css?v=1"),
            Some(LocalRef::File("a.css"))
        ));
        assert!(matches!(
            local_reference("./a.css"),
            Some(LocalRef::File("a.css"))
        ));
        assert!(matches!(
            local_reference("/_vendor/preact.js"),
            Some(LocalRef::Vendor("preact.js"))
        ));
        for remote in [
            "https://x/y",
            "//cdn/x",
            "/abs/x",
            "data:image/png;base64,",
            "#frag",
            "mailto:a@b",
            "javascript:alert(1)",
        ] {
            assert!(local_reference(remote).is_none(), "{remote}");
        }
    }
}
