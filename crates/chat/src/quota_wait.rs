//! Park a run whose providers are all out of quota and resume it after the
//! reset.
//!
//! When a run fails with a typed rate-limit or quota failure whose reset is
//! known and within `chat.quota_wait.max_wait_hours`, the error broadcast and
//! the channel message say when the run will resume, the original `chat.send`
//! parameters are persisted under `data_dir()/chat/quota-waits.json`, and a
//! task re-sends them at the reset time plus a little jitter (with
//! `_quota_resume`, so the stored user message is not appended twice). A
//! restart reloads the file and reschedules every wait.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
};

use {
    moltis_agents::provider_error::{ProviderFailure, QuotaWindow},
    moltis_config::schema::QuotaWaitConfig,
    serde::{Deserialize, Serialize},
    serde_json::Value,
    time::OffsetDateTime,
    tokio::task::AbortHandle,
    tracing::{info, warn},
};

use crate::runtime::ChatRuntime;

/// Upper bound of the random delay added after a reset, so many parked runs
/// do not hit the provider in the same second.
const MAX_JITTER_SECS: u64 = 30;

/// Shorter waits are retried in place by the runner's own backoff.
const MIN_PARK_SECS: i64 = 60;

/// Parameter flag telling `chat.send` it is resuming a parked run.
pub const RESUME_FLAG: &str = "_quota_resume";

/// A parked run, as persisted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuotaWait {
    pub session_key: String,
    pub provider: String,
    #[serde(with = "time::serde::rfc3339")]
    pub resets_at: OffsetDateTime,
    /// The `chat.send` parameters to replay. Their shape is the RPC's own.
    pub params: Value,
}

#[derive(Default)]
struct Registry {
    config: QuotaWaitConfig,
    store_path: Option<PathBuf>,
    /// Failures noted while a run errored, picked up by `chat.send`.
    run_failures: HashMap<String, (ProviderFailure, OffsetDateTime)>,
    waits: HashMap<String, (QuotaWait, Option<AbortHandle>)>,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}

fn with_registry<R>(f: impl FnOnce(&mut Registry) -> R) -> R {
    let mut guard = registry().lock().unwrap_or_else(|e| e.into_inner());
    f(&mut guard)
}

/// Install the config and the persistence file (`None` keeps waits in
/// memory only, as in tests).
pub fn configure(config: QuotaWaitConfig, store_path: Option<PathBuf>) {
    with_registry(|registry| {
        registry.config = config;
        registry.store_path = store_path;
    });
}

/// The reset to wait for, if this failure is worth waiting for.
#[must_use]
pub(crate) fn plan(
    failure: &ProviderFailure,
    config: &QuotaWaitConfig,
    now: OffsetDateTime,
) -> Option<OffsetDateTime> {
    if !config.enabled || !failure.error.is_temporary_limit() {
        return None;
    }
    let resets_at = failure.error.retry_instant(now)?;
    let max_wait = time::Duration::seconds_f64(config.max_wait_hours.max(0.0) * 3600.0);
    let wait = resets_at - now;
    (wait >= time::Duration::seconds(MIN_PARK_SECS) && wait <= max_wait).then_some(resets_at)
}

/// Note a typed failure for a run; returns the reset instant when the run
/// will wait.
pub(crate) fn note_failure(run_id: &str, failure: &ProviderFailure) -> Option<OffsetDateTime> {
    let now = OffsetDateTime::now_utc();
    with_registry(|registry| {
        let resets_at = plan(failure, &registry.config, now)?;
        registry
            .run_failures
            .insert(run_id.to_string(), (failure.clone(), resets_at));
        Some(resets_at)
    })
}

fn window_label(window: QuotaWindow) -> &'static str {
    match window {
        QuotaWindow::Rolling5h => "5-hour ",
        QuotaWindow::Daily => "daily ",
        QuotaWindow::Weekly => "weekly ",
        QuotaWindow::Monthly => "monthly ",
        QuotaWindow::Unknown => "",
    }
}

/// `HH:MM` in the user's timezone (IANA name), else UTC.
fn format_local(at: OffsetDateTime, timezone: Option<&str>) -> String {
    #[cfg(any(feature = "web-ui", feature = "push-notifications"))]
    if let Some(tz) = timezone.and_then(|name| name.trim().parse::<chrono_tz::Tz>().ok())
        && let Some(utc) = chrono::DateTime::from_timestamp(at.unix_timestamp(), 0)
    {
        return utc.with_timezone(&tz).format("%H:%M %Z").to_string();
    }
    let _ = timezone;
    format!("{:02}:{:02} UTC", at.hour(), at.minute())
}

/// The error object broadcast and sent to channels for a parked run.
#[must_use]
pub(crate) fn waiting_error(
    failure: &ProviderFailure,
    resets_at: OffsetDateTime,
    timezone: Option<&str>,
) -> Value {
    let window = match &failure.error {
        moltis_agents::provider_error::ProviderError::QuotaExhausted { window, .. } => *window,
        _ => QuotaWindow::Unknown,
    };
    let detail = format!(
        "The {}{} quota is used up. Resuming automatically at {}.",
        window_label(window),
        failure.provider,
        format_local(resets_at, timezone)
    );
    serde_json::json!({
        "type": "quota_wait",
        "icon": "\u{23F3}",
        "title": "Waiting for quota",
        "detail": detail,
        "provider": failure.provider,
        "resetsAt": resets_at.unix_timestamp() * 1000,
        "canCancel": true,
    })
}

/// The error object for a failed run: the waiting card when the failure is
/// worth waiting for, else `fallback()`.
pub(crate) fn error_obj_for_run(
    run_id: &str,
    failure: Option<&ProviderFailure>,
    fallback: impl FnOnce() -> Value,
) -> Value {
    let timezone = crate::types::default_user_prompt_timezone();
    failure
        .and_then(|failure| {
            let resets_at = note_failure(run_id, failure)?;
            Some(waiting_error(failure, resets_at, timezone.as_deref()))
        })
        .unwrap_or_else(fallback)
}

/// The typed failure inside an agent run error, if any.
#[must_use]
pub(crate) fn run_error_failure(
    error: &moltis_agents::runner::AgentRunError,
) -> Option<&ProviderFailure> {
    match error {
        moltis_agents::runner::AgentRunError::Other(error) => ProviderFailure::find(error),
        moltis_agents::runner::AgentRunError::ContextWindowExceeded(_) => None,
    }
}

fn persist(registry: &Registry) {
    let Some(path) = &registry.store_path else {
        return;
    };
    let waits: Vec<&QuotaWait> = registry.waits.values().map(|(wait, _)| wait).collect();
    let result = serde_json::to_vec_pretty(&waits)
        .map_err(std::io::Error::other)
        .and_then(|bytes| {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let tmp = path.with_extension("tmp");
            std::fs::write(&tmp, bytes)?;
            std::fs::rename(&tmp, path)
        });
    if let Err(error) = result {
        warn!(%error, path = %path.display(), "failed to persist quota waits");
    }
}

fn jitter() -> std::time::Duration {
    use rand::RngExt as _;
    std::time::Duration::from_millis(rand::rng().random_range(0..=MAX_JITTER_SECS * 1000))
}

fn schedule(state: Arc<dyn ChatRuntime>, wait: QuotaWait) -> AbortHandle {
    tracing::debug!(session = %wait.session_key, resets_at = %wait.resets_at, "scheduling quota resume");
    tokio::spawn(async move {
        let delay = std::time::Duration::try_from(wait.resets_at - OffsetDateTime::now_utc())
            .unwrap_or_default();
        tokio::time::sleep(delay + jitter()).await;
        let still_waiting = with_registry(|registry| {
            let removed = registry.waits.remove(&wait.session_key).is_some();
            if removed {
                persist(registry);
            }
            removed
        });
        if !still_waiting {
            return;
        }
        info!(session = %wait.session_key, provider = %wait.provider, "quota reset, resuming parked run");
        if let Err(error) = state.chat_service().await.send(wait.params).await {
            warn!(session = %wait.session_key, %error, "resuming parked run failed");
        }
    })
    .abort_handle()
}

/// Park the run noted under `run_id`: persist its parameters and schedule
/// the resume. Returns `false` when the run was not waiting.
pub(crate) fn park_noted_run(
    state: &Arc<dyn ChatRuntime>,
    run_id: &str,
    session_key: &str,
    params: &Value,
) -> bool {
    let Some((failure, resets_at)) = with_registry(|registry| registry.run_failures.remove(run_id))
    else {
        return false;
    };
    let mut params = params.clone();
    if let Some(object) = params.as_object_mut() {
        object.insert(RESUME_FLAG.into(), Value::Bool(true));
        object.insert(
            "_session_key".into(),
            Value::String(session_key.to_string()),
        );
    }
    let wait = QuotaWait {
        session_key: session_key.to_string(),
        provider: failure.provider,
        resets_at,
        params,
    };
    let handle = schedule(Arc::clone(state), wait.clone());
    with_registry(|registry| {
        if let Some((_, Some(previous))) = registry
            .waits
            .insert(session_key.to_string(), (wait, Some(handle)))
        {
            previous.abort();
        }
        persist(registry);
    });
    true
}

/// Cancel a session's parked run. Returns `true` when one was waiting.
pub fn cancel(session_key: &str) -> bool {
    with_registry(|registry| {
        let Some((_, handle)) = registry.waits.remove(session_key) else {
            return false;
        };
        if let Some(handle) = handle {
            handle.abort();
        }
        persist(registry);
        true
    })
}

/// The parked run of a session, for the UI after a reload.
#[must_use]
pub fn pending(session_key: &str) -> Option<QuotaWait> {
    with_registry(|registry| {
        registry
            .waits
            .get(session_key)
            .map(|(wait, _)| wait.clone())
    })
}

/// Reload persisted waits after a restart and schedule them again.
pub fn restore(state: &Arc<dyn ChatRuntime>) {
    let Some(path) = with_registry(|registry| registry.store_path.clone()) else {
        return;
    };
    let waits: Vec<QuotaWait> = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            warn!(%error, path = %path.display(), "ignoring unreadable quota waits");
            Vec::new()
        }),
        Err(_) => Vec::new(),
    };
    for wait in waits {
        info!(session = %wait.session_key, resets_at = %wait.resets_at, "restoring parked run");
        let handle = schedule(Arc::clone(state), wait.clone());
        with_registry(|registry| {
            registry
                .waits
                .insert(wait.session_key.clone(), (wait, Some(handle)));
        });
    }
}

/// A resumed run replays the user message persisted by the failed turn, so
/// it is taken off the loaded history (the runner adds it back as the
/// current message).
pub(crate) fn drop_resumed_user_message(history: &mut Vec<Value>) {
    if history
        .last()
        .and_then(|message| message.get("role"))
        .and_then(Value::as_str)
        == Some("user")
    {
        history.pop();
    }
}

/// `true` when `params` resume a parked run.
#[must_use]
pub(crate) fn is_resume(params: &Value) -> bool {
    params.get(RESUME_FLAG).and_then(Value::as_bool) == Some(true)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use {
        super::*,
        moltis_agents::provider_error::{LimitScope, ProviderError},
        time::macros::datetime,
    };

    const NOW: OffsetDateTime = datetime!(2026-11-27 12:00 UTC);

    fn quota(resets_at: Option<OffsetDateTime>) -> ProviderFailure {
        ProviderFailure::new(
            ProviderError::QuotaExhausted {
                resets_at,
                window: QuotaWindow::Rolling5h,
            },
            "zai-code",
            "Usage limit reached",
        )
    }

    #[test]
    fn plan_waits_only_for_known_resets_within_the_horizon() {
        let config = QuotaWaitConfig::default();
        let soon = datetime!(2026-11-27 15:00 UTC);
        assert_eq!(plan(&quota(Some(soon)), &config, NOW), Some(soon));
        assert_eq!(
            plan(&quota(Some(datetime!(2026-11-28 12:00 UTC))), &config, NOW),
            None
        );
        assert_eq!(plan(&quota(None), &config, NOW), None);
        let disabled = QuotaWaitConfig {
            enabled: false,
            ..QuotaWaitConfig::default()
        };
        assert_eq!(plan(&quota(Some(soon)), &disabled, NOW), None);
        let auth = ProviderFailure::new(ProviderError::Auth, "p", "denied");
        assert_eq!(plan(&auth, &config, NOW), None);
        let rate = ProviderFailure::new(
            ProviderError::RateLimited {
                retry_after: Some(std::time::Duration::from_secs(600)),
                scope: LimitScope::Tokens,
            },
            "p",
            "slow",
        );
        assert_eq!(
            plan(&rate, &config, NOW),
            Some(datetime!(2026-11-27 12:10 UTC))
        );
        let short = ProviderFailure::new(
            ProviderError::RateLimited {
                retry_after: Some(std::time::Duration::from_secs(20)),
                scope: LimitScope::Requests,
            },
            "p",
            "slow",
        );
        assert_eq!(plan(&short, &config, NOW), None);
    }

    #[test]
    fn error_obj_for_run_falls_back_without_a_waitable_failure() {
        let fallback = || serde_json::json!({ "type": "fallback" });
        assert_eq!(
            error_obj_for_run("run-a", None, fallback)["type"],
            "fallback"
        );
        let auth = ProviderFailure::new(ProviderError::Auth, "p", "denied");
        assert_eq!(
            error_obj_for_run("run-b", Some(&auth), fallback)["type"],
            "fallback"
        );
        let soon = OffsetDateTime::now_utc() + time::Duration::hours(1);
        let waiting = error_obj_for_run("run-c", Some(&quota(Some(soon))), fallback);
        assert_eq!(waiting["type"], "quota_wait");
    }

    #[test]
    fn waiting_error_carries_reset_and_cancel() {
        let error = waiting_error(&quota(None), datetime!(2026-11-27 17:30 UTC), None);
        assert_eq!(error["type"], "quota_wait");
        assert_eq!(error["canCancel"], true);
        assert_eq!(
            error["resetsAt"],
            datetime!(2026-11-27 17:30 UTC).unix_timestamp() * 1000
        );
        let detail = error["detail"].as_str().unwrap();
        assert!(detail.contains("5-hour zai-code quota"));
        assert!(detail.contains("17:30 UTC"));
    }

    #[cfg(any(feature = "web-ui", feature = "push-notifications"))]
    #[test]
    fn reset_time_is_shown_in_the_user_timezone() {
        let at = datetime!(2026-11-27 17:30 UTC);
        assert_eq!(format_local(at, Some("Europe/Paris")), "18:30 CET");
        assert_eq!(format_local(at, Some("Not/AZone")), "17:30 UTC");
    }

    #[test]
    fn resume_flag_detection() {
        assert!(is_resume(&serde_json::json!({ RESUME_FLAG: true })));
        assert!(!is_resume(&serde_json::json!({ "text": "hi" })));
    }

    #[test]
    fn resumed_history_drops_only_a_trailing_user_message() {
        let mut history = vec![
            serde_json::json!({"role": "user", "content": "a"}),
            serde_json::json!({"role": "assistant", "content": "b"}),
            serde_json::json!({"role": "user", "content": "c"}),
        ];
        drop_resumed_user_message(&mut history);
        assert_eq!(history.len(), 2);
        drop_resumed_user_message(&mut history);
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn cancel_and_pending_without_waits() {
        assert!(!cancel("no-such-session"));
        assert!(pending("no-such-session").is_none());
    }

    /// Records every replayed `chat.send`.
    #[derive(Default)]
    struct RecordingChat {
        sent: Mutex<Vec<Value>>,
    }

    #[async_trait::async_trait]
    impl moltis_service_traits::ChatService for RecordingChat {
        async fn send(&self, params: Value) -> moltis_service_traits::ServiceResult {
            self.sent
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(params);
            Ok(Value::Null)
        }

        async fn abort(&self, _: Value) -> moltis_service_traits::ServiceResult {
            Ok(Value::Null)
        }

        async fn cancel_queued(&self, _: Value) -> moltis_service_traits::ServiceResult {
            Ok(Value::Null)
        }

        async fn history(&self, _: Value) -> moltis_service_traits::ServiceResult {
            Ok(Value::Null)
        }

        async fn inject(&self, _: Value) -> moltis_service_traits::ServiceResult {
            Ok(Value::Null)
        }

        async fn clear(&self, _: Value) -> moltis_service_traits::ServiceResult {
            Ok(Value::Null)
        }

        async fn compact(&self, _: Value) -> moltis_service_traits::ServiceResult {
            Ok(Value::Null)
        }

        async fn context(&self, _: Value) -> moltis_service_traits::ServiceResult {
            Ok(Value::Null)
        }

        async fn raw_prompt(&self, _: Value) -> moltis_service_traits::ServiceResult {
            Ok(Value::Null)
        }

        async fn full_context(&self, _: Value) -> moltis_service_traits::ServiceResult {
            Ok(Value::Null)
        }
    }

    fn sent(chat: &RecordingChat) -> Vec<Value> {
        chat.sent.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn stored(path: &std::path::Path) -> Vec<QuotaWait> {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// One test, because the registry is process-wide: park, resume after a
    /// simulated reset, cancel, and restore after a restart.
    #[tokio::test(start_paused = true)]
    async fn parked_runs_resume_cancel_and_survive_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat").join("quota-waits.json");
        configure(QuotaWaitConfig::default(), Some(path.clone()));
        let chat = Arc::new(RecordingChat::default());
        let state: Arc<dyn ChatRuntime> =
            Arc::new(crate::test_runtime::TestRuntime::new(chat.clone()));
        let params = serde_json::json!({ "text": "hello", "_session_key": "s1" });

        // Park: the run is persisted and resumes after the reset.
        let failure = quota(Some(OffsetDateTime::now_utc() + time::Duration::minutes(2)));
        assert!(note_failure("run-1", &failure).is_some());
        assert!(park_noted_run(&state, "run-1", "s1", &params));
        assert!(!park_noted_run(&state, "run-1", "s1", &params));
        assert_eq!(
            pending("s1").map(|wait| wait.provider),
            Some("zai-code".to_string())
        );
        assert_eq!(stored(&path).len(), 1);
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        assert!(sent(&chat).is_empty(), "resumed before the reset");
        tokio::time::sleep(std::time::Duration::from_secs(120)).await;
        let replayed = sent(&chat);
        assert_eq!(replayed.len(), 1);
        assert!(is_resume(&replayed[0]));
        assert_eq!(replayed[0]["text"], "hello");
        assert!(pending("s1").is_none());
        assert!(stored(&path).is_empty());

        // Cancel: nothing is replayed.
        let failure = quota(Some(OffsetDateTime::now_utc() + time::Duration::minutes(5)));
        note_failure("run-2", &failure);
        assert!(park_noted_run(&state, "run-2", "s2", &params));
        assert!(cancel("s2"));
        tokio::time::sleep(std::time::Duration::from_secs(600)).await;
        assert_eq!(sent(&chat).len(), 1);
        assert!(stored(&path).is_empty());

        // Restart: a wait written by a previous process is rescheduled; one
        // whose reset already passed resumes right away (plus jitter).
        let leftover = QuotaWait {
            session_key: "s3".into(),
            provider: "kimi-code".into(),
            resets_at: OffsetDateTime::now_utc() - time::Duration::minutes(1),
            params: serde_json::json!({ "text": "again", RESUME_FLAG: true }),
        };
        std::fs::write(&path, serde_json::to_vec(&vec![leftover]).unwrap()).unwrap();
        restore(&state);
        assert!(pending("s3").is_some());
        tokio::time::sleep(std::time::Duration::from_secs(MAX_JITTER_SECS + 1)).await;
        let replayed = sent(&chat);
        assert_eq!(replayed.len(), 2);
        assert_eq!(replayed[1]["text"], "again");
        assert!(stored(&path).is_empty());
    }
}
