//! A `ChatRuntime` for unit tests: no channels, no sandbox, and a chat
//! service supplied by the test.

use std::sync::Arc;

use {async_trait::async_trait, moltis_service_traits::ChatService, serde_json::Value};

use crate::runtime::ChatRuntime;

pub(crate) struct TestRuntime {
    chat: Arc<dyn ChatService>,
    tts: moltis_service_traits::NoopTtsService,
    project: moltis_service_traits::NoopProjectService,
    mcp: moltis_service_traits::NoopMcpService,
}

#[async_trait]
impl ChatRuntime for TestRuntime {
    async fn broadcast(&self, _topic: &str, _payload: Value) {}

    async fn push_channel_reply(
        &self,
        _session_key: &str,
        _target: moltis_channels::ChannelReplyTarget,
    ) {
    }

    async fn drain_channel_replies(
        &self,
        _session_key: &str,
    ) -> Vec<moltis_channels::ChannelReplyTarget> {
        Vec::new()
    }

    async fn peek_channel_replies(
        &self,
        _session_key: &str,
    ) -> Vec<moltis_channels::ChannelReplyTarget> {
        Vec::new()
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
        None
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
            tts: moltis_service_traits::NoopTtsService,
            project: moltis_service_traits::NoopProjectService,
            mcp: moltis_service_traits::NoopMcpService,
        }
    }
}
