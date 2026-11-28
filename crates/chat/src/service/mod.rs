//! `LiveChatService` struct, constructors, and helper methods.

mod admission;
mod chat_impl;
mod queue;
mod types;

use types::QueuedMessage;
pub(crate) use types::{
    ActiveAssistantDraft, EventForwarder, build_persisted_assistant_message,
    build_tool_call_assistant_message, commit_terminal_run, persist_tool_history_pair,
};
pub use types::{ActiveToolCall, LiveChatService};
