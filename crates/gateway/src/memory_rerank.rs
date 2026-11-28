//! Build the memory search reranker from `[memory]` config.

use std::{collections::HashMap, sync::Arc};

use {
    async_trait::async_trait,
    moltis_agents::model::ChatMessage,
    moltis_config::{MemoryReranker, schema::MemoryEmbeddingConfig},
    moltis_memory::{
        error::{Error as MemoryError, Result as MemoryResult},
        reranking::{LlmClient, LlmReranker, RerankerProvider},
        reranking_api::ApiReranker,
    },
    moltis_model_catalog::{ModelKind, global},
    moltis_providers::ProviderRegistry,
    secrecy::Secret,
    tokio::sync::RwLock,
    tracing::{info, warn},
};

/// Scores passages with a chat model from the registry: the configured
/// `provider::model`, else the first registered model.
struct RegistryLlmClient {
    registry: Arc<RwLock<ProviderRegistry>>,
    model: Option<String>,
}

#[async_trait]
impl LlmClient for RegistryLlmClient {
    async fn complete(&self, prompt: &str, model: Option<&str>) -> MemoryResult<String> {
        let provider = {
            let registry = self.registry.read().await;
            match model.or(self.model.as_deref()) {
                Some(id) => registry.get(id),
                None => registry.first(),
            }
        }
        .ok_or_else(|| MemoryError::Reranking("no chat model available for reranking".into()))?;
        let response = provider
            .complete(&[ChatMessage::user(prompt)], &[])
            .await
            .map_err(|e| MemoryError::Reranking(e.to_string()))?;
        Ok(response.text.unwrap_or_default())
    }
}

/// The configured reranker: `memory.reranker`, or `llm` when only
/// `llm_reranking = true` is set. `None` when reranking is off or cannot be
/// set up (logged).
pub(crate) fn build_reranker(
    config: &MemoryEmbeddingConfig,
    registry: &Arc<RwLock<ProviderRegistry>>,
    env_overrides: &HashMap<String, String>,
) -> Option<Arc<dyn RerankerProvider>> {
    let kind = config
        .reranker
        .or_else(|| config.llm_reranking.then_some(MemoryReranker::Llm))?;
    if kind == MemoryReranker::Llm {
        info!(model = ?config.rerank_model, "memory: reranking search results with a chat model");
        return Some(Arc::new(LlmReranker::new(Arc::new(RegistryLlmClient {
            registry: Arc::clone(registry),
            model: config.rerank_model.clone(),
        }))));
    }
    let Some(model) = config.rerank_model.clone().or_else(|| {
        global::pick_role(kind.as_str(), ModelKind::Rerank).map(|model| model.id().to_string())
    }) else {
        warn!(
            reranker = kind.as_str(),
            "memory: no rerank model known for this reranker, set memory.rerank_model"
        );
        return None;
    };
    let api_key = config.rerank_api_key.clone().or_else(|| {
        kind.env_key()
            .and_then(|key| moltis_config::env_value_with_overrides(env_overrides, key))
            .map(Secret::new)
    });
    if api_key.is_none() && kind != MemoryReranker::Custom {
        warn!(
            reranker = kind.as_str(),
            "memory: rerank API key missing, set memory.rerank_api_key or {}",
            kind.env_key().unwrap_or_default()
        );
        return None;
    }
    let Some(reranker) = ApiReranker::new(
        kind,
        config.rerank_base_url.as_deref(),
        api_key,
        model.clone(),
    ) else {
        warn!(
            reranker = kind.as_str(),
            "memory: rerank base URL missing, set memory.rerank_base_url"
        );
        return None;
    };
    info!(reranker = kind.as_str(), %model, "memory: reranking search results");
    Some(Arc::new(reranker))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn registry() -> Arc<RwLock<ProviderRegistry>> {
        Arc::new(RwLock::new(ProviderRegistry::empty()))
    }

    #[test]
    fn off_unless_configured() {
        let config = MemoryEmbeddingConfig::default();
        assert!(build_reranker(&config, &registry(), &HashMap::new()).is_none());
    }

    #[test]
    fn llm_reranking_flag_builds_the_llm_reranker() {
        let config = MemoryEmbeddingConfig {
            llm_reranking: true,
            ..Default::default()
        };
        assert!(build_reranker(&config, &registry(), &HashMap::new()).is_some());
    }

    #[test]
    fn api_reranker_takes_the_catalog_model_and_env_key() {
        let config = MemoryEmbeddingConfig {
            reranker: Some(MemoryReranker::Jina),
            ..Default::default()
        };
        let env = HashMap::from([("JINA_API_KEY".to_string(), "jina-test".to_string())]);
        assert!(build_reranker(&config, &registry(), &env).is_some());
    }

    #[test]
    fn api_reranker_without_key_is_skipped() {
        if std::env::var_os("VOYAGE_API_KEY").is_some() {
            return;
        }
        let config = MemoryEmbeddingConfig {
            reranker: Some(MemoryReranker::Voyage),
            ..Default::default()
        };
        assert!(build_reranker(&config, &registry(), &HashMap::new()).is_none());
    }

    #[test]
    fn custom_needs_a_model_and_base_url() {
        let without_model = MemoryEmbeddingConfig {
            reranker: Some(MemoryReranker::Custom),
            rerank_base_url: Some("http://127.0.0.1:8080/v1".into()),
            ..Default::default()
        };
        assert!(build_reranker(&without_model, &registry(), &HashMap::new()).is_none());

        let without_url = MemoryEmbeddingConfig {
            reranker: Some(MemoryReranker::Custom),
            rerank_model: Some("bge-reranker-v2-m3".into()),
            ..Default::default()
        };
        assert!(build_reranker(&without_url, &registry(), &HashMap::new()).is_none());

        let complete = MemoryEmbeddingConfig {
            rerank_base_url: Some("http://127.0.0.1:8080/v1".into()),
            ..without_url
        };
        assert!(build_reranker(&complete, &registry(), &HashMap::new()).is_some());
    }

    #[tokio::test]
    async fn llm_client_without_models_reports_an_error() {
        let client = RegistryLlmClient {
            registry: registry(),
            model: None,
        };
        assert!(client.complete("score", None).await.is_err());
    }
}
