//! `canvas.*` RPC methods: design listing, versions, preview links, revert
//! and publishing. Binary exports are REST-only (`/api/canvas/...`).

use std::sync::Arc;

use {
    moltis_canvas::{CanvasService, DesignFilter, DesignId, Visibility},
    moltis_protocol::{ErrorShape, error_codes},
    serde::{Deserialize, de::DeserializeOwned},
    serde_json::{Value, json},
};

use super::{MethodContext, MethodRegistry};

/// Map a canvas error to an RPC error shape.
pub(crate) fn error_shape(error: &moltis_canvas::Error) -> ErrorShape {
    let code = if error.is_not_found() {
        error_codes::NOT_FOUND
    } else if error.is_client_error() {
        error_codes::INVALID_REQUEST
    } else if matches!(error, moltis_canvas::Error::Unavailable(_)) {
        error_codes::UNAVAILABLE
    } else {
        error_codes::INTERNAL
    };
    ErrorShape::new(code, error.to_string())
}

fn service(ctx: &MethodContext) -> Result<Arc<CanvasService>, ErrorShape> {
    ctx.state
        .services
        .canvas
        .clone()
        .ok_or_else(|| ErrorShape::new(error_codes::UNAVAILABLE, "canvas is disabled"))
}

fn params<T: DeserializeOwned>(ctx: &MethodContext) -> Result<T, ErrorShape> {
    serde_json::from_value(ctx.params.clone())
        .map_err(|error| ErrorShape::new(error_codes::INVALID_REQUEST, error.to_string()))
}

fn to_value<T: serde::Serialize>(value: &T) -> Result<Value, ErrorShape> {
    serde_json::to_value(value)
        .map_err(|error| ErrorShape::new(error_codes::INTERNAL, error.to_string()))
}

fn design_id(raw: &str) -> Result<DesignId, ErrorShape> {
    raw.parse().map_err(|error| error_shape(&error))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct HostParams {
    /// `location.host` of the web UI, so preview links use the same host.
    host: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct ListParams {
    session_key: Option<String>,
    project_id: Option<String>,
    limit: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DesignParams {
    design_id: String,
    #[serde(default)]
    version: Option<u32>,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    host: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RevertParams {
    design_id: String,
    version: u32,
    #[serde(default, rename = "_session_key")]
    session_key: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenameParams {
    design_id: String,
    title: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PublishParams {
    design_id: String,
    #[serde(default)]
    version: Option<u32>,
    #[serde(default = "default_visibility")]
    visibility: Visibility,
    #[serde(default)]
    host: Option<String>,
}

fn default_visibility() -> Visibility {
    Visibility::Private
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UnpublishParams {
    share_id: String,
}

/// Register the `canvas.*` namespace.
pub(super) fn register(reg: &mut MethodRegistry) {
    reg.register(
        "canvas.config",
        Box::new(|ctx| {
            Box::pin(async move {
                let service = service(&ctx)?;
                let params: HostParams = params(&ctx)?;
                to_value(&service.info(params.host.as_deref()).await)
            })
        }),
    );

    reg.register(
        "canvas.list",
        Box::new(|ctx| {
            Box::pin(async move {
                let service = service(&ctx)?;
                let params: ListParams = params(&ctx)?;
                let designs = service
                    .list(&DesignFilter {
                        session_key: params.session_key,
                        project_id: params.project_id,
                        limit: params.limit,
                    })
                    .await
                    .map_err(|error| error_shape(&error))?;
                Ok(json!({ "designs": to_value(&designs)? }))
            })
        }),
    );

    reg.register(
        "canvas.get",
        Box::new(|ctx| {
            Box::pin(async move {
                let service = service(&ctx)?;
                let params: DesignParams = params(&ctx)?;
                let details = service
                    .open(&design_id(&params.design_id)?)
                    .await
                    .map_err(|error| error_shape(&error))?;
                Ok(json!({
                    "design": to_value(&details.design)?,
                    "files": to_value(&details.files)?,
                    "versions": to_value(&details.versions)?,
                }))
            })
        }),
    );

    reg.register(
        "canvas.versions",
        Box::new(|ctx| {
            Box::pin(async move {
                let service = service(&ctx)?;
                let params: DesignParams = params(&ctx)?;
                let versions = service
                    .versions(&design_id(&params.design_id)?, params.limit.unwrap_or(100))
                    .await
                    .map_err(|error| error_shape(&error))?;
                Ok(json!({ "versions": to_value(&versions)? }))
            })
        }),
    );

    reg.register(
        "canvas.preview_url",
        Box::new(|ctx| {
            Box::pin(async move {
                let service = service(&ctx)?;
                let params: DesignParams = params(&ctx)?;
                let link = service
                    .preview_link(
                        &design_id(&params.design_id)?,
                        params.version,
                        params.host.as_deref(),
                    )
                    .await
                    .map_err(|error| error_shape(&error))?;
                to_value(&link)
            })
        }),
    );

    reg.register(
        "canvas.design_systems",
        Box::new(|ctx| {
            Box::pin(async move {
                let service = service(&ctx)?;
                let catalog = service.catalog().clone();
                let systems = tokio::task::spawn_blocking(move || catalog.list())
                    .await
                    .unwrap_or_default();
                Ok(json!({ "designSystems": to_value(&systems)? }))
            })
        }),
    );

    reg.register(
        "canvas.revert",
        Box::new(|ctx| {
            Box::pin(async move {
                let service = service(&ctx)?;
                let params: RevertParams = params(&ctx)?;
                let version = service
                    .revert(
                        &design_id(&params.design_id)?,
                        params.version,
                        params.session_key,
                    )
                    .await
                    .map_err(|error| error_shape(&error))?;
                Ok(json!({ "version": to_value(&version)? }))
            })
        }),
    );

    reg.register(
        "canvas.rename",
        Box::new(|ctx| {
            Box::pin(async move {
                let service = service(&ctx)?;
                let params: RenameParams = params(&ctx)?;
                let design = service
                    .rename(&design_id(&params.design_id)?, &params.title)
                    .await
                    .map_err(|error| error_shape(&error))?;
                Ok(json!({ "design": to_value(&design)? }))
            })
        }),
    );

    reg.register(
        "canvas.publish",
        Box::new(|ctx| {
            Box::pin(async move {
                let service = service(&ctx)?;
                let params: PublishParams = params(&ctx)?;
                let published = service
                    .publish(
                        &design_id(&params.design_id)?,
                        params.version,
                        params.visibility,
                        None,
                        params.host.as_deref(),
                    )
                    .await
                    .map_err(|error| error_shape(&error))?;
                to_value(&published)
            })
        }),
    );

    reg.register(
        "canvas.unpublish",
        Box::new(|ctx| {
            Box::pin(async move {
                let service = service(&ctx)?;
                let params: UnpublishParams = params(&ctx)?;
                let revoked = service
                    .unpublish(&params.share_id)
                    .await
                    .map_err(|error| error_shape(&error))?;
                Ok(json!({ "revoked": revoked }))
            })
        }),
    );
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn errors_map_to_protocol_codes() {
        let id = DesignId::generate();
        assert_eq!(
            error_shape(&moltis_canvas::Error::NotFound(id.to_string())).code,
            error_codes::NOT_FOUND
        );
        assert_eq!(
            error_shape(&moltis_canvas::Error::InvalidId("x".into())).code,
            error_codes::INVALID_REQUEST
        );
        assert_eq!(
            error_shape(&moltis_canvas::Error::Unavailable("x")).code,
            error_codes::UNAVAILABLE
        );
        assert_eq!(
            error_shape(&moltis_canvas::Error::message("boom")).code,
            error_codes::INTERNAL
        );
    }

    #[test]
    fn publish_defaults_to_private() {
        let params: PublishParams =
            serde_json::from_value(json!({"designId": DesignId::generate()})).unwrap();
        assert_eq!(params.visibility, Visibility::Private);
        assert!(serde_json::from_value::<PublishParams>(json!({})).is_err());
    }
}
