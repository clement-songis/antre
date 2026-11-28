#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    collections::HashSet,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use {
    antre_channels::{ChannelAckOutcome, ChannelActivity, ChannelType},
    antre_common::types::ReplyPayload,
    antre_delivery::{DeliveryStore, RetryPolicy},
};

use {
    super::*,
    crate::{test_runtime::TestRuntime, types::ReplyMedium},
};

/// Fails the first `failures` sends with a retryable error, then records.
#[derive(Default)]
struct FlakyOutbound {
    failures: AtomicUsize,
    calls: Mutex<Vec<(&'static str, String, Option<String>)>>,
}

impl FlakyOutbound {
    fn failing(failures: usize) -> Arc<Self> {
        Arc::new(Self {
            failures: AtomicUsize::new(failures),
            ..Self::default()
        })
    }

    fn record(
        &self,
        method: &'static str,
        text: &str,
        reply_to: Option<&str>,
    ) -> antre_channels::Result<Vec<String>> {
        if self
            .failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok()
        {
            return Err(antre_channels::Error::Unavailable {
                message: "telegram 502".into(),
            });
        }
        let mut calls = self.calls.lock().unwrap();
        calls.push((method, text.to_string(), reply_to.map(String::from)));
        Ok(vec![format!("id-{}", calls.len())])
    }

    fn calls(&self) -> Vec<(&'static str, String, Option<String>)> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl antre_channels::ChannelOutbound for FlakyOutbound {
    async fn send_text(
        &self,
        _account_id: &str,
        _to: &str,
        text: &str,
        reply_to: Option<&str>,
    ) -> antre_channels::Result<()> {
        self.record("send_text", text, reply_to).map(|_| ())
    }

    async fn send_media(
        &self,
        _account_id: &str,
        _to: &str,
        payload: &ReplyPayload,
        reply_to: Option<&str>,
    ) -> antre_channels::Result<()> {
        self.record("send_media", &payload.text, reply_to)
            .map(|_| ())
    }

    async fn send_text_reporting_ids(
        &self,
        _account_id: &str,
        _to: &str,
        text: &str,
        reply_to: Option<&str>,
    ) -> antre_channels::Result<Vec<String>> {
        self.record("send_text_reporting_ids", text, reply_to)
    }

    async fn send_text_with_suffix_reporting_ids(
        &self,
        _account_id: &str,
        _to: &str,
        text: &str,
        suffix_html: &str,
        reply_to: Option<&str>,
    ) -> antre_channels::Result<Vec<String>> {
        self.record(
            "send_text_with_suffix_reporting_ids",
            &format!("{text}|{suffix_html}"),
            reply_to,
        )
    }

    async fn send_html_reporting_ids(
        &self,
        _account_id: &str,
        _to: &str,
        html: &str,
        reply_to: Option<&str>,
    ) -> antre_channels::Result<Vec<String>> {
        self.record("send_html_reporting_ids", html, reply_to)
    }
}

fn policy(max_attempts: u32) -> RetryPolicy {
    RetryPolicy {
        max_attempts,
        initial_delay: Duration::from_millis(5),
        max_delay: Duration::from_millis(10),
        attempt_timeout: Duration::from_millis(500),
        lease: Duration::from_secs(1),
        retention: Duration::from_secs(60),
    }
}

fn target() -> ChannelReplyTarget {
    ChannelReplyTarget {
        channel_type: ChannelType::Telegram,
        account_id: "bot".into(),
        chat_id: "42".into(),
        message_id: Some("100".into()),
        thread_id: None,
        ack_message_id: Some("100".into()),
        direct_chat: true,
    }
}

const SESSION: &str = "telegram:bot:42";

async fn runtime_with(
    outbound: Option<Arc<FlakyOutbound>>,
    max_attempts: u32,
) -> (Arc<TestRuntime>, Arc<dyn ChatRuntime>, Arc<Outbox>) {
    let outbox = Outbox::new(
        DeliveryStore::in_memory().await.unwrap(),
        policy(max_attempts),
    );
    let mut runtime = TestRuntime::new(Arc::new(antre_service_traits::NoopChatService))
        .with_outbox(Arc::clone(&outbox));
    if let Some(outbound) = outbound {
        runtime = runtime.with_outbound(outbound);
    }
    let runtime = Arc::new(runtime);
    let dyn_runtime: Arc<dyn ChatRuntime> = runtime.clone();
    install(&outbox, &dyn_runtime);
    (runtime, dyn_runtime, outbox)
}

async fn drain(outbox: &Arc<Outbox>) {
    for _ in 0..200 {
        outbox.run_due().await.unwrap();
        let counts = outbox.store().counts().await.unwrap();
        if counts.queued == 0 && counts.sending == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("outbox did not drain");
}

fn states(runtime: &TestRuntime) -> Vec<String> {
    runtime
        .broadcasts_on("delivery")
        .iter()
        .map(|event| event["state"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[tokio::test]
async fn a_failed_reply_is_retried_and_reported_on_the_web() {
    let outbound = FlakyOutbound::failing(1);
    let (runtime, state, outbox) = runtime_with(Some(outbound.clone()), 5).await;
    state.push_channel_reply(SESSION, target()).await;

    crate::channels::deliver_channel_replies(
        &state,
        "run-1",
        SESSION,
        "the answer",
        ReplyMedium::Text,
        &HashSet::new(),
    )
    .await;
    assert!(outbound.calls().is_empty(), "first send failed");
    drain(&outbox).await;

    assert_eq!(outbound.calls(), vec![(
        "send_text_reporting_ids",
        "the answer".to_string(),
        Some("100".to_string())
    )]);
    assert_eq!(states(&runtime), vec!["queued", "delivered"]);
    let event = runtime.broadcasts_on("delivery").pop().unwrap();
    assert_eq!(event["sessionKey"], SESSION);
    assert_eq!(event["runId"], "run-1");
    assert_eq!(event["kind"], "reply");
    assert_eq!(event["channelType"], "telegram");
    assert!(runtime.activities.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_reply_that_cannot_be_delivered_marks_the_turn_failed() {
    let outbound = FlakyOutbound::failing(10);
    let (runtime, state, outbox) = runtime_with(Some(outbound.clone()), 2).await;
    state.push_channel_reply(SESSION, target()).await;

    crate::channels::deliver_channel_replies(
        &state,
        "run-1",
        SESSION,
        "the answer",
        ReplyMedium::Text,
        &HashSet::new(),
    )
    .await;
    drain(&outbox).await;

    assert_eq!(states(&runtime), vec!["queued", "failed"]);
    let event = runtime.broadcasts_on("delivery").pop().unwrap();
    assert_eq!(
        event["error"],
        "channel operation unavailable: telegram 502"
    );
    let activities = runtime.activities.lock().unwrap().clone();
    assert_eq!(activities.len(), 1);
    assert_eq!(activities[0].0, "run-1");
    assert!(matches!(
        activities[0].1,
        ChannelActivity::Finished(ChannelAckOutcome::Failure)
    ));

    // The recipient writes again: the reply is sent once more, flagged.
    outbound.failures.store(0, Ordering::SeqCst);
    assert_eq!(outbox.redeliver_failed(&target()).await.unwrap(), 1);
    drain(&outbox).await;
    let calls = outbound.calls();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].1.starts_with("(Resent:"), "{}", calls[0].1);
    assert!(calls[0].1.ends_with("the answer"));
}

#[tokio::test]
async fn replies_wait_in_the_outbox_while_channels_are_down() {
    let (runtime, state, outbox) = runtime_with(None, 5).await;
    state.push_channel_reply(SESSION, target()).await;

    crate::channels::deliver_channel_replies(
        &state,
        "run-1",
        SESSION,
        "the answer",
        ReplyMedium::Text,
        &HashSet::new(),
    )
    .await;

    let pending = outbox.store().list_for_session(SESSION, 10).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].state, DeliveryState::Queued);
    assert_eq!(
        pending[0].last_error.as_deref(),
        Some("channel outbound unavailable")
    );
    assert_eq!(pending[0].reply_to.as_deref(), Some("100"));
    // Not a failure yet, so the acknowledgment is left alone.
    assert!(runtime.activities.lock().unwrap().is_empty());
}

#[tokio::test]
async fn terminal_errors_go_through_the_outbox_without_a_trace() {
    let outbound = FlakyOutbound::failing(0);
    let (runtime, state, _outbox) = runtime_with(Some(outbound.clone()), 3).await;
    state.push_channel_reply(SESSION, target()).await;

    crate::channels::deliver_channel_error(
        &state,
        SESSION,
        &serde_json::json!({"type": "timeout", "title": "Timed out", "detail": "too slow"}),
    )
    .await;

    let calls = outbound.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "send_text_reporting_ids");
    let event = runtime.broadcasts_on("delivery").pop().unwrap();
    assert_eq!(event["kind"], "error");
    assert_eq!(event["state"], "delivered");
}

#[tokio::test]
async fn the_sink_picks_the_send_for_each_body() {
    let outbound = FlakyOutbound::failing(0);
    let (_runtime, state, outbox) = runtime_with(Some(outbound.clone()), 3).await;
    let sink = ChannelDeliverySink::new(state);
    let stored = |body: DeliveryBody, redelivered: bool| {
        let outbox = Arc::clone(&outbox);
        async move {
            let id = outbox
                .store()
                .enqueue(
                    &NewDelivery {
                        session_key: SESSION.into(),
                        run_id: Some("run-1".into()),
                        kind: DeliveryKind::Logbook,
                        target: target(),
                        body,
                        reply_to: None,
                    },
                    time::OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            let mut delivery = outbox.store().get(id).await.unwrap().unwrap();
            delivery.redelivered = redelivered;
            delivery
        }
    };

    sink.send(
        &stored(
            DeliveryBody::Text {
                text: "t".into(),
                suffix_html: Some("<b>log</b>".into()),
            },
            false,
        )
        .await,
    )
    .await
    .unwrap();
    sink.send(
        &stored(
            DeliveryBody::Text {
                text: "t".into(),
                suffix_html: Some(String::new()),
            },
            true,
        )
        .await,
    )
    .await
    .unwrap();
    sink.send(
        &stored(
            DeliveryBody::Html {
                html: "<i>h</i>".into(),
            },
            false,
        )
        .await,
    )
    .await
    .unwrap();

    let methods: Vec<_> = outbound
        .calls()
        .into_iter()
        .map(|call| (call.0, call.1))
        .collect();
    assert_eq!(methods, vec![
        (
            "send_text_with_suffix_reporting_ids",
            "t|<b>log</b>".to_string()
        ),
        ("send_text_reporting_ids", format!("{REDELIVERY_NOTE}t")),
        ("send_html_reporting_ids", "<i>h</i>".to_string()),
    ]);
}

#[tokio::test]
async fn the_sink_retries_when_there_is_no_outbound() {
    let (_runtime, state, outbox) = runtime_with(None, 3).await;
    let id = outbox
        .store()
        .enqueue(
            &NewDelivery {
                session_key: SESSION.into(),
                run_id: None,
                kind: DeliveryKind::Notice,
                target: target(),
                body: DeliveryBody::text("x"),
                reply_to: None,
            },
            time::OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();
    let delivery = outbox.store().get(id).await.unwrap().unwrap();
    let error = ChannelDeliverySink::new(state)
        .send(&delivery)
        .await
        .unwrap_err();
    assert!(matches!(error, SinkError::Retryable(_)));
}
