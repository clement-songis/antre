#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use {
    antre_channels::{ChannelReplyTarget, ChannelType},
    time::OffsetDateTime,
};

use {
    super::*,
    crate::model::{DeliveryBody, DeliveryKind},
};

const LEASE: Duration = Duration::from_secs(60);

fn target(chat: &str) -> ChannelReplyTarget {
    ChannelReplyTarget {
        channel_type: ChannelType::Discord,
        account_id: "acct".into(),
        chat_id: chat.into(),
        message_id: Some("m1".into()),
        thread_id: None,
        ack_message_id: None,
        direct_chat: false,
    }
}

fn new(chat: &str, text: &str) -> NewDelivery {
    NewDelivery {
        session_key: "discord:acct:chan".into(),
        run_id: Some("run-1".into()),
        kind: DeliveryKind::Reply,
        target: target(chat),
        body: DeliveryBody::text(text),
        reply_to: Some("m1".into()),
    }
}

fn t0() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap()
}

#[tokio::test]
async fn enqueue_persists_every_field() {
    let store = DeliveryStore::in_memory().await.unwrap();
    let id = store.enqueue(&new("c1", "hello"), t0()).await.unwrap();
    let delivery = store.get(id).await.unwrap().unwrap();
    assert_eq!(delivery.state, DeliveryState::Queued);
    assert_eq!(delivery.attempts, 0);
    assert_eq!(delivery.body, DeliveryBody::text("hello"));
    assert_eq!(delivery.target.chat_id, "c1");
    assert_eq!(delivery.reply_to.as_deref(), Some("m1"));
    assert_eq!(delivery.run_id.as_deref(), Some("run-1"));
    assert_eq!(delivery.next_attempt_at, t0());
    assert!(delivery.message_ids.is_empty());
    assert!(!delivery.redelivered);
    assert!(store.get(id + 1).await.unwrap().is_none());
}

#[tokio::test]
async fn full_lifecycle_and_guards_against_stale_owners() {
    let store = DeliveryStore::in_memory().await.unwrap();
    let id = store.enqueue(&new("c1", "hello"), t0()).await.unwrap();

    let claimed = store.try_claim(id, t0(), LEASE).await.unwrap().unwrap();
    assert_eq!(claimed.state, DeliveryState::Sending);
    assert_eq!(claimed.attempts, 1);
    // Already sending: a second claim gets nothing.
    assert!(store.try_claim(id, t0(), LEASE).await.unwrap().is_none());

    assert!(
        store
            .mark_retry(id, "timeout", Duration::from_secs(5), t0())
            .await
            .unwrap()
    );
    let queued = store.get(id).await.unwrap().unwrap();
    assert_eq!(queued.state, DeliveryState::Queued);
    assert_eq!(queued.last_error.as_deref(), Some("timeout"));
    assert_eq!(queued.next_attempt_at, t0() + time::Duration::seconds(5));
    // Not ours any more: outcome writes are refused.
    assert!(!store.mark_delivered(id, &[], t0()).await.unwrap());
    assert!(!store.mark_failed(id, "x", t0()).await.unwrap());

    // Not due yet.
    assert!(store.claim_due(t0(), LEASE, 10).await.unwrap().is_empty());
    let later = t0() + time::Duration::seconds(5);
    let due = store.claim_due(later, LEASE, 10).await.unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].attempts, 2);

    assert!(
        store
            .mark_delivered(id, &["a".into(), "b".into()], later)
            .await
            .unwrap()
    );
    let delivered = store.get(id).await.unwrap().unwrap();
    assert_eq!(delivered.state, DeliveryState::Delivered);
    assert_eq!(delivered.message_ids, vec!["a", "b"]);
    assert!(delivered.last_error.is_none());
}

#[tokio::test]
async fn one_target_is_served_in_order_while_others_proceed() {
    let store = DeliveryStore::in_memory().await.unwrap();
    let first = store.enqueue(&new("c1", "one"), t0()).await.unwrap();
    let second = store.enqueue(&new("c1", "two"), t0()).await.unwrap();
    let other = store.enqueue(&new("c2", "other"), t0()).await.unwrap();

    // The second reply to c1 cannot jump ahead of the first.
    assert!(
        store
            .try_claim(second, t0(), LEASE)
            .await
            .unwrap()
            .is_none()
    );
    let due = store.claim_due(t0(), LEASE, 10).await.unwrap();
    let ids: Vec<i64> = due.iter().map(|delivery| delivery.id).collect();
    assert_eq!(ids, vec![first, other]);

    // While the first is sending, the second still waits.
    assert!(store.claim_due(t0(), LEASE, 10).await.unwrap().is_empty());
    store.mark_failed(first, "gone", t0()).await.unwrap();
    // A failed older delivery no longer blocks the target.
    let due = store.claim_due(t0(), LEASE, 10).await.unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].id, second);
}

#[tokio::test]
async fn expired_leases_are_released_but_live_ones_are_kept() {
    let store = DeliveryStore::in_memory().await.unwrap();
    let id = store.enqueue(&new("c1", "hello"), t0()).await.unwrap();
    store.try_claim(id, t0(), LEASE).await.unwrap().unwrap();

    let before_expiry = t0() + time::Duration::seconds(59);
    assert_eq!(
        store.release_expired_leases(before_expiry).await.unwrap(),
        0
    );
    let expiry = t0() + time::Duration::seconds(60);
    assert_eq!(store.release_expired_leases(expiry).await.unwrap(), 1);
    let released = store.get(id).await.unwrap().unwrap();
    assert_eq!(released.state, DeliveryState::Queued);
    // The interrupted attempt still counts.
    assert_eq!(released.attempts, 1);

    store.try_claim(id, expiry, LEASE).await.unwrap().unwrap();
    assert_eq!(store.release_all_leases(expiry).await.unwrap(), 1);
    assert_eq!(
        store.get(id).await.unwrap().unwrap().state,
        DeliveryState::Queued
    );
}

#[tokio::test]
async fn failed_deliveries_are_requeued_with_a_fresh_budget() {
    let store = DeliveryStore::in_memory().await.unwrap();
    let first = store.enqueue(&new("c1", "one"), t0()).await.unwrap();
    let second = store.enqueue(&new("c1", "two"), t0()).await.unwrap();
    let elsewhere = store.enqueue(&new("c2", "x"), t0()).await.unwrap();
    for id in [first, second, elsewhere] {
        store.try_claim(id, t0(), LEASE).await.unwrap();
        store.mark_failed(id, "down", t0()).await.unwrap();
    }
    // `second` became claimable once `first` failed, so it failed too.
    assert_eq!(
        store.get(second).await.unwrap().unwrap().state,
        DeliveryState::Failed
    );

    let key = target_key(&target("c1"));
    let later = t0() + time::Duration::minutes(10);
    assert_eq!(store.requeue_failed(&key, later).await.unwrap(), vec![
        first, second
    ]);
    let requeued = store.get(first).await.unwrap().unwrap();
    assert_eq!(requeued.state, DeliveryState::Queued);
    assert_eq!(requeued.attempts, 0);
    assert!(requeued.redelivered);
    assert_eq!(requeued.next_attempt_at, later);
    assert_eq!(
        store.get(elsewhere).await.unwrap().unwrap().state,
        DeliveryState::Failed
    );
    assert!(store.requeue_failed(&key, later).await.unwrap().is_empty());
}

#[tokio::test]
async fn next_due_counts_listing_and_pruning() {
    let store = DeliveryStore::in_memory().await.unwrap();
    assert!(store.next_due_at().await.unwrap().is_none());
    let delivered = store.enqueue(&new("c1", "one"), t0()).await.unwrap();
    let queued = store
        .enqueue(&new("c2", "two"), t0() + time::Duration::seconds(3))
        .await
        .unwrap();
    store.try_claim(delivered, t0(), LEASE).await.unwrap();
    store.mark_delivered(delivered, &[], t0()).await.unwrap();

    assert_eq!(
        store.next_due_at().await.unwrap(),
        Some(t0() + time::Duration::seconds(3))
    );
    assert_eq!(store.counts().await.unwrap(), StateCounts {
        queued: 1,
        sending: 0,
        delivered: 1,
        failed: 0,
    });
    let listed = store
        .list_for_session("discord:acct:chan", 10)
        .await
        .unwrap();
    assert_eq!(listed.iter().map(|d| d.id).collect::<Vec<_>>(), vec![
        queued, delivered
    ]);
    assert_eq!(
        store
            .list_for_session("discord:acct:chan", 1)
            .await
            .unwrap()
            .len(),
        1
    );

    // Only terminal rows older than the cut-off go.
    let cutoff = t0() + time::Duration::days(1);
    assert_eq!(store.prune(cutoff).await.unwrap(), 1);
    assert!(store.get(delivered).await.unwrap().is_none());
    assert!(store.get(queued).await.unwrap().is_some());
}

/// Two workers racing over the same due rows on a shared pool: every row is
/// claimed exactly once.
#[tokio::test]
async fn concurrent_claimers_never_take_the_same_row() {
    let dir = tempfile::tempdir().unwrap();
    let url = format!(
        "sqlite://{}?mode=rwc",
        dir.path().join("outbox.db").display()
    );
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await
        .unwrap();
    crate::run_migrations(&pool).await.unwrap();
    let store = DeliveryStore::new(pool);
    let mut ids = Vec::new();
    for chat in 0..40 {
        ids.push(
            store
                .enqueue(&new(&format!("c{chat}"), "x"), t0())
                .await
                .unwrap(),
        );
    }

    let mut tasks = Vec::new();
    for _ in 0..8 {
        let store = store.clone();
        tasks.push(tokio::spawn(async move {
            let mut mine = Vec::new();
            loop {
                let batch = store.claim_due(t0(), LEASE, 3).await.unwrap();
                if batch.is_empty() {
                    break;
                }
                mine.extend(batch.into_iter().map(|delivery| delivery.id));
            }
            mine
        }));
    }
    let mut claimed = Vec::new();
    for task in tasks {
        claimed.extend(task.await.unwrap());
    }
    claimed.sort_unstable();
    assert_eq!(claimed, ids);
}
