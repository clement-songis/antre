//! Antre as an ACP *agent*.
//!
//! Antre has long been an ACP client (`antre-external-agents` spawns and
//! drives `codex-acp`, `claude-agent-acp`, and friends). This crate is the
//! inverse: it lets any ACP client — Zed, `buzz-acp`, a bespoke harness — drive
//! Antre the way it would drive any other agent, with Antre's sessions,
//! memory, tool policy, and providers staying behind the protocol.
//!
//! ACP is a *surface*, not a channel: there is no correspondent to allowlist
//! and no account to configure, so it sits beside the Web UI and GraphQL rather
//! than alongside Telegram or Nostr.
//!
//! # Layout
//!
//! - [`backend`] — the `Send` seam Antre implements ([`AcpBackend`])
//! - [`agent`] — the `!Send` protocol handler ([`AntreAgent`])
//! - [`session`] — ACP session ids ↔ namespaced Antre session keys
//! - [`server`] — transport wiring ([`server::run_stdio`])
//! - [`echo`] — a backend-free implementation for smoke tests

pub mod agent;
pub mod backend;
pub mod echo;
pub mod server;
pub mod session;
pub mod setup;

pub use crate::{
    agent::AntreAgent,
    backend::{AcpBackend, BackendCapabilities, SessionNotFound, TurnUpdates, validate_history},
    echo::EchoBackend,
    server::{run_stdio, serve},
    session::{ACP_SESSION_NAMESPACE, SessionKey, SessionRegistry},
    setup::SessionSetup,
};
