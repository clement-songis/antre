use {
    antre_tools::checkpoints::{CheckpointManager, TurnRecord},
    serde::{Deserialize, Serialize},
};

use super::*;

/// Most turns a single `checkpoints.turns` call returns.
const MAX_TURNS: usize = 200;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct TurnsParams {
    session_key: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct RestoreParams {
    session_key: Option<String>,
    /// Runs to undo. Every turn recorded for one of them is restored.
    run_ids: Vec<String>,
    /// 1-based position in the newest-first list, as `/rollback N` uses.
    turn: Option<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TurnView {
    run_id: String,
    created_at: i64,
    paths: Vec<String>,
    checkpoint_count: usize,
}

impl From<&TurnRecord> for TurnView {
    fn from(turn: &TurnRecord) -> Self {
        let mut paths: Vec<String> = Vec::new();
        for path in &turn.source_paths {
            if !paths.contains(path) {
                paths.push(path.clone());
            }
        }
        Self {
            run_id: turn.run_id.clone(),
            created_at: turn.created_at,
            paths,
            checkpoint_count: turn.checkpoint_ids.len(),
        }
    }
}

fn invalid(message: impl Into<String>) -> ErrorShape {
    ErrorShape::new(error_codes::INVALID_REQUEST, message.into())
}

async fn resolve_session_key(
    ctx: &MethodContext,
    explicit: Option<String>,
) -> Result<String, ErrorShape> {
    if let Some(key) = explicit.filter(|key| !key.trim().is_empty()) {
        return Ok(key);
    }
    active_session_key_for_ctx(ctx)
        .await
        .ok_or_else(|| invalid("missing 'sessionKey' parameter"))
}

/// Pick the turns a restore request names. `turns` is newest first.
fn select_turns(turns: Vec<TurnRecord>, params: &RestoreParams) -> Result<Vec<TurnRecord>, String> {
    if !params.run_ids.is_empty() {
        let selected: Vec<TurnRecord> = turns
            .into_iter()
            .filter(|turn| params.run_ids.contains(&turn.run_id))
            .collect();
        return Ok(selected);
    }
    let Some(position) = params.turn else {
        return Err("pass 'runIds' or 'turn'".into());
    };
    if position == 0 || position > turns.len() {
        return Err(format!("invalid turn number. Use 1–{}.", turns.len()));
    }
    Ok(turns.into_iter().nth(position - 1).into_iter().collect())
}

pub(super) fn register(reg: &mut MethodRegistry) {
    reg.register(
        "checkpoints.turns",
        Box::new(|ctx| {
            Box::pin(async move {
                let params: TurnsParams =
                    serde_json::from_value(ctx.params.clone()).unwrap_or_default();
                let session_key = resolve_session_key(&ctx, params.session_key).await?;
                let limit = params.limit.unwrap_or(20).clamp(1, MAX_TURNS);
                let manager = CheckpointManager::new(antre_config::data_dir());
                let turns = manager
                    .read_turns(limit, Some(&session_key))
                    .map_err(|error| ErrorShape::from(ServiceError::message(error)))?;
                let views: Vec<TurnView> = turns.iter().map(TurnView::from).collect();
                Ok(serde_json::json!({ "sessionKey": session_key, "turns": views }))
            })
        }),
    );

    reg.register(
        "checkpoints.restore",
        Box::new(|ctx| {
            Box::pin(async move {
                let params: RestoreParams = serde_json::from_value(ctx.params.clone())
                    .map_err(|error| invalid(format!("invalid params: {error}")))?;
                let session_key = resolve_session_key(&ctx, params.session_key.clone()).await?;
                let manager = CheckpointManager::new(antre_config::data_dir());
                let turns = manager
                    .read_turns(usize::MAX, Some(&session_key))
                    .map_err(|error| ErrorShape::from(ServiceError::message(error)))?;
                let selected = select_turns(turns, &params).map_err(invalid)?;
                let outcome = manager.restore_turns(&selected).await;
                broadcast(
                    &ctx.state,
                    "session",
                    serde_json::json!({
                        "kind": "rollback",
                        "sessionKey": session_key,
                        "restored": outcome.restored,
                    }),
                    BroadcastOpts {
                        drop_if_slow: true,
                        ..Default::default()
                    },
                )
                .await;
                Ok(serde_json::json!({
                    "sessionKey": session_key,
                    "turns": selected.len(),
                    "restored": outcome.restored,
                    "paths": outcome.paths,
                    "errors": outcome.errors,
                }))
            })
        }),
    );
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    fn turn(run_id: &str, created_at: i64) -> TurnRecord {
        TurnRecord {
            run_id: run_id.into(),
            session_key: "s".into(),
            created_at,
            checkpoint_ids: vec!["a".into(), "b".into()],
            source_paths: vec!["/x/a.rs".into(), "/x/a.rs".into()],
        }
    }

    #[test]
    fn select_by_run_ids_keeps_matching_turns() {
        let turns = vec![turn("r3", 3), turn("r2", 2), turn("r1", 1)];
        let params = RestoreParams {
            run_ids: vec!["r1".into(), "r3".into()],
            ..Default::default()
        };
        let selected = select_turns(turns, &params).unwrap();
        let ids: Vec<&str> = selected.iter().map(|t| t.run_id.as_str()).collect();
        assert_eq!(ids, vec!["r3", "r1"]);
    }

    #[test]
    fn select_by_position_is_one_based_newest_first() {
        let turns = vec![turn("r3", 3), turn("r2", 2)];
        let params = RestoreParams {
            turn: Some(2),
            ..Default::default()
        };
        let selected = select_turns(turns, &params).unwrap();
        assert_eq!(selected[0].run_id, "r2");
    }

    #[test]
    fn select_rejects_out_of_range_and_missing_selector() {
        let params = RestoreParams {
            turn: Some(3),
            ..Default::default()
        };
        assert!(select_turns(vec![turn("r1", 1)], &params).is_err());
        assert!(select_turns(vec![turn("r1", 1)], &RestoreParams::default()).is_err());
    }

    #[test]
    fn turn_view_dedups_paths() {
        let view = TurnView::from(&turn("r1", 1));
        assert_eq!(view.paths, vec!["/x/a.rs".to_string()]);
        assert_eq!(view.checkpoint_count, 2);
    }
}
