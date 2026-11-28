//! The outbox: persist, send, retry, report.

use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};

use {
    async_trait::async_trait,
    moltis_channels::ChannelReplyTarget,
    time::OffsetDateTime,
    tokio::{
        sync::{Notify, Semaphore},
        task::JoinHandle,
    },
    tracing::{debug, info, warn},
};

use crate::{
    error::Result,
    model::{Delivery, DeliveryState, NewDelivery, target_key},
    policy::RetryPolicy,
    store::DeliveryStore,
};

/// Longest the worker sleeps with nothing due, which also bounds how late
/// an expired lease is noticed.
const IDLE_POLL: Duration = Duration::from_secs(30);
/// Due deliveries claimed per worker pass.
const CLAIM_BATCH: u32 = 32;
/// Sends running at once in the worker. Each target is sequential anyway,
/// so this only bounds fan-out across targets.
const MAX_CONCURRENT_SENDS: usize = 16;
const PRUNE_INTERVAL: Duration = Duration::from_secs(3600);

/// Why a send did not go through.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SinkError {
    /// Worth trying again later (network, rate limit, channel not started).
    #[error("{0}")]
    Retryable(String),
    /// Will never succeed (unknown account, invalid destination).
    #[error("{0}")]
    Rejected(String),
}

impl From<moltis_channels::Error> for SinkError {
    /// An unknown account is retried: after a restart the outbox can run
    /// before every channel account has registered again.
    fn from(error: moltis_channels::Error) -> Self {
        let message = error.to_string();
        if error.is_retryable() || matches!(error, moltis_channels::Error::UnknownAccount { .. }) {
            Self::Retryable(message)
        } else {
            Self::Rejected(message)
        }
    }
}

/// Sends one delivery to its channel.
#[async_trait]
pub trait DeliverySink: Send + Sync {
    /// Returns the platform ids of the messages sent.
    async fn send(&self, delivery: &Delivery) -> std::result::Result<Vec<String>, SinkError>;
}

/// Told about every outcome of a send attempt.
#[async_trait]
pub trait DeliveryObserver: Send + Sync {
    async fn on_change(&self, delivery: &Delivery);
}

/// What [`Outbox::submit`] did with a delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitOutcome {
    Delivered {
        id: i64,
        message_ids: Vec<String>,
    },
    /// Persisted and waiting: behind an earlier delivery to the same target,
    /// or scheduled for a retry.
    Pending {
        id: i64,
    },
    Failed {
        id: i64,
        error: String,
    },
    /// The store could not persist it, so it was sent once without a record.
    Unpersisted {
        result: std::result::Result<Vec<String>, SinkError>,
    },
}

impl SubmitOutcome {
    /// `true` when the recipient has the message.
    #[must_use]
    pub fn is_delivered(&self) -> bool {
        matches!(
            self,
            Self::Delivered { .. } | Self::Unpersisted { result: Ok(_) }
        )
    }

    /// `true` when the message is lost for good (only reachable through an
    /// explicit redelivery).
    #[must_use]
    pub fn is_failed(&self) -> bool {
        matches!(
            self,
            Self::Failed { .. } | Self::Unpersisted { result: Err(_) }
        )
    }
}

pub struct Outbox {
    store: DeliveryStore,
    policy: RetryPolicy,
    sink: OnceLock<Arc<dyn DeliverySink>>,
    observer: OnceLock<Arc<dyn DeliveryObserver>>,
    wake: Notify,
    sends: Arc<Semaphore>,
}

impl Outbox {
    #[must_use]
    pub fn new(store: DeliveryStore, policy: RetryPolicy) -> Arc<Self> {
        Arc::new(Self {
            store,
            policy,
            sink: OnceLock::new(),
            observer: OnceLock::new(),
            wake: Notify::new(),
            sends: Arc::new(Semaphore::new(MAX_CONCURRENT_SENDS)),
        })
    }

    /// Install the sender. Until then every attempt is retried later. Returns
    /// `false` if one was already installed.
    pub fn set_sink(&self, sink: Arc<dyn DeliverySink>) -> bool {
        let installed = self.sink.set(sink).is_ok();
        self.wake.notify_one();
        installed
    }

    /// Returns `false` if one was already installed.
    pub fn set_observer(&self, observer: Arc<dyn DeliveryObserver>) -> bool {
        self.observer.set(observer).is_ok()
    }

    #[must_use]
    pub fn store(&self) -> &DeliveryStore {
        &self.store
    }

    #[must_use]
    pub fn policy(&self) -> &RetryPolicy {
        &self.policy
    }

    /// Persist a delivery and send it now, unless an earlier delivery to the
    /// same target is still pending.
    ///
    /// The attempt runs in its own task, so a caller that gives up waiting
    /// (a timeout around this call) does not cancel a send half-way and
    /// leave the record claimed.
    #[tracing::instrument(skip_all, fields(session = %new.session_key, kind = %new.kind))]
    pub async fn submit(self: &Arc<Self>, new: NewDelivery) -> SubmitOutcome {
        let now = OffsetDateTime::now_utc();
        let id = match self.store.enqueue(&new, now).await {
            Ok(id) => id,
            Err(error) => {
                warn!(%error, "delivery store unavailable, sending without a record");
                return SubmitOutcome::Unpersisted {
                    result: self.send_unpersisted(new, now).await,
                };
            },
        };
        record_enqueued(&new);
        let claimed = match self.store.try_claim(id, now, self.policy.lease).await {
            Ok(Some(delivery)) => delivery,
            Ok(None) => {
                debug!(
                    id,
                    "delivery waits behind an earlier one to the same target"
                );
                self.wake.notify_one();
                return SubmitOutcome::Pending { id };
            },
            Err(error) => {
                warn!(id, %error, "could not claim delivery, leaving it to the worker");
                self.wake.notify_one();
                return SubmitOutcome::Pending { id };
            },
        };
        let outbox = Arc::clone(self);
        match tokio::spawn(async move { outbox.attempt(claimed).await }).await {
            Ok(delivery) => match delivery.state {
                DeliveryState::Delivered => SubmitOutcome::Delivered {
                    id,
                    message_ids: delivery.message_ids,
                },
                DeliveryState::Failed => SubmitOutcome::Failed {
                    id,
                    error: delivery.last_error.unwrap_or_default(),
                },
                DeliveryState::Queued | DeliveryState::Sending => SubmitOutcome::Pending { id },
            },
            Err(error) => {
                warn!(id, %error, "delivery attempt task failed; the lease will expire");
                SubmitOutcome::Pending { id }
            },
        }
    }

    async fn send_unpersisted(
        &self,
        new: NewDelivery,
        now: OffsetDateTime,
    ) -> std::result::Result<Vec<String>, SinkError> {
        let delivery = Delivery {
            id: 0,
            session_key: new.session_key,
            run_id: new.run_id,
            kind: new.kind,
            target: new.target,
            body: new.body,
            reply_to: new.reply_to,
            state: DeliveryState::Sending,
            attempts: 1,
            redelivered: false,
            next_attempt_at: now,
            last_error: None,
            message_ids: Vec::new(),
            created_at: now,
            updated_at: now,
        };
        self.send_once(&delivery).await
    }

    async fn send_once(&self, delivery: &Delivery) -> std::result::Result<Vec<String>, SinkError> {
        let Some(sink) = self.sink.get() else {
            return Err(SinkError::Retryable(
                "channel delivery is not ready yet".into(),
            ));
        };
        match tokio::time::timeout(self.policy.attempt_timeout, sink.send(delivery)).await {
            Ok(result) => result,
            Err(_) => Err(SinkError::Retryable(format!(
                "send timed out after {}s",
                self.policy.attempt_timeout.as_secs()
            ))),
        }
    }

    /// Send a claimed delivery and record the outcome. Returns the delivery
    /// as it now stands.
    #[tracing::instrument(skip_all, fields(id = delivery.id, attempt = delivery.attempts))]
    async fn attempt(&self, mut delivery: Delivery) -> Delivery {
        let result = self.send_once(&delivery).await;
        let now = OffsetDateTime::now_utc();
        let recorded = match result {
            Ok(message_ids) => {
                let recorded = self
                    .store
                    .mark_delivered(delivery.id, &message_ids, now)
                    .await;
                delivery.state = DeliveryState::Delivered;
                delivery.message_ids = message_ids;
                delivery.last_error = None;
                recorded
            },
            Err(error) => {
                let retry = matches!(error, SinkError::Retryable(_))
                    && self.policy.may_retry(delivery.attempts);
                let message = error.to_string();
                delivery.last_error = Some(message.clone());
                if retry {
                    let delay = self.policy.retry_delay(delivery.attempts);
                    delivery.state = DeliveryState::Queued;
                    delivery.next_attempt_at = now + delay;
                    info!(
                        id = delivery.id,
                        attempts = delivery.attempts,
                        retry_in_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
                        error = %message,
                        "channel delivery failed, will retry"
                    );
                    self.store
                        .mark_retry(delivery.id, &message, delay, now)
                        .await
                } else {
                    delivery.state = DeliveryState::Failed;
                    warn!(
                        id = delivery.id,
                        session = %delivery.session_key,
                        attempts = delivery.attempts,
                        error = %message,
                        "channel delivery failed for good"
                    );
                    self.store.mark_failed(delivery.id, &message, now).await
                }
            },
        };
        delivery.updated_at = now;
        record_outcome(&delivery);
        // The next delivery to this target, or the retry, may now be due.
        self.wake.notify_one();
        match recorded {
            Ok(true) => {
                if let Some(observer) = self.observer.get() {
                    observer.on_change(&delivery).await;
                }
            },
            Ok(false) => {
                warn!(
                    id = delivery.id,
                    "delivery lease was taken back before the outcome was recorded"
                );
            },
            Err(error) => {
                warn!(id = delivery.id, %error, "could not record the delivery outcome");
            },
        }
        delivery
    }

    /// Claim due deliveries and start sending them. Returns the started
    /// tasks; they finish on their own.
    async fn spawn_due(self: &Arc<Self>) -> Result<Vec<JoinHandle<Delivery>>> {
        let now = OffsetDateTime::now_utc();
        let released = self.store.release_expired_leases(now).await?;
        if released > 0 {
            info!(released, "requeued deliveries whose send was interrupted");
        }
        let due = self
            .store
            .claim_due(now, self.policy.lease, CLAIM_BATCH)
            .await?;
        let mut tasks = Vec::with_capacity(due.len());
        for delivery in due {
            let outbox = Arc::clone(self);
            let sends = Arc::clone(&self.sends);
            tasks.push(tokio::spawn(async move {
                let _permit = sends.acquire_owned().await;
                outbox.attempt(delivery).await
            }));
        }
        Ok(tasks)
    }

    /// One worker pass, waiting for the sends it started. Returns how many
    /// deliveries were attempted.
    pub async fn run_due(self: &Arc<Self>) -> Result<usize> {
        let tasks = self.spawn_due().await?;
        let count = tasks.len();
        for task in tasks {
            if let Err(error) = task.await {
                warn!(%error, "delivery attempt task failed");
            }
        }
        Ok(count)
    }

    /// Queue every failed delivery to `target` again, for example when the
    /// recipient writes again after an outage. Returns how many were queued.
    pub async fn redeliver_failed(&self, target: &ChannelReplyTarget) -> Result<usize> {
        let ids = self
            .store
            .requeue_failed(&target_key(target), OffsetDateTime::now_utc())
            .await?;
        if !ids.is_empty() {
            info!(
                count = ids.len(),
                "requeued failed deliveries for redelivery"
            );
            self.wake.notify_one();
        }
        Ok(ids.len())
    }

    /// Start the background worker: recovers sends interrupted by a restart,
    /// retries due deliveries, and prunes old records.
    pub fn spawn_worker(self: &Arc<Self>) -> JoinHandle<()> {
        let outbox = Arc::clone(self);
        tokio::spawn(async move { outbox.worker_loop().await })
    }

    async fn worker_loop(self: Arc<Self>) {
        match self
            .store
            .release_all_leases(OffsetDateTime::now_utc())
            .await
        {
            Ok(0) => {},
            Ok(released) => info!(released, "resuming deliveries interrupted by a restart"),
            Err(error) => warn!(%error, "could not recover interrupted deliveries"),
        }
        let mut next_prune = tokio::time::Instant::now();
        loop {
            if let Err(error) = self.spawn_due().await {
                warn!(%error, "delivery worker pass failed");
            }
            if tokio::time::Instant::now() >= next_prune {
                next_prune = tokio::time::Instant::now() + PRUNE_INTERVAL;
                let cutoff = OffsetDateTime::now_utc() - self.policy.retention;
                match self.store.prune(cutoff).await {
                    Ok(0) => {},
                    Ok(pruned) => debug!(pruned, "pruned old delivery records"),
                    Err(error) => warn!(%error, "could not prune delivery records"),
                }
            }
            let sleep = match self.store.next_due_at().await {
                Ok(Some(due)) => Duration::try_from(due - OffsetDateTime::now_utc())
                    .unwrap_or_default()
                    .min(IDLE_POLL),
                Ok(None) | Err(_) => IDLE_POLL,
            };
            tokio::select! {
                () = tokio::time::sleep(sleep) => {},
                () = self.wake.notified() => {},
            }
        }
    }
}

#[cfg(feature = "metrics")]
fn record_enqueued(new: &NewDelivery) {
    use moltis_metrics::{counter, delivery, labels};
    counter!(delivery::ENQUEUED_TOTAL, labels::KIND => new.kind.as_str()).increment(1);
}

#[cfg(not(feature = "metrics"))]
fn record_enqueued(_new: &NewDelivery) {}

#[cfg(feature = "metrics")]
fn record_outcome(delivery: &Delivery) {
    use moltis_metrics::{counter, delivery as metrics, labels};
    let status = match delivery.state {
        DeliveryState::Delivered => "delivered",
        DeliveryState::Failed => "failed",
        DeliveryState::Queued | DeliveryState::Sending => "retry",
    };
    counter!(
        metrics::ATTEMPTS_TOTAL,
        labels::KIND => delivery.kind.as_str(),
        labels::STATUS => status,
        labels::CHANNEL => delivery.target.channel_type.as_str()
    )
    .increment(1);
}

#[cfg(not(feature = "metrics"))]
fn record_outcome(_delivery: &Delivery) {}

#[cfg(test)]
#[path = "outbox_tests.rs"]
mod tests;
