//! The model the review and consolidation passes call. The gateway adapts
//! its provider registry to this; tests use a canned implementation.

use async_trait::async_trait;

/// One request: a fixed system prompt, then a single user message whose
/// stable part (the library) comes before the part that changes on every
/// call (the conversation), so providers can cache the prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewRequest {
    pub system: &'static str,
    pub user: String,
}

/// A model that answers one request with text.
#[async_trait]
pub trait ReviewModel: Send + Sync {
    /// Registry id of the model, for logs and the ledger reason.
    fn id(&self) -> &str;

    async fn complete(&self, request: &ReviewRequest) -> Result<String, String>;
}
