//! The merged catalog: one [`ModelSpec`] per (provider, model) built from
//! several sources, highest priority first, field by field.
//!
//! Priority (see `docs/src/model-catalog.md`):
//! 1. user overrides and static custom providers
//! 2. runtime probe results
//! 3. live provider metadata
//! 4. models.dev refreshed at runtime
//! 5. bundled models.dev snapshot plus Antre's supplement
//! 6. id heuristics (supplied by the caller)
//! 7. provider defaults

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::{
    models_dev::ModelsDevCatalog,
    names::{PrettyName, prettify_id},
    providers::{FIRST_PARTY_MODELS_DEV_IDS, models_dev_ids},
    snapshot::Supplement,
    types::{ModelKind, ModelSpec, ModelStatus},
};

/// Where a piece of metadata came from, highest priority first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Override,
    Probe,
    Live,
    Refreshed,
    Bundled,
    Heuristic,
    ProviderDefault,
}

/// Provider key for override rows that apply to every provider.
pub const ANY_PROVIDER: &str = "*";

/// Sources that hold per-provider rows set by Antre at runtime.
const PROVIDER_SOURCES: [Source; 3] = [Source::Override, Source::Probe, Source::Live];

/// models.dev data indexed by models.dev provider id, then model id.
#[derive(Debug, Clone, Default)]
struct IndexedModelsDev {
    providers: HashMap<String, HashMap<String, ModelSpec>>,
    provider_api: HashMap<String, String>,
}

impl From<ModelsDevCatalog> for IndexedModelsDev {
    fn from(catalog: ModelsDevCatalog) -> Self {
        let providers = catalog
            .providers
            .into_iter()
            .map(|(provider, models)| {
                let by_id = models.into_iter().map(|m| (m.id.clone(), m)).collect();
                (provider, by_id)
            })
            .collect();
        Self {
            providers,
            provider_api: catalog.provider_api,
        }
    }
}

/// `scheme://host/path` lowercased without the trailing slash, for
/// comparing configured base URLs with models.dev's published ones.
fn normalize_base_url(url: &str) -> String {
    let lower = url.trim().to_ascii_lowercase();
    let without_scheme = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
        .unwrap_or(&lower);
    without_scheme.trim_end_matches('/').to_string()
}

impl IndexedModelsDev {
    /// Find metadata for a Antre provider's model: the provider's own
    /// models.dev entries first, then first-party labs by id (a custom
    /// provider serving `kimi-k3` gets Moonshot's metadata). Router-style
    /// `vendor/model` ids also match the bare model id at first-party labs.
    fn lookup(&self, own_ids: &[&str], id: &str) -> Option<&ModelSpec> {
        let own = own_ids
            .iter()
            .find_map(|md| self.providers.get(*md)?.get(id));
        own.or_else(|| self.first_party(id)).or_else(|| {
            id.split_once('/')
                .and_then(|(_, bare)| self.first_party(bare))
        })
    }

    fn first_party(&self, id: &str) -> Option<&ModelSpec> {
        FIRST_PARTY_MODELS_DEV_IDS
            .iter()
            .find_map(|md| self.providers.get(*md)?.get(id))
    }

    fn provider_models<'a>(
        &'a self,
        own_ids: &'a [&'a str],
    ) -> impl Iterator<Item = &'a ModelSpec> {
        own_ids
            .iter()
            .filter_map(|md| self.providers.get(*md))
            .flat_map(HashMap::values)
    }

    fn provider_for_base_url(&self, base_url: &str) -> Option<String> {
        let wanted = normalize_base_url(base_url);
        let mut matches: Vec<&String> = self
            .provider_api
            .iter()
            .filter(|(_, api)| normalize_base_url(api) == wanted)
            .map(|(provider, _)| provider)
            .collect();
        matches.sort();
        matches.first().map(|provider| (*provider).clone())
    }
}

/// A model with every source merged.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResolvedModel {
    pub provider: String,
    pub spec: ModelSpec,
    /// Source of `context_window`, `None` when no source knows it.
    pub context_source: Option<Source>,
    /// Source of `display_name`, `None` when the name is prettified from the id.
    pub name_source: Option<Source>,
}

impl ResolvedModel {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.spec.id
    }

    #[must_use]
    pub fn kind(&self) -> ModelKind {
        self.spec.kind_or_default()
    }

    #[must_use]
    pub fn status(&self) -> ModelStatus {
        self.spec.status_or_default()
    }

    /// Human-readable name: provider or catalog name, else prettified id.
    #[must_use]
    pub fn display_name(&self) -> PrettyName {
        match &self.spec.display_name {
            Some(name) => PrettyName {
                name: name.clone(),
                version: None,
            },
            None => prettify_id(&self.spec.id),
        }
    }

    /// Context window only when a real source (not a guess) knows it.
    #[must_use]
    pub fn known_context_window(&self) -> Option<u32> {
        match self.context_source {
            Some(Source::Heuristic | Source::ProviderDefault) | None => None,
            Some(_) => self.spec.context_window,
        }
    }
}

/// The merged model catalog.
#[derive(Debug, Clone, Default)]
pub struct ModelCatalog {
    provider_rows: HashMap<Source, HashMap<String, HashMap<String, ModelSpec>>>,
    refreshed: Option<IndexedModelsDev>,
    bundled: IndexedModelsDev,
    supplement: HashMap<String, HashMap<String, ModelSpec>>,
    provider_defaults: HashMap<String, ModelSpec>,
    /// Providers the remote catalog may describe. `None` until the registry
    /// reports what is configured.
    configured: Option<HashSet<String>>,
    /// Antre provider -> models.dev provider, for custom providers whose
    /// base URL matches a provider models.dev knows.
    provider_aliases: HashMap<String, String>,
}

impl ModelCatalog {
    /// Empty catalog (tests, or when the bundled data fails to load).
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Catalog seeded with the data compiled into the binary.
    pub fn bundled() -> crate::Result<Self> {
        let mut catalog = Self::default();
        catalog.set_bundled(
            crate::snapshot::bundled_models_dev()?,
            crate::snapshot::bundled_supplement()?,
        );
        Ok(catalog)
    }

    pub fn set_bundled(&mut self, models_dev: ModelsDevCatalog, supplement: Supplement) {
        self.bundled = models_dev.into();
        self.supplement = supplement
            .models
            .into_iter()
            .map(|(provider, models)| {
                (
                    provider,
                    models.into_iter().map(|m| (m.id.clone(), m)).collect(),
                )
            })
            .collect();
        self.provider_defaults = supplement.provider_defaults;
    }

    /// Install a runtime refresh of models.dev.
    pub fn set_refreshed(&mut self, models_dev: ModelsDevCatalog) {
        self.refreshed = Some(models_dev.into());
    }

    /// Restrict what the refreshed remote catalog may describe to the
    /// providers that are configured. A remote catalog never adds providers.
    pub fn set_configured_providers<I, S>(&mut self, providers: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.configured = Some(providers.into_iter().map(Into::into).collect());
    }

    /// Replace a provider's rows for one runtime source (override, probe or
    /// live); other sources are ignored.
    pub fn set_provider_rows(&mut self, source: Source, provider: &str, rows: Vec<ModelSpec>) {
        if !PROVIDER_SOURCES.contains(&source) {
            crate::macros::log_debug!(?source, "ignoring provider rows for a non-runtime source");
            return;
        }
        let by_id = rows.into_iter().map(|m| (m.id.clone(), m)).collect();
        self.provider_rows
            .entry(source)
            .or_default()
            .insert(provider.to_string(), by_id);
    }

    /// Merge one row into a provider's rows for a runtime source.
    pub fn upsert_provider_row(&mut self, source: Source, provider: &str, row: ModelSpec) {
        if !PROVIDER_SOURCES.contains(&source) {
            return;
        }
        let rows = self
            .provider_rows
            .entry(source)
            .or_default()
            .entry(provider.to_string())
            .or_default();
        match rows.get_mut(&row.id) {
            Some(existing) => {
                let mut merged = row;
                merged.fill_from(existing);
                *existing = merged;
            },
            None => {
                rows.insert(row.id.clone(), row);
            },
        }
    }

    /// Rows a runtime source holds for a provider.
    #[must_use]
    pub fn provider_rows(&self, source: Source, provider: &str) -> Vec<ModelSpec> {
        self.provider_rows
            .get(&source)
            .and_then(|providers| providers.get(provider))
            .map(|rows| rows.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Treat a Antre provider (typically `custom-*`) as a models.dev
    /// provider for metadata lookups.
    pub fn set_provider_alias(&mut self, provider: &str, models_dev_provider: &str) {
        self.provider_aliases
            .insert(provider.to_string(), models_dev_provider.to_string());
    }

    /// The models.dev provider publishing `base_url`, if any. Matching is
    /// exact on host and path, so a custom provider only inherits metadata
    /// from the endpoint it actually calls.
    #[must_use]
    pub fn models_dev_provider_for_base_url(&self, base_url: &str) -> Option<String> {
        self.refreshed
            .as_ref()
            .and_then(|refreshed| refreshed.provider_for_base_url(base_url))
            .or_else(|| self.bundled.provider_for_base_url(base_url))
    }

    fn own_ids<'a>(&'a self, provider: &str) -> Vec<&'a str> {
        let mut ids: Vec<&str> = self
            .provider_aliases
            .get(provider)
            .map(String::as_str)
            .into_iter()
            .collect();
        ids.extend(models_dev_ids(provider).iter().copied());
        ids
    }

    fn remote_allowed(&self, provider: &str) -> bool {
        self.configured
            .as_ref()
            .is_none_or(|configured| configured.contains(provider))
    }

    fn layer(&self, source: Source, provider: &str, id: &str) -> Option<ModelSpec> {
        match source {
            Source::Override => {
                let rows = self.provider_rows.get(&source)?;
                let specific = rows.get(provider).and_then(|r| r.get(id));
                let any = rows.get(ANY_PROVIDER).and_then(|r| r.get(id));
                match (specific, any) {
                    (Some(specific), Some(any)) => {
                        let mut merged = specific.clone();
                        merged.fill_from(any);
                        Some(merged)
                    },
                    (Some(row), None) | (None, Some(row)) => Some(row.clone()),
                    (None, None) => None,
                }
            },
            Source::Probe | Source::Live => self
                .provider_rows
                .get(&source)?
                .get(provider)?
                .get(id)
                .cloned(),
            Source::Refreshed => {
                if !self.remote_allowed(provider) {
                    return None;
                }
                self.refreshed
                    .as_ref()?
                    .lookup(&self.own_ids(provider), id)
                    .cloned()
            },
            Source::Bundled => {
                let supplement = self.supplement.get(provider).and_then(|rows| rows.get(id));
                let own_ids = self.own_ids(provider);
                let models_dev = self.bundled.lookup(&own_ids, id);
                match (supplement, models_dev) {
                    (Some(supplement), Some(models_dev)) => {
                        let mut merged = supplement.clone();
                        merged.fill_from(models_dev);
                        Some(merged)
                    },
                    (Some(row), None) | (None, Some(row)) => Some(row.clone()),
                    (None, None) => None,
                }
            },
            Source::Heuristic => None,
            Source::ProviderDefault => self.provider_defaults.get(provider).cloned(),
        }
    }

    /// Merge every source for one model. `heuristic` is the caller's id-based
    /// guess, used only for fields no other source sets.
    #[must_use]
    pub fn resolve(
        &self,
        provider: &str,
        id: &str,
        heuristic: Option<&ModelSpec>,
    ) -> ResolvedModel {
        let mut spec = ModelSpec::new(id);
        let mut context_source = None;
        let mut name_source = None;
        for source in [
            Source::Override,
            Source::Probe,
            Source::Live,
            Source::Refreshed,
            Source::Bundled,
            Source::Heuristic,
            Source::ProviderDefault,
        ] {
            let layer = match source {
                Source::Heuristic => heuristic.cloned(),
                _ => self.layer(source, provider, id),
            };
            let Some(layer) = layer else {
                continue;
            };
            if context_source.is_none() && layer.context_window.is_some() {
                context_source = Some(source);
            }
            if name_source.is_none() && layer.display_name.is_some() {
                name_source = Some(source);
            }
            spec.fill_from(&layer);
        }
        spec.id = id.to_string();
        ResolvedModel {
            provider: provider.to_string(),
            spec,
            context_source,
            name_source,
        }
    }

    /// Every model id any source knows for a provider (live rows, overrides,
    /// the provider's own models.dev entries and the supplement).
    #[must_use]
    pub fn known_ids(&self, provider: &str) -> Vec<String> {
        let mut ids: HashSet<String> = HashSet::new();
        for source in PROVIDER_SOURCES {
            if let Some(rows) = self
                .provider_rows
                .get(&source)
                .and_then(|p| p.get(provider))
            {
                ids.extend(rows.keys().cloned());
            }
        }
        let own_ids = self.own_ids(provider);
        if let Some(refreshed) = &self.refreshed
            && self.remote_allowed(provider)
        {
            ids.extend(refreshed.provider_models(&own_ids).map(|m| m.id.clone()));
        }
        ids.extend(self.bundled.provider_models(&own_ids).map(|m| m.id.clone()));
        if let Some(rows) = self.supplement.get(provider) {
            ids.extend(rows.keys().cloned());
        }
        let mut ids: Vec<String> = ids.into_iter().collect();
        ids.sort();
        ids
    }

    /// Resolved models of one kind for a provider, newest release first.
    #[must_use]
    pub fn models_of_kind(&self, provider: &str, kind: ModelKind) -> Vec<ResolvedModel> {
        let mut models: Vec<ResolvedModel> = self
            .known_ids(provider)
            .iter()
            .map(|id| self.resolve(provider, id, None))
            .filter(|model| model.kind() == kind)
            .collect();
        models.sort_by(|a, b| {
            b.spec
                .release_date
                .cmp(&a.spec.release_date)
                .then_with(|| a.spec.id.cmp(&b.spec.id))
        });
        models
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn spec(id: &str) -> ModelSpec {
        ModelSpec::new(id)
    }

    fn catalog() -> ModelCatalog {
        let mut models_dev = ModelsDevCatalog::default();
        models_dev
            .providers
            .insert("anthropic".into(), vec![ModelSpec {
                display_name: Some("Claude Opus 5.5".into()),
                context_window: Some(1_000_000),
                max_output: Some(128_000),
                tools: Some(true),
                release_date: Some("2026-09-21".into()),
                ..spec("claude-opus-5-5")
            }]);
        models_dev
            .providers
            .insert("moonshotai".into(), vec![ModelSpec {
                display_name: Some("Kimi K3".into()),
                context_window: Some(1_000_000),
                ..spec("kimi-k3")
            }]);
        models_dev.providers.insert("openai".into(), vec![
            ModelSpec {
                kind: Some(ModelKind::Image),
                status: Some(ModelStatus::Deprecated),
                release_date: Some("2026-04-21".into()),
                ..spec("gpt-image-2")
            },
            ModelSpec {
                kind: Some(ModelKind::Chat),
                ..spec("gpt-oss-120b")
            },
        ]);
        let mut supplement = Supplement::default();
        supplement.models.insert("openai".into(), vec![ModelSpec {
            kind: Some(ModelKind::Image),
            release_date: Some("2026-10-01".into()),
            ..spec("gpt-image-2.6-flare")
        }]);
        supplement
            .provider_defaults
            .insert("zai".into(), ModelSpec {
                rate_limit_tz: Some(crate::types::RateLimitTz::CHINA),
                ..spec("*")
            });
        let mut catalog = ModelCatalog::empty();
        catalog.set_bundled(models_dev, supplement);
        catalog
    }

    #[test]
    fn override_beats_live_beats_bundled_field_by_field() {
        let mut catalog = catalog();
        catalog.set_provider_rows(Source::Live, "anthropic", vec![ModelSpec {
            context_window: Some(900_000),
            display_name: Some("Claude Opus 5.5 (live)".into()),
            ..spec("claude-opus-5-5")
        }]);
        catalog.set_provider_rows(Source::Override, "anthropic", vec![ModelSpec {
            context_window: Some(500_000),
            ..spec("claude-opus-5-5")
        }]);
        let resolved = catalog.resolve("anthropic", "claude-opus-5-5", None);
        assert_eq!(resolved.spec.context_window, Some(500_000));
        assert_eq!(resolved.context_source, Some(Source::Override));
        assert_eq!(
            resolved.spec.display_name.as_deref(),
            Some("Claude Opus 5.5 (live)")
        );
        assert_eq!(resolved.name_source, Some(Source::Live));
        assert_eq!(resolved.spec.max_output, Some(128_000));
        assert_eq!(resolved.spec.tools, Some(true));
    }

    #[test]
    fn custom_provider_matched_by_base_url_uses_that_provider() {
        let mut models_dev = ModelsDevCatalog::default();
        models_dev
            .providers
            .insert("greenpt".into(), vec![ModelSpec {
                context_window: Some(1_000_000),
                display_name: Some("DeepSeek V4.1 Flash".into()),
                ..spec("deepseek-v4.1-flash")
            }]);
        models_dev
            .provider_api
            .insert("greenpt".into(), "https://api.greenpt.ai/v1".into());
        let mut catalog = ModelCatalog::empty();
        catalog.set_bundled(models_dev, Supplement::default());
        assert_eq!(
            catalog.models_dev_provider_for_base_url("HTTPS://api.greenpt.ai/v1/"),
            Some("greenpt".to_string())
        );
        assert_eq!(
            catalog.models_dev_provider_for_base_url("https://api.greenpt.ai"),
            None
        );
        assert!(catalog.known_ids("custom-greenpt").is_empty());
        catalog.set_provider_alias("custom-greenpt", "greenpt");
        assert_eq!(catalog.known_ids("custom-greenpt"), ["deepseek-v4.1-flash"]);
        let resolved = catalog.resolve("custom-greenpt", "deepseek-v4.1-flash", None);
        assert_eq!(resolved.known_context_window(), Some(1_000_000));
    }

    #[test]
    fn provider_override_beats_any_provider_override() {
        let mut catalog = catalog();
        catalog.set_provider_rows(Source::Override, ANY_PROVIDER, vec![ModelSpec {
            context_window: Some(10),
            max_output: Some(5),
            ..spec("claude-opus-5-5")
        }]);
        assert_eq!(
            catalog
                .resolve("custom-y", "claude-opus-5-5", None)
                .spec
                .context_window,
            Some(10)
        );
        catalog.set_provider_rows(Source::Override, "anthropic", vec![ModelSpec {
            context_window: Some(20),
            ..spec("claude-opus-5-5")
        }]);
        let resolved = catalog.resolve("anthropic", "claude-opus-5-5", None);
        assert_eq!(resolved.spec.context_window, Some(20));
        assert_eq!(resolved.spec.max_output, Some(5));
    }

    #[test]
    fn heuristic_fills_only_unknown_fields() {
        let catalog = catalog();
        let guess = ModelSpec {
            context_window: Some(200_000),
            tools: Some(false),
            ..spec("claude-opus-5-5")
        };
        let resolved = catalog.resolve("anthropic", "claude-opus-5-5", Some(&guess));
        assert_eq!(resolved.spec.context_window, Some(1_000_000));
        assert_eq!(resolved.spec.tools, Some(true));

        let unknown = catalog.resolve("anthropic", "claude-future", Some(&guess));
        assert_eq!(unknown.spec.context_window, Some(200_000));
        assert_eq!(unknown.context_source, Some(Source::Heuristic));
        assert_eq!(unknown.known_context_window(), None);
    }

    #[test]
    fn custom_provider_falls_back_to_first_party_metadata() {
        let catalog = catalog();
        let resolved = catalog.resolve("custom-kimi", "kimi-k3", None);
        assert_eq!(resolved.spec.context_window, Some(1_000_000));
        assert_eq!(resolved.known_context_window(), Some(1_000_000));
        let routed = catalog.resolve("custom-router", "moonshotai/kimi-k3", None);
        assert_eq!(routed.spec.display_name.as_deref(), Some("Kimi K3"));
    }

    #[test]
    fn refreshed_catalog_is_limited_to_configured_providers() {
        let mut catalog = catalog();
        let mut refreshed = ModelsDevCatalog::default();
        refreshed
            .providers
            .insert("anthropic".into(), vec![ModelSpec {
                context_window: Some(2_000_000),
                ..spec("claude-opus-5-5")
            }]);
        catalog.set_refreshed(refreshed);
        assert_eq!(
            catalog
                .resolve("anthropic", "claude-opus-5-5", None)
                .spec
                .context_window,
            Some(2_000_000)
        );
        catalog.set_configured_providers(["openai"]);
        let resolved = catalog.resolve("anthropic", "claude-opus-5-5", None);
        assert_eq!(resolved.spec.context_window, Some(1_000_000));
        assert_eq!(resolved.context_source, Some(Source::Bundled));
    }

    #[test]
    fn supplement_merges_over_models_dev_and_defaults_apply() {
        let catalog = catalog();
        let flare = catalog.resolve("openai", "gpt-image-2.6-flare", None);
        assert_eq!(flare.kind(), ModelKind::Image);
        let glm = catalog.resolve("zai", "glm-5.3", None);
        assert_eq!(
            glm.spec.rate_limit_tz,
            Some(crate::types::RateLimitTz::CHINA)
        );
        assert_eq!(glm.context_source, None);
    }

    #[test]
    fn models_of_kind_sorts_newest_first() {
        let catalog = catalog();
        let images = catalog.models_of_kind("openai", ModelKind::Image);
        let ids: Vec<&str> = images.iter().map(ResolvedModel::id).collect();
        assert_eq!(ids, ["gpt-image-2.6-flare", "gpt-image-2"]);
        assert!(images[1].status().is_deprecated());
    }

    #[test]
    fn upsert_merges_rows_and_ignores_non_runtime_sources() {
        let mut catalog = catalog();
        catalog.upsert_provider_row(Source::Probe, "custom-x", ModelSpec {
            context_window: Some(32_768),
            ..spec("m")
        });
        catalog.upsert_provider_row(Source::Probe, "custom-x", ModelSpec {
            tools: Some(true),
            ..spec("m")
        });
        let rows = catalog.provider_rows(Source::Probe, "custom-x");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].context_window, Some(32_768));
        assert_eq!(rows[0].tools, Some(true));

        catalog.set_provider_rows(Source::Bundled, "custom-x", vec![spec("n")]);
        catalog.upsert_provider_row(Source::Bundled, "custom-x", spec("n"));
        assert!(!catalog.known_ids("custom-x").contains(&"n".to_string()));
    }

    #[test]
    fn display_name_prefers_catalog_then_prettifier() {
        let catalog = catalog();
        assert_eq!(
            catalog
                .resolve("anthropic", "claude-opus-5-5", None)
                .display_name()
                .name,
            "Claude Opus 5.5"
        );
        let pretty = catalog
            .resolve("zai", "glm-4.6-air-0925", None)
            .display_name();
        assert_eq!(pretty.name, "GLM 4.6 Air");
        assert_eq!(pretty.version.as_deref(), Some("0925"));
    }
}
