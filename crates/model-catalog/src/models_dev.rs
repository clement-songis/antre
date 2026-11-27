//! Typed parser for the models.dev `api.json` (v1 shape).
//!
//! Only model metadata is read, plus each provider's `api` base URL, which
//! is used solely to recognise a configured custom provider (its configured
//! base URL must match). `npm`, `env` and per-model `provider` overrides are
//! never parsed: a remote catalog cannot redirect requests or inject headers.

use std::collections::HashMap;

use serde::{Deserialize, de::IgnoredAny};

use crate::types::{Cost, Modality, ModelKind, ModelSpec, ModelStatus, Reasoning, ReasoningLevel};

/// Parsed models.dev catalog: model specs keyed by models.dev provider id.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelsDevCatalog {
    pub providers: HashMap<String, Vec<ModelSpec>>,
    /// Published API base URL per provider, for matching only.
    pub provider_api: HashMap<String, String>,
}

impl ModelsDevCatalog {
    /// Parse `api.json`. Malformed models are skipped rather than failing
    /// the whole file, since the catalog is community-edited.
    pub fn parse(bytes: &[u8]) -> Result<Self, crate::Error> {
        let raw: HashMap<String, RawProvider> = serde_json::from_slice(bytes)?;
        let provider_api = raw
            .iter()
            .filter_map(|(provider, raw_provider)| {
                Some((provider.clone(), raw_provider.api.clone()?))
            })
            .collect();
        let providers = raw
            .into_iter()
            .map(|(provider, raw_provider)| {
                let mut models: Vec<ModelSpec> = raw_provider
                    .models
                    .into_values()
                    .filter_map(|entry| match entry {
                        MaybeModel::Model(model) => Some(model.into_spec()),
                        MaybeModel::Invalid(_) => None,
                    })
                    .collect();
                models.sort_by(|a, b| a.id.cmp(&b.id));
                (provider, models)
            })
            .collect();
        Ok(Self {
            providers,
            provider_api,
        })
    }

    #[must_use]
    pub fn model_count(&self) -> usize {
        self.providers.values().map(Vec::len).sum()
    }

    /// Keep only the given models.dev providers.
    pub fn retain_providers(&mut self, keep: &[&str]) {
        self.providers
            .retain(|provider, _| keep.contains(&provider.as_str()));
        self.provider_api
            .retain(|provider, _| keep.contains(&provider.as_str()));
    }
}

#[derive(Deserialize)]
struct RawProvider {
    #[serde(default)]
    api: Option<String>,
    #[serde(default)]
    models: HashMap<String, MaybeModel>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum MaybeModel {
    Model(Box<RawModel>),
    Invalid(IgnoredAny),
}

#[derive(Deserialize)]
struct RawModel {
    id: String,
    name: Option<String>,
    family: Option<String>,
    release_date: Option<String>,
    knowledge: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    limit: Option<RawLimit>,
    modalities: Option<RawModalities>,
    tool_call: Option<bool>,
    structured_output: Option<bool>,
    reasoning: Option<bool>,
    reasoning_options: Option<Vec<RawReasoningOption>>,
    cost: Option<RawCost>,
    status: Option<String>,
}

#[derive(Deserialize)]
struct RawLimit {
    context: Option<f64>,
    output: Option<f64>,
}

#[derive(Deserialize)]
struct RawModalities {
    #[serde(default)]
    input: Vec<Modality>,
    #[serde(default)]
    output: Vec<Modality>,
}

#[derive(Deserialize)]
struct RawReasoningOption {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    values: Vec<String>,
}

#[derive(Deserialize)]
struct RawCost {
    input: Option<f64>,
    output: Option<f64>,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
}

fn tokens(value: Option<f64>) -> Option<u32> {
    value
        .filter(|tokens| tokens.is_finite() && *tokens > 0.0)
        .map(|tokens| tokens.min(f64::from(u32::MAX)) as u32)
}

fn reasoning_level(value: &str) -> Option<ReasoningLevel> {
    match value {
        "none" => Some(ReasoningLevel::None),
        "minimal" => Some(ReasoningLevel::Minimal),
        "low" => Some(ReasoningLevel::Low),
        "medium" => Some(ReasoningLevel::Medium),
        "high" => Some(ReasoningLevel::High),
        "xhigh" => Some(ReasoningLevel::Xhigh),
        "max" => Some(ReasoningLevel::Max),
        _ => None,
    }
}

impl RawModel {
    fn into_spec(self) -> ModelSpec {
        let efforts: Vec<ReasoningLevel> = self
            .reasoning_options
            .iter()
            .flatten()
            .filter(|option| option.kind == "effort")
            .flat_map(|option| option.values.iter().filter_map(|v| reasoning_level(v)))
            .collect();
        let reasoning = if efforts.is_empty() {
            self.reasoning.map(Reasoning::Flag)
        } else {
            Some(Reasoning::Efforts(efforts))
        };
        let status = self.status.as_deref().and_then(|status| match status {
            "deprecated" => Some(ModelStatus::Deprecated),
            "beta" => Some(ModelStatus::Beta),
            "alpha" => Some(ModelStatus::Alpha),
            _ => None,
        });
        let caching = self
            .cost
            .as_ref()
            .map(|cost| cost.cache_read.is_some() || cost.cache_write.is_some());
        let cost = self.cost.map(|cost| Cost {
            input: cost.input.unwrap_or_default(),
            output: cost.output.unwrap_or_default(),
            cache_read: cost.cache_read,
            cache_write: cost.cache_write,
        });
        let (input_modalities, output_modalities) = match self.modalities {
            Some(modalities) => (Some(modalities.input), Some(modalities.output)),
            None => (None, None),
        };
        ModelSpec {
            id: self.id,
            display_name: self.name.filter(|name| !name.trim().is_empty()),
            family: self.family,
            release_date: self.release_date,
            knowledge_cutoff: self.knowledge,
            kind: Some(
                self.kind
                    .as_deref()
                    .and_then(ModelKind::from_models_dev)
                    .unwrap_or_default(),
            ),
            context_window: self.limit.as_ref().and_then(|limit| tokens(limit.context)),
            max_output: self.limit.as_ref().and_then(|limit| tokens(limit.output)),
            input_modalities,
            output_modalities,
            tools: self.tool_call,
            structured_output: self.structured_output,
            caching,
            reasoning,
            cost,
            cost_note: None,
            status,
            retires_at: None,
            replacement: None,
            rate_limit_tz: None,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "anthropic": {
        "id": "anthropic", "name": "Anthropic", "npm": "@ai-sdk/anthropic",
        "api": "https://evil.example/v1", "env": ["ANTHROPIC_API_KEY"],
        "models": {
          "claude-fable-5-1": {
            "id": "claude-fable-5-1", "name": "Claude Fable 5.1", "family": "claude-fable",
            "description": "long text", "reasoning": true,
            "reasoning_options": [{"type": "effort", "values": ["low", "high", "max", "turbo"]}],
            "tool_call": true, "structured_output": true, "knowledge": "2026-06",
            "release_date": "2026-09-01",
            "modalities": {"input": ["text", "image", "pdf"], "output": ["text"]},
            "limit": {"context": 1000000, "output": 128000},
            "cost": {"input": 10, "output": 50, "cache_read": 0.25, "cache_write": 12.5,
                     "tiers": [{"input": 20}]},
            "provider": {"api": "https://evil.example"}
          },
          "broken": {"name": 42}
        }
      },
      "openai": {
        "id": "openai",
        "models": {
          "gpt-image-2": {"id": "gpt-image-2", "type": "image", "status": "deprecated",
                          "limit": {"context": 0, "output": 0}},
          "whisper-x": {"id": "whisper-x", "type": "transcription", "reasoning": false}
        }
      },
      "some-router": {"id": "some-router", "models": {}}
    }"#;

    #[test]
    fn parses_models_and_skips_invalid_rows() {
        let catalog = ModelsDevCatalog::parse(SAMPLE.as_bytes()).unwrap();
        let anthropic = &catalog.providers["anthropic"];
        assert_eq!(anthropic.len(), 1);
        let fable = &anthropic[0];
        assert_eq!(fable.display_name.as_deref(), Some("Claude Fable 5.1"));
        assert_eq!(fable.context_window, Some(1_000_000));
        assert_eq!(fable.max_output, Some(128_000));
        assert_eq!(fable.kind, Some(ModelKind::Chat));
        assert_eq!(fable.caching, Some(true));
        assert_eq!(
            fable.reasoning,
            Some(Reasoning::Efforts(vec![
                ReasoningLevel::Low,
                ReasoningLevel::High,
                ReasoningLevel::Max
            ]))
        );
        assert_eq!(fable.cost.unwrap().cache_write, Some(12.5));
    }

    #[test]
    fn maps_kinds_status_and_zero_limits() {
        let catalog = ModelsDevCatalog::parse(SAMPLE.as_bytes()).unwrap();
        let openai = &catalog.providers["openai"];
        let image = openai.iter().find(|m| m.id == "gpt-image-2").unwrap();
        assert_eq!(image.kind, Some(ModelKind::Image));
        assert_eq!(image.status, Some(ModelStatus::Deprecated));
        assert_eq!(image.context_window, None);
        let stt = openai.iter().find(|m| m.id == "whisper-x").unwrap();
        assert_eq!(stt.kind, Some(ModelKind::Stt));
        assert_eq!(stt.reasoning, Some(Reasoning::Flag(false)));
        assert_eq!(catalog.model_count(), 3);
    }

    #[test]
    fn provider_overrides_never_reach_model_specs() {
        let catalog = ModelsDevCatalog::parse(SAMPLE.as_bytes()).unwrap();
        let text = format!("{:?}", catalog.providers);
        assert!(!text.contains("evil.example"));
        assert!(!text.contains("@ai-sdk"));
        assert!(!text.contains("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn provider_api_is_kept_for_matching_only() {
        let catalog = ModelsDevCatalog::parse(SAMPLE.as_bytes()).unwrap();
        assert_eq!(
            catalog.provider_api.get("anthropic").map(String::as_str),
            Some("https://evil.example/v1")
        );
        assert!(!catalog.provider_api.contains_key("openai"));
    }

    #[test]
    fn retain_providers_filters() {
        let mut catalog = ModelsDevCatalog::parse(SAMPLE.as_bytes()).unwrap();
        catalog.retain_providers(&["openai"]);
        assert_eq!(catalog.providers.len(), 1);
    }
}
