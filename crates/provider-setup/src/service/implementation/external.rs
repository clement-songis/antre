//! `providers.import_external`: copy another tool's credentials into Moltis's
//! own store, only when the user asks for it.

use {
    moltis_config::schema::ExternalCredentialSource,
    moltis_service_traits::{ServiceError, ServiceResult},
    serde_json::Value,
    tracing::info,
};

use {
    super::{LiveProviderSetupService, oauth::ensure_provider_enabled},
    crate::{config_helpers::set_provider_enabled_in_config, oauth::import_codex_cli_tokens},
};

impl LiveProviderSetupService {
    pub(super) async fn import_external_inner(&self, params: Value) -> ServiceResult {
        let source: ExternalCredentialSource = params
            .get("source")
            .cloned()
            .ok_or_else(|| "missing 'source' parameter".to_string())
            .and_then(|value| {
                serde_json::from_value(value)
                    .map_err(|_| "unknown credential source (expected \"codex-cli\")".to_string())
            })?;
        let (provider, path) = match source {
            ExternalCredentialSource::CodexCli => (
                "openai-codex",
                import_codex_cli_tokens(&self.token_store).map_err(ServiceError::message)?,
            ),
        };

        set_provider_enabled_in_config(provider, true)?;
        self.set_provider_enabled_in_memory(provider, true);
        let mut effective = self.effective_config();
        ensure_provider_enabled(&mut effective, provider);
        let new_registry = self.build_registry(&effective);
        *self.registry.write().await = new_registry;
        info!(
            provider,
            source = %path.display(),
            "imported external credentials at the user's request"
        );
        Ok(serde_json::json!({ "provider": provider, "imported": true }))
    }
}
