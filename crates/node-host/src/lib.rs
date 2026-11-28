//! Headless node host: connects to a gateway as a node and executes commands.
//!
//! Usage: `antre node run --host <gateway> --token <device-token>`
//!
//! The node host establishes a WebSocket connection to the gateway,
//! authenticates with a device token, and handles structured `system.exec.v1` commands
//! by executing them locally.

pub mod error;
pub mod identity;
pub mod runner;
pub mod service;

pub use {
    antre_protocol::SYSTEM_EXEC_COMMAND,
    error::Error,
    identity::NodeIdentity,
    runner::{NodeConfig, NodeHost},
    service::ServiceConfig,
};
