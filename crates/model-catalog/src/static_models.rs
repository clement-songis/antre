//! Statically declared models: `models = [...]` entries in a provider's
//! config (bare ids or full objects) and OpenClaw-shaped model files
//! (`{ "models": [...] }`) referenced by `models_file`.

use std::{collections::HashSet, path::Path};

use serde::{Deserialize, Serialize};

use crate::types::ModelSpec;

/// One `models` entry: a bare wire id, or a full model description.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ModelEntry {
    Id(String),
    Spec(Box<ModelSpec>),
}

impl ModelEntry {
    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            Self::Id(id) => id,
            Self::Spec(spec) => &spec.id,
        }
    }

    /// The description, when the entry is more than an id.
    #[must_use]
    pub fn spec(&self) -> Option<&ModelSpec> {
        match self {
            Self::Id(_) => None,
            Self::Spec(spec) => Some(spec),
        }
    }
}

impl From<String> for ModelEntry {
    fn from(id: String) -> Self {
        Self::Id(id)
    }
}

impl From<&str> for ModelEntry {
    fn from(id: &str) -> Self {
        Self::Id(id.to_string())
    }
}

impl PartialEq<&str> for ModelEntry {
    fn eq(&self, other: &&str) -> bool {
        self.id() == *other
    }
}

/// How a provider's model list is decided.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CatalogMode {
    /// Configured models first, then whatever `/models` discovery returns.
    #[default]
    #[serde(rename = "live")]
    Live,
    /// Only the configured models; no discovery.
    #[serde(rename = "static")]
    Static,
    /// Discovery decides what exists; static entries describe it. Served
    /// models without an entry are added, entries the server no longer
    /// serves are dropped with a warning (OpenClaw's `mergedModels`).
    #[serde(rename = "live+static")]
    LiveStatic,
}

#[derive(Deserialize)]
struct ModelsFile {
    #[serde(default)]
    models: Vec<ModelSpec>,
}

/// Generated files often append the provider to the name
/// (`"kimi-k3 (Kimi)"`). The UI shows the provider separately, so such a
/// name is dropped when the rest is just the id.
fn strip_provider_suffix(mut spec: ModelSpec) -> ModelSpec {
    if let Some(name) = &spec.display_name
        && let Some((base, rest)) = name.rsplit_once(" (")
        && rest.ends_with(')')
        && base.trim() == spec.id
    {
        spec.display_name = None;
    }
    spec
}

/// Parse an OpenClaw-shaped models file. Rows without an id are skipped.
pub fn parse_models_file(bytes: &[u8]) -> crate::Result<Vec<ModelSpec>> {
    let file: ModelsFile = serde_json::from_slice(bytes)?;
    Ok(file
        .models
        .into_iter()
        .filter(|spec| !spec.id.trim().is_empty())
        .map(strip_provider_suffix)
        .collect())
}

/// Read and parse a models file.
pub fn load_models_file(path: &Path) -> crate::Result<Vec<ModelSpec>> {
    parse_models_file(&std::fs::read(path)?)
}

/// Combine `models` entries with a models file: config entries win field by
/// field, file rows fill the rest. Order: config entries, then file rows.
#[must_use]
pub fn merge_static_entries(entries: &[ModelEntry], file_rows: Vec<ModelSpec>) -> Vec<ModelSpec> {
    let mut merged: Vec<ModelSpec> = entries
        .iter()
        .map(|entry| {
            entry
                .spec()
                .cloned()
                .unwrap_or_else(|| ModelSpec::new(entry.id()))
        })
        .collect();
    for row in file_rows {
        match merged.iter_mut().find(|spec| spec.id == row.id) {
            Some(existing) => existing.fill_from(&row),
            None => merged.push(row),
        }
    }
    merged
}

/// Result of [`merge_live_and_static`].
#[derive(Debug, Default, PartialEq)]
pub struct MergedModels {
    /// Model ids to register, static order first.
    pub ids: Vec<String>,
    /// Static entries the server no longer serves.
    pub missing: Vec<String>,
    /// Served models without a static entry.
    pub unknown: Vec<String>,
}

/// OpenClaw's `mergedModels`: served ∩ known keeps the static metadata,
/// served-only models are added, known-only models are dropped.
#[must_use]
pub fn merge_live_and_static(static_ids: &[String], served: &[String]) -> MergedModels {
    let served_set: HashSet<&str> = served.iter().map(String::as_str).collect();
    let static_set: HashSet<&str> = static_ids.iter().map(String::as_str).collect();
    let mut merged = MergedModels::default();
    for id in static_ids {
        if served_set.contains(id.as_str()) {
            merged.ids.push(id.clone());
        } else {
            merged.missing.push(id.clone());
        }
    }
    for id in served {
        if !static_set.contains(id.as_str()) {
            merged.ids.push(id.clone());
            merged.unknown.push(id.clone());
        }
    }
    merged
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn entries_accept_ids_and_objects_in_toml() {
        #[derive(Deserialize)]
        struct Provider {
            models: Vec<ModelEntry>,
        }
        let provider: Provider = toml::from_str(
            r#"
            models = [
              "kimi-k3",
              { id = "qwen3.8-27b", display_name = "Qwen 3.8 27B (box)", context_window = 131072, max_output = 16384, tools = true, reasoning = ["low", "medium", "high"] },
              { id = "nomic-embed", kind = "embedding" },
            ]
            "#,
        )
        .unwrap();
        assert_eq!(provider.models[0], "kimi-k3");
        assert_eq!(provider.models[0].spec(), None);
        let qwen = provider.models[1].spec().unwrap();
        assert_eq!(qwen.context_window, Some(131_072));
        assert_eq!(qwen.display_name.as_deref(), Some("Qwen 3.8 27B (box)"));
        assert_eq!(
            provider.models[2].spec().unwrap().kind,
            Some(crate::types::ModelKind::Embedding)
        );
    }

    #[test]
    fn entries_round_trip_through_toml() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Provider {
            models: Vec<ModelEntry>,
        }
        let provider = Provider {
            models: vec![
                ModelEntry::from("a"),
                ModelEntry::Spec(Box::new(ModelSpec {
                    context_window: Some(8_192),
                    ..ModelSpec::new("b")
                })),
            ],
        };
        let text = toml::to_string(&provider).unwrap();
        assert_eq!(toml::from_str::<Provider>(&text).unwrap(), provider);
    }

    #[test]
    fn parses_openclaw_models_file_as_is() {
        let file = r#"{
          "currency": "USD per million tokens, VAT included",
          "vatRate": 0.2,
          "models": [
            {"id": "deepseek-v4-flash-0731", "name": "deepseek-v4-flash-0731 (GreenPT)",
             "cost": {"input": 0.18, "output": 0.46, "cacheRead": 0.05, "cacheWrite": 0.18},
             "contextWindow": 1000000, "reasoning": false, "input": ["text"]},
            {"id": "kimi-k3", "name": "Kimi K3 (fast lane)", "maxTokens": 32768},
            {"name": "no id"}
          ]
        }"#;
        let rows = parse_models_file(file.as_bytes()).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].display_name, None);
        assert_eq!(rows[0].context_window, Some(1_000_000));
        assert_eq!(rows[0].cost.unwrap().cache_read, Some(0.05));
        assert_eq!(rows[1].display_name.as_deref(), Some("Kimi K3 (fast lane)"));
        assert_eq!(rows[1].max_output, Some(32_768));
        assert!(parse_models_file(br#"{"models": 3}"#).is_err());
        assert!(parse_models_file(b"not json").is_err());
    }

    #[test]
    fn config_entries_win_over_file_rows() {
        let entries = vec![
            ModelEntry::from("kimi-k3"),
            ModelEntry::Spec(Box::new(ModelSpec {
                context_window: Some(64_000),
                ..ModelSpec::new("glm-5.3")
            })),
        ];
        let file = vec![
            ModelSpec {
                context_window: Some(1_000_000),
                max_output: Some(8_000),
                ..ModelSpec::new("glm-5.3")
            },
            ModelSpec::new("extra"),
        ];
        let merged = merge_static_entries(&entries, file);
        let ids: Vec<&str> = merged.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["kimi-k3", "glm-5.3", "extra"]);
        assert_eq!(merged[1].context_window, Some(64_000));
        assert_eq!(merged[1].max_output, Some(8_000));
    }

    #[test]
    fn merged_models_semantics() {
        let static_ids = vec!["a".to_string(), "b".to_string()];
        let served = vec!["b".to_string(), "c".to_string()];
        let merged = merge_live_and_static(&static_ids, &served);
        assert_eq!(merged.ids, ["b", "c"]);
        assert_eq!(merged.missing, ["a"]);
        assert_eq!(merged.unknown, ["c"]);
    }

    #[test]
    fn catalog_mode_names() {
        #[derive(Deserialize)]
        struct Provider {
            catalog: CatalogMode,
        }
        for (raw, mode) in [
            ("static", CatalogMode::Static),
            ("live", CatalogMode::Live),
            ("live+static", CatalogMode::LiveStatic),
        ] {
            let provider: Provider = toml::from_str(&format!("catalog = \"{raw}\"")).unwrap();
            assert_eq!(provider.catalog, mode);
        }
    }
}
