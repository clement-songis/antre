//! Build test providers through the same static registration path as the daemon.

use {
    antre_agents::model::LlmProvider,
    antre_config::schema::{ProviderEntry, ProvidersConfig},
    antre_providers::ProviderRegistry,
    secrecy::Secret,
    std::{collections::HashMap, sync::Arc},
};

pub fn registered_provider(
    name: &str,
    model: &str,
    base_url: &str,
    key: Secret<String>,
) -> Arc<dyn LlmProvider> {
    let mut config = ProvidersConfig {
        offered: vec![name.into()],
        ..Default::default()
    };
    config.providers.insert(name.into(), ProviderEntry {
        api_key: Some(key),
        base_url: Some(base_url.into()),
        models: vec![model.into()],
        ..Default::default()
    });
    ProviderRegistry::from_config_with_static_catalogs(&config, &HashMap::new(), HashMap::new())
        .get(&format!("{name}::{model}"))
        .expect("test model must be registered")
}
