//! Gateway side of server-initiated MCP requests: sampling through the
//! session's provider, elicitation through the UI event bus (with a channel
//! text fallback), approvals for tools of `ask`/`untrusted` servers, and
//! server notifications.

use std::sync::Arc;

use {
    async_trait::async_trait,
    futures::StreamExt,
    serde_json::json,
    tokio::sync::RwLock,
    tracing::{debug, info, warn},
};

use {
    moltis_agents::model::{ChatMessage, ContentPart, LlmProvider, StreamEvent},
    moltis_mcp::{
        ElicitationRequest, McpHost, McpToolApprover, PendingElicitations, SamplingRequest,
        SamplingResponse, ToolApprovalRequest,
        protocol::{
            CreateMessageParams, CreateMessageResult, ElicitResult, Role, Root, ServerNotification,
        },
        types::ContentBlock,
    },
    moltis_providers::ProviderRegistry,
    moltis_tools::{
        approval::{ApprovalDecision, ApprovalManager},
        exec::ApprovalBroadcaster,
    },
};

use crate::{
    approval::{MAX_COMMAND_PREVIEW_LEN, notify_session_channel, truncate_command_preview},
    broadcast::{BroadcastOpts, broadcast},
    mcp_service::LiveMcpService,
    state::GatewayState,
};

/// Rough characters-per-token ratio used to stop a sampling stream at the
/// `maxTokens` budget when the provider API takes no output limit.
const CHARS_PER_TOKEN: usize = 4;

pub struct GatewayMcpHost {
    state: Arc<GatewayState>,
    providers: Arc<RwLock<ProviderRegistry>>,
    approvals: Arc<ApprovalManager>,
    broadcaster: Arc<dyn ApprovalBroadcaster>,
    elicitations: Arc<PendingElicitations>,
    mcp: Arc<LiveMcpService>,
}

impl GatewayMcpHost {
    pub fn new(
        state: Arc<GatewayState>,
        providers: Arc<RwLock<ProviderRegistry>>,
        approvals: Arc<ApprovalManager>,
        broadcaster: Arc<dyn ApprovalBroadcaster>,
        mcp: Arc<LiveMcpService>,
    ) -> Self {
        Self {
            state,
            providers,
            approvals,
            broadcaster,
            elicitations: mcp.elicitations(),
            mcp,
        }
    }

    /// Ask the user through the shared approval flow (web UI and channels).
    async fn ask(&self, description: &str, session_key: Option<&str>) -> bool {
        let (request_id, rx) = self
            .approvals
            .create_request(description, session_key)
            .await;
        if let Err(error) = self
            .broadcaster
            .broadcast_request(&request_id, description, session_key)
            .await
        {
            warn!(%error, "failed to broadcast MCP approval request");
        }
        matches!(
            self.approvals.wait_for_decision(rx).await,
            ApprovalDecision::Approved
        )
    }

    async fn resolve_provider(
        &self,
        model: Option<&str>,
        session_key: Option<&str>,
    ) -> moltis_mcp::Result<Arc<dyn LlmProvider>> {
        let session_model = match (session_key, self.state.services.session_metadata.as_ref()) {
            (Some(key), Some(metadata)) => metadata.get(key).await.and_then(|entry| entry.model),
            _ => None,
        };
        let registry = self.providers.read().await;
        if let Some(provider) = model
            .or(session_model.as_deref())
            .and_then(|id| registry.get(id))
        {
            return Ok(provider);
        }
        registry
            .first_with_tools()
            .or_else(|| registry.first())
            .ok_or_else(|| moltis_mcp::Error::message("no LLM provider is configured"))
    }
}

/// Roots exposed to MCP servers: the agent workspace and every project
/// directory.
pub async fn workspace_roots(
    data_dir: &std::path::Path,
    projects: &dyn moltis_projects::ProjectStore,
) -> Vec<Root> {
    let mut roots: Vec<Root> = Root::from_path(data_dir, Some("workspace".into()))
        .into_iter()
        .collect();
    match projects.list().await {
        Ok(projects) => roots.extend(projects.iter().filter_map(|project| {
            Root::from_path(&project.directory, Some(project.label.clone()))
        })),
        Err(error) => warn!(%error, "could not list projects for MCP roots"),
    }
    roots.dedup_by(|left, right| left.uri == right.uri);
    roots
}

/// Token estimate of a request's input, used when the provider reports no usage.
pub(crate) fn sampling_input_estimate(params: &CreateMessageParams) -> usize {
    let chars = params.system_prompt.as_deref().map_or(0, str::len)
        + params
            .messages
            .iter()
            .map(|message| message.text().len())
            .sum::<usize>();
    chars / CHARS_PER_TOKEN
}

/// Convert an MCP sampling request to provider messages.
pub(crate) fn sampling_chat_messages(params: &CreateMessageParams) -> Vec<ChatMessage> {
    let mut messages = Vec::with_capacity(params.messages.len() + 1);
    let limit = format!("Answer in at most {} tokens.", params.max_tokens);
    let system = match params.system_prompt.as_deref() {
        Some(prompt) if !prompt.trim().is_empty() => format!("{prompt}\n\n{limit}"),
        _ => limit,
    };
    messages.push(ChatMessage::system(system));
    for message in &params.messages {
        match message.role {
            Role::Assistant => messages.push(ChatMessage::assistant(message.text())),
            Role::User => {
                let parts: Vec<ContentPart> = message
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text { text } => Some(ContentPart::Text(text.clone())),
                        ContentBlock::Image { data, mime_type } => Some(ContentPart::Image {
                            media_type: mime_type.clone(),
                            data: data.clone(),
                        }),
                        _ => None,
                    })
                    .collect();
                let has_image = parts
                    .iter()
                    .any(|part| matches!(part, ContentPart::Image { .. }));
                if has_image {
                    messages.push(ChatMessage::user_multimodal(parts));
                } else {
                    messages.push(ChatMessage::user(message.text()));
                }
            },
        }
    }
    messages
}

/// Stream a completion and stop once the text reaches the token budget.
/// Returns the text, tokens to charge, and whether it was cut short.
pub(crate) async fn stream_sampling(
    provider: &dyn LlmProvider,
    messages: Vec<ChatMessage>,
    max_tokens: u32,
    input_estimate: usize,
) -> moltis_mcp::Result<(String, u64, bool)> {
    let char_budget = usize::try_from(max_tokens)
        .unwrap_or(usize::MAX)
        .saturating_mul(CHARS_PER_TOKEN);
    let mut stream = provider.stream(messages);
    let mut text = String::new();
    while let Some(event) = stream.next().await {
        match event {
            StreamEvent::Delta(delta) => {
                text.push_str(&delta);
                if text.len() >= char_budget {
                    let cut = text.floor_char_boundary(char_budget);
                    text.truncate(cut);
                    let tokens = (input_estimate + text.len() / CHARS_PER_TOKEN) as u64;
                    return Ok((text, tokens, true));
                }
            },
            StreamEvent::Done(usage) => {
                let reported = u64::from(usage.input_tokens)
                    + u64::from(usage.output_tokens)
                    + u64::from(usage.cache_read_tokens)
                    + u64::from(usage.cache_write_tokens);
                let tokens = if reported > 0 {
                    reported
                } else {
                    (input_estimate + text.len() / CHARS_PER_TOKEN) as u64
                };
                return Ok((text, tokens, false));
            },
            StreamEvent::Error(error) => {
                return Err(moltis_mcp::Error::message(format!("model error: {error}")));
            },
            _ => {},
        }
    }
    let tokens = (input_estimate + text.len() / CHARS_PER_TOKEN) as u64;
    Ok((text, tokens, false))
}

fn sampling_preview(request: &SamplingRequest) -> String {
    let last = request
        .params
        .messages
        .last()
        .map(|message| message.text())
        .unwrap_or_default();
    let preview = truncate_command_preview(&last, MAX_COMMAND_PREVIEW_LEN);
    format!(
        "MCP server '{}' wants to ask the model (max {} tokens): {preview}",
        request.server, request.params.max_tokens
    )
}

#[async_trait]
impl McpHost for GatewayMcpHost {
    async fn create_message(
        &self,
        request: SamplingRequest,
    ) -> moltis_mcp::Result<SamplingResponse> {
        let session_key = request.session_key.as_deref();
        if request.requires_approval && !self.ask(&sampling_preview(&request), session_key).await {
            return Err(moltis_mcp::Error::message(
                "the user declined the sampling request",
            ));
        }
        let provider = self
            .resolve_provider(request.model.as_deref(), session_key)
            .await?;
        info!(
            server = %request.server,
            model = provider.id(),
            max_tokens = request.params.max_tokens,
            "answering MCP sampling request"
        );
        let messages = sampling_chat_messages(&request.params);
        let (text, tokens_used, truncated) = stream_sampling(
            provider.as_ref(),
            messages,
            request.params.max_tokens,
            sampling_input_estimate(&request.params),
        )
        .await?;
        let mut result = CreateMessageResult::text(provider.id(), text);
        if truncated {
            result.stop_reason = Some("maxTokens".into());
        }
        broadcast(
            &self.state,
            "mcp.sampling.completed",
            json!({
                "server": request.server,
                "sessionKey": request.session_key,
                "model": provider.id(),
                "tokens": tokens_used,
            }),
            BroadcastOpts::default(),
        )
        .await;
        Ok(SamplingResponse {
            result,
            tokens_used,
        })
    }

    async fn elicit(&self, request: ElicitationRequest) -> moltis_mcp::Result<ElicitResult> {
        let (view, rx) = self
            .elicitations
            .create(
                &request.server,
                request.session_key.as_deref(),
                &request.message,
                request.schema,
            )
            .await;
        broadcast(
            &self.state,
            "mcp.elicitation.requested",
            serde_json::to_value(&view)?,
            BroadcastOpts::default(),
        )
        .await;
        if let Err(error) = notify_session_channel(
            &self.state,
            view.session_key.as_deref(),
            &view.channel_text(),
        )
        .await
        {
            warn!(%error, server = %view.server, "failed to send elicitation to channel");
        }
        let result = self.elicitations.wait(&view.id, rx).await;
        broadcast(
            &self.state,
            "mcp.elicitation.resolved",
            json!({ "id": view.id, "server": view.server, "action": result.action }),
            BroadcastOpts::default(),
        )
        .await;
        if result.action == moltis_mcp::protocol::ElicitAction::Cancel
            && let Err(error) = notify_session_channel(
                &self.state,
                view.session_key.as_deref(),
                &format!(
                    "MCP server '{}' stopped waiting for an answer.",
                    view.server
                ),
            )
            .await
        {
            debug!(%error, "failed to send elicitation cancel notice");
        }
        Ok(result)
    }

    async fn on_notification(&self, server: &str, notification: ServerNotification) {
        match notification {
            ServerNotification::ToolsListChanged => {
                match self.mcp.manager().refresh_tools(server).await {
                    Ok(tools) => {
                        info!(server, tools = tools.len(), "MCP tools changed");
                        self.mcp.sync_tools_if_ready().await;
                    },
                    Err(error) => warn!(server, %error, "failed to refresh MCP tools"),
                }
                broadcast(
                    &self.state,
                    "mcp.tools.changed",
                    json!({ "server": server }),
                    BroadcastOpts::default(),
                )
                .await;
            },
            ServerNotification::ResourcesListChanged => {
                broadcast(
                    &self.state,
                    "mcp.resources.changed",
                    json!({ "server": server }),
                    BroadcastOpts::default(),
                )
                .await;
            },
            ServerNotification::ResourceUpdated { uri } => {
                broadcast(
                    &self.state,
                    "mcp.resource.updated",
                    json!({ "server": server, "uri": uri }),
                    BroadcastOpts::default(),
                )
                .await;
            },
            ServerNotification::PromptsListChanged => {
                broadcast(
                    &self.state,
                    "mcp.prompts.changed",
                    json!({ "server": server }),
                    BroadcastOpts::default(),
                )
                .await;
            },
            ServerNotification::Log { level, logger, .. } => {
                debug!(server, level, logger, "MCP server log message");
            },
            ServerNotification::Progress { .. } | ServerNotification::Other { .. } => {},
        }
    }
}

#[async_trait]
impl McpToolApprover for GatewayMcpHost {
    async fn approve(&self, request: ToolApprovalRequest) -> moltis_mcp::Result<bool> {
        let arguments = request.arguments.to_string();
        let preview = truncate_command_preview(&arguments, MAX_COMMAND_PREVIEW_LEN);
        let description = format!(
            "MCP tool {}/{} ({} server) {preview}",
            request.server, request.tool, request.trust
        );
        Ok(self.ask(&description, request.session_key.as_deref()).await)
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use {
        super::*,
        futures::stream,
        moltis_agents::model::{CompletionResponse, Usage, UserContent},
        std::pin::Pin,
    };

    struct ScriptedProvider {
        events: Vec<StreamEvent>,
    }

    #[async_trait]
    impl LlmProvider for ScriptedProvider {
        fn name(&self) -> &str {
            "scripted"
        }

        fn id(&self) -> &str {
            "scripted/model"
        }

        async fn complete(
            &self,
            _messages: &[ChatMessage],
            _tools: &[serde_json::Value],
        ) -> anyhow::Result<CompletionResponse> {
            anyhow::bail!("not used")
        }

        fn stream(
            &self,
            _messages: Vec<ChatMessage>,
        ) -> Pin<Box<dyn futures::Stream<Item = StreamEvent> + Send + '_>> {
            Box::pin(stream::iter(self.events.clone()))
        }
    }

    fn params(value: serde_json::Value) -> CreateMessageParams {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn messages_include_system_limit_roles_and_images() {
        let messages = sampling_chat_messages(&params(json!({
            "systemPrompt": "Be terse.",
            "maxTokens": 50,
            "messages": [
                {"role": "user", "content": {"type": "text", "text": "hi"}},
                {"role": "assistant", "content": {"type": "text", "text": "hello"}},
                {"role": "user", "content": [{"type": "text", "text": "look"}, {"type": "image", "data": "AAA", "mimeType": "image/png"}]}
            ]
        })));
        assert_eq!(messages.len(), 4);
        match &messages[0] {
            ChatMessage::System { content } => {
                assert!(content.starts_with("Be terse."));
                assert!(content.contains("at most 50 tokens"));
            },
            other => panic!("unexpected {other:?}"),
        }
        assert!(
            matches!(&messages[2], ChatMessage::Assistant { content: Some(text), .. } if text == "hello")
        );
        assert!(
            matches!(&messages[3], ChatMessage::User { content: UserContent::Multimodal(parts), .. } if parts.len() == 2)
        );

        let bare = sampling_chat_messages(&params(json!({"maxTokens": 5, "messages": []})));
        assert!(
            matches!(&bare[0], ChatMessage::System { content } if content == "Answer in at most 5 tokens.")
        );
    }

    #[tokio::test]
    async fn stream_sampling_uses_reported_usage() {
        let provider = ScriptedProvider {
            events: vec![
                StreamEvent::Delta("Hello ".into()),
                StreamEvent::Delta("world".into()),
                StreamEvent::Done(Usage {
                    input_tokens: 10,
                    output_tokens: 3,
                    ..Usage::default()
                }),
            ],
        };
        let (text, tokens, truncated) =
            stream_sampling(&provider, vec![ChatMessage::user("x")], 100, 0)
                .await
                .unwrap();
        assert_eq!(text, "Hello world");
        assert_eq!(tokens, 13);
        assert!(!truncated);
    }

    #[tokio::test]
    async fn stream_sampling_stops_at_budget_and_reports_errors() {
        let provider = ScriptedProvider {
            events: vec![
                StreamEvent::Delta("x".repeat(100)),
                StreamEvent::Delta("never".into()),
            ],
        };
        let (text, tokens, truncated) =
            stream_sampling(&provider, vec![ChatMessage::user("x")], 2, 1)
                .await
                .unwrap();
        assert_eq!(text.len(), 2 * CHARS_PER_TOKEN);
        assert!(truncated);
        assert!(tokens >= 2);

        let failing = ScriptedProvider {
            events: vec![StreamEvent::Error("quota".into())],
        };
        assert!(stream_sampling(&failing, vec![], 10, 0).await.is_err());

        let unfinished = ScriptedProvider {
            events: vec![StreamEvent::Delta("abcd".into())],
        };
        let (text, _, truncated) = stream_sampling(&unfinished, vec![], 10, 0).await.unwrap();
        assert_eq!(text, "abcd");
        assert!(!truncated);
    }

    #[test]
    fn sampling_preview_is_truncated() {
        let request = SamplingRequest {
            server: "notes".into(),
            session_key: None,
            params: params(
                json!({"maxTokens": 7, "messages": [{"role": "user", "content": {"type": "text", "text": "y".repeat(200)}}]}),
            ),
            requires_approval: true,
            model: None,
        };
        let preview = sampling_preview(&request);
        assert!(preview.starts_with("MCP server 'notes' wants to ask the model (max 7 tokens)"));
        assert!(preview.ends_with('…'));
    }
}
