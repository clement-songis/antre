//! `/api/mods` REST routes. Reads need an authenticated session; anything
//! that changes which code runs needs `operator.admin`, like the RPC side.

use {
    antre_gateway::mods::{self as runtime, ModsApiError, ModsErrorKind},
    antre_httpd::{AppState, auth_middleware::RequireAdmin},
    axum::{
        Json, Router,
        extract::{Path, State},
        http::StatusCode,
        response::{IntoResponse, Response},
        routing::{get, post},
    },
    serde::Deserialize,
};

fn respond(result: Result<serde_json::Value, ModsApiError>) -> Response {
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => {
            let status = match error.kind {
                ModsErrorKind::BadRequest => StatusCode::BAD_REQUEST,
                ModsErrorKind::NotFound => StatusCode::NOT_FOUND,
                ModsErrorKind::Forbidden => StatusCode::FORBIDDEN,
                ModsErrorKind::Conflict => StatusCode::CONFLICT,
                ModsErrorKind::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
                ModsErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
            };
            (
                status,
                Json(serde_json::json!({ "code": error.code(), "error": error.message })),
            )
                .into_response()
        },
    }
}

#[derive(Deserialize)]
struct InstallBody {
    path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GrantBody {
    consent_hash: String,
    #[serde(default)]
    enable: bool,
}

async fn list_handler() -> Response {
    respond(runtime::list().await)
}

async fn get_handler(Path(id): Path<String>) -> Response {
    respond(runtime::get(&id).await)
}

async fn install_handler(
    _admin: RequireAdmin,
    State(state): State<AppState>,
    Json(body): Json<InstallBody>,
) -> Response {
    respond(runtime::install(&state.gateway, &body.path).await)
}

async fn grant_handler(
    _admin: RequireAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<GrantBody>,
) -> Response {
    respond(runtime::grant(&state.gateway, &id, &body.consent_hash, body.enable).await)
}

async fn revoke_handler(
    _admin: RequireAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    respond(runtime::revoke(&state.gateway, &id).await)
}

async fn enable_handler(
    _admin: RequireAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    respond(runtime::enable(&state.gateway, &id).await)
}

async fn disable_handler(
    _admin: RequireAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    respond(runtime::disable(&state.gateway, &id).await)
}

async fn uninstall_handler(
    _admin: RequireAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    respond(runtime::uninstall(&state.gateway, &id).await)
}

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/mods", get(list_handler))
        .route("/api/mods/install", post(install_handler))
        .route("/api/mods/{id}", get(get_handler).delete(uninstall_handler))
        .route("/api/mods/{id}/grant", post(grant_handler))
        .route("/api/mods/{id}/revoke", post(revoke_handler))
        .route("/api/mods/{id}/enable", post(enable_handler))
        .route("/api/mods/{id}/disable", post(disable_handler))
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_map_to_http_status() {
        let response = respond(Err(ModsApiError {
            kind: ModsErrorKind::Conflict,
            message: "consent required".into(),
        }));
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let response = respond(Err(ModsApiError {
            kind: ModsErrorKind::Unavailable,
            message: "off".into(),
        }));
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(respond(Ok(serde_json::json!({}))).status(), StatusCode::OK);
    }
}
