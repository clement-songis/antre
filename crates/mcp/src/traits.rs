//! Trait abstractions for MCP transport and client layers.
//!
//! These traits allow swapping the underlying MCP implementation (e.g. stdio vs rmcp-sdk)
//! without changing call sites in `manager.rs`, `tool_bridge.rs`, or `mcp_service.rs`.

use std::{collections::HashMap, sync::Arc};

use {async_trait::async_trait, serde_json::Value};

use crate::{
    client::McpClientState,
    error::{Error, Result},
    incoming::IncomingDispatcher,
    protocol::{
        McpPrompt, McpResource, McpResourceTemplate, PromptsGetResult, ResourcesReadResult,
    },
    types::{JsonRpcResponse, McpToolDef, ServerCapabilities, ToolsCallResult},
};

fn unsupported(feature: &str) -> Error {
    Error::message(format!("this MCP client does not support {feature}"))
}

/// Transport layer for MCP communication (JSON-RPC).
///
/// `StdioTransport` implements this over stdin/stdout of a child process.
/// A future HTTP/SSE transport (e.g. via rmcp-sdk) would also implement this trait.
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// Send a JSON-RPC request and wait for the response.
    async fn request(&self, method: &str, params: Option<Value>) -> Result<JsonRpcResponse>;

    /// Send a JSON-RPC notification (no response expected).
    async fn notify(&self, method: &str, params: Option<Value>) -> Result<()>;

    /// Check if the underlying connection/process is still alive.
    async fn is_alive(&self) -> bool;

    /// Kill/close the underlying connection/process.
    async fn kill(&self);

    /// Install the handler for server-to-client requests and notifications.
    fn set_dispatcher(&self, _dispatcher: Arc<dyn IncomingDispatcher>) {}

    /// Whether the server can reach the client with requests on this
    /// transport. Capabilities that need it are only advertised when true.
    fn accepts_server_requests(&self) -> bool {
        false
    }

    /// Record the protocol version negotiated during `initialize`.
    fn set_protocol_version(&self, _version: &str) {}

    /// Open the long-lived server-to-client stream, when the transport has one.
    async fn start_server_stream(&self) {}
}

/// Client-level abstraction for an MCP server connection.
///
/// `McpClient` implements this over `StdioTransport`. A future `RmcpClient`
/// wrapper would also implement this trait.
#[async_trait]
pub trait McpClientTrait: Send + Sync {
    /// The display name of the connected server.
    fn server_name(&self) -> &str;

    /// Current connection state.
    fn state(&self) -> McpClientState;

    /// Cached tool definitions (call `list_tools` first to populate).
    fn tools(&self) -> &[McpToolDef];

    /// Fetch the list of tools from the server, caching the result.
    async fn list_tools(&mut self) -> Result<&[McpToolDef]>;

    /// Call a tool on the server.
    async fn call_tool(&self, name: &str, arguments: Value) -> Result<ToolsCallResult>;

    /// Check if the server process/connection is still alive.
    async fn is_alive(&self) -> bool;

    /// Shut down the server connection.
    async fn shutdown(&mut self);

    /// Capabilities the server announced during `initialize`.
    fn server_capabilities(&self) -> Option<&ServerCapabilities> {
        None
    }

    /// All resources (every page of `resources/list`).
    async fn list_resources(&self) -> Result<Vec<McpResource>> {
        Err(unsupported("resources"))
    }

    /// All resource templates (every page of `resources/templates/list`).
    async fn list_resource_templates(&self) -> Result<Vec<McpResourceTemplate>> {
        Err(unsupported("resources"))
    }

    async fn read_resource(&self, _uri: &str) -> Result<ResourcesReadResult> {
        Err(unsupported("resources"))
    }

    async fn subscribe_resource(&self, _uri: &str) -> Result<()> {
        Err(unsupported("resource subscriptions"))
    }

    async fn unsubscribe_resource(&self, _uri: &str) -> Result<()> {
        Err(unsupported("resource subscriptions"))
    }

    /// All prompts (every page of `prompts/list`).
    async fn list_prompts(&self) -> Result<Vec<McpPrompt>> {
        Err(unsupported("prompts"))
    }

    async fn get_prompt(
        &self,
        _name: &str,
        _arguments: HashMap<String, String>,
    ) -> Result<PromptsGetResult> {
        Err(unsupported("prompts"))
    }

    /// Tell the server the roots list changed.
    async fn notify_roots_changed(&self) -> Result<()> {
        Ok(())
    }
}
