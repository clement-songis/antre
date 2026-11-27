//! Data compiled into the binary: the gzip-compressed models.dev snapshot
//! and Moltis's curated supplement.
//!
//! Refresh the snapshot with `scripts/update-model-catalog.sh`.

use std::{collections::HashMap, io::Read};

use {flate2::read::GzDecoder, serde::Deserialize};

use crate::{models_dev::ModelsDevCatalog, types::ModelSpec};

const BUNDLED_MODELS_DEV: &[u8] = include_bytes!("../data/models-dev.json.gz");
const SUPPLEMENT: &str = include_str!("../data/supplement.toml");

/// Decompress gzip bytes, refusing to inflate past `limit` bytes.
pub fn gunzip(bytes: &[u8], limit: usize) -> crate::Result<Vec<u8>> {
    let mut out = Vec::new();
    let read = GzDecoder::new(bytes)
        .take(limit as u64 + 1)
        .read_to_end(&mut out)?;
    if read > limit {
        return Err(crate::Error::TooLarge(limit));
    }
    Ok(out)
}

/// Gzip-compress bytes.
pub fn gzip(bytes: &[u8]) -> crate::Result<Vec<u8>> {
    use std::io::Write as _;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    encoder.write_all(bytes)?;
    Ok(encoder.finish()?)
}

/// Upper bound on a decompressed models.dev file (full file is ~7 MB).
pub const MAX_CATALOG_BYTES: usize = 64 * 1024 * 1024;

/// The models.dev snapshot embedded at build time.
pub fn bundled_models_dev() -> crate::Result<ModelsDevCatalog> {
    let json = gunzip(BUNDLED_MODELS_DEV, MAX_CATALOG_BYTES)?;
    ModelsDevCatalog::parse(&json)
}

/// Moltis's curated additions: models models.dev does not list yet (mostly
/// image, voice and other non-chat models) and provider facts such as the
/// timezone of reset times printed without an offset.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Supplement {
    /// Model specs keyed by Moltis provider id.
    pub models: HashMap<String, Vec<ModelSpec>>,
    /// Defaults applied to every model of a provider, lowest priority.
    pub provider_defaults: HashMap<String, ModelSpec>,
}

#[derive(Deserialize)]
struct RawSupplement {
    #[serde(default)]
    models: Vec<ProviderRow>,
    #[serde(default)]
    provider_defaults: Vec<ProviderRow>,
}

#[derive(Deserialize)]
struct ProviderRow {
    provider: String,
    #[serde(flatten)]
    spec: ModelSpec,
}

impl Supplement {
    pub fn parse(text: &str) -> crate::Result<Self> {
        let raw: RawSupplement = toml::from_str(text)?;
        let mut models: HashMap<String, Vec<ModelSpec>> = HashMap::new();
        for row in raw.models {
            models.entry(row.provider).or_default().push(row.spec);
        }
        let provider_defaults = raw
            .provider_defaults
            .into_iter()
            .map(|row| (row.provider, row.spec))
            .collect();
        Ok(Self {
            models,
            provider_defaults,
        })
    }
}

/// The supplement embedded at build time.
pub fn bundled_supplement() -> crate::Result<Supplement> {
    Supplement::parse(SUPPLEMENT)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn bundled_snapshot_loads_offline() {
        let catalog = bundled_models_dev().unwrap();
        assert!(catalog.model_count() > 100);
        assert!(catalog.providers.contains_key("anthropic"));
        assert!(catalog.providers.contains_key("openai"));
    }

    #[test]
    fn bundled_supplement_parses() {
        let supplement = bundled_supplement().unwrap();
        assert!(supplement.models.contains_key("openai"));
        assert!(supplement.provider_defaults.contains_key("zai-code"));
    }

    #[test]
    fn gzip_round_trip_and_limit() {
        let data = b"hello catalog".repeat(100);
        let packed = gzip(&data).unwrap();
        assert_eq!(gunzip(&packed, data.len()).unwrap(), data);
        assert!(matches!(
            gunzip(&packed, 10),
            Err(crate::Error::TooLarge(10))
        ));
    }

    #[test]
    fn supplement_rows_group_by_provider() {
        let supplement = Supplement::parse(
            r#"
            [[models]]
            provider = "openai"
            id = "a"
            kind = "image"
            [[models]]
            provider = "openai"
            id = "b"
            [[provider_defaults]]
            provider = "zai"
            id = "*"
            rate_limit_tz = "+08:00"
            "#,
        )
        .unwrap();
        assert_eq!(supplement.models["openai"].len(), 2);
        assert_eq!(
            supplement.provider_defaults["zai"].rate_limit_tz,
            Some(crate::types::RateLimitTz::CHINA)
        );
    }
}
