//! Content-Security-Policy and response headers of the render origin.
//!
//! Every design page is served with a CSP `sandbox` directive (without
//! `allow-same-origin`), so it runs in an opaque origin even when opened
//! top-level, not only inside the preview iframe. See `docs/src/canvas.md`.

use axum::http::{HeaderMap, HeaderName, HeaderValue, header};

/// Sandbox flags shared by the CSP directive and the preview `<iframe>`.
/// No `allow-same-origin`, `allow-forms`, `allow-popups` or
/// `allow-top-navigation`.
pub const SANDBOX_FLAGS: &str = "allow-scripts";

/// Path prefix of vendored scripts on the render origin.
pub const VENDOR_PREFIX: &str = "/_vendor/";

/// How a page may be framed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framing {
    /// Live and version previews: framed by the web UI, whose public origin
    /// the server cannot know for sure. The signed link is the protection.
    AnyAncestor,
    /// Published pages: top-level only.
    TopLevelOnly,
    /// Published content: only inside the render origin's own wrapper page.
    RenderOrigin,
}

/// Inputs of a design page policy.
#[derive(Debug, Clone)]
pub struct CspOptions<'a> {
    /// Origin of the render server as seen by the browser
    /// (`http://127.0.0.1:18790`).
    pub origin: &'a str,
    /// Path prefix of the design's own files (`/d/<token>/`), allowed for
    /// scripts. `None` for single-file pages.
    pub design_path: Option<&'a str>,
    /// Extra validated `https://` origins for `script-src`.
    pub extra_script_sources: &'a [String],
    pub allow_eval: bool,
    pub framing: Framing,
    /// `https://` origins an MCP App declared for `connect-src`.
    pub connect_sources: &'a [String],
    /// `https://` origins an MCP App declared for scripts, styles, images,
    /// fonts and media.
    pub resource_sources: &'a [String],
}

fn valid_sources(sources: &[String]) -> impl Iterator<Item = &str> {
    sources
        .iter()
        .map(String::as_str)
        .filter(|source| moltis_config::schema::is_https_origin(source))
}

fn with_sources(base: &str, sources: &[String]) -> String {
    std::iter::once(base)
        .chain(valid_sources(sources))
        .collect::<Vec<_>>()
        .join(" ")
}

/// CSP of a design page on the render origin.
#[must_use]
pub fn design_policy(options: &CspOptions<'_>) -> String {
    let origin = options.origin;
    let mut scripts = vec![
        "'unsafe-inline'".to_string(),
        format!("{origin}{VENDOR_PREFIX}"),
    ];
    if let Some(path) = options.design_path {
        scripts.push(format!("{origin}{path}"));
    }
    if options.allow_eval {
        scripts.push("'unsafe-eval'".to_string());
    }
    scripts.extend(valid_sources(options.extra_script_sources).map(str::to_string));
    scripts.extend(valid_sources(options.resource_sources).map(str::to_string));
    let resources = options.resource_sources;
    let connect = if valid_sources(options.connect_sources).next().is_some() {
        valid_sources(options.connect_sources)
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        "'none'".to_string()
    };
    let frame_ancestors = match options.framing {
        Framing::AnyAncestor => "*".to_string(),
        Framing::TopLevelOnly => "'none'".to_string(),
        Framing::RenderOrigin => origin.to_string(),
    };
    [
        format!("sandbox {SANDBOX_FLAGS}"),
        "default-src 'none'".to_string(),
        format!("script-src {}", scripts.join(" ")),
        format!(
            "style-src {}",
            with_sources(&format!("'unsafe-inline' {origin}"), resources)
        ),
        format!(
            "img-src {}",
            with_sources(&format!("{origin} data: blob:"), resources)
        ),
        format!(
            "font-src {}",
            with_sources(&format!("{origin} data:"), resources)
        ),
        format!(
            "media-src {}",
            with_sources(&format!("{origin} data: blob:"), resources)
        ),
        format!("connect-src {connect}"),
        "worker-src 'none'".to_string(),
        "frame-src 'none'".to_string(),
        "child-src 'none'".to_string(),
        "manifest-src 'none'".to_string(),
        "object-src 'none'".to_string(),
        "base-uri 'none'".to_string(),
        "form-action 'none'".to_string(),
        format!("frame-ancestors {frame_ancestors}"),
    ]
    .join("; ")
}

/// CSP embedded as a `<meta>` tag in documents rendered headlessly to PNG or
/// PDF: everything is inlined, so nothing may load from the network.
pub const OFFLINE_META_POLICY: &str = "default-src 'none'; script-src 'unsafe-inline'; \
     style-src 'unsafe-inline'; img-src data: blob:; font-src data:; media-src data: blob:; \
     connect-src 'none'; worker-src 'none'; frame-src 'none'; object-src 'none'; \
     base-uri 'none'; form-action 'none'";

/// Policy for non-HTML responses: nothing may execute or embed.
pub const ASSET_POLICY: &str = "default-src 'none'; sandbox; frame-ancestors 'none'";

/// Headers sent with every render-origin response.
pub fn apply_common_headers(headers: &mut HeaderMap) {
    let fixed: [(HeaderName, &str); 6] = [
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::REFERRER_POLICY, "no-referrer"),
        (header::CACHE_CONTROL, "no-store"),
        (
            HeaderName::from_static("x-robots-tag"),
            "noindex, nofollow, noarchive",
        ),
        // Opaque-origin documents loading their own assets are cross-origin
        // requests; `same-site` would block them.
        (
            HeaderName::from_static("cross-origin-resource-policy"),
            "cross-origin",
        ),
        (
            HeaderName::from_static("permissions-policy"),
            "camera=(), microphone=(), geolocation=(), usb=(), serial=(), hid=(), \
             bluetooth=(), payment=(), clipboard-read=(), display-capture=()",
        ),
    ];
    for (name, value) in fixed {
        headers.insert(name, HeaderValue::from_static(value));
    }
    headers.remove(header::SET_COOKIE);
}

/// Set the CSP header; values that are not valid header text are replaced by
/// the most restrictive policy instead of being dropped.
pub fn apply_policy(headers: &mut HeaderMap, policy: &str) {
    let value =
        HeaderValue::from_str(policy).unwrap_or_else(|_| HeaderValue::from_static(ASSET_POLICY));
    headers.insert(header::CONTENT_SECURITY_POLICY, value);
}

/// Host part of a `Host` header (`example.com:8080` → `example.com`,
/// `[::1]:9` → `[::1]`), or `None` if it holds unexpected characters.
#[must_use]
pub fn host_without_port(host: &str) -> Option<&str> {
    let host = host.trim();
    let name = if host.starts_with('[') {
        &host[..=host.find(']')?]
    } else {
        host.rsplit_once(':').map_or(host, |(name, _)| name)
    };
    let valid = !name.is_empty()
        && name.len() <= 253
        && name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b':' | b'[' | b']')
        });
    valid.then_some(name)
}

/// Origin of the render server for a request: the configured public URL, or
/// the scheme and `Host` header the browser used. Hosts with characters that
/// could break out of a CSP source expression are refused.
#[must_use]
pub fn request_origin(public_url: Option<&str>, host: Option<&str>) -> Option<String> {
    if let Some(url) = public_url.filter(|url| moltis_config::schema::is_http_origin(url)) {
        return Some(url.trim_end_matches('/').to_string());
    }
    let host = host?.trim();
    let valid = !host.is_empty()
        && host.len() <= 255
        && host.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b':' | b'[' | b']')
        });
    valid.then(|| format!("http://{host}"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn directives(policy: &str) -> Vec<String> {
        policy.split(';').map(|d| d.trim().to_string()).collect()
    }

    fn options<'a>(extra: &'a [String]) -> CspOptions<'a> {
        CspOptions {
            origin: "http://127.0.0.1:18790",
            design_path: Some("/d/tok/"),
            extra_script_sources: extra,
            allow_eval: false,
            framing: Framing::AnyAncestor,
            connect_sources: &[],
            resource_sources: &[],
        }
    }

    #[test]
    fn declared_app_domains_are_validated() {
        let connect = vec!["https://api.example.com".to_string(), "*".to_string()];
        let resources = vec![
            "https://cdn.example.com".to_string(),
            "https://x; script-src *".to_string(),
        ];
        let mut opts = options(&[]);
        opts.connect_sources = &connect;
        opts.resource_sources = &resources;
        let policy = design_policy(&opts);
        let parts = directives(&policy);
        assert!(parts.contains(&"connect-src https://api.example.com".to_string()));
        for prefix in [
            "script-src",
            "style-src",
            "img-src",
            "font-src",
            "media-src",
        ] {
            let directive = parts.iter().find(|d| d.starts_with(prefix)).unwrap();
            assert!(
                directive.ends_with("https://cdn.example.com"),
                "{directive}"
            );
        }
        assert!(!policy.contains("script-src *"));
        assert!(!policy.contains("connect-src *"));

        let invalid_only = vec!["http://plain.example.com".to_string()];
        opts.connect_sources = &invalid_only;
        assert!(directives(&design_policy(&opts)).contains(&"connect-src 'none'".to_string()));
    }

    #[test]
    fn design_policy_locks_down_network_forms_and_navigation() {
        let policy = design_policy(&options(&[]));
        let parts = directives(&policy);
        for required in [
            "sandbox allow-scripts",
            "default-src 'none'",
            "connect-src 'none'",
            "form-action 'none'",
            "base-uri 'none'",
            "object-src 'none'",
            "worker-src 'none'",
            "frame-src 'none'",
            "frame-ancestors *",
            "img-src http://127.0.0.1:18790 data: blob:",
        ] {
            assert!(
                parts.iter().any(|p| p == required),
                "missing {required} in {policy}"
            );
        }
        assert!(!policy.contains("allow-same-origin"));
        assert!(!policy.contains("allow-top-navigation"));
        assert!(!policy.contains("allow-forms"));
        assert!(!policy.contains("allow-popups"));
        assert!(!policy.contains("unsafe-eval"));
        assert!(!policy.contains("https:"), "no wildcard scheme sources");
    }

    #[test]
    fn script_src_is_vendor_and_design_only_by_default() {
        let policy = design_policy(&options(&[]));
        let script = directives(&policy)
            .into_iter()
            .find(|d| d.starts_with("script-src"))
            .unwrap();
        assert_eq!(
            script,
            "script-src 'unsafe-inline' http://127.0.0.1:18790/_vendor/ http://127.0.0.1:18790/d/tok/"
        );
    }

    #[test]
    fn extra_sources_are_filtered_and_eval_is_opt_in() {
        let extra = vec![
            "https://cdn.example.com".to_string(),
            "https://x; connect-src *".to_string(),
            "*".to_string(),
        ];
        let mut opts = options(&extra);
        opts.allow_eval = true;
        opts.framing = Framing::TopLevelOnly;
        let policy = design_policy(&opts);
        assert!(policy.contains("https://cdn.example.com"));
        assert!(!policy.contains("connect-src *"));
        assert!(policy.contains("'unsafe-eval'"));
        assert!(policy.contains("frame-ancestors 'none'"));
        assert_eq!(
            directives(&policy)
                .iter()
                .filter(|d| d.starts_with("connect-src"))
                .count(),
            1
        );
    }

    #[test]
    fn offline_policy_blocks_all_network() {
        let parts = directives(OFFLINE_META_POLICY);
        assert!(parts.contains(&"connect-src 'none'".to_string()));
        assert!(parts.contains(&"img-src data: blob:".to_string()));
        assert!(!OFFLINE_META_POLICY.contains("http"));
    }

    #[test]
    fn common_headers_are_set_and_cookies_stripped() {
        let mut headers = HeaderMap::new();
        headers.insert(header::SET_COOKIE, HeaderValue::from_static("a=b"));
        apply_common_headers(&mut headers);
        apply_policy(&mut headers, "default-src 'none'");
        assert!(headers.get(header::SET_COOKIE).is_none());
        assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(headers[header::REFERRER_POLICY], "no-referrer");
        assert_eq!(headers["cross-origin-resource-policy"], "cross-origin");
        assert_eq!(
            headers[header::CONTENT_SECURITY_POLICY],
            "default-src 'none'"
        );
    }

    #[test]
    fn invalid_policy_falls_back_to_most_restrictive() {
        let mut headers = HeaderMap::new();
        apply_policy(&mut headers, "default-src 'none'\r\nX-Evil: 1");
        assert_eq!(headers[header::CONTENT_SECURITY_POLICY], ASSET_POLICY);
    }

    #[test]
    fn host_without_port_strips_ports_and_validates() {
        assert_eq!(host_without_port("example.com:8080"), Some("example.com"));
        assert_eq!(host_without_port("example.com"), Some("example.com"));
        assert_eq!(host_without_port("[::1]:9"), Some("[::1]"));
        assert_eq!(host_without_port("[::1]"), Some("[::1]"));
        assert_eq!(host_without_port("evil.com/x:1"), None);
        assert_eq!(host_without_port("a b:1"), None);
        assert_eq!(host_without_port(""), None);
        assert_eq!(host_without_port("[::1"), None);
    }

    #[test]
    fn render_origin_framing_names_the_origin() {
        let mut opts = options(&[]);
        opts.framing = Framing::RenderOrigin;
        assert!(design_policy(&opts).contains("frame-ancestors http://127.0.0.1:18790"));
    }

    #[test]
    fn request_origin_prefers_public_url_and_validates_host() {
        assert_eq!(
            request_origin(Some("https://designs.example.com/"), Some("evil")).unwrap(),
            "https://designs.example.com"
        );
        assert_eq!(
            request_origin(None, Some("127.0.0.1:18790")).unwrap(),
            "http://127.0.0.1:18790"
        );
        assert_eq!(
            request_origin(None, Some("[::1]:9")).unwrap(),
            "http://[::1]:9"
        );
        assert!(request_origin(None, Some("a; script-src *")).is_none());
        assert!(request_origin(None, Some("a b")).is_none());
        assert!(request_origin(None, None).is_none());
        assert!(request_origin(Some("not a url"), None).is_none());
    }
}
