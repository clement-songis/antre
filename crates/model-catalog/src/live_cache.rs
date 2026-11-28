//! On-disk copy of each provider's last successful live `/models` result, so
//! a failed discovery keeps the last good rows instead of falling back to
//! guesses.

use std::path::PathBuf;

use {
    serde::{Deserialize, Serialize},
    time::OffsetDateTime,
};

use crate::types::ModelSpec;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveSnapshot {
    #[serde(with = "time::serde::rfc3339")]
    pub fetched_at: OffsetDateTime,
    pub models: Vec<ModelSpec>,
}

/// Per-provider live results under `data_dir()/model-catalog/live`.
#[derive(Debug, Clone)]
pub struct LiveCache {
    dir: PathBuf,
}

/// File name for a provider id. Provider ids are config keys, so anything
/// outside `[A-Za-z0-9._-]` is hex-escaped to keep the path inside `dir`.
fn file_name(provider: &str) -> String {
    let mut name = String::with_capacity(provider.len() + 5);
    for byte in provider.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') {
            name.push(char::from(byte));
        } else {
            name.push_str(&format!("%{byte:02x}"));
        }
    }
    name.push_str(".json");
    name
}

impl LiveCache {
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn save(&self, provider: &str, snapshot: &LiveSnapshot) -> crate::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let path = self.dir.join(file_name(provider));
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec(snapshot)?)?;
        std::fs::rename(&tmp, &path)?;
        #[cfg(feature = "metrics")]
        antre_metrics::counter!(antre_metrics::model_catalog::LIVE_CACHE_WRITES_TOTAL).increment(1);
        Ok(())
    }

    pub fn load(&self, provider: &str) -> crate::Result<Option<LiveSnapshot>> {
        match std::fs::read(self.dir.join(file_name(provider))) {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_escapes_names() {
        let dir = tempfile::tempdir().unwrap();
        let cache = LiveCache::new(dir.path());
        assert!(cache.load("openai").unwrap().is_none());
        let snapshot = LiveSnapshot {
            fetched_at: time::macros::datetime!(2026-11-27 10:00 UTC),
            models: vec![ModelSpec {
                context_window: Some(128_000),
                ..ModelSpec::new("gpt-6.2-sol")
            }],
        };
        cache.save("../custom/evil", &snapshot).unwrap();
        assert_eq!(cache.load("../custom/evil").unwrap(), Some(snapshot));
        let entries: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(file_name("../a"), "%2e%2e%2fa.json");
        assert_eq!(file_name("custom-x_1"), "custom-x_1.json");
    }

    #[test]
    fn corrupt_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("p.json"), b"nope").unwrap();
        assert!(LiveCache::new(dir.path()).load("p").is_err());
    }
}
