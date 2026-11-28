//! Glue between the provider registry and the model catalog: config
//! overrides and live discovery feed the catalog, registration reads it back.

use std::{collections::HashMap, path::PathBuf, sync::OnceLock};

use antre_model_catalog::{
    ModelKind, ModelSpec, ResolvedModel, Source,
    catalog::ANY_PROVIDER,
    global,
    live_cache::{LiveCache, LiveSnapshot},
};

use crate::{
    discovered_model::DiscoveredModel, model_capabilities::ModelInfo, model_id::capability_model_id,
};

static LIVE_CACHE: OnceLock<LiveCache> = OnceLock::new();

/// Persist live `/models` results under `dir` (the gateway passes
/// `data_dir()/model-catalog/live`). Without it nothing is written.
pub fn set_live_cache_dir(dir: PathBuf) {
    let _ = LIVE_CACHE.set(LiveCache::new(dir));
}

/// Resolve a registered model against the global catalog.
pub(crate) fn resolve(
    provider: &str,
    model_id: &str,
    heuristic: Option<&ModelSpec>,
) -> ResolvedModel {
    let id = capability_model_id(model_id);
    global::read(|catalog| catalog.resolve(provider, id, heuristic))
}

/// Feed `[models.<id>]` and `[providers.<name>.model_overrides.<id>]`
/// context windows into the catalog's override layer.
pub(crate) fn record_context_overrides(
    global_overrides: &HashMap<String, u32>,
    provider_overrides: &[(String, HashMap<String, u32>)],
) {
    let to_rows = |overrides: &HashMap<String, u32>| -> Vec<ModelSpec> {
        overrides
            .iter()
            .map(|(id, window)| ModelSpec {
                context_window: Some(*window),
                ..ModelSpec::new(id.clone())
            })
            .collect()
    };
    global::write(|catalog| {
        catalog.set_provider_rows(Source::Override, ANY_PROVIDER, to_rows(global_overrides));
        for (provider, overrides) in provider_overrides {
            for row in to_rows(overrides) {
                catalog.upsert_provider_row(Source::Override, provider, row);
            }
        }
    });
}

/// Feed statically declared models (`models` objects and `models_file`
/// rows) into the override layer, replacing the provider's previous rows.
/// Call before [`record_context_overrides`] so explicit context-window
/// overrides win.
pub(crate) fn record_static_models(config: &antre_config::schema::ProvidersConfig) {
    global::write(|catalog| {
        for (provider, entry) in &config.providers {
            let rows: Vec<ModelSpec> = crate::config_helpers::static_model_specs(entry)
                .into_iter()
                .filter(|spec| !spec.is_empty())
                .collect();
            catalog.set_provider_rows(Source::Override, provider, rows);
        }
    });
}

static CATALOG_MISMATCHES: OnceLock<std::sync::Mutex<HashMap<String, CatalogMismatch>>> =
    OnceLock::new();

/// Static models a `live+static` provider no longer serves, and served
/// models it has no static entry for, for the providers settings page.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogMismatch {
    pub missing: Vec<String>,
    pub unknown: Vec<String>,
}

fn mismatches() -> &'static std::sync::Mutex<HashMap<String, CatalogMismatch>> {
    CATALOG_MISMATCHES.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

pub(crate) fn record_catalog_mismatch(
    provider: &str,
    merged: &antre_model_catalog::static_models::MergedModels,
) {
    let mismatch = CatalogMismatch {
        missing: merged.missing.clone(),
        unknown: merged.unknown.clone(),
    };
    let mut map = mismatches().lock().unwrap_or_else(|e| e.into_inner());
    if mismatch == CatalogMismatch::default() {
        map.remove(provider);
    } else {
        map.insert(provider.to_string(), mismatch);
    }
}

/// The last `live+static` mismatch recorded for a provider.
#[must_use]
pub fn catalog_mismatch(provider: &str) -> Option<CatalogMismatch> {
    mismatches()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(provider)
        .cloned()
}

/// Let `custom-*` providers inherit metadata from the models.dev provider
/// that publishes exactly their base URL (GreenPT, CompactifAI and the like
/// list ids only on `/models`).
pub(crate) fn record_custom_aliases(config: &antre_config::schema::ProvidersConfig) {
    global::write(|catalog| {
        for (name, entry) in &config.providers {
            if !name.starts_with("custom-") {
                continue;
            }
            let Some(base_url) = entry.base_url.as_deref() else {
                continue;
            };
            if let Some(models_dev) = catalog.models_dev_provider_for_base_url(base_url) {
                tracing::debug!(provider = %name, models_dev = %models_dev, "custom provider matched a models.dev provider");
                catalog.set_provider_alias(name, &models_dev);
            }
        }
    });
}

/// Record a provider's live discovery result in the catalog and on disk.
pub(crate) fn record_live_models(provider: &str, models: &[DiscoveredModel]) {
    if models.is_empty() {
        return;
    }
    let rows: Vec<ModelSpec> = models.iter().map(|model| model.metadata.clone()).collect();
    if let Some(cache) = LIVE_CACHE.get() {
        let snapshot = LiveSnapshot {
            fetched_at: time::OffsetDateTime::now_utc(),
            models: rows.clone(),
        };
        if let Err(error) = cache.save(provider, &snapshot) {
            tracing::debug!(provider, %error, "failed to persist live model list");
        }
    }
    global::write(|catalog| catalog.set_provider_rows(Source::Live, provider, rows));
}

/// Probe the context window of every model of `custom-*` providers with
/// `probe_context_window = true`, record the results in the catalog's probe
/// layer and persist them next to the live cache. Earlier results are
/// loaded first, so a restart does not wait for the probe.
pub async fn run_context_probes(
    config: antre_config::schema::ProvidersConfig,
    models: Vec<(String, String)>,
) {
    for (provider, entry) in &config.providers {
        if !entry.probe_context_window || !provider.starts_with("custom-") {
            continue;
        }
        let cache_key = format!("probe-{provider}");
        if let Some(snapshot) = LIVE_CACHE
            .get()
            .and_then(|cache| cache.load(&cache_key).ok().flatten())
        {
            global::write(|catalog| {
                catalog.set_provider_rows(Source::Probe, provider, snapshot.models)
            });
        }
        let Some((api_key, base_url)) = crate::config_helpers::custom_openai_endpoint(entry) else {
            continue;
        };
        let headers = crate::config_helpers::provider_headers(entry);
        let mut rows = Vec::new();
        for (_, model) in models.iter().filter(|(p, _)| p == provider) {
            let probed = crate::context_probe::probe_context_window(
                crate::shared_http_client(),
                &base_url,
                &api_key,
                headers.clone(),
                model,
            )
            .await;
            if let Some(window) = probed {
                tracing::info!(provider = %provider, model = %model, window, "probed context window");
                rows.push(ModelSpec {
                    context_window: Some(window),
                    ..ModelSpec::new(model.clone())
                });
            }
        }
        if rows.is_empty() {
            continue;
        }
        if let Some(cache) = LIVE_CACHE.get() {
            let snapshot = LiveSnapshot {
                fetched_at: time::OffsetDateTime::now_utc(),
                models: rows.clone(),
            };
            if let Err(error) = cache.save(&cache_key, &snapshot) {
                tracing::debug!(%error, "failed to persist probe results");
            }
        }
        global::write(|catalog| catalog.set_provider_rows(Source::Probe, provider, rows));
    }
}

/// Last good live results for providers whose discovery did not answer this
/// time, so they keep their real model list instead of the fallback.
pub(crate) fn cached_discoveries(
    providers: &[String],
    fresh: &HashMap<String, Vec<DiscoveredModel>>,
) -> HashMap<String, Vec<DiscoveredModel>> {
    let Some(cache) = LIVE_CACHE.get() else {
        return HashMap::new();
    };
    providers
        .iter()
        .filter(|provider| !fresh.contains_key(*provider))
        .filter_map(|provider| {
            let snapshot = cache.load(provider).ok().flatten()?;
            let models = snapshot
                .models
                .into_iter()
                .map(|spec| {
                    let name = spec.display_name.clone().unwrap_or_else(|| spec.id.clone());
                    DiscoveredModel::new(spec.id.clone(), name).with_metadata(spec)
                })
                .collect();
            Some((provider.clone(), models))
        })
        .collect()
}

/// Catalog details pickers show next to a registered model.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalogDetails {
    /// Id sent to the API, without the provider namespace.
    pub wire_id: String,
    /// Date or snapshot suffix split off the id ("0925", "2026-01-23").
    pub version_label: Option<String>,
    /// Context window from a real source; `None` means unknown.
    pub context_window: Option<u32>,
    pub max_output: Option<u32>,
    pub deprecated: bool,
    /// `YYYY-MM-DD`.
    pub retires_at: Option<String>,
    /// For `live+static` providers: static models no longer served and
    /// served models without a static entry.
    pub provider_mismatch: Option<CatalogMismatch>,
}

/// Look up the details of a registered model.
#[must_use]
pub fn model_catalog_details(info: &ModelInfo) -> ModelCatalogDetails {
    let raw = crate::model_id::raw_model_id(&info.id);
    let wire_id = crate::model_id::split_reasoning_suffix(raw).0.to_string();
    let resolved = resolve(&info.provider, &wire_id, None);
    let date_format = time::macros::format_description!("[year]-[month]-[day]");
    ModelCatalogDetails {
        version_label: antre_model_catalog::prettify_id(&wire_id).version,
        context_window: resolved.known_context_window(),
        max_output: resolved.spec.max_output,
        deprecated: resolved.status().is_deprecated(),
        retires_at: resolved
            .spec
            .retires_at
            .and_then(|date| date.format(&date_format).ok()),
        provider_mismatch: catalog_mismatch(&info.provider),
        wire_id,
    }
}

/// Apply catalog data to a model being registered: human name, capabilities
/// known from a real source. Returns `false` when the catalog says the model
/// is not a chat model, so it must not be offered in chat pickers.
pub(crate) fn enrich_model_info(info: &mut ModelInfo, raw_id: &str) -> bool {
    let known = resolve(&info.provider, raw_id, None);
    if known.spec.kind.is_some_and(|kind| kind != ModelKind::Chat) {
        tracing::debug!(
            provider = %info.provider,
            model = raw_id,
            kind = ?known.spec.kind,
            "skipping non-chat model from the chat registry"
        );
        return false;
    }

    // Name priority: user override, provider API, catalog, then a name the
    // registration path already chose, then the prettified id.
    let id_echoed = info.display_name.trim().is_empty() || info.display_name == raw_id;
    let authoritative = known
        .name_source
        .is_some_and(|source| source <= Source::Live);
    if let Some(name) = &known.spec.display_name
        && (authoritative || id_echoed)
    {
        info.display_name.clone_from(name);
    } else if id_echoed {
        info.display_name = known.display_name().name;
    }

    if let Some(window) = known.known_context_window() {
        info.capabilities.context_window = window;
    }
    if let Some(tools) = known.spec.tools {
        info.capabilities.tools = tools;
    }
    if known.spec.input_modalities.is_some() {
        info.capabilities.vision = known.spec.accepts_images();
    }
    if let Some(reasoning) = &known.spec.reasoning {
        info.capabilities.reasoning = reasoning.is_supported();
    }
    true
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use {super::*, crate::model_capabilities::ModelCapabilities};

    fn info(provider: &str, id: &str, name: &str) -> ModelInfo {
        ModelInfo {
            id: id.into(),
            provider: provider.into(),
            display_name: name.into(),
            created_at: None,
            recommended: false,
            capabilities: ModelCapabilities::infer(id),
        }
    }

    #[test]
    fn enrich_fills_catalog_name_and_window() {
        let mut model = info("anthropic", "claude-opus-5-5", "claude-opus-5-5");
        assert!(enrich_model_info(&mut model, "claude-opus-5-5"));
        assert_eq!(model.display_name, "Claude Opus 5.5");
        assert_eq!(model.capabilities.context_window, 1_000_000);
    }

    #[test]
    fn enrich_prettifies_unknown_ids_and_keeps_given_names() {
        let mut unknown = info("custom-box", "glm-4.6-air-0925", "glm-4.6-air-0925");
        assert!(enrich_model_info(&mut unknown, "glm-4.6-air-0925"));
        assert_eq!(unknown.display_name, "GLM 4.6 Air");

        let mut named = info("custom-box", "my-model", "My Model");
        assert!(enrich_model_info(&mut named, "my-model"));
        assert_eq!(named.display_name, "My Model");
    }

    #[test]
    fn details_expose_wire_id_version_and_known_window() {
        let model = info("zai", "zai::glm-4.6-air-0925", "GLM 4.6 Air");
        let details = model_catalog_details(&model);
        assert_eq!(details.wire_id, "glm-4.6-air-0925");
        assert_eq!(details.version_label.as_deref(), Some("0925"));
        assert_eq!(details.context_window, None);

        let opus = info(
            "anthropic",
            "anthropic::claude-opus-5-5@reasoning-high",
            "Claude Opus 5.5",
        );
        let details = model_catalog_details(&opus);
        assert_eq!(details.wire_id, "claude-opus-5-5");
        assert_eq!(details.context_window, Some(1_000_000));
        assert!(!details.deprecated);
        let json = serde_json::to_value(&details).unwrap();
        assert_eq!(json["wireId"], "claude-opus-5-5");
    }

    #[test]
    fn enrich_rejects_non_chat_models() {
        let mut image = info("openai", "gpt-image-2.6-flare", "gpt-image-2.6-flare");
        assert!(!enrich_model_info(&mut image, "gpt-image-2.6-flare"));
    }

    #[test]
    fn overrides_win_over_catalog() {
        record_context_overrides(
            &HashMap::from([("override-test-model".to_string(), 42_000)]),
            &[(
                "anthropic".to_string(),
                HashMap::from([("claude-sonnet-5-5".to_string(), 64_000)]),
            )],
        );
        assert_eq!(
            resolve("custom-x", "override-test-model", None).known_context_window(),
            Some(42_000)
        );
        let sonnet = resolve("anthropic", "claude-sonnet-5-5", None);
        assert_eq!(sonnet.known_context_window(), Some(64_000));
        assert_eq!(sonnet.context_source, Some(Source::Override));
    }

    #[test]
    fn static_models_feed_overrides_and_context_overrides_win() {
        let mut config = antre_config::schema::ProvidersConfig::default();
        config.providers.insert(
            "custom-static-test".into(),
            antre_config::schema::ProviderEntry {
                models: vec![
                    "bare-id".into(),
                    antre_model_catalog::ModelEntry::Spec(Box::new(ModelSpec {
                        display_name: Some("Qwen Box".into()),
                        context_window: Some(131_072),
                        kind: Some(ModelKind::Chat),
                        ..ModelSpec::new("qwen-box")
                    })),
                    antre_model_catalog::ModelEntry::Spec(Box::new(ModelSpec {
                        kind: Some(ModelKind::Embedding),
                        ..ModelSpec::new("embed-box")
                    })),
                ],
                ..Default::default()
            },
        );
        record_static_models(&config);
        let qwen = resolve("custom-static-test", "qwen-box", None);
        assert_eq!(qwen.known_context_window(), Some(131_072));
        assert_eq!(qwen.context_source, Some(Source::Override));

        let mut embed = info("custom-static-test", "embed-box", "embed-box");
        assert!(!enrich_model_info(&mut embed, "embed-box"));
        let mut named = info("custom-static-test", "qwen-box", "qwen-box");
        assert!(enrich_model_info(&mut named, "qwen-box"));
        assert_eq!(named.display_name, "Qwen Box");

        record_context_overrides(&HashMap::new(), &[(
            "custom-static-test".to_string(),
            HashMap::from([("qwen-box".to_string(), 65_536)]),
        )]);
        let qwen = resolve("custom-static-test", "qwen-box", None);
        assert_eq!(qwen.known_context_window(), Some(65_536));
        assert_eq!(qwen.spec.display_name.as_deref(), Some("Qwen Box"));
    }

    #[test]
    fn catalog_mismatches_are_recorded_and_cleared() {
        let merged =
            antre_model_catalog::static_models::merge_live_and_static(&["a".to_string()], &[
                "b".to_string()
            ]);
        record_catalog_mismatch("custom-mismatch-test", &merged);
        let mismatch = catalog_mismatch("custom-mismatch-test").unwrap();
        assert_eq!(mismatch.missing, ["a"]);
        assert_eq!(mismatch.unknown, ["b"]);
        let clean =
            antre_model_catalog::static_models::merge_live_and_static(&["a".to_string()], &[
                "a".to_string()
            ]);
        record_catalog_mismatch("custom-mismatch-test", &clean);
        assert!(catalog_mismatch("custom-mismatch-test").is_none());
    }

    #[test]
    fn live_rows_feed_the_catalog_and_the_disk_cache() {
        let models = vec![
            DiscoveredModel::new("live-only-model", "live-only-model").with_metadata(ModelSpec {
                context_window: Some(77_000),
                display_name: Some("Live Only".into()),
                ..ModelSpec::default()
            }),
        ];
        record_live_models("live-test-provider", &models);
        let resolved = resolve("live-test-provider", "live-only-model", None);
        assert_eq!(resolved.known_context_window(), Some(77_000));
        assert_eq!(resolved.context_source, Some(Source::Live));
        record_live_models("live-test-provider", &[]);
        assert_eq!(
            resolve("live-test-provider", "live-only-model", None).known_context_window(),
            Some(77_000)
        );
    }
}
