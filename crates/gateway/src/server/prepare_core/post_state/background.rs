//! Background services that need the chat runtime: quota-wait resumes and
//! the learning loop.

use std::{path::Path, sync::Arc};

use crate::state::GatewayState;

pub(super) fn start(state: &Arc<GatewayState>, data_dir: &Path) {
    antre_chat::quota_wait::configure(
        state.config.chat.quota_wait.clone(),
        Some(data_dir.join("chat").join("quota-waits.json")),
    );
    antre_chat::quota_wait::restore(&crate::chat::GatewayChatRuntime::from_state(Arc::clone(
        state,
    )));
    crate::learning::start(state, data_dir);
}
