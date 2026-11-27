//! The host side of server-initiated MCP requests.
//!
//! [`McpHost`] is implemented by the gateway (UI events, providers). The
//! per-server [`ServerRequestDispatcher`] enforces trust tiers and sampling
//! limits before anything reaches the host.

use std::sync::{Arc, RwLock};

use {
    async_trait::async_trait,
    serde_json::Value,
    tracing::{debug, info},
};

use crate::{
    elicitation::ElicitationSchema,
    error::{Error, Result},
    incoming::IncomingDispatcher,
    protocol::{
        CreateMessageParams, CreateMessageResult, ElicitRequestParams, ElicitResult,
        ElicitationMode, ListRootsResult, MCP_APP_MIME_TYPE, MCP_APPS_EXTENSION, Root,
        ServerNotification,
    },
    sampling::{CallContexts, SamplingBudget, SamplingConfig},
    types::{ClientCapabilities, JsonRpcError, RootsCapability},
};

pub type TrustTier = moltis_config::schema::McpTrustTier;

/// A sampling request that passed policy and budget checks.
#[derive(Debug, Clone)]
pub struct SamplingRequest {
    pub server: String,
    pub session_key: Option<String>,
    /// `max_tokens` is already clamped to the budget.
    pub params: CreateMessageParams,
    /// The host must ask the user before calling the model.
    pub requires_approval: bool,
    /// Model to use instead of the session's model.
    pub model: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SamplingResponse {
    pub result: CreateMessageResult,
    /// Input + output tokens, counted against the session budget.
    pub tokens_used: u64,
}

/// A form elicitation to show to the user.
#[derive(Debug, Clone)]
pub struct ElicitationRequest {
    pub server: String,
    pub session_key: Option<String>,
    pub message: String,
    pub schema: ElicitationSchema,
}

/// Host services used to answer server requests.
#[async_trait]
pub trait McpHost: Send + Sync {
    async fn create_message(&self, _request: SamplingRequest) -> Result<SamplingResponse> {
        Err(Error::message("sampling is not available on this host"))
    }

    /// Ask the user. Errors are reported to the server as `cancel`.
    async fn elicit(&self, _request: ElicitationRequest) -> Result<ElicitResult> {
        Ok(ElicitResult::cancel())
    }

    async fn on_notification(&self, _server: &str, _notification: ServerNotification) {}
}

/// Host used until the gateway installs its own.
pub struct NoopHost;

impl McpHost for NoopHost {}

/// Shared, replaceable host handle.
#[derive(Clone)]
pub struct HostCell(Arc<RwLock<Arc<dyn McpHost>>>);

impl Default for HostCell {
    fn default() -> Self {
        Self(Arc::new(RwLock::new(Arc::new(NoopHost))))
    }
}

impl HostCell {
    pub fn set(&self, host: Arc<dyn McpHost>) {
        *self.0.write().unwrap_or_else(|e| e.into_inner()) = host;
    }

    #[must_use]
    pub fn get(&self) -> Arc<dyn McpHost> {
        Arc::clone(&self.0.read().unwrap_or_else(|e| e.into_inner()))
    }
}

/// Shared roots list exposed to servers.
#[derive(Clone, Default)]
pub struct RootsCell(Arc<RwLock<Vec<Root>>>);

impl RootsCell {
    pub fn set(&self, roots: Vec<Root>) -> bool {
        let mut current = self.0.write().unwrap_or_else(|e| e.into_inner());
        let changed = *current != roots;
        *current = roots;
        changed
    }

    #[must_use]
    pub fn get(&self) -> Vec<Root> {
        self.0.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// Per-server settings that govern server-initiated requests.
#[derive(Debug, Clone, Default)]
pub struct ServerPolicy {
    pub trust: TrustTier,
    pub sampling: SamplingConfig,
}

impl ServerPolicy {
    #[must_use]
    pub fn sampling_allowed(&self) -> bool {
        self.sampling.enabled && self.trust != TrustTier::Untrusted
    }

    #[must_use]
    pub fn elicitation_allowed(&self) -> bool {
        self.trust != TrustTier::Untrusted
    }

    /// Capabilities to advertise. Transports that cannot receive server
    /// requests (legacy SSE) advertise none of the request-based ones.
    #[must_use]
    pub fn client_capabilities(
        &self,
        accepts_server_requests: bool,
        apps_enabled: bool,
    ) -> ClientCapabilities {
        let extensions = apps_enabled.then(|| {
            let mut map = serde_json::Map::new();
            map.insert(
                MCP_APPS_EXTENSION.into(),
                serde_json::json!({ "mimeTypes": [MCP_APP_MIME_TYPE] }),
            );
            map
        });
        if !accepts_server_requests {
            return ClientCapabilities {
                extensions,
                ..ClientCapabilities::default()
            };
        }
        ClientCapabilities {
            roots: Some(RootsCapability { list_changed: true }),
            sampling: self.sampling_allowed().then(|| serde_json::json!({})),
            elicitation: self.elicitation_allowed().then(|| serde_json::json!({})),
            extensions,
        }
    }
}

/// Shared state the manager hands to every server dispatcher.
#[derive(Clone, Default)]
pub struct HostContext {
    pub host: HostCell,
    pub roots: RootsCell,
    pub contexts: CallContexts,
    pub budget: Arc<SamplingBudget>,
}

/// Answers requests from one server according to its policy.
pub struct ServerRequestDispatcher {
    server: String,
    policy: ServerPolicy,
    context: HostContext,
}

impl ServerRequestDispatcher {
    #[must_use]
    pub fn new(server: &str, policy: ServerPolicy, context: HostContext) -> Self {
        Self {
            server: server.to_string(),
            policy,
            context,
        }
    }

    fn session_key(&self) -> Option<String> {
        self.context
            .contexts
            .current(&self.server)
            .and_then(|context| context.session_key)
    }

    fn list_roots(&self) -> ListRootsResult {
        if self.policy.trust == TrustTier::Untrusted {
            return ListRootsResult::default();
        }
        ListRootsResult {
            roots: self.context.roots.get(),
        }
    }

    async fn create_message(
        &self,
        params: Option<Value>,
    ) -> std::result::Result<Value, JsonRpcError> {
        if !self.policy.sampling_allowed() {
            info!(server = %self.server, "MCP sampling request refused by policy");
            return Err(JsonRpcError::rejected(
                "sampling is disabled for this server",
            ));
        }
        let mut params: CreateMessageParams = parse_params(params)?;
        let session_key = self.session_key();
        let max_tokens = self
            .context
            .budget
            .reserve(
                &self.server,
                session_key.as_deref(),
                &self.policy.sampling,
                params.max_tokens,
            )
            .map_err(|error| JsonRpcError::rejected(error.to_string()))?;
        params.max_tokens = max_tokens;

        let request = SamplingRequest {
            server: self.server.clone(),
            session_key: session_key.clone(),
            params,
            requires_approval: self.policy.trust != TrustTier::Trusted,
            model: self.policy.sampling.model.clone(),
        };
        let response = self
            .context
            .host
            .get()
            .create_message(request)
            .await
            .map_err(|error| JsonRpcError::rejected(error.to_string()))?;
        self.context
            .budget
            .record(&self.server, session_key.as_deref(), response.tokens_used);
        serde_json::to_value(response.result)
            .map_err(|error| JsonRpcError::new(JsonRpcError::INTERNAL_ERROR, error.to_string()))
    }

    async fn elicit(&self, params: Option<Value>) -> std::result::Result<Value, JsonRpcError> {
        let result = if self.policy.elicitation_allowed() {
            let params: ElicitRequestParams = parse_params(params)?;
            if params.mode == ElicitationMode::Url {
                return Err(JsonRpcError::invalid_params(
                    "URL-mode elicitation is not supported by this client",
                ));
            }
            let request = ElicitationRequest {
                server: self.server.clone(),
                session_key: self.session_key(),
                message: params.message,
                schema: params.requested_schema.unwrap_or_default(),
            };
            self.context
                .host
                .get()
                .elicit(request)
                .await
                .unwrap_or_else(|error| {
                    debug!(server = %self.server, %error, "elicitation failed, cancelling");
                    ElicitResult::cancel()
                })
        } else {
            info!(server = %self.server, "MCP elicitation from untrusted server declined");
            ElicitResult::decline()
        };
        serde_json::to_value(result)
            .map_err(|error| JsonRpcError::new(JsonRpcError::INTERNAL_ERROR, error.to_string()))
    }
}

fn parse_params<T: serde::de::DeserializeOwned>(
    params: Option<Value>,
) -> std::result::Result<T, JsonRpcError> {
    serde_json::from_value(params.unwrap_or(Value::Null))
        .map_err(|error| JsonRpcError::invalid_params(error.to_string()))
}

#[async_trait]
impl IncomingDispatcher for ServerRequestDispatcher {
    async fn on_request(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> std::result::Result<Value, JsonRpcError> {
        match method {
            "roots/list" => serde_json::to_value(self.list_roots()).map_err(|error| {
                JsonRpcError::new(JsonRpcError::INTERNAL_ERROR, error.to_string())
            }),
            "sampling/createMessage" => self.create_message(params).await,
            "elicitation/create" => self.elicit(params).await,
            other => Err(JsonRpcError::method_not_found(other)),
        }
    }

    async fn on_notification(&self, method: &str, params: Option<Value>) {
        let notification = ServerNotification::parse(method, params);
        self.context
            .host
            .get()
            .on_notification(&self.server, notification)
            .await;
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{protocol::ElicitAction, sampling::CallContext},
        serde_json::json,
        std::sync::Mutex,
    };

    #[derive(Default)]
    struct RecordingHost {
        sampling: Mutex<Vec<SamplingRequest>>,
        elicitations: Mutex<Vec<ElicitationRequest>>,
        notifications: Mutex<Vec<(String, ServerNotification)>>,
        fail_elicitation: bool,
    }

    #[async_trait]
    impl McpHost for RecordingHost {
        async fn create_message(&self, request: SamplingRequest) -> Result<SamplingResponse> {
            let max = request.params.max_tokens;
            self.sampling.lock().unwrap().push(request);
            Ok(SamplingResponse {
                result: CreateMessageResult::text("test-model", format!("max={max}")),
                tokens_used: 40,
            })
        }

        async fn elicit(&self, request: ElicitationRequest) -> Result<ElicitResult> {
            self.elicitations.lock().unwrap().push(request);
            if self.fail_elicitation {
                return Err(Error::message("no UI"));
            }
            let mut content = serde_json::Map::new();
            content.insert("name".into(), json!("Ada"));
            Ok(ElicitResult::accept(content))
        }

        async fn on_notification(&self, server: &str, notification: ServerNotification) {
            self.notifications
                .lock()
                .unwrap()
                .push((server.to_string(), notification));
        }
    }

    fn dispatcher(
        trust: TrustTier,
        sampling_enabled: bool,
    ) -> (ServerRequestDispatcher, Arc<RecordingHost>, HostContext) {
        let host = Arc::new(RecordingHost::default());
        let context = HostContext::default();
        context.host.set(host.clone());
        let policy = ServerPolicy {
            trust,
            sampling: SamplingConfig {
                enabled: sampling_enabled,
                max_tokens_per_request: 64,
                max_requests_per_session: 1,
                ..SamplingConfig::default()
            },
        };
        (
            ServerRequestDispatcher::new("srv", policy, context.clone()),
            host,
            context,
        )
    }

    fn sampling_params() -> Value {
        json!({
            "messages": [{"role": "user", "content": {"type": "text", "text": "hi"}}],
            "maxTokens": 500
        })
    }

    #[tokio::test]
    async fn roots_hidden_from_untrusted_servers() {
        let (trusted, _, context) = dispatcher(TrustTier::Trusted, false);
        let root = Root::from_path(std::path::Path::new("/work"), None).unwrap();
        assert!(context.roots.set(vec![root.clone()]));
        assert!(!context.roots.set(vec![root]));
        let roots = trusted.on_request("roots/list", None).await.unwrap();
        assert_eq!(roots["roots"][0]["uri"], "file:///work/");

        let (untrusted, _, context) = dispatcher(TrustTier::Untrusted, false);
        context.roots.set(vec![Root {
            uri: "file:///work/".into(),
            name: None,
        }]);
        let roots = untrusted.on_request("roots/list", None).await.unwrap();
        assert_eq!(roots, json!({"roots": []}));
    }

    #[tokio::test]
    async fn sampling_respects_policy_budget_and_session() {
        let (disabled, host, _) = dispatcher(TrustTier::Trusted, false);
        let error = disabled
            .on_request("sampling/createMessage", Some(sampling_params()))
            .await
            .unwrap_err();
        assert_eq!(error.code, JsonRpcError::USER_REJECTED);
        assert!(host.sampling.lock().unwrap().is_empty());

        let (ask, host, context) = dispatcher(TrustTier::Ask, true);
        let _call = context.contexts.enter("srv", CallContext {
            session_key: Some("session:1".into()),
        });
        let result = ask
            .on_request("sampling/createMessage", Some(sampling_params()))
            .await
            .unwrap();
        assert_eq!(result["content"]["text"], "max=64");
        {
            let requests = host.sampling.lock().unwrap();
            assert!(requests[0].requires_approval);
            assert_eq!(requests[0].session_key.as_deref(), Some("session:1"));
        }
        assert_eq!(context.budget.usage("srv", Some("session:1")).tokens, 40);

        let limited = ask
            .on_request("sampling/createMessage", Some(sampling_params()))
            .await
            .unwrap_err();
        assert!(limited.message.contains("request limit"));

        let invalid = ask
            .on_request("sampling/createMessage", Some(json!({"messages": "x"})))
            .await
            .unwrap_err();
        assert_eq!(invalid.code, JsonRpcError::INVALID_PARAMS);
    }

    #[tokio::test]
    async fn sampling_never_allowed_for_untrusted() {
        let (untrusted, ..) = dispatcher(TrustTier::Untrusted, true);
        assert!(
            untrusted
                .on_request("sampling/createMessage", Some(sampling_params()))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn noop_host_rejects_sampling() {
        let context = HostContext::default();
        let policy = ServerPolicy {
            trust: TrustTier::Trusted,
            sampling: SamplingConfig {
                enabled: true,
                ..SamplingConfig::default()
            },
        };
        let dispatcher = ServerRequestDispatcher::new("srv", policy, context);
        let error = dispatcher
            .on_request("sampling/createMessage", Some(sampling_params()))
            .await
            .unwrap_err();
        assert!(error.message.contains("not available"));
        let elicit = dispatcher
            .on_request("elicitation/create", Some(json!({"message": "q"})))
            .await
            .unwrap();
        assert_eq!(elicit["action"], "cancel");
    }

    #[tokio::test]
    async fn elicitation_routes_to_host_or_declines() {
        let (trusted, host, _) = dispatcher(TrustTier::Trusted, false);
        let params = json!({
            "message": "Your name?",
            "requestedSchema": {"type": "object", "properties": {"name": {"type": "string"}}}
        });
        let result = trusted
            .on_request("elicitation/create", Some(params.clone()))
            .await
            .unwrap();
        assert_eq!(result["action"], "accept");
        assert_eq!(host.elicitations.lock().unwrap()[0].message, "Your name?");

        let url_mode = trusted
            .on_request(
                "elicitation/create",
                Some(json!({"mode": "url", "message": "m", "url": "https://x", "elicitationId": "1"})),
            )
            .await
            .unwrap_err();
        assert_eq!(url_mode.code, JsonRpcError::INVALID_PARAMS);

        let (untrusted, host, _) = dispatcher(TrustTier::Untrusted, false);
        let result = untrusted
            .on_request("elicitation/create", Some(params.clone()))
            .await
            .unwrap();
        assert_eq!(result["action"], "decline");
        assert!(host.elicitations.lock().unwrap().is_empty());

        let failing = Arc::new(RecordingHost {
            fail_elicitation: true,
            ..RecordingHost::default()
        });
        let context = HostContext::default();
        context.host.set(failing);
        let dispatcher = ServerRequestDispatcher::new("srv", ServerPolicy::default(), context);
        let result: ElicitResult = serde_json::from_value(
            dispatcher
                .on_request("elicitation/create", Some(params))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(result.action, ElicitAction::Cancel);
    }

    #[tokio::test]
    async fn unknown_methods_and_notifications() {
        let (dispatcher, host, _) = dispatcher(TrustTier::Trusted, false);
        let error = dispatcher.on_request("tasks/get", None).await.unwrap_err();
        assert_eq!(error.code, JsonRpcError::METHOD_NOT_FOUND);
        dispatcher
            .on_notification("notifications/tools/list_changed", None)
            .await;
        assert_eq!(
            host.notifications.lock().unwrap()[0],
            ("srv".to_string(), ServerNotification::ToolsListChanged)
        );
    }

    #[test]
    fn capabilities_follow_policy_and_transport() {
        let trusted = ServerPolicy {
            trust: TrustTier::Trusted,
            sampling: SamplingConfig {
                enabled: true,
                ..SamplingConfig::default()
            },
        };
        let caps = trusted.client_capabilities(true, true);
        assert!(caps.roots.is_some() && caps.sampling.is_some() && caps.elicitation.is_some());
        assert!(caps.extensions.unwrap().contains_key(MCP_APPS_EXTENSION));

        let legacy = trusted.client_capabilities(false, false);
        assert_eq!(legacy, ClientCapabilities::default());

        let untrusted = ServerPolicy {
            trust: TrustTier::Untrusted,
            ..trusted
        };
        let caps = untrusted.client_capabilities(true, false);
        assert!(caps.roots.is_some());
        assert!(caps.sampling.is_none() && caps.elicitation.is_none() && caps.extensions.is_none());
    }
}
