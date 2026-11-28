//! Registering the sub-agent tools during startup, and posting background
//! sub-agent results to the session that spawned them.

use std::sync::{Arc, OnceLock, Weak};

use {
    moltis_agents::{model::LlmProvider, tool_registry::ToolRegistry},
    moltis_providers::ProviderRegistry,
    moltis_tools::spawn_agent_tasks::SpawnTaskStore,
};

use crate::{
    broadcast::{BroadcastOpts, broadcast},
    chat::LiveChatService,
    state::GatewayState,
};

/// Posts a background sub-agent's result to its parent session once the
/// chat service exists (it is built after the tools).
pub(super) struct BackgroundAnnouncer {
    enabled: bool,
    chat: Arc<OnceLock<Weak<LiveChatService>>>,
}

impl BackgroundAnnouncer {
    pub(super) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            chat: Arc::new(OnceLock::new()),
        }
    }

    pub(super) fn attach(&self, chat: &Arc<LiveChatService>) {
        let _ = self.chat.set(Arc::downgrade(chat));
    }

    fn callback(
        &self,
        store: &Arc<SpawnTaskStore>,
    ) -> Option<moltis_tools::spawn_agent::OnBackgroundComplete> {
        if !self.enabled {
            return None;
        }
        let chat = Arc::clone(&self.chat);
        let store = Arc::clone(store);
        Some(Arc::new(move |completion| {
            let Some(chat) = chat.get().and_then(Weak::upgrade) else {
                tracing::debug!("chat service not ready; background result not posted");
                return;
            };
            let store = Arc::clone(&store);
            tokio::spawn(async move {
                if let Err(error) = chat
                    .announce_background_task(&completion.session_key, &completion.task_id, &store)
                    .await
                {
                    tracing::warn!(
                        session = %completion.session_key,
                        task_id = %completion.task_id,
                        %error,
                        "could not post background task result"
                    );
                }
            });
        }))
    }
}

pub(super) fn register_spawn_tools(
    tool_registry: &mut ToolRegistry,
    registry: &Arc<tokio::sync::RwLock<ProviderRegistry>>,
    default_provider: Arc<dyn LlmProvider>,
    state: &Arc<GatewayState>,
    agents_config: Arc<tokio::sync::RwLock<moltis_config::AgentsConfig>>,
    announcer: &BackgroundAnnouncer,
) {
    let spawn_task_store = Arc::new(SpawnTaskStore::default());
    tool_registry.register(Box::new(
        moltis_tools::spawn_agent_tasks::SpawnStatusTool::new(Arc::clone(&spawn_task_store)),
    ));
    tool_registry.register(Box::new(
        moltis_tools::spawn_agent_tasks::SpawnResultTool::new(Arc::clone(&spawn_task_store)),
    ));
    tool_registry.register(Box::new(
        moltis_tools::spawn_agent_tasks::SpawnListTool::new(Arc::clone(&spawn_task_store)),
    ));
    tool_registry.register(Box::new(
        moltis_tools::spawn_agent_tasks::SpawnCancelTool::new(Arc::clone(&spawn_task_store)),
    ));
    let base_tools = Arc::new(tool_registry.clone_without(&[]));
    let state_for_spawn = Arc::clone(state);
    let on_spawn_event: moltis_tools::spawn_agent::OnSpawnEvent = Arc::new(move |event| {
        use moltis_agents::runner::RunnerEvent;
        let state = Arc::clone(&state_for_spawn);
        let payload = match &event {
            RunnerEvent::SubAgentStart { task, model, depth } => {
                serde_json::json!({
                    "state": "sub_agent_start",
                    "task": task,
                    "model": model,
                    "depth": depth,
                })
            },
            RunnerEvent::SubAgentEnd {
                task,
                model,
                depth,
                iterations,
                tool_calls_made,
            } => serde_json::json!({
                "state": "sub_agent_end",
                "task": task,
                "model": model,
                "depth": depth,
                "iterations": iterations,
                "toolCallsMade": tool_calls_made,
            }),
            _ => return,
        };
        tokio::spawn(async move {
            broadcast(&state, "chat", payload, BroadcastOpts::default()).await;
        });
    });
    let spawn_tool = moltis_tools::spawn_agent::SpawnAgentTool::new(
        Arc::clone(registry),
        default_provider,
        base_tools,
    )
    .with_on_event(on_spawn_event)
    .with_agents_config(agents_config)
    .with_task_store(Arc::clone(&spawn_task_store));
    let spawn_tool = match announcer.callback(&spawn_task_store) {
        Some(callback) => spawn_tool.with_on_background_complete(callback),
        None => spawn_tool,
    };
    tool_registry.register(Box::new(spawn_tool));
}
