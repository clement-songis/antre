//! `canvas.*` RPC methods: design listing, versions, preview links, revert
//! and publishing. Binary exports are REST-only (`/api/canvas/...`).

use std::sync::Arc;

use {
    antre_canvas::{
        CanvasService, DesignFilter, DesignId, FrameKind, FrameNetwork, NewComment, StageFrame,
        Visibility,
    },
    antre_protocol::{ErrorShape, error_codes},
    serde::{Deserialize, de::DeserializeOwned},
    serde_json::{Value, json},
};

use super::{MethodContext, MethodRegistry};

/// Map a canvas error to an RPC error shape.
pub(crate) fn error_shape(error: &antre_canvas::Error) -> ErrorShape {
    let code = if error.is_not_found() {
        error_codes::NOT_FOUND
    } else if error.is_client_error() {
        error_codes::INVALID_REQUEST
    } else if matches!(error, antre_canvas::Error::Unavailable(_)) {
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateParams {
    title: String,
    #[serde(default)]
    html: Option<String>,
    #[serde(default)]
    design_system: Option<String>,
    #[serde(default)]
    session_key: Option<String>,
    #[serde(default)]
    project_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StageParams {
    kind: FrameKind,
    html: String,
    #[serde(default)]
    network: FrameNetwork,
    #[serde(default)]
    host: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommentListParams {
    design_id: String,
    #[serde(default)]
    version: Option<u32>,
    #[serde(default)]
    include_resolved: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommentAddParams {
    design_id: String,
    #[serde(default)]
    version: Option<u32>,
    #[serde(flatten)]
    comment: NewComment,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommentResolveParams {
    design_id: String,
    comment_ids: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommentDeleteParams {
    design_id: String,
    comment_id: String,
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
                    "dir": to_value(&details.dir)?,
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
        "canvas.create",
        Box::new(|ctx| {
            Box::pin(async move {
                let service = service(&ctx)?;
                let params: CreateParams = params(&ctx)?;
                let created = service
                    .create(antre_canvas::service::CreateDesign {
                        title: params.title,
                        html: params.html,
                        design_system: params.design_system,
                        session_key: params.session_key,
                        project_id: params.project_id,
                        run_id: None,
                    })
                    .await
                    .map_err(|error| error_shape(&error))?;
                Ok(json!({
                    "design": to_value(&created.design)?,
                    "version": to_value(&created.version)?,
                }))
            })
        }),
    );

    reg.register(
        "canvas.stage",
        Box::new(|ctx| {
            Box::pin(async move {
                let service = service(&ctx)?;
                let params: StageParams = params(&ctx)?;
                let link = service
                    .stage_frame(
                        StageFrame {
                            kind: params.kind,
                            html: params.html,
                            network: params.network,
                        },
                        params.host.as_deref(),
                    )
                    .map_err(|error| error_shape(&error))?;
                to_value(&link)
            })
        }),
    );

    reg.register(
        "canvas.comments.list",
        Box::new(|ctx| {
            Box::pin(async move {
                let service = service(&ctx)?;
                let params: CommentListParams = params(&ctx)?;
                let comments = service
                    .comments(
                        &design_id(&params.design_id)?,
                        params.version,
                        params.include_resolved,
                    )
                    .await
                    .map_err(|error| error_shape(&error))?;
                Ok(json!({ "comments": to_value(&comments)? }))
            })
        }),
    );

    reg.register(
        "canvas.comments.add",
        Box::new(|ctx| {
            Box::pin(async move {
                let service = service(&ctx)?;
                let params: CommentAddParams = params(&ctx)?;
                let comment = service
                    .add_comment(
                        &design_id(&params.design_id)?,
                        params.version,
                        params.comment,
                    )
                    .await
                    .map_err(|error| error_shape(&error))?;
                Ok(json!({ "comment": to_value(&comment)? }))
            })
        }),
    );

    reg.register(
        "canvas.comments.resolve",
        Box::new(|ctx| {
            Box::pin(async move {
                let service = service(&ctx)?;
                let params: CommentResolveParams = params(&ctx)?;
                let resolved = service
                    .resolve_comments(&design_id(&params.design_id)?, &params.comment_ids)
                    .await
                    .map_err(|error| error_shape(&error))?;
                Ok(json!({ "resolved": resolved }))
            })
        }),
    );

    reg.register(
        "canvas.comments.delete",
        Box::new(|ctx| {
            Box::pin(async move {
                let service = service(&ctx)?;
                let params: CommentDeleteParams = params(&ctx)?;
                let deleted = service
                    .delete_comment(&design_id(&params.design_id)?, &params.comment_id)
                    .await
                    .map_err(|error| error_shape(&error))?;
                Ok(json!({ "deleted": deleted }))
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
            error_shape(&antre_canvas::Error::NotFound(id.to_string())).code,
            error_codes::NOT_FOUND
        );
        assert_eq!(
            error_shape(&antre_canvas::Error::InvalidId("x".into())).code,
            error_codes::INVALID_REQUEST
        );
        assert_eq!(
            error_shape(&antre_canvas::Error::Unavailable("x")).code,
            error_codes::UNAVAILABLE
        );
        assert_eq!(
            error_shape(&antre_canvas::Error::message("boom")).code,
            error_codes::INTERNAL
        );
    }

    #[test]
    fn stage_and_comment_params_parse() {
        let stage: StageParams = serde_json::from_value(json!({
            "kind": "mcp-app",
            "html": "<p>x</p>",
            "network": {"connectDomains": ["https://a.example.com"]},
        }))
        .unwrap();
        assert_eq!(stage.kind, FrameKind::McpApp);
        assert_eq!(stage.network.connect_domains, vec!["https://a.example.com"]);
        assert!(
            serde_json::from_value::<StageParams>(json!({"kind": "script", "html": "x"})).is_err()
        );

        let add: CommentAddParams = serde_json::from_value(json!({
            "designId": DesignId::generate(),
            "mid": "hero",
            "elementText": "Welcome",
            "body": "Bigger",
        }))
        .unwrap();
        assert_eq!(add.comment.mid.as_deref(), Some("hero"));
        assert_eq!(add.comment.element_text.as_deref(), Some("Welcome"));
        assert_eq!(add.version, None);
    }

    #[test]
    fn publish_defaults_to_private() {
        let params: PublishParams =
            serde_json::from_value(json!({"designId": DesignId::generate()})).unwrap();
        assert_eq!(params.visibility, Visibility::Private);
        assert!(serde_json::from_value::<PublishParams>(json!({})).is_err());
    }
}
