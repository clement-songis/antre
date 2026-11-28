//! The review model: `learning.model`, else the catalog's utility model for
//! the session's provider, else the session's own model.

use std::sync::Arc;

use {
    async_trait::async_trait,
    moltis_agents::model::{ChatMessage, LlmProvider},
    moltis_learning::{ReviewModel, ReviewRequest},
    moltis_providers::{
        ModelInfo,
        model_id::{namespaced_model_id, raw_model_id},
    },
};

use crate::state::GatewayState;

/// A registry provider used as the review model.
pub(crate) struct ProviderModel {
    provider: Arc<dyn LlmProvider>,
    id: String,
}

impl ProviderModel {
    /// Context window of the model, in tokens.
    pub(crate) fn context_window(&self) -> u32 {
        self.provider.context_window()
    }
}

#[async_trait]
impl ReviewModel for ProviderModel {
    fn id(&self) -> &str {
        &self.id
    }

    async fn complete(&self, request: &ReviewRequest) -> Result<String, String> {
        let messages = [
            ChatMessage::system(request.system),
            ChatMessage::user(&request.user),
        ];
        self.provider
            .complete(&messages, &[])
            .await
            .map(|response| response.text.unwrap_or_default())
            .map_err(|error| error.to_string())
    }
}

/// Registry id of the utility model for the session model's provider, when
/// the catalog curates one and the registry has it.
pub(crate) fn utility_model_id(models: &[ModelInfo], session_model: &str) -> Option<String> {
    let raw = raw_model_id(session_model);
    let provider = models
        .iter()
        .find(|model| model.id == session_model || raw_model_id(&model.id) == raw)?
        .provider
        .clone();
    let pick = moltis_model_catalog::global::pick_utility_chat(&provider)?;
    let id = namespaced_model_id(&provider, &pick.spec.id);
    models.iter().any(|model| model.id == id).then_some(id)
}

/// Pick the review model for a session.
pub(crate) async fn resolve_model(
    state: &Arc<GatewayState>,
    session_model: Option<&str>,
) -> Option<ProviderModel> {
    let inner = state.inner.read().await;
    let registry = inner.llm_providers.as_ref()?.read().await;
    let configured = state
        .config
        .learning
        .model
        .as_deref()
        .filter(|id| !id.trim().is_empty());
    if let Some(id) = configured {
        if let Some(provider) = registry.get(id) {
            return Some(ProviderModel {
                provider,
                id: id.to_string(),
            });
        }
        tracing::warn!(model = id, "learning.model is not available; falling back");
    }
    let candidates = session_model
        .and_then(|session| utility_model_id(registry.list_models(), session))
        .into_iter()
        .chain(session_model.map(str::to_string));
    for id in candidates {
        if let Some(provider) = registry.get(&id) {
            return Some(ProviderModel { provider, id });
        }
    }
    let provider = registry.first()?;
    let id = provider.id().to_string();
    Some(ProviderModel { provider, id })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use {super::*, moltis_providers::ModelCapabilities};

    fn info(provider: &str, model: &str) -> ModelInfo {
        ModelInfo {
            id: namespaced_model_id(provider, model),
            provider: provider.into(),
            display_name: model.into(),
            created_at: None,
            recommended: false,
            capabilities: ModelCapabilities::default(),
        }
    }

    #[test]
    fn picks_the_providers_utility_model_when_registered() {
        let models = [
            info("anthropic", "claude-opus-5-5"),
            info("anthropic", "claude-haiku-5-5"),
            info("moonshot", "kimi-k3"),
        ];
        assert_eq!(
            utility_model_id(&models, "anthropic::claude-opus-5-5").as_deref(),
            Some("anthropic::claude-haiku-5-5")
        );
        assert_eq!(
            utility_model_id(&models, "claude-opus-5-5@reasoning-high").as_deref(),
            Some("anthropic::claude-haiku-5-5")
        );
        // No curated utility model for this provider.
        assert_eq!(utility_model_id(&models, "moonshot::kimi-k3"), None);
        // Curated but not registered.
        assert_eq!(
            utility_model_id(&models[..1], "anthropic::claude-opus-5-5"),
            None
        );
        assert_eq!(utility_model_id(&models, "unknown::model"), None);
    }
}
