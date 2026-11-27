//! MCP (Model Context Protocol) client support for moltis.
//!
//! This crate provides:
//! - JSON-RPC 2.0 over stdio transport (`transport`)
//! - MCP client for protocol handshake, tools, resources and prompts (`client`)
//! - Server-initiated requests: roots, sampling, elicitation (`host`)
//! - Tool bridge adapting MCP tools to the agent tool interface (`tool_bridge`)
//! - Server lifecycle management (`manager`)
//! - Persisted server registry (`registry`)
//!
//! Remote HTTP/SSE servers keep secret-bearing values (URLs, header values)
//! in secret-aware types and only expose sanitized display projections.

pub mod auth;
pub mod client;
mod client_features;
mod client_slot;
pub mod config_parsing;
pub mod elicitation;
pub mod error;
pub mod host;
pub mod incoming;
pub mod legacy_sse_transport;
pub mod manager;
mod manager_features;
pub mod protocol;
pub mod registry;
pub mod remote;
pub mod sampling;
mod sse_stream;
pub mod sse_transport;
pub mod tool_bridge;
pub mod traits;
pub mod transport;
pub mod types;

pub use {
    auth::{McpAuthProvider, McpAuthState, McpOAuthOverride, McpOAuthProvider, SharedAuthProvider},
    client::{ClientSetup, McpClient, McpClientState},
    config_parsing::{merge_env_overrides, parse_server_config},
    elicitation::{ElicitationSchema, ElicitationView, PendingElicitations},
    error::{Context, Error, Result},
    host::{
        ElicitationRequest, HostContext, McpHost, McpToolApprover, SamplingRequest,
        SamplingResponse, ServerPolicy, ToolApprovalRequest, TrustTier,
    },
    manager::McpManager,
    manager_features::McpServerSummary,
    registry::{McpOAuthConfig, McpRegistry, McpServerConfig, TransportType},
    tool_bridge::{McpAgentTool, McpToolBridge},
    traits::{McpClientTrait, McpTransport},
    transport::StdioLaunchOptions,
    types::{McpManagerError, McpTransportError},
};
