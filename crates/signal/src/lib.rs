//! Signal channel integration backed by an external `signal-cli` HTTP daemon.

mod attachments;
pub mod client;
pub mod config;
pub mod inbound;
pub mod outbound;
pub mod plugin;
pub mod sse;
pub mod state;
mod voice;

pub use plugin::SignalPlugin;
