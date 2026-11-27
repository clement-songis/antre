//! `mods.*` RPC methods. `mods.list` and `mods.get` are reads; everything
//! else changes what code runs and is admin-only (not listed in the scope
//! tables in `dispatch.rs`).

use super::*;

use crate::mods::{self as runtime, ModsApiError};

fn to_error(error: ModsApiError) -> ErrorShape {
    ErrorShape::new(error.code(), error.message)
}

fn string_param(ctx: &MethodContext, key: &str) -> Result<String, ErrorShape> {
    ctx.params
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| ErrorShape::new(error_codes::INVALID_REQUEST, format!("missing {key}")))
}

pub(super) fn register(reg: &mut MethodRegistry) {
    reg.register(
        "mods.list",
        Box::new(|_ctx| Box::pin(async move { runtime::list().await.map_err(to_error) })),
    );

    reg.register(
        "mods.get",
        Box::new(|ctx| {
            Box::pin(async move {
                let id = string_param(&ctx, "id")?;
                runtime::get(&id).await.map_err(to_error)
            })
        }),
    );

    reg.register(
        "mods.install",
        Box::new(|ctx| {
            Box::pin(async move {
                let path = string_param(&ctx, "path")?;
                runtime::install(&ctx.state, &path).await.map_err(to_error)
            })
        }),
    );

    reg.register(
        "mods.grant",
        Box::new(|ctx| {
            Box::pin(async move {
                let id = string_param(&ctx, "id")?;
                let consent_hash = string_param(&ctx, "consentHash")?;
                let enable = ctx
                    .params
                    .get("enable")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false);
                runtime::grant(&ctx.state, &id, &consent_hash, enable)
                    .await
                    .map_err(to_error)
            })
        }),
    );

    reg.register(
        "mods.revoke",
        Box::new(|ctx| {
            Box::pin(async move {
                let id = string_param(&ctx, "id")?;
                runtime::revoke(&ctx.state, &id).await.map_err(to_error)
            })
        }),
    );

    reg.register(
        "mods.enable",
        Box::new(|ctx| {
            Box::pin(async move {
                let id = string_param(&ctx, "id")?;
                runtime::enable(&ctx.state, &id).await.map_err(to_error)
            })
        }),
    );

    reg.register(
        "mods.disable",
        Box::new(|ctx| {
            Box::pin(async move {
                let id = string_param(&ctx, "id")?;
                runtime::disable(&ctx.state, &id).await.map_err(to_error)
            })
        }),
    );

    reg.register(
        "mods.uninstall",
        Box::new(|ctx| {
            Box::pin(async move {
                let id = string_param(&ctx, "id")?;
                runtime::uninstall(&ctx.state, &id).await.map_err(to_error)
            })
        }),
    );
}
