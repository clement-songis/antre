//! Building OpenAI-compatible `custom-*` providers from their config entry.

use moltis_config::schema::ProviderEntry;

use {
    super::ProviderRegistry,
    crate::{
        config_helpers::provider_headers,
        model_capabilities::{ModelCapabilities, extract_cw_overrides},
        openai,
    },
};

/// Custom endpoints often serve these families, which need
/// `reasoning_content` replayed on tool turns.
pub(crate) const CUSTOM_REASONING_CONTENT_MODEL_PREFIXES: &[&str] = &["kimi-", "deepseek-v4"];

impl ProviderRegistry {
    /// An OpenAI-compatible provider for one model of a `custom-*` entry,
    /// with the entry's transport, wire API, tool settings and headers.
    pub(crate) fn custom_openai_provider(
        &self,
        name: &str,
        entry: &ProviderEntry,
        api_key: &secrecy::Secret<String>,
        base_url: &str,
        model_id: String,
        capabilities: ModelCapabilities,
    ) -> openai::OpenAiProvider {
        let mut provider = openai::OpenAiProvider::new_with_name(
            api_key.clone(),
            model_id,
            base_url.to_string(),
            name.to_string(),
        )
        .with_stream_transport(entry.stream_transport)
        .with_cache_retention(entry.cache_retention)
        .with_model_capabilities(capabilities)
        .with_capabilities(openai::OpenAiProviderCapabilities {
            reasoning_content_model_prefixes: CUSTOM_REASONING_CONTENT_MODEL_PREFIXES,
            qwen_models_require_single_leading_system: true,
            ..openai::OpenAiProviderCapabilities::DEFAULT
        })
        .with_context_window_overrides(
            self.global_cw_overrides.clone(),
            extract_cw_overrides(&entry.model_overrides),
        )
        .with_extra_headers(provider_headers(entry))
        .with_probe_timeout_secs(entry.probe_timeout_secs);
        if !matches!(entry.wire_api, moltis_config::WireApi::ChatCompletions) {
            provider = provider.with_wire_api(entry.wire_api);
        }
        if !matches!(entry.tool_mode, moltis_config::ToolMode::Auto) {
            provider = provider.with_tool_mode(entry.tool_mode);
        }
        if let Some(strict) = entry.strict_tools {
            provider = provider.with_strict_tools(strict);
        }
        provider
    }
}
