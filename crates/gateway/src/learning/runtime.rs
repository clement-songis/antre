//! Service lifecycle, review scheduling and the idle curator.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::{
        Arc, LazyLock, Mutex, OnceLock,
        atomic::{AtomicI64, Ordering},
    },
};

use {
    antre_learning::{
        LearningService,
        curator::{SkillActivity, activity_from_usage},
        service::ReviewReport,
    },
    antre_skills::{
        discover::{FsSkillDiscoverer, SkillDiscoverer},
        types::SkillSource,
    },
    time::{Duration, OffsetDateTime},
    tracing::{debug, info, warn},
};

use {
    super::{history::turns_from_messages, model::resolve_model},
    crate::{
        broadcast::{BroadcastOpts, broadcast},
        state::GatewayState,
    },
};

/// How often the curator checks whether a run is due.
const CURATOR_CHECK: std::time::Duration = std::time::Duration::from_secs(30 * 60);
/// The digest stays small: a cheap model reads it after every busy session.
const MAX_DIGEST_TOKENS: usize = 12_000;
const CHARS_PER_TOKEN: usize = 4;

static SERVICE: OnceLock<Arc<LearningService>> = OnceLock::new();
static LAST_ACTIVITY: AtomicI64 = AtomicI64::new(0);
/// In-flight reviews by session, aborted when the user starts a new run.
static REVIEWS: LazyLock<Mutex<HashMap<String, tokio::task::AbortHandle>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The learning service, once the gateway started it.
pub(crate) fn service() -> Option<Arc<LearningService>> {
    SERVICE.get().cloned()
}

fn touch() {
    LAST_ACTIVITY.store(
        OffsetDateTime::now_utc().unix_timestamp(),
        Ordering::Relaxed,
    );
}

fn idle_for() -> Duration {
    let last = LAST_ACTIVITY.load(Ordering::Relaxed);
    Duration::seconds(OffsetDateTime::now_utc().unix_timestamp() - last)
}

/// Start the service and, when enabled, the curator schedule.
pub(crate) fn start(state: &Arc<GatewayState>, data_dir: &Path) {
    let config = state.config.learning.clone();
    let enabled = config.enabled;
    let curator = config.curator.enabled;
    let service = Arc::new(LearningService::new(
        data_dir,
        config,
        state.config.memory.agent_write_mode,
    ));
    if SERVICE.set(service).is_err() {
        debug!("learning service already started");
        return;
    }
    touch();
    if enabled && curator {
        spawn_curator(Arc::clone(state));
    }
    info!(enabled, "learning service ready");
}

/// Names of non-personal skills, which a learned skill must not shadow.
pub(crate) async fn reserved_skill_names(data_dir: &Path) -> HashSet<String> {
    let discoverer = FsSkillDiscoverer::new(FsSkillDiscoverer::default_paths_for(data_dir));
    match discoverer.discover().await {
        Ok(skills) => skills
            .into_iter()
            .filter(|skill| skill.source != Some(SkillSource::Personal))
            .map(|skill| skill.name)
            .collect(),
        Err(error) => {
            warn!(%error, "learning: skill discovery failed; no reserved names");
            HashSet::new()
        },
    }
}

/// Usage of the personal skills on disk.
pub(crate) async fn skill_activity(
    state: &GatewayState,
    service: &LearningService,
) -> Vec<SkillActivity> {
    let names = service.personal_skill_names();
    let usage = match state.skill_usage_store.get() {
        Some(store) => store.get_all().await,
        None => HashMap::new(),
    };
    activity_from_usage(&names, &usage)
}

/// A run started: the user is active, so a review of that session yields.
pub(crate) fn on_run_started(session_key: &str) {
    touch();
    let handle = REVIEWS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(session_key);
    if let Some(handle) = handle {
        handle.abort();
        debug!(
            session = session_key,
            "learning review cancelled by a new run"
        );
    }
}

/// A run finished: review the session in the background when it is due.
pub(crate) fn on_run_finished(state: &Arc<GatewayState>, session_key: &str) {
    touch();
    let Some(service) = service() else {
        return;
    };
    let config = service.config();
    if !config.enabled || !config.review.enabled {
        return;
    }
    let state = Arc::clone(state);
    let key = session_key.to_string();
    let task = tokio::spawn(async move {
        if let Err(error) = review_if_due(&state, &service, &key).await {
            warn!(session = %key, %error, "learning review failed");
        }
        REVIEWS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&key);
    });
    let previous = REVIEWS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(session_key.to_string(), task.abort_handle());
    if let Some(previous) = previous {
        previous.abort();
    }
}

async fn session_turns(
    state: &GatewayState,
    session_key: &str,
) -> Result<
    (
        Vec<antre_learning::review_digest::Turn>,
        HashSet<String>,
        Option<String>,
    ),
    String,
> {
    let store = state
        .services
        .session_store
        .as_ref()
        .ok_or("no session store")?;
    let history = store
        .read(session_key)
        .await
        .map_err(|error| error.to_string())?;
    let messages = antre_agents::model::values_to_chat_messages_with_tool_result_limit(
        &history,
        state.config.tools.max_tool_result_bytes,
    );
    let (turns, loaded) = turns_from_messages(&messages);
    let session_model = match state.services.session_metadata.as_ref() {
        Some(metadata) => metadata
            .get(session_key)
            .await
            .and_then(|entry| entry.model),
        None => None,
    };
    Ok((turns, loaded, session_model))
}

async fn review_if_due(
    state: &Arc<GatewayState>,
    service: &LearningService,
    session_key: &str,
) -> Result<(), String> {
    let (turns, ..) = session_turns(state, session_key).await?;
    if !service.review_due(session_key, &turns).await {
        return Ok(());
    }
    review_now(state, service, session_key).await.map(|_| ())
}

/// Review a session now, whatever the thresholds say.
pub(crate) async fn review_now(
    state: &Arc<GatewayState>,
    service: &LearningService,
    session_key: &str,
) -> Result<ReviewReport, String> {
    let (turns, loaded, session_model) = session_turns(state, session_key).await?;
    if turns.is_empty() {
        return Err("the session has no messages".into());
    }
    let model = resolve_model(state, session_model.as_deref())
        .await
        .ok_or("no model available for the learning review")?;
    let window = usize::try_from(model.context_window()).unwrap_or(usize::MAX);
    let budget = (window / 4).min(MAX_DIGEST_TOKENS) * CHARS_PER_TOKEN;
    let reserved = reserved_skill_names(service.paths().data_dir()).await;
    let report = service
        .run_review(&model, session_key, &turns, &loaded, &reserved, budget)
        .await
        .map_err(|error| error.to_string())?;
    announce(
        state,
        "review",
        serde_json::json!({
            "sessionKey": session_key,
            "outcomes": report.outcomes.iter().map(|outcome| outcome.kind()).collect::<Vec<_>>(),
        }),
    )
    .await;
    Ok(report)
}

/// Tell connected clients that learned changes moved.
pub(crate) async fn announce(state: &Arc<GatewayState>, kind: &str, detail: serde_json::Value) {
    broadcast(
        state,
        "learning",
        serde_json::json!({ "kind": kind, "detail": detail }),
        BroadcastOpts::default(),
    )
    .await;
}

fn spawn_curator(state: Arc<GatewayState>) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(CURATOR_CHECK);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            let Some(service) = service() else {
                continue;
            };
            if let Err(error) = curator_tick(&state, &service).await {
                warn!(%error, "learning curator tick failed");
            }
        }
    });
}

async fn curator_tick(state: &Arc<GatewayState>, service: &LearningService) -> Result<(), String> {
    let activity = skill_activity(state, service).await;
    let reserved = reserved_skill_names(service.paths().data_dir()).await;
    let Some(report) = service
        .curator_tick(&activity, idle_for(), &reserved)
        .await
        .map_err(|error| error.to_string())?
    else {
        return Ok(());
    };
    announce(
        state,
        "curator",
        serde_json::to_value(&report).unwrap_or_default(),
    )
    .await;
    if service.config().curator.consolidate {
        let model = resolve_model(state, None)
            .await
            .ok_or("no model available for consolidation")?;
        let outcomes = service
            .run_consolidation(&model, &reserved)
            .await
            .map_err(|error| error.to_string())?;
        announce(
            state,
            "consolidation",
            serde_json::to_value(&outcomes).unwrap_or_default(),
        )
        .await;
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn run_start_aborts_the_sessions_review() {
        let task = tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        });
        REVIEWS
            .lock()
            .unwrap()
            .insert("learning-test-session".into(), task.abort_handle());
        on_run_started("learning-test-session");
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(
            !REVIEWS
                .lock()
                .unwrap()
                .contains_key("learning-test-session")
        );
        assert!(idle_for() < Duration::seconds(5));
    }

    #[tokio::test]
    async fn reserved_names_skip_personal_skills() {
        let tmp = tempfile::tempdir().unwrap();
        for (dir, name) in [("skills/mine", "mine"), (".antre/skills/theirs", "theirs")] {
            let path = tmp.path().join(dir);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(
                path.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: x\n---\nbody"),
            )
            .unwrap();
        }
        let reserved = reserved_skill_names(tmp.path()).await;
        assert!(!reserved.contains("mine"));
        assert!(reserved.contains("theirs"));
    }
}
