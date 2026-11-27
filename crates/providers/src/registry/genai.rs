//! Registration of genai-crate backed providers (backups for the native
//! Anthropic and OpenAI providers).

use std::{collections::HashMap, sync::Arc};

use moltis_config::schema::ProvidersConfig;

use super::ProviderRegistry;
#[cfg(feature = "provider-genai")]
use crate::{
    config_helpers::{configured_models_for_provider, resolve_api_key},
    model_capabilities::{ModelCapabilities, ModelInfo},
};

impl ProviderRegistry {
    #[cfg(feature = "provider-genai")]
    pub(crate) fn register_genai_providers(
        &mut self,
        config: &ProvidersConfig,
        env_overrides: &HashMap<String, String>,
    ) {
        use crate::genai_provider;

        // (env_key, provider_config_name, model_id, display_name)
        // Groq and xAI live in the OpenAI-compatible table (tool calling,
        // live discovery); genai only backs the native providers when those
        // are compiled out, and skips ids a native provider already serves.
        let genai_models: &[(&str, &str, &str, &str)] = &[
            (
                "ANTHROPIC_API_KEY",
                "anthropic",
                "claude-sonnet-5-5",
                "Claude Sonnet 5.5 (genai)",
            ),
            (
                "OPENAI_API_KEY",
                "openai",
                "gpt-6.2-sol",
                "GPT-6.2 Sol (genai)",
            ),
        ];

        for &(env_key, provider_name, default_model_id, display_name) in genai_models {
            if !config.is_enabled(provider_name) {
                continue;
            }

            // Use config api_key or fall back to env var.
            let Some(resolved_key) = resolve_api_key(config, provider_name, env_key, env_overrides)
            else {
                continue;
            };

            let model_id = configured_models_for_provider(config, provider_name)
                .into_iter()
                .next()
                .unwrap_or_else(|| default_model_id.to_string());

            // Get alias if configured (for metrics differentiation).
            let alias = config.get(provider_name).and_then(|e| e.alias.clone());
            let genai_provider_name = alias.unwrap_or_else(|| format!("genai/{provider_name}"));
            if self.has_model_any_provider(&model_id) {
                continue;
            }

            let provider = Arc::new(genai_provider::GenaiProvider::new(
                model_id.clone(),
                genai_provider_name.clone(),
                // genai normalizes every adapter to inclusive prompt tokens,
                // including Anthropic's otherwise-exclusive API counters.
                moltis_agents::model::InputTokenAccounting::Inclusive,
                resolved_key,
            ));
            self.register(
                ModelInfo {
                    id: model_id.clone(),
                    provider: genai_provider_name,
                    display_name: display_name.into(),
                    created_at: None,
                    recommended: false,
                    capabilities: ModelCapabilities::infer(&model_id),
                },
                provider,
            );
        }
    }
}
