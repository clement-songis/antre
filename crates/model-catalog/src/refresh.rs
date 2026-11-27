//! Background refresh of the models.dev catalog with a conditional GET,
//! cached on disk so a restart or an offline start keeps the last good copy.

use std::path::{Path, PathBuf};

use {
    serde::{Deserialize, Serialize},
    time::{Duration, OffsetDateTime},
};

use crate::{
    macros::{log_debug, log_info, log_warn},
    models_dev::ModelsDevCatalog,
    snapshot::{MAX_CATALOG_BYTES, gunzip, gzip},
};

/// Tried in order: models.dev, then the fork's committed snapshot, so an
/// outage or a format change upstream falls back to a known-good copy.
pub const DEFAULT_REFRESH_URLS: &[&str] = &[
    "https://models.dev/api.json",
    "https://raw.githubusercontent.com/clement-songis/moltis/main/crates/model-catalog/data/models-dev.json.gz",
];
/// A download with fewer models is rejected as truncated or reformatted.
pub const MIN_MODELS: usize = 500;
const CATALOG_FILE: &str = "models-dev.json.gz";
const META_FILE: &str = "models-dev.meta.json";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct CacheMeta {
    etag: Option<String>,
    #[serde(with = "time::serde::rfc3339::option", default)]
    fetched_at: Option<OffsetDateTime>,
    url: String,
}

/// On-disk cache of the refreshed catalog, under `data_dir()/model-catalog`.
#[derive(Debug, Clone)]
pub struct RefreshCache {
    dir: PathBuf,
}

/// What a refresh attempt did.
#[derive(Debug)]
pub enum RefreshOutcome {
    /// The cache is younger than the refresh interval; nothing was fetched.
    StillFresh,
    /// The server answered 304; the cached copy is current.
    NotModified,
    /// A new catalog was downloaded, validated and cached.
    Updated(ModelsDevCatalog),
}

async fn write_atomic(path: &Path, bytes: &[u8]) -> crate::Result<()> {
    let tmp = path.with_extension("tmp");
    tokio::fs::write(&tmp, bytes).await?;
    tokio::fs::rename(&tmp, path).await?;
    Ok(())
}

impl RefreshCache {
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    async fn read_meta(&self) -> Option<CacheMeta> {
        let bytes = tokio::fs::read(self.dir.join(META_FILE)).await.ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    async fn write_meta(&self, meta: &CacheMeta) -> crate::Result<()> {
        write_atomic(&self.dir.join(META_FILE), &serde_json::to_vec(meta)?).await
    }

    /// Load the cached catalog, if any.
    pub async fn load(&self) -> crate::Result<Option<ModelsDevCatalog>> {
        let path = self.dir.join(CATALOG_FILE);
        let compressed = match tokio::fs::read(&path).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let json = gunzip(&compressed, MAX_CATALOG_BYTES)?;
        Ok(Some(ModelsDevCatalog::parse(&json)?))
    }

    /// Refresh unless the cache is younger than `options.max_age`, trying
    /// each URL in order until one yields a valid catalog. The cache is only
    /// replaced by a catalog that parses and has at least
    /// `options.min_models` models.
    pub async fn refresh(
        &self,
        client: &reqwest::Client,
        urls: &[String],
        options: RefreshOptions,
        now: OffsetDateTime,
    ) -> crate::Result<RefreshOutcome> {
        tokio::fs::create_dir_all(&self.dir).await?;
        let meta = self.read_meta().await.unwrap_or_default();
        let has_cache = tokio::fs::try_exists(self.dir.join(CATALOG_FILE))
            .await
            .unwrap_or(false);
        if has_cache
            && meta
                .fetched_at
                .is_some_and(|fetched| now - fetched < options.max_age)
        {
            return Ok(RefreshOutcome::StillFresh);
        }

        let mut last_error = crate::Error::Empty;
        for url in urls {
            let etag = (has_cache && meta.url == *url)
                .then(|| meta.etag.clone())
                .flatten();
            match self.fetch_one(client, url, etag, options, now).await {
                Ok(outcome) => return Ok(outcome),
                Err(error) => {
                    log_warn!(url, %error, "model catalog refresh failed, trying next source");
                    #[cfg(feature = "metrics")]
                    moltis_metrics::counter!(moltis_metrics::model_catalog::REFRESH_ERRORS_TOTAL)
                        .increment(1);
                    last_error = error;
                },
            }
        }
        Err(last_error)
    }

    async fn fetch_one(
        &self,
        client: &reqwest::Client,
        url: &str,
        etag: Option<String>,
        options: RefreshOptions,
        now: OffsetDateTime,
    ) -> crate::Result<RefreshOutcome> {
        let mut request = client.get(url).header("accept", "application/json");
        if let Some(etag) = &etag {
            request = request.header("if-none-match", etag);
        }
        let mut response = request.send().await?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_MODIFIED && etag.is_some() {
            self.write_meta(&CacheMeta {
                etag,
                fetched_at: Some(now),
                url: url.to_string(),
            })
            .await?;
            log_debug!(url, "model catalog not modified");
            return Ok(RefreshOutcome::NotModified);
        }
        if !status.is_success() {
            return Err(crate::Error::HttpStatus(status.as_u16()));
        }
        let new_etag = response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);

        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if body.len() + chunk.len() > MAX_CATALOG_BYTES {
                return Err(crate::Error::TooLarge(MAX_CATALOG_BYTES));
            }
            body.extend_from_slice(&chunk);
        }
        // The fork's mirror serves the committed snapshot gzip-compressed.
        if body.starts_with(&[0x1f, 0x8b]) {
            body = gunzip(&body, MAX_CATALOG_BYTES)?;
        }

        let catalog = ModelsDevCatalog::parse(&body)?;
        if catalog.model_count() < options.min_models.max(1) {
            return Err(crate::Error::Empty);
        }
        write_atomic(&self.dir.join(CATALOG_FILE), &gzip(&body)?).await?;
        self.write_meta(&CacheMeta {
            etag: new_etag,
            fetched_at: Some(now),
            url: url.to_string(),
        })
        .await?;
        log_info!(
            url,
            models = catalog.model_count(),
            "model catalog refreshed"
        );
        #[cfg(feature = "metrics")]
        moltis_metrics::counter!(moltis_metrics::model_catalog::REFRESH_UPDATES_TOTAL).increment(1);
        Ok(RefreshOutcome::Updated(catalog))
    }
}

/// How a refresh validates and stores what it downloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefreshOptions {
    pub min_models: usize,
    pub max_age: Duration,
}

impl Default for RefreshOptions {
    fn default() -> Self {
        Self {
            min_models: MIN_MODELS,
            max_age: Duration::hours(12),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const BODY: &str = r#"{
      "anthropic": {"id": "anthropic", "api": "https://api.anthropic.com/v1",
        "models": {"claude-x": {"id": "claude-x", "limit": {"context": 1000, "output": 10},
                                "provider": {"api": "https://evil.example"}}}},
      "unmapped-router": {"id": "unmapped-router",
        "models": {"m": {"id": "m"}}}
    }"#;

    fn now() -> OffsetDateTime {
        time::macros::datetime!(2026-11-27 12:00 UTC)
    }

    fn options() -> RefreshOptions {
        RefreshOptions {
            min_models: 1,
            max_age: Duration::hours(12),
        }
    }

    #[tokio::test]
    async fn downloads_caches_then_sends_etag() {
        let dir = tempfile::tempdir().unwrap();
        let cache = RefreshCache::new(dir.path());
        let mut server = mockito::Server::new_async().await;
        let first = server
            .mock("GET", "/api.json")
            .with_status(200)
            .with_header("etag", "\"v1\"")
            .with_body(BODY)
            .expect(1)
            .create_async()
            .await;
        let urls = vec![format!("{}/api.json", server.url())];
        let client = reqwest::Client::new();

        let outcome = cache
            .refresh(&client, &urls, options(), now())
            .await
            .unwrap();
        let RefreshOutcome::Updated(catalog) = outcome else {
            panic!("expected update");
        };
        first.assert_async().await;
        assert!(catalog.providers.contains_key("unmapped-router"));

        let outcome = cache
            .refresh(&client, &urls, options(), now())
            .await
            .unwrap();
        assert!(matches!(outcome, RefreshOutcome::StillFresh));

        let second = server
            .mock("GET", "/api.json")
            .match_header("if-none-match", "\"v1\"")
            .with_status(304)
            .expect(1)
            .create_async()
            .await;
        let later = now() + Duration::hours(13);
        let outcome = cache
            .refresh(&client, &urls, options(), later)
            .await
            .unwrap();
        second.assert_async().await;
        assert!(matches!(outcome, RefreshOutcome::NotModified));

        let cached = cache.load().await.unwrap().unwrap();
        assert_eq!(cached.providers["anthropic"][0].context_window, Some(1000));
    }

    #[tokio::test]
    async fn falls_back_to_gzip_mirror_when_primary_fails_validation() {
        let dir = tempfile::tempdir().unwrap();
        let cache = RefreshCache::new(dir.path());
        let mut server = mockito::Server::new_async().await;
        let _broken = server
            .mock("GET", "/api.json")
            .with_status(200)
            .with_body(r#"{"new-format": true}"#)
            .create_async()
            .await;
        let _mirror = server
            .mock("GET", "/mirror.json.gz")
            .with_status(200)
            .with_body(gzip(BODY.as_bytes()).unwrap())
            .create_async()
            .await;
        let urls = vec![
            format!("{}/api.json", server.url()),
            format!("{}/mirror.json.gz", server.url()),
        ];
        let RefreshOutcome::Updated(catalog) = cache
            .refresh(&reqwest::Client::new(), &urls, options(), now())
            .await
            .unwrap()
        else {
            panic!("expected update from mirror");
        };
        assert_eq!(catalog.model_count(), 2);
    }

    #[tokio::test]
    async fn failures_keep_previous_cache() {
        let dir = tempfile::tempdir().unwrap();
        let cache = RefreshCache::new(dir.path());
        assert!(cache.load().await.unwrap().is_none());
        let mut server = mockito::Server::new_async().await;
        let _error = server
            .mock("GET", "/api.json")
            .with_status(500)
            .create_async()
            .await;
        let _small = server
            .mock("GET", "/small.json")
            .with_status(200)
            .with_body(BODY)
            .create_async()
            .await;
        let urls = vec![format!("{}/api.json", server.url())];
        let err = cache
            .refresh(&reqwest::Client::new(), &urls, options(), now())
            .await
            .unwrap_err();
        assert!(matches!(err, crate::Error::HttpStatus(500)));

        let strict = RefreshOptions {
            min_models: 100,
            ..options()
        };
        let err = cache
            .refresh(
                &reqwest::Client::new(),
                &[format!("{}/small.json", server.url())],
                strict,
                now(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, crate::Error::Empty));
        assert!(cache.load().await.unwrap().is_none());
        assert!(matches!(
            cache
                .refresh(&reqwest::Client::new(), &[], strict, now())
                .await
                .unwrap_err(),
            crate::Error::Empty
        ));
    }

    #[test]
    fn default_options() {
        let defaults = RefreshOptions::default();
        assert_eq!(defaults.min_models, MIN_MODELS);
        assert!(DEFAULT_REFRESH_URLS[0].contains("models.dev"));
    }
}
