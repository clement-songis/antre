//! Who gets a session's single run permit, and who waits in its FIFO.

use std::{collections::HashMap, sync::Arc};

use {
    serde_json::Value,
    tokio::sync::{RwLock, Semaphore},
};

use super::types::{QueuedMessage, SessionMessageQueue, TurnAdmission};

/// Whether the session's lane is held by a run that is parked until a
/// provider quota resets.
///
/// A parked run holds no permit, but it still owns the lane: a turn that
/// started meanwhile would race it for the same history, and its answer
/// could land before the parked one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::service) enum LaneState {
    Open,
    /// A run waits for quota; new turns queue behind it.
    Parked,
    /// This turn is the parked run coming back; it goes first.
    Resuming,
}

impl LaneState {
    pub(in crate::service) fn of(session_key: &str, quota_resume: bool) -> Self {
        if quota_resume {
            Self::Resuming
        } else if crate::quota_wait::pending(session_key).is_some() {
            Self::Parked
        } else {
            Self::Open
        }
    }
}

/// Arbitrate the permit and FIFO under one queue lock. Ordinary arrivals
/// cannot pass queued work or a parked run, while the drainer's replay and a
/// resuming parked run can consume their reservation. If either cannot
/// acquire, it returns to the front.
pub(in crate::service) async fn admit_queued_turn(
    queues: &Arc<RwLock<HashMap<String, SessionMessageQueue>>>,
    session_sem: Arc<Semaphore>,
    session_key: &str,
    params: Value,
    queued_replay: bool,
    lane: LaneState,
) -> TurnAdmission {
    let mut queues = queues.write().await;
    let queue = queues.entry(session_key.to_string()).or_default();
    let reserved = (queued_replay && queue.draining) || lane == LaneState::Resuming;

    if !reserved && (lane == LaneState::Parked || queue.draining || !queue.messages.is_empty()) {
        queue.messages.push_back(QueuedMessage { params });
        return TurnAdmission::Queued(queue.messages.len());
    }

    match session_sem.try_acquire_owned() {
        Ok(permit) => TurnAdmission::Acquired(permit),
        Err(_) => {
            let message = QueuedMessage { params };
            if reserved {
                queue.messages.push_front(message);
            } else {
                queue.messages.push_back(message);
            }
            TurnAdmission::Queued(queue.messages.len())
        },
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use {super::*, serde_json::json};

    type Queues = Arc<RwLock<HashMap<String, SessionMessageQueue>>>;

    fn queues_with(texts: &[&str], draining: bool) -> Queues {
        Arc::new(RwLock::new(HashMap::from([(
            "s".to_string(),
            SessionMessageQueue {
                messages: texts
                    .iter()
                    .map(|text| QueuedMessage {
                        params: json!({ "text": text }),
                    })
                    .collect(),
                draining,
            },
        )])))
    }

    async fn texts(queues: &Queues) -> Vec<String> {
        queues.read().await["s"]
            .messages
            .iter()
            .filter_map(|message| message.params["text"].as_str().map(String::from))
            .collect()
    }

    #[tokio::test]
    async fn replay_reservation_prevents_new_arrival_overtaking_fifo() {
        let queues = queues_with(&["second"], true);
        let semaphore = Arc::new(Semaphore::new(1));
        let replay = admit_queued_turn(
            &queues,
            Arc::clone(&semaphore),
            "s",
            json!({"text": "first"}),
            true,
            LaneState::Open,
        )
        .await;
        let TurnAdmission::Acquired(permit) = replay else {
            panic!("reserved replay should acquire");
        };

        let arrival = admit_queued_turn(
            &queues,
            semaphore,
            "s",
            json!({"text": "third"}),
            false,
            LaneState::Open,
        )
        .await;
        assert!(matches!(arrival, TurnAdmission::Queued(2)));
        assert_eq!(texts(&queues).await, ["second", "third"]);
        drop(permit);
    }

    /// A message sent while the session waits for quota must not start a
    /// second run beside the parked one, even though no permit is held.
    #[tokio::test]
    async fn arrivals_queue_behind_a_parked_run() {
        let queues: Queues = Arc::default();
        let semaphore = Arc::new(Semaphore::new(1));
        let arrival = admit_queued_turn(
            &queues,
            Arc::clone(&semaphore),
            "s",
            json!({"text": "while parked"}),
            false,
            LaneState::Parked,
        )
        .await;
        assert!(matches!(arrival, TurnAdmission::Queued(1)));
        assert_eq!(semaphore.available_permits(), 1, "no permit was taken");
        assert_eq!(texts(&queues).await, ["while parked"]);
    }

    /// The parked run comes back ahead of what queued behind it.
    #[tokio::test]
    async fn a_resuming_run_goes_first() {
        let queues = queues_with(&["queued while parked"], false);
        let semaphore = Arc::new(Semaphore::new(1));
        let resume = admit_queued_turn(
            &queues,
            Arc::clone(&semaphore),
            "s",
            json!({"text": "parked"}),
            false,
            LaneState::Resuming,
        )
        .await;
        assert!(matches!(resume, TurnAdmission::Acquired(_)));
        assert_eq!(texts(&queues).await, ["queued while parked"]);
    }

    /// If something holds the permit when the parked run resumes (a reply
    /// that slipped in), the resume waits at the front, not the back.
    #[tokio::test]
    async fn a_resuming_run_that_cannot_start_waits_at_the_front() {
        let queues = queues_with(&["later"], false);
        let semaphore = Arc::new(Semaphore::new(1));
        let _held = Arc::clone(&semaphore).try_acquire_owned().unwrap();
        let resume = admit_queued_turn(
            &queues,
            semaphore,
            "s",
            json!({"text": "parked"}),
            false,
            LaneState::Resuming,
        )
        .await;
        assert!(matches!(resume, TurnAdmission::Queued(2)));
        assert_eq!(texts(&queues).await, ["parked", "later"]);
    }

    /// Two arrivals racing for an idle session: exactly one runs, the other
    /// queues.
    #[tokio::test]
    async fn concurrent_arrivals_admit_exactly_one_run() {
        let queues: Queues = Arc::default();
        let semaphore = Arc::new(Semaphore::new(1));
        let mut tasks = Vec::new();
        for index in 0..16 {
            let queues = Arc::clone(&queues);
            let semaphore = Arc::clone(&semaphore);
            tasks.push(tokio::spawn(async move {
                admit_queued_turn(
                    &queues,
                    semaphore,
                    "s",
                    json!({ "text": index.to_string() }),
                    false,
                    LaneState::Open,
                )
                .await
            }));
        }
        let mut permits = Vec::new();
        let mut queued = 0;
        for task in tasks {
            match task.await.unwrap() {
                TurnAdmission::Acquired(permit) => permits.push(permit),
                TurnAdmission::Queued(_) => queued += 1,
            }
        }
        assert_eq!(permits.len(), 1);
        assert_eq!(queued, 15);
        assert_eq!(texts(&queues).await.len(), 15);
    }

    #[test]
    fn lane_state_follows_the_quota_registry() {
        assert_eq!(LaneState::of("no-wait-session", false), LaneState::Open);
        assert_eq!(LaneState::of("no-wait-session", true), LaneState::Resuming);
    }
}
