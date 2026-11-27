//! MCP client: manages the protocol handshake and tool interactions with a single MCP server.

use std::{collections::HashMap, sync::Arc, time::Duration};

use {
    secrecy::Secret,
    tracing::{debug, info, warn},
};

#[cfg(feature = "metrics")]
use std::time::Instant;

#[cfg(feature = "metrics")]
use moltis_metrics::{counter, gauge, histogram, labels, mcp as mcp_metrics};

use crate::{
    auth::SharedAuthProvider,
    error::{Context, Error, Result},
    host::{HostContext, ServerPolicy, ServerRequestDispatcher},
    legacy_sse_transport::LegacySseTransport,
    protocol::{
        McpPrompt, McpResource, McpResourceTemplate, PromptsGetResult, ResourcesReadResult,
    },
    remote::ResolvedRemoteConfig,
    sse_transport::SseTransport,
    traits::{McpClientTrait, McpTransport},
    transport::{StdioLaunchOptions, StdioTransport},
    types::{
        ClientCapabilities, ClientInfo, InitializeParams, InitializeResult, McpToolDef,
        McpTransportError, PROTOCOL_VERSION, SUPPORTED_PROTOCOL_VERSIONS, ServerCapabilities,
        ToolsCallParams, ToolsCallResult, ToolsListResult,
    },
};

/// State of an MCP client connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpClientState {
    /// Transport spawned, not yet initialized.
    Connected,
    /// `initialize` completed, `initialized` notification sent.
    Ready,
    /// OAuth authentication in progress (waiting for browser).
    Authenticating,
    /// Server process exited or was shut down.
    Closed,
}

/// How a client presents itself to a server: the policy that decides which
/// capabilities are advertised and the host that answers server requests.
#[derive(Clone, Default)]
pub struct ClientSetup {
    pub policy: ServerPolicy,
    pub apps_enabled: bool,
    /// `None` leaves server requests unanswered (method not found) and
    /// advertises no request-based capability.
    pub host: Option<HostContext>,
}

/// An MCP client connected to a single server.
pub struct McpClient {
    pub(crate) server_name: String,
    pub(crate) transport: Arc<dyn McpTransport>,
    pub(crate) state: McpClientState,
    pub(crate) server_info: Option<InitializeResult>,
    pub(crate) tools: Vec<McpToolDef>,
}

impl McpClient {
    /// Spawn the server process and perform the MCP handshake (initialize + initialized).
    pub async fn connect(
        server_name: &str,
        command: &str,
        args: &[String],
        env: &HashMap<String, Secret<String>>,
        request_timeout: Duration,
    ) -> Result<Self> {
        Self::connect_with_options(
            server_name,
            command,
            args,
            env,
            request_timeout,
            &StdioLaunchOptions::default(),
            &ClientSetup::default(),
        )
        .await
    }

    pub async fn connect_with_options(
        server_name: &str,
        command: &str,
        args: &[String],
        env: &HashMap<String, Secret<String>>,
        request_timeout: Duration,
        options: &StdioLaunchOptions,
        setup: &ClientSetup,
    ) -> Result<Self> {
        info!(server = %server_name, command = %command, arg_count = args.len(), "connecting to MCP server");
        let transport =
            StdioTransport::spawn_with_options(command, args, env, request_timeout, options)
                .await?;
        Self::from_transport(server_name, transport, setup).await
    }

    /// Connect to a remote MCP server over HTTP/SSE or Streamable HTTP.
    pub async fn connect_sse(
        server_name: &str,
        remote: &ResolvedRemoteConfig,
        request_timeout: Duration,
        setup: &ClientSetup,
    ) -> Result<Self> {
        info!(
            server = %server_name,
            url = %remote.display_url(),
            "connecting to remote MCP server"
        );
        let transport = SseTransport::new_with_remote(remote.clone(), request_timeout)?;
        Self::from_transport(server_name, transport, setup).await
    }

    /// Connect to a remote MCP server using the legacy SSE transport.
    ///
    /// The legacy SSE protocol first GETs the SSE endpoint to discover the
    /// message URL (with sessionId), then POSTs JSON-RPC messages to it.
    pub async fn connect_legacy_sse(
        server_name: &str,
        remote: &ResolvedRemoteConfig,
        request_timeout: Duration,
        setup: &ClientSetup,
    ) -> Result<Self> {
        info!(
            server = %server_name,
            url = %remote.display_url(),
            "connecting to MCP server via legacy SSE"
        );
        let transport = LegacySseTransport::new_with_remote(remote.clone(), request_timeout)?;
        Self::from_transport(server_name, transport, setup).await
    }

    /// Connect to a remote MCP server using the legacy SSE transport with an OAuth auth provider.
    pub async fn connect_legacy_sse_with_auth(
        server_name: &str,
        remote: &ResolvedRemoteConfig,
        auth: SharedAuthProvider,
        request_timeout: Duration,
        setup: &ClientSetup,
    ) -> Result<Self> {
        info!(
            server = %server_name,
            url = %remote.display_url(),
            "connecting to MCP server via legacy SSE (with auth)"
        );
        let transport =
            LegacySseTransport::with_auth_remote(remote.clone(), auth, request_timeout)?;
        Self::from_transport(server_name, transport, setup).await
    }

    /// Connect to a remote MCP server over HTTP/SSE or Streamable HTTP with an OAuth auth provider.
    pub async fn connect_sse_with_auth(
        server_name: &str,
        remote: &ResolvedRemoteConfig,
        auth: SharedAuthProvider,
        request_timeout: Duration,
        setup: &ClientSetup,
    ) -> Result<Self> {
        info!(
            server = %server_name,
            url = %remote.display_url(),
            "connecting to remote MCP server (with auth)"
        );
        let transport = SseTransport::with_auth_remote(remote.clone(), auth, request_timeout)?;
        Self::from_transport(server_name, transport, setup).await
    }

    /// Perform the handshake over an already-open transport.
    pub async fn from_transport(
        server_name: &str,
        transport: Arc<dyn McpTransport>,
        setup: &ClientSetup,
    ) -> Result<Self> {
        let answers_requests = transport.accepts_server_requests() && setup.host.is_some();
        if let Some(context) = &setup.host
            && answers_requests
        {
            transport.set_dispatcher(Arc::new(ServerRequestDispatcher::new(
                server_name,
                setup.policy.clone(),
                context.clone(),
            )));
        }
        let capabilities = setup
            .policy
            .client_capabilities(answers_requests, setup.apps_enabled);

        let mut client = Self {
            server_name: server_name.into(),
            transport,
            state: McpClientState::Connected,
            server_info: None,
            tools: Vec::new(),
        };

        if let Err(e) = client.initialize(capabilities).await {
            warn!(server = %server_name, "MCP initialize handshake failed");
            return Err(e);
        }
        client.transport.start_server_stream().await;

        #[cfg(feature = "metrics")]
        {
            counter!(mcp_metrics::SERVER_CONNECTIONS_TOTAL, labels::SERVER => server_name.to_string())
                .increment(1);
            gauge!(mcp_metrics::SERVERS_CONNECTED).increment(1.0);
        }

        Ok(client)
    }

    async fn initialize(&mut self, capabilities: ClientCapabilities) -> Result<()> {
        let params = InitializeParams {
            protocol_version: PROTOCOL_VERSION.into(),
            capabilities,
            client_info: ClientInfo {
                name: "moltis".into(),
                version: moltis_config::VERSION.into(),
            },
        };

        let resp = match self
            .transport
            .request("initialize", Some(serde_json::to_value(&params)?))
            .await
        {
            Ok(r) => r,
            // Preserve 401 so the manager can trigger the OAuth re-auth flow.
            Err(e @ Error::Transport(McpTransportError::Unauthorized { .. })) => return Err(e),
            Err(e) => {
                return Err(Error::message(format!(
                    "MCP initialize request failed: {e}"
                )));
            },
        };

        let result: InitializeResult =
            serde_json::from_value(resp.result.context("MCP initialize returned no result")?)
                .context("failed to parse MCP initialize result")?;

        if !SUPPORTED_PROTOCOL_VERSIONS.contains(&result.protocol_version.as_str()) {
            warn!(
                server = %self.server_name,
                version = %result.protocol_version,
                "MCP server negotiated an unknown protocol version; continuing"
            );
        }
        self.transport
            .set_protocol_version(&result.protocol_version);

        info!(
            server = %self.server_name,
            version = %result.protocol_version,
            "MCP server initialized"
        );

        self.server_info = Some(result);

        // Send `initialized` notification to complete handshake.
        self.transport
            .notify("notifications/initialized", None)
            .await?;
        self.state = McpClientState::Ready;

        Ok(())
    }

    pub(crate) fn ensure_ready(&self) -> Result<()> {
        if self.state != McpClientState::Ready {
            return Err(Error::message(format!(
                "MCP client for '{}' is not ready (state: {:?})",
                self.server_name, self.state
            )));
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl McpClientTrait for McpClient {
    fn server_name(&self) -> &str {
        &self.server_name
    }

    fn state(&self) -> McpClientState {
        self.state
    }

    fn tools(&self) -> &[McpToolDef] {
        &self.tools
    }

    async fn list_tools(&mut self) -> Result<&[McpToolDef]> {
        self.ensure_ready()?;

        let mut tools: Vec<McpToolDef> = self
            .paginate("tools/list", |page: ToolsListResult| {
                (page.tools, page.next_cursor)
            })
            .await?;
        tools.sort_by(|left, right| left.name.cmp(&right.name));

        debug!(
            server = %self.server_name,
            count = tools.len(),
            "fetched MCP tools"
        );

        self.tools = tools;
        Ok(&self.tools)
    }

    async fn call_tool(&self, name: &str, arguments: serde_json::Value) -> Result<ToolsCallResult> {
        self.ensure_ready()?;

        #[cfg(feature = "metrics")]
        let start = Instant::now();

        let params = ToolsCallParams {
            name: name.into(),
            arguments,
        };

        let result = self
            .transport
            .request("tools/call", Some(serde_json::to_value(&params)?))
            .await;

        match result {
            Ok(resp) => {
                let result: ToolsCallResult =
                    serde_json::from_value(resp.result.context("tools/call returned no result")?)?;

                #[cfg(feature = "metrics")]
                {
                    let duration = start.elapsed().as_secs_f64();

                    counter!(
                        mcp_metrics::TOOL_CALLS_TOTAL,
                        labels::SERVER => self.server_name.clone(),
                        labels::TOOL => name.to_string()
                    )
                    .increment(1);

                    histogram!(
                        mcp_metrics::TOOL_CALL_DURATION_SECONDS,
                        labels::SERVER => self.server_name.clone(),
                        labels::TOOL => name.to_string()
                    )
                    .record(duration);
                }

                Ok(result)
            },
            Err(e) => {
                #[cfg(feature = "metrics")]
                {
                    counter!(
                        mcp_metrics::TOOL_CALL_ERRORS_TOTAL,
                        labels::SERVER => self.server_name.clone(),
                        labels::TOOL => name.to_string()
                    )
                    .increment(1);
                }
                Err(e)
            },
        }
    }

    async fn is_alive(&self) -> bool {
        self.transport.is_alive().await
    }

    async fn shutdown(&mut self) {
        self.state = McpClientState::Closed;
        self.transport.kill().await;

        // Decrement connected servers gauge
        #[cfg(feature = "metrics")]
        {
            gauge!(mcp_metrics::SERVERS_CONNECTED).decrement(1.0);
        }
    }

    fn server_capabilities(&self) -> Option<&ServerCapabilities> {
        self.server_info.as_ref().map(|info| &info.capabilities)
    }

    async fn list_resources(&self) -> Result<Vec<McpResource>> {
        McpClient::list_resources(self).await
    }

    async fn list_resource_templates(&self) -> Result<Vec<McpResourceTemplate>> {
        McpClient::list_resource_templates(self).await
    }

    async fn read_resource(&self, uri: &str) -> Result<ResourcesReadResult> {
        McpClient::read_resource(self, uri).await
    }

    async fn subscribe_resource(&self, uri: &str) -> Result<()> {
        McpClient::subscribe_resource(self, uri).await
    }

    async fn unsubscribe_resource(&self, uri: &str) -> Result<()> {
        McpClient::unsubscribe_resource(self, uri).await
    }

    async fn list_prompts(&self) -> Result<Vec<McpPrompt>> {
        McpClient::list_prompts(self).await
    }

    async fn get_prompt(
        &self,
        name: &str,
        arguments: HashMap<String, String>,
    ) -> Result<PromptsGetResult> {
        McpClient::get_prompt(self, name, arguments).await
    }

    async fn notify_roots_changed(&self) -> Result<()> {
        McpClient::notify_roots_changed(self).await
    }
}

#[cfg(test)]
mod tests {
    use {super::*, std::sync::Arc};

    use {
        async_trait::async_trait,
        serde_json::{Value, json},
    };

    use crate::{error::Result, traits::McpTransport, types::JsonRpcResponse};

    struct StubTransport {
        result: Value,
    }

    #[async_trait]
    impl McpTransport for StubTransport {
        async fn request(&self, _method: &str, _params: Option<Value>) -> Result<JsonRpcResponse> {
            Ok(JsonRpcResponse {
                jsonrpc: "2.0".into(),
                id: json!(1),
                result: Some(self.result.clone()),
                error: None,
            })
        }

        async fn notify(&self, _method: &str, _params: Option<Value>) -> Result<()> {
            Ok(())
        }

        async fn is_alive(&self) -> bool {
            true
        }

        async fn kill(&self) {}
    }

    #[test]
    fn test_client_state_debug() {
        assert_eq!(format!("{:?}", McpClientState::Connected), "Connected");
        assert_eq!(format!("{:?}", McpClientState::Ready), "Ready");
        assert_eq!(
            format!("{:?}", McpClientState::Authenticating),
            "Authenticating"
        );
        assert_eq!(format!("{:?}", McpClientState::Closed), "Closed");
    }

    #[tokio::test]
    async fn test_list_tools_sorts_by_name() {
        let transport = Arc::new(StubTransport {
            result: json!({
                "tools": [
                    {"name": "zeta", "description": "Z", "inputSchema": {"type": "object"}},
                    {"name": "alpha", "description": "A", "inputSchema": {"type": "object"}},
                    {"name": "mu", "description": "M", "inputSchema": {"type": "object"}}
                ]
            }),
        });
        let mut client = McpClient {
            server_name: "test".into(),
            transport,
            state: McpClientState::Ready,
            server_info: None,
            tools: Vec::new(),
        };

        let tools = client.list_tools().await.unwrap();
        let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "mu", "zeta"]);
    }
}
