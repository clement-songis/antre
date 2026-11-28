//! The render origin: a separate HTTP listener that serves designs.
//!
//! It shares nothing with the app origin: no cookies are read or set, no
//! `/api` routes exist, and access is granted only by signed links
//! (`/d/<token>/...`, `/f/<token>/`) or published share ids (`/p/<id>`).

use std::{net::SocketAddr, sync::Arc};

use {
    axum::{
        Router,
        body::Body,
        extract::{Path, Query, State},
        http::{HeaderMap, HeaderValue, StatusCode, header},
        response::{IntoResponse, Redirect, Response},
        routing::get,
    },
    serde::Deserialize,
    tracing::{debug, info, warn},
};

use crate::{
    CanvasService, Error, Result,
    bridge::inject_bridge,
    csp::{
        ASSET_POLICY, CspOptions, Framing, SANDBOX_FLAGS, apply_common_headers, apply_policy,
        design_policy, request_origin,
    },
    export::ENTRY_FILE,
    frames::FrameKind,
    publish::valid_share_id,
};

type AppState = Arc<CanvasService>;

/// Router of the render origin.
pub fn router(service: Arc<CanvasService>) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/_vendor/{name}", get(vendor_handler))
        .route("/d/{token}", get(design_root_redirect))
        .route("/d/{token}/", get(design_index_handler))
        .route("/d/{token}/{*path}", get(design_file_handler))
        .route("/f/{token}", get(frame_root_redirect))
        .route("/f/{token}/", get(frame_handler))
        .route("/p/{share_id}", get(published_wrapper_handler))
        .route("/p/{share_id}/content", get(published_content_handler))
        .fallback(|| async { not_found() })
        .with_state(service)
}

/// Bind the render origin and serve it in the background.
pub async fn spawn(
    service: Arc<CanvasService>,
    address: SocketAddr,
) -> Result<(SocketAddr, tokio::task::JoinHandle<()>)> {
    let listener = tokio::net::TcpListener::bind(address).await?;
    let bound = listener.local_addr()?;
    service.set_render_address(bound);
    let app = router(service);
    let handle = tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, app).await {
            warn!(%error, "canvas render origin stopped");
        }
    });
    info!(address = %bound, "canvas render origin listening");
    Ok((bound, handle))
}

fn finish(mut response: Response, policy: &str) -> Response {
    let headers = response.headers_mut();
    apply_common_headers(headers);
    apply_policy(headers, policy);
    response
}

fn not_found() -> Response {
    #[cfg(feature = "metrics")]
    moltis_metrics::counter!(moltis_metrics::canvas::REQUESTS_REJECTED_TOTAL).increment(1);
    finish(
        (StatusCode::NOT_FOUND, "not found").into_response(),
        ASSET_POLICY,
    )
}

fn error_response(error: &Error) -> Response {
    if error.is_client_error() || error.is_not_found() {
        debug!(%error, "render origin request refused");
        return not_found();
    }
    warn!(%error, "render origin request failed");
    finish(
        (StatusCode::INTERNAL_SERVER_ERROR, "error").into_response(),
        ASSET_POLICY,
    )
}

fn host(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
}

fn origin_for(service: &CanvasService, headers: &HeaderMap) -> Option<String> {
    request_origin(service.settings.public_url.as_deref(), host(headers))
}

fn bytes_response(bytes: Vec<u8>, content_type: &str) -> Response {
    let mut response = Response::new(Body::from(bytes));
    if let Ok(value) = HeaderValue::from_str(content_type) {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    response
}

async fn vendor_handler(State(service): State<AppState>, Path(name): Path<String>) -> Response {
    match service.vendor.get(&name).await {
        Some(file) => finish(bytes_response(file.bytes, file.content_type), ASSET_POLICY),
        None => not_found(),
    }
}

async fn design_root_redirect(Path(token): Path<String>) -> Response {
    // Relative asset URLs only resolve under the trailing slash.
    finish(
        Redirect::permanent(&format!("/d/{token}/")).into_response(),
        ASSET_POLICY,
    )
}

async fn design_index_handler(
    State(service): State<AppState>,
    Path(token): Path<String>,
    headers: HeaderMap,
) -> Response {
    serve_design_file(&service, &token, ENTRY_FILE, &headers).await
}

async fn design_file_handler(
    State(service): State<AppState>,
    Path((token, path)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let path = if path.is_empty() || path.ends_with('/') {
        format!("{path}{ENTRY_FILE}")
    } else {
        path
    };
    serve_design_file(&service, &token, &path, &headers).await
}

async fn serve_design_file(
    service: &CanvasService,
    token: &str,
    path: &str,
    headers: &HeaderMap,
) -> Response {
    #[cfg(feature = "metrics")]
    let started = std::time::Instant::now();
    let Ok(claims) = service.verify_token(token) else {
        return not_found();
    };
    let Some(origin) = origin_for(service, headers) else {
        return not_found();
    };
    let file = match service.serve_file(&claims, path).await {
        Ok(file) => file,
        Err(error) => return error_response(&error),
    };
    #[cfg(feature = "metrics")]
    moltis_metrics::histogram!(moltis_metrics::canvas::CONTENT_SIZE_BYTES)
        .record(file.bytes.len() as f64);
    let policy = if file.is_html {
        let design_path = format!("/d/{token}/");
        design_policy(&CspOptions {
            origin: &origin,
            design_path: Some(&design_path),
            extra_script_sources: &service.settings.script_sources,
            allow_eval: service.settings.allow_eval,
            framing: Framing::AnyAncestor,
            connect_sources: &[],
            resource_sources: &[],
        })
    } else {
        ASSET_POLICY.to_string()
    };
    let bytes = if file.is_html {
        with_bridge(file.bytes)
    } else {
        file.bytes
    };
    let mut response = finish(bytes_response(bytes, &file.content_type), &policy);
    if is_font(&file.content_type) {
        // Fonts are fetched in CORS mode and the sandboxed page's origin is
        // opaque (`Origin: null`).
        response.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_static("*"),
        );
    }
    #[cfg(feature = "metrics")]
    moltis_metrics::histogram!(moltis_metrics::canvas::PAGE_SERVE_DURATION_SECONDS)
        .record(started.elapsed().as_secs_f64());
    response
}

/// Previewed HTML with the bridge script; non-UTF-8 documents are served
/// unchanged rather than corrupted.
fn with_bridge(bytes: Vec<u8>) -> Vec<u8> {
    match String::from_utf8(bytes) {
        Ok(html) => inject_bridge(&html).into_bytes(),
        Err(error) => error.into_bytes(),
    }
}

async fn frame_root_redirect(Path(token): Path<String>) -> Response {
    finish(
        Redirect::permanent(&format!("/f/{token}/")).into_response(),
        ASSET_POLICY,
    )
}

async fn frame_handler(
    State(service): State<AppState>,
    Path(token): Path<String>,
    headers: HeaderMap,
) -> Response {
    let frame = match service.frame_for_token(&token) {
        Ok(frame) => frame,
        Err(error) => return error_response(&error),
    };
    let Some(origin) = origin_for(&service, &headers) else {
        return not_found();
    };
    let (body, network) = match frame.kind {
        FrameKind::Artifact => (inject_bridge(&frame.html), None),
        FrameKind::McpApp => (frame.html.clone(), Some(&frame.network)),
    };
    let policy = design_policy(&CspOptions {
        origin: &origin,
        design_path: None,
        extra_script_sources: &service.settings.script_sources,
        allow_eval: service.settings.allow_eval,
        framing: Framing::AnyAncestor,
        connect_sources: network.map_or(&[], |network| &network.connect_domains),
        resource_sources: network.map_or(&[], |network| &network.resource_domains),
    });
    finish(
        bytes_response(body.into_bytes(), "text/html; charset=utf-8"),
        &policy,
    )
}

fn is_font(content_type: &str) -> bool {
    content_type.starts_with("font/") || content_type == "application/vnd.ms-fontobject"
}

#[derive(Debug, Deserialize)]
struct AccessQuery {
    k: Option<String>,
}

fn valid_access_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 128
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn escape_attr(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

async fn published_wrapper_handler(
    State(service): State<AppState>,
    Path(share_id): Path<String>,
    Query(query): Query<AccessQuery>,
    headers: HeaderMap,
) -> Response {
    let key = query.k.as_deref().filter(|key| valid_access_key(key));
    let Some(origin) = origin_for(&service, &headers) else {
        return not_found();
    };
    let snapshot = match service.published(&share_id, key).await {
        Ok(Some(snapshot)) => snapshot,
        Ok(None) => return not_found(),
        Err(error) => return error_response(&error),
    };
    let content_url = match key {
        Some(key) => format!("/p/{share_id}/content?k={key}"),
        None => format!("/p/{share_id}/content"),
    };
    let title = escape_attr(&snapshot.title);
    let body = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<meta name=\"robots\" content=\"noindex, nofollow\">\
<title>{title}</title><style>\
html,body{{margin:0;height:100%;font:13px/1.4 system-ui,sans-serif}}\
body{{display:flex;flex-direction:column}}\
.notice{{padding:6px 12px;background:#1f2937;color:#f9fafb}}\
iframe{{flex:1;border:0;width:100%}}</style></head><body>\
<div class=\"notice\" role=\"note\">Shared design generated by an AI assistant \
(version {version}). Do not enter passwords or personal information.</div>\
<iframe sandbox=\"{SANDBOX_FLAGS}\" referrerpolicy=\"no-referrer\" title=\"{title}\" \
src=\"{src}\"></iframe></body></html>",
        version = snapshot.version,
        src = escape_attr(&content_url),
    );
    let policy = format!(
        "default-src 'none'; style-src 'unsafe-inline'; frame-src {origin}; \
         base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
    );
    finish(
        bytes_response(body.into_bytes(), "text/html; charset=utf-8"),
        &policy,
    )
}

async fn published_content_handler(
    State(service): State<AppState>,
    Path(share_id): Path<String>,
    Query(query): Query<AccessQuery>,
    headers: HeaderMap,
) -> Response {
    if !valid_share_id(&share_id) {
        return not_found();
    }
    let key = query.k.as_deref().filter(|key| valid_access_key(key));
    let Some(origin) = origin_for(&service, &headers) else {
        return not_found();
    };
    let snapshot = match service.published(&share_id, key).await {
        Ok(Some(snapshot)) => snapshot,
        Ok(None) => return not_found(),
        Err(error) => return error_response(&error),
    };
    let policy = design_policy(&CspOptions {
        origin: &origin,
        design_path: None,
        extra_script_sources: &service.settings.script_sources,
        allow_eval: service.settings.allow_eval,
        framing: Framing::RenderOrigin,
        connect_sources: &[],
        resource_sources: &[],
    });
    finish(
        bytes_response(snapshot.html.into_bytes(), "text/html; charset=utf-8"),
        &policy,
    )
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
