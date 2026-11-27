//! REST API of the design canvas: `/api/canvas/...`.
//!
//! Mirrors the `canvas.*` RPC methods and adds binary exports. Exports are
//! always sent as attachments with a CSP `sandbox` header so generated HTML
//! never runs on the app origin.

use std::sync::Arc;

use {
    axum::{
        Extension, Json, Router,
        body::Body,
        extract::{Path, Query, State},
        http::{HeaderMap, HeaderValue, StatusCode, header},
        response::{IntoResponse, Response},
        routing::{delete, get, post},
    },
    moltis_canvas::{
        CanvasService, DesignFilter, DesignId, Error as CanvasError, ExportFormat, Viewport,
        Visibility,
    },
    moltis_gateway::auth::AuthIdentity,
    moltis_protocol::scopes,
    serde::Deserialize,
    serde_json::json,
};

use crate::AppState;

#[derive(Clone, Copy)]
enum Access {
    Read,
    Write,
}

/// JSON error response, kept small so handler results stay cheap to move.
#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}

fn api_error(status: StatusCode, message: impl Into<String>) -> ApiError {
    ApiError {
        status,
        message: message.into(),
    }
}

fn canvas_error(error: &CanvasError) -> ApiError {
    let status = if error.is_not_found() {
        StatusCode::NOT_FOUND
    } else if error.is_client_error() {
        StatusCode::BAD_REQUEST
    } else if matches!(error, CanvasError::Unavailable(_)) {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        tracing::warn!(%error, "canvas request failed");
        StatusCode::INTERNAL_SERVER_ERROR
    };
    api_error(status, error.to_string())
}

fn require(identity: Option<&AuthIdentity>, access: Access) -> Result<(), ApiError> {
    let Some(identity) = identity else {
        return Ok(());
    };
    let required = match access {
        Access::Read => scopes::READ,
        Access::Write => scopes::WRITE,
    };
    if identity.has_scope(required) || identity.has_scope(scopes::ADMIN) {
        Ok(())
    } else {
        Err(api_error(
            StatusCode::FORBIDDEN,
            "insufficient scope for canvas operation",
        ))
    }
}

fn service(state: &AppState) -> Result<Arc<CanvasService>, ApiError> {
    state
        .gateway
        .services
        .canvas
        .clone()
        .ok_or_else(|| api_error(StatusCode::SERVICE_UNAVAILABLE, "canvas is disabled"))
}

fn design_id(raw: &str) -> Result<DesignId, ApiError> {
    raw.parse().map_err(|error| canvas_error(&error))
}

fn host(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
}

fn json_ok<T: serde::Serialize>(value: &T) -> Response {
    match serde_json::to_value(value) {
        Ok(value) => Json(value).into_response(),
        Err(error) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response()
        },
    }
}

/// Turn a handler body's result into a response.
fn respond(result: Result<Response, ApiError>) -> Response {
    result.unwrap_or_else(IntoResponse::into_response)
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct ListQuery {
    session_key: Option<String>,
    project_id: Option<String>,
    limit: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct VersionQuery {
    version: Option<u32>,
    limit: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct ExportQuery {
    version: Option<u32>,
    width: Option<u32>,
    height: Option<u32>,
    full_page: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct RevertBody {
    version: u32,
}

#[derive(Debug, Deserialize)]
struct PublishBody {
    #[serde(default)]
    version: Option<u32>,
    #[serde(default = "default_visibility")]
    visibility: Visibility,
}

fn default_visibility() -> Visibility {
    Visibility::Private
}

type Identity = Option<Extension<AuthIdentity>>;

fn identity_ref(identity: &Identity) -> Option<&AuthIdentity> {
    identity.as_ref().map(|Extension(identity)| identity)
}

async fn config_handler(
    State(state): State<AppState>,
    identity: Identity,
    headers: HeaderMap,
) -> Response {
    respond(
        async {
            require(identity_ref(&identity), Access::Read)?;
            Ok(json_ok(&service(&state)?.info(host(&headers)).await))
        }
        .await,
    )
}

async fn list_handler(
    State(state): State<AppState>,
    identity: Identity,
    Query(query): Query<ListQuery>,
) -> Response {
    respond(
        async {
            require(identity_ref(&identity), Access::Read)?;
            let designs = service(&state)?
                .list(&DesignFilter {
                    session_key: query.session_key,
                    project_id: query.project_id,
                    limit: query.limit,
                })
                .await
                .map_err(|error| canvas_error(&error))?;
            Ok(json_ok(&json!({ "designs": designs })))
        }
        .await,
    )
}

async fn get_handler(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
) -> Response {
    respond(
        async {
            require(identity_ref(&identity), Access::Read)?;
            let details = service(&state)?
                .open(&design_id(&id)?)
                .await
                .map_err(|error| canvas_error(&error))?;
            Ok(json_ok(&json!({
                "design": details.design,
                "files": details.files,
                "versions": details.versions,
            })))
        }
        .await,
    )
}

async fn versions_handler(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
    Query(query): Query<VersionQuery>,
) -> Response {
    respond(
        async {
            require(identity_ref(&identity), Access::Read)?;
            let versions = service(&state)?
                .versions(&design_id(&id)?, query.limit.unwrap_or(100))
                .await
                .map_err(|error| canvas_error(&error))?;
            Ok(json_ok(&json!({ "versions": versions })))
        }
        .await,
    )
}

async fn preview_url_handler(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
    Query(query): Query<VersionQuery>,
    headers: HeaderMap,
) -> Response {
    respond(
        async {
            require(identity_ref(&identity), Access::Read)?;
            let link = service(&state)?
                .preview_link(&design_id(&id)?, query.version, host(&headers))
                .await
                .map_err(|error| canvas_error(&error))?;
            Ok(json_ok(&link))
        }
        .await,
    )
}

async fn revert_handler(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
    Json(body): Json<RevertBody>,
) -> Response {
    respond(
        async {
            require(identity_ref(&identity), Access::Write)?;
            let version = service(&state)?
                .revert(&design_id(&id)?, body.version, None)
                .await
                .map_err(|error| canvas_error(&error))?;
            Ok(json_ok(&json!({ "version": version })))
        }
        .await,
    )
}

/// Headers of a downloaded export.
fn attachment(bytes: Vec<u8>, content_type: &'static str, file_name: &str) -> Response {
    let mut response = Response::new(Body::from(bytes));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    let safe_name: String = file_name
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'))
        .collect();
    if let Ok(value) = HeaderValue::from_str(&format!("attachment; filename=\"{safe_name}\"")) {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("sandbox; default-src 'none'"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn export_handler(
    State(state): State<AppState>,
    identity: Identity,
    Path((id, format)): Path<(String, String)>,
    Query(query): Query<ExportQuery>,
) -> Response {
    respond(
        async {
            require(identity_ref(&identity), Access::Read)?;
            let format: ExportFormat = format.parse().map_err(|error| canvas_error(&error))?;
            let defaults = Viewport::default();
            let viewport = Viewport {
                width: query.width.unwrap_or(defaults.width).clamp(200, 3840),
                height: query.height.unwrap_or(defaults.height).clamp(200, 4000),
                full_page: query.full_page.unwrap_or(defaults.full_page),
            };
            let export = service(&state)?
                .export(&design_id(&id)?, query.version, format, viewport)
                .await
                .map_err(|error| canvas_error(&error))?;
            Ok(attachment(
                export.bytes,
                export.content_type,
                &export.file_name,
            ))
        }
        .await,
    )
}

async fn publish_handler(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<PublishBody>,
) -> Response {
    respond(
        async {
            require(identity_ref(&identity), Access::Write)?;
            let published = service(&state)?
                .publish(
                    &design_id(&id)?,
                    body.version,
                    body.visibility,
                    None,
                    host(&headers),
                )
                .await
                .map_err(|error| canvas_error(&error))?;
            Ok(json_ok(&published))
        }
        .await,
    )
}

async fn unpublish_handler(
    State(state): State<AppState>,
    identity: Identity,
    Path(share_id): Path<String>,
) -> Response {
    respond(
        async {
            require(identity_ref(&identity), Access::Write)?;
            let revoked = service(&state)?
                .unpublish(&share_id)
                .await
                .map_err(|error| canvas_error(&error))?;
            if !revoked {
                return Err(api_error(StatusCode::NOT_FOUND, "share not found"));
            }
            Ok(json_ok(&json!({ "revoked": true })))
        }
        .await,
    )
}

/// Router nested at `/api/canvas`.
pub fn canvas_router() -> Router<AppState> {
    Router::new()
        .route("/config", get(config_handler))
        .route("/designs", get(list_handler))
        .route("/designs/{id}", get(get_handler))
        .route("/designs/{id}/versions", get(versions_handler))
        .route("/designs/{id}/preview-url", get(preview_url_handler))
        .route("/designs/{id}/revert", post(revert_handler))
        .route("/designs/{id}/export/{format}", get(export_handler))
        .route("/designs/{id}/publish", post(publish_handler))
        .route("/shares/{share_id}", delete(unpublish_handler))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use {
        super::*,
        moltis_gateway::auth::{AuthIdentity, AuthMethod},
    };

    fn identity(scopes: &[&str]) -> AuthIdentity {
        AuthIdentity {
            method: AuthMethod::ApiKey,
            scopes: scopes.iter().map(|scope| (*scope).to_string()).collect(),
        }
    }

    #[test]
    fn scopes_gate_reads_and_writes() {
        let read = identity(&[scopes::READ]);
        let write = identity(&[scopes::WRITE]);
        let admin = identity(&[scopes::ADMIN]);
        assert!(require(Some(&read), Access::Read).is_ok());
        assert_eq!(
            require(Some(&read), Access::Write).unwrap_err().status,
            StatusCode::FORBIDDEN
        );
        assert!(require(Some(&write), Access::Write).is_ok());
        assert!(require(Some(&admin), Access::Write).is_ok());
        assert!(require(None, Access::Write).is_ok());
    }

    #[test]
    fn errors_map_to_statuses() {
        let id = DesignId::generate();
        assert_eq!(
            canvas_error(&CanvasError::NotFound(id.to_string())).status,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            canvas_error(&CanvasError::InvalidId("x".into())).status,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            canvas_error(&CanvasError::Unavailable("x")).status,
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            canvas_error(&CanvasError::message("boom")).status,
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn exports_download_sandboxed_with_safe_names() {
        let response = attachment(
            b"<script>alert(1)</script>".to_vec(),
            "text/html; charset=utf-8",
            "evil\"\r\nX: y/../name.html",
        );
        let headers = response.headers();
        let disposition = headers[header::CONTENT_DISPOSITION].to_str().unwrap();
        assert_eq!(disposition, "attachment; filename=\"evilXy..name.html\"");
        assert_eq!(
            headers[header::CONTENT_SECURITY_POLICY],
            "sandbox; default-src 'none'"
        );
        assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    }

    #[test]
    fn publish_body_defaults_to_private() {
        let body: PublishBody = serde_json::from_str("{}").unwrap();
        assert_eq!(body.visibility, Visibility::Private);
        assert!(body.version.is_none());
    }
}
