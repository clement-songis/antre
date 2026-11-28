//! Model metadata types shared by the catalog, the provider registry and the
//! config schema (static custom providers).

use {
    serde::{Deserialize, Deserializer, Serialize, Serializer},
    time::{Date, UtcOffset},
};

/// What a model is for. Only [`ModelKind::Chat`] models appear in chat pickers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelKind {
    #[default]
    Chat,
    Embedding,
    Rerank,
    Image,
    Video,
    Tts,
    Stt,
    Realtime,
}

impl ModelKind {
    /// Map a models.dev `type` value. Absent means chat there.
    #[must_use]
    pub fn from_models_dev(value: &str) -> Option<Self> {
        match value {
            "chat" => Some(Self::Chat),
            "embedding" => Some(Self::Embedding),
            "reranking" => Some(Self::Rerank),
            "image" => Some(Self::Image),
            "video" => Some(Self::Video),
            "speech" => Some(Self::Tts),
            "transcription" => Some(Self::Stt),
            "realtime" => Some(Self::Realtime),
            _ => None,
        }
    }
}

/// Input or output modality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Modality {
    Text,
    Image,
    Pdf,
    Audio,
    Video,
    #[serde(other)]
    Other,
}

/// Lifecycle status.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelStatus {
    #[default]
    Active,
    Alpha,
    Beta,
    Deprecated,
    Retired,
}

impl ModelStatus {
    /// `true` when the model should no longer be offered as a default.
    #[must_use]
    pub fn is_deprecated(self) -> bool {
        matches!(self, Self::Deprecated | Self::Retired)
    }
}

/// A reasoning effort level, as the providers name them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningLevel {
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

/// Reasoning support: a plain flag (OpenClaw's `reasoning = true`) or the list
/// of supported efforts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Reasoning {
    Flag(bool),
    Efforts(Vec<ReasoningLevel>),
}

impl Reasoning {
    #[must_use]
    pub fn is_supported(&self) -> bool {
        match self {
            Self::Flag(flag) => *flag,
            Self::Efforts(efforts) => efforts.iter().any(|e| *e != ReasoningLevel::None),
        }
    }
}

/// Price in USD per million tokens.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    #[serde(default, alias = "cacheRead", skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
    #[serde(default, alias = "cacheWrite", skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
}

/// Timezone of reset times that a provider prints without an offset.
///
/// Only fixed offsets are supported: every provider seen so far that omits
/// the offset uses a zone without daylight saving (China Standard Time for
/// Z.AI/GLM). Named zones are accepted only when they have no DST.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitTz(pub UtcOffset);

impl RateLimitTz {
    /// China Standard Time, UTC+8 all year.
    pub const CHINA: Self = Self(time::macros::offset!(+8));
    pub const UTC: Self = Self(UtcOffset::UTC);

    /// Parse `UTC`, `+08:00`, `-05:30` or a DST-free zone name.
    pub fn parse(value: &str) -> Result<Self, crate::Error> {
        let trimmed = value.trim();
        match trimmed {
            "UTC" | "Z" | "Etc/UTC" => return Ok(Self::UTC),
            "Asia/Shanghai" | "Asia/Chongqing" | "PRC" | "Asia/Hong_Kong" | "Asia/Singapore"
            | "Asia/Taipei" => return Ok(Self::CHINA),
            "Asia/Tokyo" | "Asia/Seoul" => return Ok(Self(time::macros::offset!(+9))),
            "Asia/Kolkata" => return Ok(Self(time::macros::offset!(+5:30))),
            _ => {},
        }
        let format =
            time::macros::format_description!("[offset_hour sign:mandatory]:[offset_minute]");
        UtcOffset::parse(trimmed, &format)
            .map(Self)
            .map_err(|_| crate::Error::InvalidTimezone(trimmed.to_string()))
    }
}

impl Serialize for RateLimitTz {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let (hours, minutes, _) = self.0.as_hms();
        let sign = if hours < 0 || minutes < 0 {
            '-'
        } else {
            '+'
        };
        serializer.serialize_str(&format!(
            "{sign}{:02}:{:02}",
            hours.unsigned_abs(),
            minutes.unsigned_abs()
        ))
    }
}

impl<'de> Deserialize<'de> for RateLimitTz {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// Metadata for one model, every field optional so sources can be merged
/// field by field. Field names follow the Antre config; OpenClaw's
/// camelCase names are accepted as aliases so its model files load as-is.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelSpec {
    /// Wire id sent to the API.
    pub id: String,
    #[serde(alias = "name", skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    /// `YYYY-MM-DD`, compared lexically to find the newest model.
    #[serde(alias = "releaseDate", skip_serializing_if = "Option::is_none")]
    pub release_date: Option<String>,
    #[serde(alias = "knowledgeCutoff", skip_serializing_if = "Option::is_none")]
    pub knowledge_cutoff: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<ModelKind>,
    #[serde(alias = "contextWindow", skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    #[serde(
        alias = "maxTokens",
        alias = "max_tokens",
        skip_serializing_if = "Option::is_none"
    )]
    pub max_output: Option<u32>,
    #[serde(alias = "input", skip_serializing_if = "Option::is_none")]
    pub input_modalities: Option<Vec<Modality>>,
    #[serde(alias = "output", skip_serializing_if = "Option::is_none")]
    pub output_modalities: Option<Vec<Modality>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<bool>,
    #[serde(alias = "structuredOutput", skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caching: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Reasoning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<Cost>,
    /// Free text: currency and tax assumptions behind `cost`.
    #[serde(alias = "costNote", skip_serializing_if = "Option::is_none")]
    pub cost_note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<ModelStatus>,
    #[serde(alias = "retiresAt", skip_serializing_if = "Option::is_none")]
    pub retires_at: Option<Date>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replacement: Option<String>,
    #[serde(alias = "rateLimitTz", skip_serializing_if = "Option::is_none")]
    pub rate_limit_tz: Option<RateLimitTz>,
}

macro_rules! fill_fields {
    ($target:ident, $lower:ident, $($field:ident),+ $(,)?) => {
        $(
            if $target.$field.is_none() {
                $target.$field = $lower.$field.clone();
            }
        )+
    };
}

impl ModelSpec {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            ..Self::default()
        }
    }

    /// Fill every field still unset from a lower-priority source.
    pub fn fill_from(&mut self, lower: &Self) {
        let target = self;
        fill_fields!(
            target,
            lower,
            display_name,
            family,
            release_date,
            knowledge_cutoff,
            kind,
            context_window,
            max_output,
            input_modalities,
            output_modalities,
            tools,
            structured_output,
            caching,
            reasoning,
            cost,
            cost_note,
            status,
            retires_at,
            replacement,
            rate_limit_tz,
        );
    }

    /// `true` when no metadata field is set.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::new(self.id.clone())
    }

    #[must_use]
    pub fn kind_or_default(&self) -> ModelKind {
        self.kind.unwrap_or_default()
    }

    #[must_use]
    pub fn status_or_default(&self) -> ModelStatus {
        self.status.unwrap_or_default()
    }

    #[must_use]
    pub fn accepts_images(&self) -> bool {
        self.input_modalities
            .as_ref()
            .is_some_and(|inputs| inputs.contains(&Modality::Image))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn fill_from_keeps_higher_priority_fields() {
        let mut high = ModelSpec {
            context_window: Some(100),
            ..ModelSpec::new("m")
        };
        let low = ModelSpec {
            context_window: Some(200),
            max_output: Some(50),
            tools: Some(true),
            ..ModelSpec::new("m")
        };
        high.fill_from(&low);
        assert_eq!(high.context_window, Some(100));
        assert_eq!(high.max_output, Some(50));
        assert_eq!(high.tools, Some(true));
    }

    #[test]
    fn is_empty_ignores_id() {
        assert!(ModelSpec::new("x").is_empty());
        assert!(
            !ModelSpec {
                tools: Some(false),
                ..ModelSpec::new("x")
            }
            .is_empty()
        );
    }

    #[test]
    fn deserializes_openclaw_camel_case_row() {
        let row = r#"{
            "id": "deepseek-v4.1-flash",
            "name": "DeepSeek V4.1 Flash",
            "cost": {"input": 0.29, "output": 1.47, "cacheRead": 0.01, "cacheWrite": 0.29},
            "contextWindow": 1000000,
            "maxTokens": 65536,
            "reasoning": true,
            "input": ["text", "image"]
        }"#;
        let spec: ModelSpec = serde_json::from_str(row).unwrap();
        assert_eq!(spec.display_name.as_deref(), Some("DeepSeek V4.1 Flash"));
        assert_eq!(spec.context_window, Some(1_000_000));
        assert_eq!(spec.max_output, Some(65_536));
        assert_eq!(spec.reasoning, Some(Reasoning::Flag(true)));
        assert!(spec.accepts_images());
        assert_eq!(spec.cost.unwrap().cache_read, Some(0.01));
    }

    #[test]
    fn deserializes_toml_with_efforts_and_dates() {
        let spec: ModelSpec = toml::from_str(
            r#"
            id = "qwen3.8-27b"
            kind = "chat"
            context_window = 131072
            reasoning = ["low", "medium", "high"]
            status = "deprecated"
            retires_at = "2026-12-11"
            rate_limit_tz = "Asia/Shanghai"
            "#,
        )
        .unwrap();
        assert_eq!(
            spec.reasoning,
            Some(Reasoning::Efforts(vec![
                ReasoningLevel::Low,
                ReasoningLevel::Medium,
                ReasoningLevel::High
            ]))
        );
        assert_eq!(spec.retires_at, Some(time::macros::date!(2026 - 12 - 11)));
        assert_eq!(spec.rate_limit_tz, Some(RateLimitTz::CHINA));
        assert!(spec.status_or_default().is_deprecated());
    }

    #[test]
    fn unknown_modality_is_tolerated() {
        let spec: ModelSpec =
            serde_json::from_str(r#"{"id":"m","input":["text","file"]}"#).unwrap();
        assert_eq!(
            spec.input_modalities,
            Some(vec![Modality::Text, Modality::Other])
        );
    }

    #[test]
    fn rate_limit_tz_round_trips() {
        for (raw, expected) in [
            ("+08:00", "+08:00"),
            ("UTC", "+00:00"),
            ("-05:30", "-05:30"),
        ] {
            let tz = RateLimitTz::parse(raw).unwrap();
            assert_eq!(
                serde_json::to_string(&tz).unwrap(),
                format!("\"{expected}\"")
            );
        }
        assert!(RateLimitTz::parse("Europe/Paris").is_err());
    }

    #[test]
    fn reasoning_support() {
        assert!(!Reasoning::Flag(false).is_supported());
        assert!(!Reasoning::Efforts(vec![ReasoningLevel::None]).is_supported());
        assert!(
            Reasoning::Efforts(vec![ReasoningLevel::None, ReasoningLevel::High]).is_supported()
        );
    }

    #[test]
    fn models_dev_kinds() {
        assert_eq!(ModelKind::from_models_dev("speech"), Some(ModelKind::Tts));
        assert_eq!(
            ModelKind::from_models_dev("transcription"),
            Some(ModelKind::Stt)
        );
        assert_eq!(
            ModelKind::from_models_dev("reranking"),
            Some(ModelKind::Rerank)
        );
        assert_eq!(ModelKind::from_models_dev("music"), None);
    }
}
