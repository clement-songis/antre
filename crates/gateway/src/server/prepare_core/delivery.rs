//! Starting the channel delivery outbox during startup.

use std::sync::Arc;

use sqlx::SqlitePool;

use crate::state::GatewayState;

/// Create the outbox, connect it to the channels and start its worker.
///
/// The worker first requeues sends a previous process left half-done, so
/// replies interrupted by a restart are not lost.
pub(super) fn install_delivery(state: &Arc<GatewayState>, db_pool: &SqlitePool) {
    let outbox = antre_delivery::Outbox::new(
        antre_delivery::DeliveryStore::new(db_pool.clone()),
        antre_delivery::RetryPolicy::from(&state.config.chat.delivery),
    );
    antre_chat::channel_delivery::install(
        &outbox,
        &crate::chat::GatewayChatRuntime::from_state(Arc::clone(state)),
    );
    if state.delivery_outbox.set(Arc::clone(&outbox)).is_err() {
        tracing::warn!("delivery outbox already installed");
        return;
    }
    outbox.spawn_worker();
}
