//! SQLite persistence of the outbox.
//!
//! Every state change is one guarded `UPDATE`: SQLite serializes writers, so
//! two workers can never claim the same row, and a worker whose lease was
//! taken back cannot overwrite the new owner's outcome.

use std::time::Duration;

use {
    sqlx::{FromRow, SqlitePool},
    time::OffsetDateTime,
};

use crate::{
    error::{Error, Result},
    model::{Delivery, DeliveryState, NewDelivery, target_key},
};

const COLUMNS: &str = "id, session_key, run_id, kind, target, body, reply_to, state, attempts, \
                       redelivered, next_attempt_at, last_error, message_ids, created_at, updated_at";

/// Rows still waiting for this one: an older delivery to the same target
/// that has not reached a terminal state.
const OLDER_PENDING_FOR_TARGET: &str = "EXISTS (SELECT 1 FROM delivery_outbox AS older \
     WHERE older.target_key = delivery_outbox.target_key \
     AND older.id < delivery_outbox.id \
     AND older.state IN ('queued', 'sending'))";

pub(crate) fn to_ms(at: OffsetDateTime) -> i64 {
    i64::try_from(at.unix_timestamp_nanos() / 1_000_000).unwrap_or(i64::MAX)
}

pub(crate) fn from_ms(ms: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

fn after(now: OffsetDateTime, delay: Duration) -> i64 {
    to_ms(now).saturating_add(i64::try_from(delay.as_millis()).unwrap_or(i64::MAX))
}

#[derive(FromRow)]
struct Row {
    id: i64,
    session_key: String,
    run_id: Option<String>,
    kind: String,
    target: String,
    body: String,
    reply_to: Option<String>,
    state: String,
    attempts: i64,
    redelivered: i64,
    next_attempt_at: i64,
    last_error: Option<String>,
    message_ids: Option<String>,
    created_at: i64,
    updated_at: i64,
}

impl TryFrom<Row> for Delivery {
    type Error = Error;

    fn try_from(row: Row) -> Result<Self> {
        Ok(Self {
            id: row.id,
            session_key: row.session_key,
            run_id: row.run_id,
            kind: row.kind.parse()?,
            target: serde_json::from_str(&row.target)?,
            body: serde_json::from_str(&row.body)?,
            reply_to: row.reply_to,
            state: row.state.parse()?,
            attempts: u32::try_from(row.attempts).unwrap_or(u32::MAX),
            redelivered: row.redelivered != 0,
            next_attempt_at: from_ms(row.next_attempt_at),
            last_error: row.last_error,
            message_ids: row
                .message_ids
                .as_deref()
                .map(serde_json::from_str)
                .transpose()?
                .unwrap_or_default(),
            created_at: from_ms(row.created_at),
            updated_at: from_ms(row.updated_at),
        })
    }
}

fn convert(rows: Vec<Row>) -> Result<Vec<Delivery>> {
    rows.into_iter().map(Delivery::try_from).collect()
}

/// Number of deliveries per state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StateCounts {
    pub queued: u64,
    pub sending: u64,
    pub delivered: u64,
    pub failed: u64,
}

#[derive(Clone)]
pub struct DeliveryStore {
    pool: SqlitePool,
}

impl DeliveryStore {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// An in-memory store with the schema applied, for tests.
    pub async fn in_memory() -> Result<Self> {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await?;
        crate::run_migrations(&pool).await?;
        Ok(Self::new(pool))
    }

    /// Close the pool; later calls fail. Used to simulate an unavailable
    /// database.
    pub async fn close(&self) {
        self.pool.close().await;
    }

    /// Persist a new delivery as `queued`, due now.
    pub async fn enqueue(&self, new: &NewDelivery, now: OffsetDateTime) -> Result<i64> {
        let now_ms = to_ms(now);
        let id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO delivery_outbox (session_key, run_id, kind, target_key, target, body, \
             reply_to, state, next_attempt_at, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, 'queued', ?, ?, ?) RETURNING id",
        )
        .bind(&new.session_key)
        .bind(&new.run_id)
        .bind(new.kind.as_str())
        .bind(target_key(&new.target))
        .bind(serde_json::to_string(&new.target)?)
        .bind(serde_json::to_string(&new.body)?)
        .bind(&new.reply_to)
        .bind(now_ms)
        .bind(now_ms)
        .bind(now_ms)
        .fetch_one(&self.pool)
        .await?;
        Ok(id)
    }

    pub async fn get(&self, id: i64) -> Result<Option<Delivery>> {
        let row = sqlx::query_as::<_, Row>(&format!(
            "SELECT {COLUMNS} FROM delivery_outbox WHERE id = ?"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(Delivery::try_from).transpose()
    }

    /// Claim one queued delivery for an immediate send, unless an older
    /// delivery to the same target is still pending (order is kept by
    /// leaving it to the worker).
    pub async fn try_claim(
        &self,
        id: i64,
        now: OffsetDateTime,
        lease: Duration,
    ) -> Result<Option<Delivery>> {
        let row = sqlx::query_as::<_, Row>(&format!(
            "UPDATE delivery_outbox SET state = 'sending', attempts = attempts + 1, \
             lease_until = ?, updated_at = ? \
             WHERE id = ? AND state = 'queued' AND NOT {OLDER_PENDING_FOR_TARGET} \
             RETURNING {COLUMNS}"
        ))
        .bind(after(now, lease))
        .bind(to_ms(now))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(Delivery::try_from).transpose()
    }

    /// Claim up to `limit` due deliveries, at most the oldest pending one
    /// per target.
    pub async fn claim_due(
        &self,
        now: OffsetDateTime,
        lease: Duration,
        limit: u32,
    ) -> Result<Vec<Delivery>> {
        let rows = sqlx::query_as::<_, Row>(&format!(
            "UPDATE delivery_outbox SET state = 'sending', attempts = attempts + 1, \
             lease_until = ?, updated_at = ? \
             WHERE id IN (SELECT id FROM delivery_outbox \
                 WHERE state = 'queued' AND next_attempt_at <= ? \
                 AND NOT {OLDER_PENDING_FOR_TARGET} ORDER BY id LIMIT ?) \
             AND state = 'queued' \
             RETURNING {COLUMNS}"
        ))
        .bind(after(now, lease))
        .bind(to_ms(now))
        .bind(to_ms(now))
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        let mut deliveries = convert(rows)?;
        deliveries.sort_by_key(|delivery| delivery.id);
        Ok(deliveries)
    }

    /// Put `sending` rows whose lease ran out back in the queue. Returns how
    /// many were released.
    pub async fn release_expired_leases(&self, now: OffsetDateTime) -> Result<u64> {
        let result = sqlx::query(
            "UPDATE delivery_outbox SET state = 'queued', lease_until = NULL, updated_at = ? \
             WHERE state = 'sending' AND (lease_until IS NULL OR lease_until <= ?)",
        )
        .bind(to_ms(now))
        .bind(to_ms(now))
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Release every `sending` row: at startup no send can be in flight.
    pub async fn release_all_leases(&self, now: OffsetDateTime) -> Result<u64> {
        let result = sqlx::query(
            "UPDATE delivery_outbox SET state = 'queued', lease_until = NULL, updated_at = ? \
             WHERE state = 'sending'",
        )
        .bind(to_ms(now))
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// `sending → delivered`. `false` when the row is no longer ours.
    pub async fn mark_delivered(
        &self,
        id: i64,
        message_ids: &[String],
        now: OffsetDateTime,
    ) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE delivery_outbox SET state = 'delivered', lease_until = NULL, \
             message_ids = ?, last_error = NULL, updated_at = ? \
             WHERE id = ? AND state = 'sending'",
        )
        .bind(serde_json::to_string(message_ids)?)
        .bind(to_ms(now))
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// `sending → queued`, due after `delay`.
    pub async fn mark_retry(
        &self,
        id: i64,
        error: &str,
        delay: Duration,
        now: OffsetDateTime,
    ) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE delivery_outbox SET state = 'queued', lease_until = NULL, last_error = ?, \
             next_attempt_at = ?, updated_at = ? WHERE id = ? AND state = 'sending'",
        )
        .bind(error)
        .bind(after(now, delay))
        .bind(to_ms(now))
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// `sending → failed`.
    pub async fn mark_failed(&self, id: i64, error: &str, now: OffsetDateTime) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE delivery_outbox SET state = 'failed', lease_until = NULL, last_error = ?, \
             updated_at = ? WHERE id = ? AND state = 'sending'",
        )
        .bind(error)
        .bind(to_ms(now))
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// `failed → queued` for every failed delivery to `target_key`, with a
    /// fresh attempt budget. Returns the ids requeued, oldest first.
    pub async fn requeue_failed(&self, target_key: &str, now: OffsetDateTime) -> Result<Vec<i64>> {
        let mut ids = sqlx::query_scalar::<_, i64>(
            "UPDATE delivery_outbox SET state = 'queued', attempts = 0, redelivered = 1, \
             next_attempt_at = ?, updated_at = ? \
             WHERE target_key = ? AND state = 'failed' RETURNING id",
        )
        .bind(to_ms(now))
        .bind(to_ms(now))
        .bind(target_key)
        .fetch_all(&self.pool)
        .await?;
        ids.sort_unstable();
        Ok(ids)
    }

    /// When the next queued delivery becomes due, if any.
    pub async fn next_due_at(&self) -> Result<Option<OffsetDateTime>> {
        let next = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT MIN(next_attempt_at) FROM delivery_outbox WHERE state = 'queued'",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(next.map(from_ms))
    }

    /// Recent deliveries of a session, newest first.
    pub async fn list_for_session(&self, session_key: &str, limit: u32) -> Result<Vec<Delivery>> {
        let rows = sqlx::query_as::<_, Row>(&format!(
            "SELECT {COLUMNS} FROM delivery_outbox WHERE session_key = ? \
             ORDER BY id DESC LIMIT ?"
        ))
        .bind(session_key)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        convert(rows)
    }

    pub async fn counts(&self) -> Result<StateCounts> {
        let rows = sqlx::query_as::<_, (String, i64)>(
            "SELECT state, COUNT(*) FROM delivery_outbox GROUP BY state",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut counts = StateCounts::default();
        for (state, count) in rows {
            let count = u64::try_from(count).unwrap_or_default();
            match state.parse::<DeliveryState>()? {
                DeliveryState::Queued => counts.queued = count,
                DeliveryState::Sending => counts.sending = count,
                DeliveryState::Delivered => counts.delivered = count,
                DeliveryState::Failed => counts.failed = count,
            }
        }
        Ok(counts)
    }

    /// Delete terminal records last updated before `before`.
    pub async fn prune(&self, before: OffsetDateTime) -> Result<u64> {
        let result = sqlx::query(
            "DELETE FROM delivery_outbox \
             WHERE state IN ('delivered', 'failed') AND updated_at < ?",
        )
        .bind(to_ms(before))
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
