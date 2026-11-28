//! Messages added to a session outside a run: `chat.inject` and the results
//! of background sub-agents.
//!
//! They take the session's run permit like a turn does, so they never land
//! in the middle of a run's history, and reach the session's channel
//! through the delivery outbox.

use {
    antre_delivery::{DeliveryBody, DeliveryKind, NewDelivery},
    antre_sessions::PersistedMessage,
    antre_tools::spawn_agent_tasks::{SpawnTask, SpawnTaskStatus, SpawnTaskStore},
    serde_json::{Value, json},
    tracing::{info, warn},
};

use {
    super::queue_drain,
    crate::{
        service::LiveChatService,
        types::{BroadcastOpts, broadcast, now_ms},
    },
};

/// Longest sub-agent result posted as is; longer ones are cut, the full text
/// stays available through `spawn_result` until the task expires.
const MAX_ANNOUNCED_CHARS: usize = 8_000;

fn assistant_message(text: &str) -> PersistedMessage {
    PersistedMessage::Assistant {
        content: text.to_string(),
        created_at: Some(now_ms()),
        model: None,
        provider: None,
        input_tokens: None,
        output_tokens: None,
        cache_read_tokens: None,
        cache_write_tokens: None,
        duration_ms: None,
        request_input_tokens: None,
        request_output_tokens: None,
        request_cache_read_tokens: None,
        request_cache_write_tokens: None,
        tool_calls: None,
        reasoning: None,
        llm_api_response: None,
        audio: None,
        seq: None,
        run_id: None,
    }
}

/// The message posted for a finished background task.
#[must_use]
pub(crate) fn announcement(task: &SpawnTask) -> (String, String) {
    let (title, body) = match task.status {
        SpawnTaskStatus::Failed => (
            "Background task failed",
            task.error.clone().unwrap_or_default(),
        ),
        _ => (
            "Background task finished",
            task.text.clone().unwrap_or_default(),
        ),
    };
    let mut body: String = body.chars().take(MAX_ANNOUNCED_CHARS).collect();
    if task.text.as_deref().unwrap_or_default().chars().count() > MAX_ANNOUNCED_CHARS {
        body.push_str("\n\n[truncated]");
    }
    let text = format!("**{title}:** {}\n\n{body}", task.task);
    (title.to_string(), text)
}

impl LiveChatService {
    /// Append `text` as an assistant message once the session is free, tell
    /// the web UI, and send it to the session's channel.
    async fn post_outside_run(
        &self,
        session_key: &str,
        title: &str,
        text: &str,
    ) -> Result<(), String> {
        self.session_store
            .append(session_key, &assistant_message(text).to_value())
            .await
            .map_err(|error| error.to_string())?;
        if let Ok(count) = self.session_store.count(session_key).await {
            self.session_metadata.touch(session_key, count).await;
        }
        broadcast(
            &self.state,
            "chat",
            json!({
                "sessionKey": session_key,
                "state": "notice",
                "title": title,
                "message": text,
            }),
            BroadcastOpts::default(),
        )
        .await;
        self.send_to_bound_channel(session_key, text).await;
        Ok(())
    }

    async fn send_to_bound_channel(&self, session_key: &str, text: &str) {
        let Some(target) = self
            .session_metadata
            .get(session_key)
            .await
            .and_then(|entry| entry.channel_binding)
            .and_then(|binding| {
                serde_json::from_str::<antre_channels::ChannelReplyTarget>(&binding).ok()
            })
        else {
            return;
        };
        if let Some(outbox) = self.state.delivery_outbox() {
            outbox
                .submit(NewDelivery {
                    session_key: session_key.to_string(),
                    run_id: None,
                    kind: DeliveryKind::Notice,
                    target,
                    body: DeliveryBody::text(text),
                    reply_to: None,
                })
                .await;
            return;
        }
        let Some(outbound) = self.state.channel_outbound() else {
            warn!(session = %session_key, "no channel outbound; message kept in the session only");
            return;
        };
        if let Err(error) = outbound
            .send_text(&target.account_id, &target.outbound_to(), text, None)
            .await
        {
            warn!(session = %session_key, %error, "failed to send message to the session's channel");
        }
    }

    /// Run `post` holding the session's run permit, then let queued turns
    /// run: an arrival that queued while the permit was held has nobody else
    /// to start it.
    async fn with_session_lane<F, T>(&self, session_key: &str, post: F) -> Result<T, String>
    where
        F: Future<Output = Result<T, String>>,
    {
        let semaphore = self.session_semaphore(session_key).await;
        let permit = semaphore
            .acquire_owned()
            .await
            .map_err(|_| "session lock closed".to_string())?;
        let result = post.await;
        drop(permit);
        queue_drain::drain_and_replay(
            &self.message_queue,
            session_key,
            self.config.chat.message_queue_mode,
            &self.state,
        )
        .await;
        result
    }

    #[tracing::instrument(skip(self, params))]
    pub(super) async fn inject_impl(&self, params: Value) -> Result<Value, String> {
        let session_key = params
            .get("sessionKey")
            .or_else(|| params.get("_session_key"))
            .and_then(Value::as_str)
            .filter(|key| !key.is_empty())
            .ok_or_else(|| "missing 'sessionKey'".to_string())?
            .to_string();
        let text = params
            .get("text")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .ok_or_else(|| "missing 'text'".to_string())?
            .to_string();
        let title = params
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Message")
            .to_string();
        self.with_session_lane(
            &session_key,
            self.post_outside_run(&session_key, &title, &text),
        )
        .await?;
        info!(session = %session_key, "chat.inject: message added");
        Ok(json!({ "ok": true }))
    }

    /// Post a background sub-agent's outcome to the session that spawned it,
    /// unless that session already read it with `spawn_result`.
    ///
    /// Waits for the session's current turn, so a parent that polls its task
    /// in the same turn reads it there and nothing is posted twice.
    #[tracing::instrument(skip(self, store))]
    pub async fn announce_background_task(
        &self,
        session_key: &str,
        task_id: &str,
        store: &SpawnTaskStore,
    ) -> Result<bool, String> {
        self.with_session_lane(session_key, async {
            let Some(task) = store.take_unread_outcome(task_id).await else {
                return Ok(false);
            };
            let (title, text) = announcement(&task);
            self.post_outside_run(session_key, &title, &text).await?;
            info!(session = %session_key, task_id, "background task outcome posted to its session");
            Ok(true)
        })
        .await
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn task(status: SpawnTaskStatus, text: Option<&str>, error: Option<&str>) -> SpawnTask {
        SpawnTask {
            id: "t1".into(),
            task: "summarize the logs".into(),
            session_key: Some("main".into()),
            status,
            model: "m".into(),
            preset: None,
            started_at: time::OffsetDateTime::now_utc(),
            finished_at: None,
            text: text.map(String::from),
            iterations: 1,
            tool_calls_made: 0,
            error: error.map(String::from),
            result_read: false,
        }
    }

    #[test]
    fn announcements_carry_the_task_and_its_outcome() {
        let (title, text) = announcement(&task(SpawnTaskStatus::Completed, Some("all good"), None));
        assert_eq!(title, "Background task finished");
        assert_eq!(
            text,
            "**Background task finished:** summarize the logs\n\nall good"
        );
        let (title, text) = announcement(&task(SpawnTaskStatus::Failed, None, Some("boom")));
        assert_eq!(title, "Background task failed");
        assert!(text.ends_with("boom"));
    }

    #[test]
    fn long_results_are_cut() {
        let long = "x".repeat(MAX_ANNOUNCED_CHARS + 10);
        let (_, text) = announcement(&task(SpawnTaskStatus::Completed, Some(&long), None));
        assert!(text.ends_with("[truncated]"));
        assert!(text.chars().count() < MAX_ANNOUNCED_CHARS + 100);
    }

    #[test]
    fn injected_messages_are_plain_assistant_turns() {
        let value = assistant_message("hello").to_value();
        assert_eq!(value["role"], "assistant");
        assert_eq!(value["content"], "hello");
        assert!(value.get("model").is_none());
    }
}
