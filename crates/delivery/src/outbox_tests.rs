#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    collections::VecDeque,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use moltis_channels::ChannelType;

use {
    super::*,
    crate::{
        model::{DeliveryBody, DeliveryKind},
        store::StateCounts,
    },
};

fn policy() -> RetryPolicy {
    RetryPolicy {
        max_attempts: 3,
        initial_delay: Duration::from_millis(5),
        max_delay: Duration::from_millis(20),
        attempt_timeout: Duration::from_millis(200),
        lease: Duration::from_millis(400),
        retention: Duration::from_secs(60),
    }
}

fn target(chat: &str) -> ChannelReplyTarget {
    ChannelReplyTarget {
        channel_type: ChannelType::Telegram,
        account_id: "bot".into(),
        chat_id: chat.into(),
        message_id: None,
        thread_id: None,
        ack_message_id: None,
        direct_chat: true,
    }
}

fn new(chat: &str, text: &str) -> NewDelivery {
    NewDelivery {
        session_key: format!("telegram:bot:{chat}"),
        run_id: None,
        kind: DeliveryKind::Reply,
        target: target(chat),
        body: DeliveryBody::text(text),
        reply_to: None,
    }
}

/// Plays back scripted results per call (then succeeds), optionally slowly
/// for one chat, and records what it sent.
#[derive(Default)]
struct ScriptedSink {
    script: Mutex<VecDeque<std::result::Result<(), SinkError>>>,
    sent: Mutex<Vec<(i64, String, bool)>>,
    calls: AtomicUsize,
    slow_chat: Option<(String, Duration)>,
}

impl ScriptedSink {
    fn with_script(script: Vec<std::result::Result<(), SinkError>>) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(script.into()),
            ..Self::default()
        })
    }

    fn sent_texts(&self) -> Vec<String> {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .map(|(_, text, _)| text.clone())
            .collect()
    }
}

#[async_trait]
impl DeliverySink for ScriptedSink {
    async fn send(&self, delivery: &Delivery) -> std::result::Result<Vec<String>, SinkError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some((chat, delay)) = &self.slow_chat
            && &delivery.target.chat_id == chat
        {
            tokio::time::sleep(*delay).await;
        }
        let next = self.script.lock().unwrap().pop_front();
        if let Some(Err(error)) = next {
            return Err(error);
        }
        let text = match &delivery.body {
            DeliveryBody::Text { text, .. } => text.clone(),
            DeliveryBody::Html { html } => html.clone(),
        };
        self.sent
            .lock()
            .unwrap()
            .push((delivery.id, text, delivery.redelivered));
        Ok(vec![format!("msg-{}", delivery.id)])
    }
}

#[derive(Default)]
struct RecordingObserver {
    seen: Mutex<Vec<(i64, DeliveryState, u32)>>,
}

#[async_trait]
impl DeliveryObserver for RecordingObserver {
    async fn on_change(&self, delivery: &Delivery) {
        self.seen
            .lock()
            .unwrap()
            .push((delivery.id, delivery.state, delivery.attempts));
    }
}

async fn outbox_with(sink: Arc<ScriptedSink>) -> (Arc<Outbox>, Arc<RecordingObserver>) {
    let outbox = Outbox::new(DeliveryStore::in_memory().await.unwrap(), policy());
    outbox.set_sink(sink);
    let observer = Arc::new(RecordingObserver::default());
    outbox.set_observer(observer.clone());
    (outbox, observer)
}

/// Run worker passes until nothing is queued or sending (or give up).
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

fn retryable(message: &str) -> std::result::Result<(), SinkError> {
    Err(SinkError::Retryable(message.into()))
}

#[tokio::test]
async fn a_healthy_channel_gets_the_reply_at_once() {
    let sink = ScriptedSink::with_script(vec![]);
    let (outbox, observer) = outbox_with(sink.clone()).await;

    let outcome = outbox.submit(new("1", "hi")).await;
    let SubmitOutcome::Delivered { id, message_ids } = outcome.clone() else {
        panic!("expected delivered, got {outcome:?}");
    };
    assert!(outcome.is_delivered() && !outcome.is_failed());
    assert_eq!(message_ids, vec![format!("msg-{id}")]);
    assert_eq!(sink.sent_texts(), vec!["hi"]);
    assert_eq!(*observer.seen.lock().unwrap(), vec![(
        id,
        DeliveryState::Delivered,
        1
    )]);
    let stored = outbox.store().get(id).await.unwrap().unwrap();
    assert_eq!(stored.state, DeliveryState::Delivered);
    assert_eq!(stored.message_ids, message_ids);
}

#[tokio::test]
async fn a_transient_failure_is_retried_until_it_succeeds() {
    let sink = ScriptedSink::with_script(vec![retryable("503"), retryable("503")]);
    let (outbox, observer) = outbox_with(sink.clone()).await;

    let outcome = outbox.submit(new("1", "hi")).await;
    let SubmitOutcome::Pending { id } = outcome else {
        panic!("expected pending, got {outcome:?}");
    };
    drain(&outbox).await;

    assert_eq!(sink.sent_texts(), vec!["hi"]);
    assert_eq!(sink.calls.load(Ordering::SeqCst), 3);
    let states: Vec<_> = observer
        .seen
        .lock()
        .unwrap()
        .iter()
        .map(|(_, state, attempts)| (*state, *attempts))
        .collect();
    assert_eq!(states, vec![
        (DeliveryState::Queued, 1),
        (DeliveryState::Queued, 2),
        (DeliveryState::Delivered, 3),
    ]);
    let stored = outbox.store().get(id).await.unwrap().unwrap();
    assert_eq!(stored.state, DeliveryState::Delivered);
    assert!(stored.last_error.is_none());
}

#[tokio::test]
async fn a_delivery_out_of_attempts_is_reported_failed() {
    let sink = ScriptedSink::with_script(vec![retryable("a"), retryable("b"), retryable("c")]);
    let (outbox, observer) = outbox_with(sink.clone()).await;

    let SubmitOutcome::Pending { id } = outbox.submit(new("1", "hi")).await else {
        panic!("expected pending");
    };
    drain(&outbox).await;

    let stored = outbox.store().get(id).await.unwrap().unwrap();
    assert_eq!(stored.state, DeliveryState::Failed);
    assert_eq!(stored.attempts, 3);
    assert_eq!(stored.last_error.as_deref(), Some("c"));
    assert_eq!(
        observer.seen.lock().unwrap().last(),
        Some(&(id, DeliveryState::Failed, 3))
    );
    assert!(sink.sent_texts().is_empty());
}

#[tokio::test]
async fn a_rejected_delivery_fails_without_retrying() {
    let sink = ScriptedSink::with_script(vec![Err(SinkError::Rejected("no such chat".into()))]);
    let (outbox, _) = outbox_with(sink.clone()).await;

    let outcome = outbox.submit(new("1", "hi")).await;
    assert!(outcome.is_failed());
    let SubmitOutcome::Failed { error, .. } = outcome else {
        panic!("expected failed");
    };
    assert_eq!(error, "no such chat");
    assert_eq!(sink.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn replies_to_one_chat_keep_their_order_across_retries() {
    let sink = ScriptedSink::with_script(vec![retryable("flaky")]);
    let (outbox, _) = outbox_with(sink.clone()).await;

    assert!(matches!(
        outbox.submit(new("1", "first")).await,
        SubmitOutcome::Pending { .. }
    ));
    // The first is waiting for its retry, so the second must wait too.
    assert!(matches!(
        outbox.submit(new("1", "second")).await,
        SubmitOutcome::Pending { .. }
    ));
    // Another chat is not held up.
    assert!(outbox.submit(new("2", "elsewhere")).await.is_delivered());
    drain(&outbox).await;

    assert_eq!(sink.sent_texts(), vec!["elsewhere", "first", "second"]);
}

#[tokio::test]
async fn deliveries_wait_for_the_sink_and_go_out_once_it_exists() {
    let outbox = Outbox::new(DeliveryStore::in_memory().await.unwrap(), policy());
    let SubmitOutcome::Pending { id } = outbox.submit(new("1", "early")).await else {
        panic!("expected pending");
    };
    let stored = outbox.store().get(id).await.unwrap().unwrap();
    assert_eq!(
        stored.last_error.as_deref(),
        Some("channel delivery is not ready yet")
    );

    let sink = ScriptedSink::with_script(vec![]);
    assert!(outbox.set_sink(sink.clone()));
    assert!(!outbox.set_sink(sink.clone()));
    drain(&outbox).await;
    assert_eq!(sink.sent_texts(), vec!["early"]);
}

#[tokio::test]
async fn a_slow_send_times_out_and_is_retried() {
    let sink = Arc::new(ScriptedSink {
        slow_chat: Some(("1".into(), Duration::from_secs(5))),
        ..ScriptedSink::default()
    });
    let mut fast_policy = policy();
    fast_policy.attempt_timeout = Duration::from_millis(20);
    fast_policy.max_attempts = 1;
    let outbox = Outbox::new(DeliveryStore::in_memory().await.unwrap(), fast_policy);
    outbox.set_sink(sink);

    let outcome = outbox.submit(new("1", "hi")).await;
    let SubmitOutcome::Failed { error, .. } = outcome else {
        panic!("expected failed, got {outcome:?}");
    };
    assert!(error.contains("timed out"), "{error}");
}

/// A caller that stops waiting must not leave the record half-sent: the
/// attempt finishes in its own task and records its outcome.
#[tokio::test]
async fn a_caller_giving_up_does_not_cancel_the_send() {
    let sink = Arc::new(ScriptedSink {
        slow_chat: Some(("1".into(), Duration::from_millis(60))),
        ..ScriptedSink::default()
    });
    let (outbox, _) = outbox_with(sink.clone()).await;

    let gave_up = tokio::time::timeout(Duration::from_millis(10), outbox.submit(new("1", "hi")))
        .await
        .is_err();
    assert!(gave_up);
    for _ in 0..100 {
        if outbox.store().counts().await.unwrap().delivered == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(outbox.store().counts().await.unwrap(), StateCounts {
        delivered: 1,
        ..StateCounts::default()
    });
    assert_eq!(sink.sent_texts(), vec!["hi"]);
}

/// A recipient that never answers delays only its own deliveries.
#[tokio::test]
async fn a_stuck_recipient_does_not_hold_up_the_others() {
    let sink = Arc::new(ScriptedSink {
        slow_chat: Some(("stuck".into(), Duration::from_millis(150))),
        ..ScriptedSink::default()
    });
    let (outbox, _) = outbox_with(sink.clone()).await;

    let stuck = {
        let outbox = Arc::clone(&outbox);
        tokio::spawn(async move { outbox.submit(new("stuck", "slow")).await })
    };
    tokio::time::sleep(Duration::from_millis(10)).await;
    let started = tokio::time::Instant::now();
    assert!(outbox.submit(new("ok", "fast")).await.is_delivered());
    assert!(started.elapsed() < Duration::from_millis(100));
    assert!(stuck.await.unwrap().is_delivered());
}

#[tokio::test]
async fn failed_deliveries_can_be_sent_again_and_say_so() {
    let sink = ScriptedSink::with_script(vec![Err(SinkError::Rejected("down".into()))]);
    let (outbox, _) = outbox_with(sink.clone()).await;
    assert!(outbox.submit(new("1", "lost")).await.is_failed());

    assert_eq!(outbox.redeliver_failed(&target("2")).await.unwrap(), 0);
    assert_eq!(outbox.redeliver_failed(&target("1")).await.unwrap(), 1);
    drain(&outbox).await;
    let sent = sink.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].1, "lost");
    assert!(sent[0].2, "a redelivery is flagged for the sink");
}

#[tokio::test]
async fn a_broken_store_still_sends_once() {
    let store = DeliveryStore::in_memory().await.unwrap();
    let outbox = Outbox::new(store.clone(), policy());
    let sink = ScriptedSink::with_script(vec![]);
    outbox.set_sink(sink.clone());
    store_pool_close(&store).await;

    let outcome = outbox.submit(new("1", "hi")).await;
    assert!(matches!(outcome, SubmitOutcome::Unpersisted {
        result: Ok(_)
    }));
    assert!(outcome.is_delivered());
    assert_eq!(sink.sent_texts(), vec!["hi"]);
}

async fn store_pool_close(store: &DeliveryStore) {
    store.close().await;
}

/// Many runs replying to the same chat at once: each reply is sent exactly
/// once, in the order it was submitted.
#[tokio::test]
async fn concurrent_submissions_to_one_chat_are_sent_once_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}?mode=rwc", dir.path().join("o.db").display());
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await
        .unwrap();
    crate::run_migrations(&pool).await.unwrap();
    let outbox = Outbox::new(DeliveryStore::new(pool), policy());
    let sink = ScriptedSink::with_script(vec![]);
    outbox.set_sink(sink.clone());

    let mut tasks = Vec::new();
    for index in 0..20 {
        let outbox = Arc::clone(&outbox);
        tasks.push(tokio::spawn(async move {
            outbox.submit(new("1", &index.to_string())).await
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    drain(&outbox).await;

    let sent = sink.sent.lock().unwrap().clone();
    let ids: Vec<i64> = sent.iter().map(|(id, ..)| *id).collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(ids.len(), 20, "every reply sent");
    assert_eq!(ids, sorted, "sent once each, in submission order");
}

#[tokio::test]
async fn the_worker_retries_in_the_background() {
    let sink = ScriptedSink::with_script(vec![retryable("later")]);
    let (outbox, _) = outbox_with(sink.clone()).await;
    let worker = outbox.spawn_worker();

    assert!(matches!(
        outbox.submit(new("1", "hi")).await,
        SubmitOutcome::Pending { .. }
    ));
    for _ in 0..200 {
        if !sink.sent_texts().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    worker.abort();
    assert_eq!(sink.sent_texts(), vec!["hi"]);
}

#[tokio::test]
async fn the_worker_resumes_sends_interrupted_by_a_restart() {
    let store = DeliveryStore::in_memory().await.unwrap();
    let id = store
        .enqueue(&new("1", "interrupted"), OffsetDateTime::now_utc())
        .await
        .unwrap();
    // A previous process claimed it and died mid-send.
    store
        .try_claim(id, OffsetDateTime::now_utc(), Duration::from_secs(3600))
        .await
        .unwrap()
        .unwrap();

    let outbox = Outbox::new(store, policy());
    let sink = ScriptedSink::with_script(vec![]);
    outbox.set_sink(sink.clone());
    let worker = outbox.spawn_worker();
    for _ in 0..200 {
        if !sink.sent_texts().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    worker.abort();
    assert_eq!(sink.sent_texts(), vec!["interrupted"]);
    let stored = outbox.store().get(id).await.unwrap().unwrap();
    assert_eq!(stored.attempts, 2);
}

#[test]
fn channel_errors_map_to_retry_or_reject() {
    let unknown = moltis_channels::Error::UnknownAccount {
        account_id: "bot".into(),
    };
    assert!(matches!(SinkError::from(unknown), SinkError::Retryable(_)));
    let unavailable = moltis_channels::Error::Unavailable {
        message: "starting".into(),
    };
    assert!(matches!(
        SinkError::from(unavailable),
        SinkError::Retryable(_)
    ));
    let invalid = moltis_channels::Error::InvalidInput {
        message: "bad chat id".into(),
    };
    assert!(matches!(SinkError::from(invalid), SinkError::Rejected(_)));
}
