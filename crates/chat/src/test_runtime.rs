//! A `ChatRuntime` for unit tests: no channels, no sandbox, and a chat
//! service supplied by the test.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use {async_trait::async_trait, moltis_service_traits::ChatService, serde_json::Value};

use crate::runtime::ChatRuntime;

pub(crate) struct TestRuntime {
    chat: Arc<dyn ChatService>,
    outbound: Option<Arc<dyn moltis_channels::ChannelOutbound>>,
    outbox: Option<Arc<moltis_delivery::Outbox>>,
    /// Every broadcast, in order.
    pub(crate) broadcasts: Mutex<Vec<(String, Value)>>,
    /// Channel acknowledgment activity reported per run.
    pub(crate) activities: Mutex<Vec<(String, moltis_channels::ChannelActivity)>>,
    replies: Mutex<HashMap<String, Vec<moltis_channels::ChannelReplyTarget>>>,
    tts: moltis_service_traits::NoopTtsService,
    project: moltis_service_traits::NoopProjectService,
    mcp: moltis_service_traits::NoopMcpService,
}

#[async_trait]
impl ChatRuntime for TestRuntime {
    async fn broadcast(&self, topic: &str, payload: Value) {
        self.broadcasts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((topic.to_string(), payload));
    }

    async fn push_channel_reply(
        &self,
        session_key: &str,
        target: moltis_channels::ChannelReplyTarget,
    ) {
        self.replies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(session_key.to_string())
            .or_default()
            .push(target);
    }

    async fn drain_channel_replies(
        &self,
        session_key: &str,
    ) -> Vec<moltis_channels::ChannelReplyTarget> {
        self.replies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(session_key)
            .unwrap_or_default()
    }

    async fn peek_channel_replies(
        &self,
        session_key: &str,
    ) -> Vec<moltis_channels::ChannelReplyTarget> {
        self.replies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_key)
            .cloned()
            .unwrap_or_default()
    }

    async fn note_channel_activity(
        &self,
        activity_id: &str,
        activity: moltis_channels::ChannelActivity,
    ) {
        self.activities
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((activity_id.to_string(), activity));
    }

    async fn push_channel_status_log(&self, _session_key: &str, _message: String) {}

    async fn drain_channel_status_log(&self, _session_key: &str) -> Vec<String> {
        Vec::new()
    }

    async fn set_run_error(&self, _run_id: &str, _error: String) {}

    async fn active_session_key(&self, _conn_id: &str) -> Option<String> {
        None
    }

    async fn active_project_id(&self, _conn_id: &str) -> Option<String> {
        None
    }

    fn hostname(&self) -> &str {
        "test"
    }

    fn sandbox_router(&self) -> Option<&Arc<moltis_tools::sandbox::SandboxRouter>> {
        None
    }

    fn memory_manager(&self) -> Option<&moltis_memory::runtime::DynMemoryRuntime> {
        None
    }

    async fn cached_location(&self) -> Option<moltis_config::GeoLocation> {
        None
    }

    async fn tts_overrides(
        &self,
        _session_key: &str,
        _channel_key: &str,
    ) -> (
        Option<crate::runtime::TtsOverride>,
        Option<crate::runtime::TtsOverride>,
    ) {
        (None, None)
    }

    fn channel_outbound(&self) -> Option<Arc<dyn moltis_channels::ChannelOutbound>> {
        self.outbound.clone()
    }

    fn delivery_outbox(&self) -> Option<Arc<moltis_delivery::Outbox>> {
        self.outbox.clone()
    }

    fn channel_stream_outbound(&self) -> Option<Arc<dyn moltis_channels::ChannelStreamOutbound>> {
        None
    }

    fn tts_service(&self) -> &dyn moltis_service_traits::TtsService {
        &self.tts
    }

    fn project_service(&self) -> &dyn moltis_service_traits::ProjectService {
        &self.project
    }

    fn mcp_service(&self) -> &dyn moltis_service_traits::McpService {
        &self.mcp
    }

    async fn chat_service(&self) -> Arc<dyn ChatService> {
        self.chat.clone()
    }

    async fn last_run_error(&self, _run_id: &str) -> Option<String> {
        None
    }

    async fn send_push_notification(
        &self,
        _title: &str,
        _body: &str,
        _url: Option<&str>,
        _session_key: Option<&str>,
    ) -> crate::error::Result<usize> {
        Ok(0)
    }

    async fn ensure_local_model_cached(&self, _model_id: &str) -> crate::error::Result<bool> {
        Ok(false)
    }

    async fn connected_nodes(&self) -> Vec<crate::runtime::ConnectedNodeSummary> {
        Vec::new()
    }
}

impl TestRuntime {
    pub(crate) fn new(chat: Arc<dyn ChatService>) -> Self {
        Self {
            chat,
            outbound: None,
            outbox: None,
            broadcasts: Mutex::new(Vec::new()),
            activities: Mutex::new(Vec::new()),
            replies: Mutex::new(HashMap::new()),
            tts: moltis_service_traits::NoopTtsService,
            project: moltis_service_traits::NoopProjectService,
            mcp: moltis_service_traits::NoopMcpService,
        }
    }

    pub(crate) fn with_outbound(
        mut self,
        outbound: Arc<dyn moltis_channels::ChannelOutbound>,
    ) -> Self {
        self.outbound = Some(outbound);
        self
    }

    pub(crate) fn with_outbox(mut self, outbox: Arc<moltis_delivery::Outbox>) -> Self {
        self.outbox = Some(outbox);
        self
    }

    pub(crate) fn broadcasts_on(&self, topic: &str) -> Vec<Value> {
        self.broadcasts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|(seen, _)| seen == topic)
            .map(|(_, payload)| payload.clone())
            .collect()
    }
}
