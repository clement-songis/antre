//! Reliable outbound delivery to channels.
//!
//! Replies are persisted in an outbox before they are sent, retried with
//! exponential backoff when the channel fails, kept in order per recipient,
//! and reported when they cannot be delivered. See `docs/src/delivery.md`.

pub mod error;
pub mod model;
pub mod outbox;
pub mod policy;
pub mod store;

pub use {
    error::{Error, Result},
    model::{Delivery, DeliveryBody, DeliveryKind, DeliveryState, NewDelivery, target_key},
    outbox::{DeliveryObserver, DeliverySink, Outbox, SinkError, SubmitOutcome},
    policy::RetryPolicy,
    store::{DeliveryStore, StateCounts},
};

/// Apply this crate's migrations (`delivery_outbox`).
pub async fn run_migrations(pool: &sqlx::SqlitePool) -> Result<()> {
    sqlx::migrate!("./migrations")
        .set_ignore_missing(true)
        .run(pool)
        .await?;
    Ok(())
}
