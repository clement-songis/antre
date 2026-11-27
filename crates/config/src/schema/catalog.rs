//! `[catalog]`: where the model catalog refreshes its models.dev data.

use serde::{Deserialize, Serialize};

/// Model catalog refresh settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CatalogConfig {
    /// Refresh the catalog in the background. Off keeps the snapshot
    /// compiled into the binary (offline installs).
    pub refresh: bool,
    /// Sources tried in order until one returns a valid catalog: models.dev,
    /// then a mirror of the fork's committed snapshot. Point these at a
    /// self-hosted mirror to avoid third-party requests.
    pub refresh_urls: Vec<String>,
    /// Minimum hours between two downloads.
    pub refresh_interval_hours: u32,
}

impl Default for CatalogConfig {
    fn default() -> Self {
        Self {
            refresh: true,
            refresh_urls: moltis_model_catalog::refresh::DEFAULT_REFRESH_URLS
                .iter()
                .map(|url| (*url).to_string())
                .collect(),
            refresh_interval_hours: 12,
        }
    }
}
