//! The channel side of the delivery outbox: how a persisted delivery is sent,
//! and how its outcome is reported.

use std::sync::Arc;

use {
    antre_channels::ChannelReplyTarget,
    antre_delivery::{
        Delivery, DeliveryBody, DeliveryKind, DeliveryObserver, DeliverySink, DeliveryState,
        NewDelivery, Outbox, SinkError,
    },
    async_trait::async_trait,
    serde_json::json,
};

use crate::{channel_acks::note_delivery_failed, runtime::ChatRuntime};

/// Prepended to a reply sent again after it failed, so the recipient knows
/// why an old answer shows up.
const REDELIVERY_NOTE: &str = "(Resent: this reply could not be delivered earlier.)\n\n";

/// Characters of the message shown in delivery events.
const PREVIEW_CHARS: usize = 120;

/// Sends deliveries through the runtime's channel outbound, using the
/// id-reporting sends so replies keep their trace link.
pub struct ChannelDeliverySink {
    runtime: Arc<dyn ChatRuntime>,
}

impl ChannelDeliverySink {
    #[must_use]
    pub fn new(runtime: Arc<dyn ChatRuntime>) -> Self {
        Self { runtime }
    }
}

#[async_trait]
impl DeliverySink for ChannelDeliverySink {
    async fn send(&self, delivery: &Delivery) -> Result<Vec<String>, SinkError> {
        let outbound = self
            .runtime
            .channel_outbound()
            .ok_or_else(|| SinkError::Retryable("channel outbound unavailable".into()))?;
        let target = &delivery.target;
        let to = target.outbound_to();
        let reply_to = delivery.reply_to.as_deref();
        let message_ids = match &delivery.body {
            DeliveryBody::Text { text, suffix_html } => {
                let text = if delivery.redelivered {
                    std::borrow::Cow::Owned(format!("{REDELIVERY_NOTE}{text}"))
                } else {
                    std::borrow::Cow::Borrowed(text.as_str())
                };
                match suffix_html.as_deref().filter(|suffix| !suffix.is_empty()) {
                    Some(suffix) => {
                        outbound
                            .send_text_with_suffix_reporting_ids(
                                &target.account_id,
                                &to,
                                &text,
                                suffix,
                                reply_to,
                            )
                            .await?
                    },
                    None => {
                        outbound
                            .send_text_reporting_ids(&target.account_id, &to, &text, reply_to)
                            .await?
                    },
                }
            },
            DeliveryBody::Html { html } => {
                outbound
                    .send_html_reporting_ids(&target.account_id, &to, html, reply_to)
                    .await?
            },
        };
        // Errors are not answers anyone reacts to; replies and logbooks are.
        if delivery.kind != DeliveryKind::Error
            && let Some(run_id) = delivery.run_id.as_deref()
        {
            crate::channel_feedback::record_reply_trace(
                self.runtime.feedback().as_deref(),
                target,
                &message_ids,
                &delivery.session_key,
                run_id,
            )
            .await;
        }
        Ok(message_ids)
    }
}

/// Broadcasts every delivery outcome to the web UI, and marks the channel
/// acknowledgment failed when a run's reply is lost.
pub struct ChannelDeliveryObserver {
    runtime: Arc<dyn ChatRuntime>,
}

impl ChannelDeliveryObserver {
    #[must_use]
    pub fn new(runtime: Arc<dyn ChatRuntime>) -> Self {
        Self { runtime }
    }
}

/// The `delivery` WebSocket event for a delivery's current state.
#[must_use]
pub fn delivery_event(delivery: &Delivery) -> serde_json::Value {
    json!({
        "id": delivery.id,
        "sessionKey": delivery.session_key,
        "runId": delivery.run_id,
        "kind": delivery.kind.as_str(),
        "state": delivery.state.as_str(),
        "attempts": delivery.attempts,
        "error": delivery.last_error,
        "channelType": delivery.target.channel_type.as_str(),
        "preview": delivery.body.preview(PREVIEW_CHARS),
        "redelivered": delivery.redelivered,
    })
}

#[async_trait]
impl DeliveryObserver for ChannelDeliveryObserver {
    async fn on_change(&self, delivery: &Delivery) {
        self.runtime
            .broadcast("delivery", delivery_event(delivery))
            .await;
        if delivery.state == DeliveryState::Failed
            && let Some(run_id) = delivery.run_id.as_deref()
        {
            note_delivery_failed(&self.runtime, run_id).await;
        }
    }
}

/// Install the channel sink and observer on `outbox`.
pub fn install(outbox: &Outbox, runtime: &Arc<dyn ChatRuntime>) {
    outbox.set_sink(Arc::new(ChannelDeliverySink::new(Arc::clone(runtime))));
    outbox.set_observer(Arc::new(ChannelDeliveryObserver::new(Arc::clone(runtime))));
}

/// Submit one delivery per target concurrently; each target keeps its own
/// order and retries. Returns how many were delivered at once.
pub(crate) async fn submit_to_targets(
    outbox: &Arc<Outbox>,
    targets: Vec<ChannelReplyTarget>,
    session_key: &str,
    run_id: Option<&str>,
    kind: DeliveryKind,
    body: &DeliveryBody,
    thread_under_message: bool,
) -> usize {
    let mut tasks = tokio::task::JoinSet::new();
    for target in targets {
        let outbox = Arc::clone(outbox);
        let reply_to = thread_under_message
            .then(|| target.message_id.clone())
            .flatten();
        let new = NewDelivery {
            session_key: session_key.to_string(),
            run_id: run_id.map(String::from),
            kind,
            target,
            body: body.clone(),
            reply_to,
        };
        tasks.spawn(async move { outbox.submit(new).await });
    }
    let mut delivered = 0;
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok(outcome) if outcome.is_delivered() => delivered += 1,
            Ok(_) => {},
            Err(error) => tracing::warn!(%error, "delivery submit task failed"),
        }
    }
    delivered
}

#[cfg(test)]
#[path = "channel_delivery_tests.rs"]
mod tests;
