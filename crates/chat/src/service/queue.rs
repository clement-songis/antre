//! Per-session message queue edits: cancel all, cancel one, promote one.

use std::collections::HashMap;

use serde_json::Value;

use super::types::{QueuedMessage, SessionMessageQueue};

pub(in crate::service) fn cancel_queued_messages(
    queues: &mut HashMap<String, SessionMessageQueue>,
    session_key: &str,
) -> Vec<QueuedMessage> {
    match queues.get_mut(session_key) {
        Some(queue) if queue.draining => queue.messages.drain(..).collect(),
        Some(_) => queues
            .remove(session_key)
            .map(|queue| queue.messages.into_iter().collect())
            .unwrap_or_default(),
        None => Vec::new(),
    }
}

/// Remove the queued message the client sent with `seq` (its `_seq` param).
pub(in crate::service) fn cancel_one_queued_message(
    queues: &mut HashMap<String, SessionMessageQueue>,
    session_key: &str,
    seq: u64,
) -> Option<QueuedMessage> {
    let queue = queues.get_mut(session_key)?;
    let position = queue
        .messages
        .iter()
        .position(|message| message.params.get("_seq").and_then(Value::as_u64) == Some(seq))?;
    let removed = queue.messages.remove(position);
    if queue.messages.is_empty() && !queue.draining {
        queues.remove(session_key);
    }
    removed
}

/// Move the queued message `seq` to the front of its session's queue.
pub(in crate::service) fn promote_queued_message(
    queues: &mut HashMap<String, SessionMessageQueue>,
    session_key: &str,
    seq: u64,
) -> bool {
    let Some(message) = cancel_one_queued_message(queues, session_key, seq) else {
        return false;
    };
    queues
        .entry(session_key.to_string())
        .or_default()
        .messages
        .push_front(message);
    true
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use {super::*, serde_json::json};

    #[test]
    fn cancellation_keeps_in_progress_replay_reserved() {
        let mut queues = HashMap::from([("s".to_string(), SessionMessageQueue {
            messages: [QueuedMessage {
                params: json!({"text": "cancel"}),
            }]
            .into_iter()
            .collect(),
            draining: true,
        })]);
        let removed = cancel_queued_messages(&mut queues, "s");
        assert_eq!(removed.len(), 1);
        assert!(queues["s"].draining);
        assert!(queues["s"].messages.is_empty());
    }

    #[test]
    fn cancelling_one_queued_message_keeps_the_others() {
        let mut queues = HashMap::from([("s".to_string(), SessionMessageQueue {
            messages: [1, 2, 3]
                .into_iter()
                .map(|seq| QueuedMessage {
                    params: json!({"text": format!("m{seq}"), "_seq": seq}),
                })
                .collect(),
            draining: false,
        })]);
        let removed = cancel_one_queued_message(&mut queues, "s", 2).unwrap();
        assert_eq!(removed.params["text"], "m2");
        let left: Vec<u64> = queues["s"]
            .messages
            .iter()
            .filter_map(|m| m.params["_seq"].as_u64())
            .collect();
        assert_eq!(left, vec![1, 3]);
        assert!(cancel_one_queued_message(&mut queues, "s", 9).is_none());
        cancel_one_queued_message(&mut queues, "s", 1).unwrap();
        cancel_one_queued_message(&mut queues, "s", 3).unwrap();
        assert!(!queues.contains_key("s"));
    }

    #[test]
    fn promoting_a_queued_message_moves_it_first() {
        let mut queues = HashMap::from([("s".to_string(), SessionMessageQueue {
            messages: [1, 2, 3]
                .into_iter()
                .map(|seq| QueuedMessage {
                    params: json!({"_seq": seq}),
                })
                .collect(),
            draining: false,
        })]);
        assert!(promote_queued_message(&mut queues, "s", 3));
        let order: Vec<u64> = queues["s"]
            .messages
            .iter()
            .filter_map(|m| m.params["_seq"].as_u64())
            .collect();
        assert_eq!(order, vec![3, 1, 2]);
        assert!(!promote_queued_message(&mut queues, "s", 7));
    }
}
