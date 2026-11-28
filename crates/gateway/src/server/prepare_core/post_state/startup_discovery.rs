//! Background model discovery for providers whose model lists were deferred
//! at startup.

use std::{collections::HashMap, sync::Arc};

use {
    antre_config::{AntreConfig, schema::ProvidersConfig},
    antre_providers::{PendingDiscoveries, ProviderRegistry},
    tracing::{debug, info, warn},
};

use crate::{
    broadcast::{BroadcastOpts, broadcast},
    server::{CoreStartupProfile, helpers::restore_saved_local_llm_models},
    state::GatewayState,
};

/// Headless profiles wait for discovery; others run it in the background.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run(
    profile: CoreStartupProfile,
    startup_discovery_pending: PendingDiscoveries,
    registry: &Arc<tokio::sync::RwLock<ProviderRegistry>>,
    state: &Arc<GatewayState>,
    effective_providers: &ProvidersConfig,
    config: &AntreConfig,
    config_env_overrides: &HashMap<String, String>,
    #[cfg(feature = "local-llm")] local_llm_service: Option<
        &Arc<crate::local_llm_setup::LiveLocalLlmService>,
    >,
) {
    if startup_discovery_pending.is_empty() {
        debug!("startup model discovery skipped, no pending provider discoveries");
    } else {
        let registry_for_startup_discovery = Arc::clone(registry);
        let state_for_startup_discovery = Arc::clone(state);
        let provider_config_for_startup_discovery = effective_providers.clone();
        let provider_config_for_registry_rebuild = provider_config_for_startup_discovery.clone();
        let global_cw_overrides = antre_providers::extract_cw_overrides(&config.models);
        let env_overrides_for_startup_discovery = config_env_overrides.clone();
        #[cfg(feature = "local-llm")]
        let local_llm_svc_for_discovery = local_llm_service.cloned();
        #[cfg(feature = "local-llm")]
        let global_timeout_for_discovery = config
            .providers
            .get("local")
            .or_else(|| config.providers.get("local-llm"))
            .and_then(|e| e.idle_timeout_secs);
        let discovery = async move {
            let startup_discovery_started = std::time::Instant::now();
            let prefetched = match tokio::task::spawn_blocking(move || {
                ProviderRegistry::collect_discoveries(startup_discovery_pending)
            })
            .await
            {
                Ok(prefetched) => prefetched,
                Err(error) => {
                    warn!(
                        error = %error,
                        "startup background model discovery worker failed while collecting results"
                    );
                    return;
                },
            };

            let prefetched_models: usize = prefetched.values().map(Vec::len).sum();
            let mut new_registry = match tokio::task::spawn_blocking(move || {
                ProviderRegistry::from_config_with_prefetched(
                    &provider_config_for_registry_rebuild,
                    &env_overrides_for_startup_discovery,
                    &prefetched,
                    global_cw_overrides.clone(),
                )
            })
            .await
            {
                Ok(new_registry) => new_registry,
                Err(error) => {
                    warn!(
                        error = %error,
                        "startup background model discovery worker failed while rebuilding registry"
                    );
                    return;
                },
            };

            restore_saved_local_llm_models(
                &mut new_registry,
                &provider_config_for_startup_discovery,
            );
            let provider_summary = new_registry.provider_summary();
            let model_count = new_registry.list_models().len();
            {
                let mut reg = registry_for_startup_discovery.write().await;
                *reg = new_registry;
            }

            // Re-populate the lifecycle manager so it tracks the same
            // Arcs the rebuilt registry uses for inference.
            #[cfg(feature = "local-llm")]
            if let Some(svc) = &local_llm_svc_for_discovery {
                svc.lifecycle().clear().await;
                svc.populate_lifecycle(global_timeout_for_discovery).await;
            }

            info!(
                provider_summary = %provider_summary,
                models = model_count,
                prefetched_models,
                elapsed_ms = startup_discovery_started.elapsed().as_millis(),
                "startup background model discovery complete, provider registry updated"
            );

            broadcast(
                &state_for_startup_discovery,
                "models.updated",
                serde_json::json!({
                    "reason": "startup-discovery",
                    "models": model_count,
                    "providerSummary": provider_summary,
                }),
                BroadcastOpts::default(),
            )
            .await;
        };
        if profile.is_headless() {
            discovery.await;
        } else {
            tokio::spawn(discovery);
        }
    }
}
