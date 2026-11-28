//! Callers of `chat.send_sync` waiting for a turn that queued behind the
//! session's active run.
//!
//! The queued params carry a waiter id; whichever path ends the turn
//! resolves the waiter: the run's completion when the replay starts a run,
//! or the replay's own result when it fails, is rejected, or is cancelled.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use {
    moltis_service_traits::{ServiceError, ServiceResult},
    serde_json::Value,
    tokio::sync::oneshot,
};

/// Params key holding the waiter id.
pub(crate) const SYNC_WAITER_KEY: &str = "_sync_waiter";

#[derive(Default)]
pub(crate) struct SyncWaiters {
    waiters: Mutex<HashMap<String, oneshot::Sender<ServiceResult>>>,
}

/// The waiter id carried by `params`, if any.
#[must_use]
pub(crate) fn waiter_id(params: &Value) -> Option<String> {
    params
        .get(SYNC_WAITER_KEY)
        .and_then(Value::as_str)
        .map(String::from)
}

/// `true` when `params` belong to a waiting `send_sync` caller; such a turn
/// is never merged with others, since its caller expects its own answer.
#[must_use]
pub(crate) fn is_sync(params: &Value) -> bool {
    params.get(SYNC_WAITER_KEY).is_some()
}

impl SyncWaiters {
    #[must_use]
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Register a waiter and mark `params` with its id.
    pub(crate) fn register(&self, params: &mut Value) -> oneshot::Receiver<ServiceResult> {
        let id = uuid::Uuid::new_v4().to_string();
        let (sender, receiver) = oneshot::channel();
        self.lock().insert(id.clone(), sender);
        if let Some(object) = params.as_object_mut() {
            object.insert(SYNC_WAITER_KEY.into(), Value::String(id));
        }
        receiver
    }

    /// Hand the waiter to the run that is about to start.
    pub(crate) fn take(&self, id: &str) -> Option<oneshot::Sender<ServiceResult>> {
        self.lock().remove(id)
    }

    /// Settle the waiter after a `chat.send` that did not start a run: an
    /// error or a rejection ends the wait, a (re)queue keeps it.
    pub(crate) fn settle_unstarted(&self, id: &str, result: &ServiceResult) {
        let outcome = match result {
            Err(error) => Err(ServiceError::message(error.to_string())),
            Ok(value) if value.get("rejected").and_then(Value::as_bool) == Some(true) => {
                Ok(value.clone())
            },
            Ok(_) => return,
        };
        if let Some(sender) = self.take(id) {
            let _ = sender.send(outcome);
        }
    }

    /// End the wait of every cancelled queued turn.
    pub(crate) fn cancel<'a>(&self, params: impl Iterator<Item = &'a Value>) {
        for id in params.filter_map(waiter_id) {
            if let Some(sender) = self.take(&id) {
                let _ = sender.send(Err("queued turn was cancelled".into()));
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.lock().len()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, oneshot::Sender<ServiceResult>>> {
        self.waiters.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use {super::*, serde_json::json};

    #[tokio::test]
    async fn a_started_run_takes_the_waiter() {
        let waiters = SyncWaiters::new();
        let mut params = json!({"text": "hi"});
        let receiver = waiters.register(&mut params);
        let id = waiter_id(&params).unwrap();
        assert!(is_sync(&params));

        let sender = waiters.take(&id).unwrap();
        assert!(waiters.take(&id).is_none());
        sender.send(Ok(json!({"text": "done"}))).unwrap();
        assert_eq!(receiver.await.unwrap().unwrap()["text"], "done");
    }

    #[tokio::test]
    async fn queued_keeps_waiting_while_errors_and_rejections_end_it() {
        let waiters = SyncWaiters::new();

        let mut queued = json!({});
        let _receiver = waiters.register(&mut queued);
        let id = waiter_id(&queued).unwrap();
        waiters.settle_unstarted(&id, &Ok(json!({"queued": true})));
        assert_eq!(waiters.len(), 1);
        waiters.settle_unstarted(&id, &Ok(json!({"runId": "r"})));
        assert_eq!(waiters.len(), 1);

        let mut failed = json!({});
        let receiver = waiters.register(&mut failed);
        waiters.settle_unstarted(&waiter_id(&failed).unwrap(), &Err("boom".into()));
        assert_eq!(receiver.await.unwrap().unwrap_err().to_string(), "boom");

        let mut rejected = json!({});
        let receiver = waiters.register(&mut rejected);
        waiters.settle_unstarted(
            &waiter_id(&rejected).unwrap(),
            &Ok(json!({"rejected": true, "reason": "hook"})),
        );
        assert_eq!(receiver.await.unwrap().unwrap()["reason"], "hook");
    }

    #[tokio::test]
    async fn cancelling_queued_turns_ends_their_waits() {
        let waiters = SyncWaiters::new();
        let mut params = json!({});
        let receiver = waiters.register(&mut params);
        waiters.cancel([&params, &json!({"text": "not a sync turn"})].into_iter());
        assert!(receiver.await.unwrap().is_err());
        assert_eq!(waiters.len(), 0);
    }
}
