//! Startup wiring for the model catalog: on-disk caches under
//! `data_dir()/model-catalog` and the background models.dev refresh.

use std::path::{Path, PathBuf};

use {
    antre_config::schema::CatalogConfig,
    antre_model_catalog::{
        global,
        refresh::{RefreshCache, RefreshOptions, RefreshOutcome},
    },
    tracing::{debug, info, warn},
};

/// How often the refresh task wakes up; the cache's own age decides whether
/// it downloads anything.
const CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60 * 60);

fn catalog_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("model-catalog")
}

/// Point the registry's live-discovery cache at `data_dir`, and install the
/// last refreshed catalog so names and limits are right from the first
/// request. Call before the provider registry is built.
pub(super) async fn init_model_catalog(data_dir: &Path) {
    let dir = catalog_dir(data_dir);
    antre_providers::registry::set_live_cache_dir(dir.join("live"));
    match RefreshCache::new(&dir).load().await {
        Ok(Some(catalog)) => {
            debug!(
                models = catalog.model_count(),
                "loaded cached model catalog"
            );
            global::write(|global| global.set_refreshed(catalog));
        },
        Ok(None) => {},
        Err(error) => warn!(%error, "ignoring unreadable cached model catalog"),
    }
}

/// Restrict what a downloaded catalog may describe to providers that are
/// configured (OpenClaw's guard): a remote catalog never adds providers.
pub(super) fn restrict_remote_catalog<I, S>(providers: I)
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    global::write(|catalog| catalog.set_configured_providers(providers));
}

/// Spawn the periodic refresh, unless disabled in `[catalog]`.
pub(super) fn spawn_catalog_refresh(config: &CatalogConfig, data_dir: &Path) {
    if !config.refresh || config.refresh_urls.is_empty() {
        info!("model catalog refresh disabled; using the bundled snapshot");
        return;
    }
    let cache = RefreshCache::new(catalog_dir(data_dir));
    let urls = config.refresh_urls.clone();
    let options = RefreshOptions {
        max_age: time::Duration::hours(i64::from(config.refresh_interval_hours.max(1))),
        ..RefreshOptions::default()
    };
    tokio::spawn(async move {
        let client = antre_common::http_client::build_default_http_client();
        loop {
            match cache
                .refresh(&client, &urls, options, time::OffsetDateTime::now_utc())
                .await
            {
                Ok(RefreshOutcome::Updated(catalog)) => {
                    global::write(|global| global.set_refreshed(catalog));
                },
                Ok(RefreshOutcome::NotModified | RefreshOutcome::StillFresh) => {},
                Err(error) => {
                    warn!(%error, "model catalog refresh failed; keeping the previous copy")
                },
            }
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn init_without_cache_keeps_bundled_catalog() {
        let dir = tempfile::tempdir().unwrap();
        init_model_catalog(dir.path()).await;
        assert!(global::read(|catalog| !catalog
            .known_ids("openai")
            .is_empty()));
    }

    #[test]
    fn disabled_refresh_spawns_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let config = CatalogConfig {
            refresh: false,
            ..CatalogConfig::default()
        };
        // No runtime: spawning would panic, so returning early proves it.
        spawn_catalog_refresh(&config, dir.path());
    }
}
