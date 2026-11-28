use std::sync::MutexGuard;

use {antre_common::types::ReplyPayload, async_trait::async_trait, tokio::sync::Mutex};

pub(crate) struct LocalModelConfigTestGuard {
    _lock: MutexGuard<'static, ()>,
}

impl LocalModelConfigTestGuard {
    pub(crate) fn new() -> Self {
        Self {
            _lock: crate::config_override_test_lock(),
        }
    }
}

impl Drop for LocalModelConfigTestGuard {
    fn drop(&mut self) {
        antre_config::clear_config_dir();
        antre_config::clear_data_dir();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeliveredMessage {
    pub(crate) account_id: String,
    pub(crate) to: String,
    pub(crate) text: String,
    pub(crate) reply_to: Option<String>,
}

#[derive(Default)]
pub(crate) struct RecordingChannelOutbound {
    pub(crate) delivered: Mutex<Vec<DeliveredMessage>>,
}

#[async_trait]
impl antre_channels::ChannelOutbound for RecordingChannelOutbound {
    async fn send_text(
        &self,
        account_id: &str,
        to: &str,
        text: &str,
        reply_to: Option<&str>,
    ) -> antre_channels::Result<()> {
        self.delivered.lock().await.push(DeliveredMessage {
            account_id: account_id.to_string(),
            to: to.to_string(),
            text: text.to_string(),
            reply_to: reply_to.map(ToString::to_string),
        });
        Ok(())
    }

    async fn send_media(
        &self,
        _account_id: &str,
        _to: &str,
        _payload: &ReplyPayload,
        _reply_to: Option<&str>,
    ) -> antre_channels::Result<()> {
        Ok(())
    }
}

pub(crate) fn cron_delivery_request() -> antre_cron::service::AgentTurnRequest {
    antre_cron::service::AgentTurnRequest {
        message: "Run background summary".to_string(),
        model: None,
        agent_id: None,
        timeout_secs: None,
        tool_controls: Default::default(),
        deliver: true,
        channel: Some("bot-main".to_string()),
        to: Some("123456".to_string()),
        session_target: antre_cron::types::SessionTarget::Isolated,
        sandbox: antre_cron::types::CronSandboxConfig::default(),
    }
}
