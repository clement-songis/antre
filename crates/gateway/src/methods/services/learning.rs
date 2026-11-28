//! `learning.*` RPC methods: review of staged changes, the ledger with
//! single-entry rollback, skill pins and locks, and on-demand review and
//! curator runs. Reads are `operator.read`; anything that changes files or
//! starts a model call is `operator.write`.

use std::sync::Arc;

use {
    antre_learning::LearningService,
    antre_protocol::{ErrorShape, error_codes},
    serde_json::Value,
};

use {
    super::MethodRegistry,
    crate::{
        learning::{self, announce},
        state::GatewayState,
    },
};

const DEFAULT_LEDGER_LIMIT: usize = 50;
const MAX_LEDGER_LIMIT: usize = 500;

fn learning_service() -> Result<Arc<LearningService>, ErrorShape> {
    learning::service().ok_or_else(|| {
        ErrorShape::new(
            error_codes::UNAVAILABLE,
            "the learning service is not running",
        )
    })
}

fn required_str<'a>(params: &'a Value, key: &str) -> Result<&'a str, ErrorShape> {
    params
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ErrorShape::new(error_codes::INVALID_REQUEST, format!("missing {key}")))
}

fn to_value<T: serde::Serialize>(value: &T) -> Result<Value, ErrorShape> {
    serde_json::to_value(value)
        .map_err(|error| ErrorShape::new(error_codes::INTERNAL, error.to_string()))
}

/// Map library errors to RPC errors: bad input and conflicts are the
/// caller's to fix, the rest is a server-side failure.
fn rpc_error(error: antre_learning::Error) -> ErrorShape {
    use antre_learning::Error;
    let code = match &error {
        Error::InvalidTarget(_)
        | Error::Conflict(_)
        | Error::StaleTarget(_)
        | Error::Refused(_)
        | Error::NotFound(_) => error_codes::INVALID_REQUEST,
        Error::Model(_) | Error::Response(_) => error_codes::UNAVAILABLE,
        Error::Io(_) | Error::Json(_) => error_codes::INTERNAL,
    };
    ErrorShape::new(code, error.to_string())
}

async fn changed(state: &Arc<GatewayState>, kind: &str, detail: Value) {
    announce(state, kind, detail).await;
}

/// Register the `learning.*` namespace.
pub(super) fn register(reg: &mut MethodRegistry) {
    reg.register(
        "learning.status",
        Box::new(|_ctx| {
            Box::pin(async move {
                let service = learning_service()?;
                to_value(&service.status().await.map_err(rpc_error)?)
            })
        }),
    );

    reg.register(
        "learning.pending.list",
        Box::new(|_ctx| {
            Box::pin(async move {
                let service = learning_service()?;
                let pending = service.pending().await.map_err(rpc_error)?;
                Ok(serde_json::json!({ "pending": to_value(&pending)? }))
            })
        }),
    );

    reg.register(
        "learning.pending.get",
        Box::new(|ctx| {
            Box::pin(async move {
                let service = learning_service()?;
                let id = required_str(&ctx.params, "id")?;
                to_value(&service.pending_get(id).await.map_err(rpc_error)?)
            })
        }),
    );

    reg.register(
        "learning.pending.approve",
        Box::new(|ctx| {
            Box::pin(async move {
                let service = learning_service()?;
                let id = required_str(&ctx.params, "id")?;
                let entry = service.approve(id).await.map_err(rpc_error)?;
                let entry = to_value(&entry)?;
                changed(&ctx.state, "approved", entry.clone()).await;
                Ok(serde_json::json!({ "entry": entry }))
            })
        }),
    );

    reg.register(
        "learning.pending.reject",
        Box::new(|ctx| {
            Box::pin(async move {
                let service = learning_service()?;
                let id = required_str(&ctx.params, "id")?;
                let note = ctx
                    .params
                    .get("note")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                service.reject(id, note).await.map_err(rpc_error)?;
                changed(&ctx.state, "rejected", serde_json::json!({ "id": id })).await;
                Ok(serde_json::json!({ "rejected": true }))
            })
        }),
    );

    reg.register(
        "learning.ledger.list",
        Box::new(|ctx| {
            Box::pin(async move {
                let service = learning_service()?;
                let skill = ctx.params.get("skill").and_then(Value::as_str);
                let limit = ctx
                    .params
                    .get("limit")
                    .and_then(Value::as_u64)
                    .and_then(|limit| usize::try_from(limit).ok())
                    .unwrap_or(DEFAULT_LEDGER_LIMIT)
                    .min(MAX_LEDGER_LIMIT);
                let entries = service.ledger(skill, limit).await.map_err(rpc_error)?;
                Ok(serde_json::json!({ "entries": to_value(&entries)? }))
            })
        }),
    );

    reg.register(
        "learning.ledger.diff",
        Box::new(|ctx| {
            Box::pin(async move {
                let service = learning_service()?;
                let id = required_str(&ctx.params, "id")?;
                let diff = service.ledger_diff(id).await.map_err(rpc_error)?;
                Ok(serde_json::json!({ "diff": diff }))
            })
        }),
    );

    reg.register(
        "learning.ledger.rollback",
        Box::new(|ctx| {
            Box::pin(async move {
                let service = learning_service()?;
                let id = required_str(&ctx.params, "id")?;
                let reason = ctx
                    .params
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("rolled back by the user");
                let entry = to_value(&service.rollback(id, reason).await.map_err(rpc_error)?)?;
                changed(&ctx.state, "rolled_back", entry.clone()).await;
                Ok(serde_json::json!({ "entry": entry }))
            })
        }),
    );

    reg.register(
        "learning.skill.set",
        Box::new(|ctx| {
            Box::pin(async move {
                let service = learning_service()?;
                let name = required_str(&ctx.params, "name")?;
                let pinned = ctx.params.get("pinned").and_then(Value::as_bool);
                let locked = ctx.params.get("locked").and_then(Value::as_bool);
                if pinned.is_none() && locked.is_none() {
                    return Err(ErrorShape::new(
                        error_codes::INVALID_REQUEST,
                        "set pinned or locked",
                    ));
                }
                service
                    .set_flags(name, pinned, locked)
                    .await
                    .map_err(rpc_error)?;
                Ok(serde_json::json!({ "name": name, "pinned": pinned, "locked": locked }))
            })
        }),
    );

    reg.register(
        "learning.review.run",
        Box::new(|ctx| {
            Box::pin(async move {
                let service = learning_service()?;
                let session_key = required_str(&ctx.params, "sessionKey")?;
                let report = learning::review_now(&ctx.state, &service, session_key)
                    .await
                    .map_err(|error| ErrorShape::new(error_codes::UNAVAILABLE, error))?;
                to_value(&report)
            })
        }),
    );

    reg.register(
        "learning.curator.run",
        Box::new(|ctx| {
            Box::pin(async move {
                let service = learning_service()?;
                let dry_run = ctx
                    .params
                    .get("dryRun")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let activity = learning::skill_activity(&ctx.state, &service).await;
                let reserved = learning::reserved_skill_names(service.paths().data_dir()).await;
                let report = service
                    .run_curator(&activity, &reserved, dry_run)
                    .await
                    .map_err(rpc_error)?;
                let value = to_value(&report)?;
                if !dry_run {
                    changed(&ctx.state, "curator", value.clone()).await;
                }
                Ok(value)
            })
        }),
    );
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn errors_map_to_request_or_server_codes() {
        let caller = rpc_error(antre_learning::Error::StaleTarget("MEMORY.md".into()));
        assert_eq!(caller.code, error_codes::INVALID_REQUEST);
        let server = rpc_error(antre_learning::Error::Model("timeout".into()));
        assert_eq!(server.code, error_codes::UNAVAILABLE);
    }

    #[test]
    fn required_params_are_checked() {
        let params = serde_json::json!({ "id": "", "name": "x" });
        assert!(required_str(&params, "id").is_err());
        assert!(required_str(&params, "missing").is_err());
        assert_eq!(required_str(&params, "name").unwrap(), "x");
    }
}
