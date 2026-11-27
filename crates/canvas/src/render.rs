//! Headless rendering of designs to PNG and PDF.

use std::sync::Arc;

use {
    async_trait::async_trait,
    moltis_browser::{BrowserConfig, BrowserManager, RenderFormat, RenderRequest},
    tokio::sync::OnceCell,
};

use crate::{Error, Result};

/// Renders a self-contained HTML document.
#[async_trait]
pub trait DesignRenderer: Send + Sync {
    async fn render(&self, request: RenderRequest) -> Result<Vec<u8>>;
}

/// Renderer with its own [`BrowserManager`], separate from the agent's
/// browser tool so renders of untrusted designs never share its profile.
pub struct BrowserRenderer {
    config: BrowserConfig,
    manager: OnceCell<Arc<BrowserManager>>,
}

impl BrowserRenderer {
    /// Build from the browser tool settings (Chrome path, container image,
    /// scale factor), forcing a non-persistent profile.
    #[must_use]
    pub fn new(browser: &moltis_config::schema::BrowserConfig, container_prefix: String) -> Self {
        let mut config = BrowserConfig::from(browser);
        config.enabled = true;
        config.persist_profile = false;
        config.profile_dir = None;
        config.allowed_domains = Vec::new();
        config.container_prefix = container_prefix;
        Self {
            config,
            manager: OnceCell::new(),
        }
    }

    async fn manager(&self) -> Result<Arc<BrowserManager>> {
        let config = self.config.clone();
        let manager = self
            .manager
            .get_or_try_init(|| async move {
                tokio::task::spawn_blocking(move || Arc::new(BrowserManager::new(config)))
                    .await
                    .map_err(|error| Error::Render(format!("browser init failed: {error}")))
            })
            .await?;
        Ok(Arc::clone(manager))
    }

    /// Close any browser this renderer started.
    pub async fn shutdown(&self) {
        if let Some(manager) = self.manager.get() {
            manager.shutdown().await;
        }
    }
}

#[async_trait]
impl DesignRenderer for BrowserRenderer {
    async fn render(&self, request: RenderRequest) -> Result<Vec<u8>> {
        #[cfg(feature = "metrics")]
        let started = std::time::Instant::now();
        let format = request.format;
        let result = self
            .manager()
            .await?
            .render_html(request)
            .await
            .map_err(|error| Error::Render(error.to_string()));
        #[cfg(feature = "metrics")]
        moltis_metrics::histogram!(
            moltis_metrics::canvas::RENDER_DURATION_SECONDS,
            "format" => format_label(format)
        )
        .record(started.elapsed().as_secs_f64());
        #[cfg(not(feature = "metrics"))]
        let _ = format;
        result
    }
}

/// Lowercase name of a render format.
#[must_use]
pub fn format_label(format: RenderFormat) -> &'static str {
    match format {
        RenderFormat::Png => "png",
        RenderFormat::Pdf => "pdf",
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Mutex;

    use super::*;

    /// Records requests and returns fixed bytes.
    #[derive(Default)]
    pub(crate) struct FakeRenderer {
        pub(crate) requests: Mutex<Vec<RenderRequest>>,
    }

    #[async_trait]
    impl DesignRenderer for FakeRenderer {
        async fn render(&self, request: RenderRequest) -> Result<Vec<u8>> {
            let bytes = match request.format {
                RenderFormat::Png => b"\x89PNG\r\n\x1a\n-fake".to_vec(),
                RenderFormat::Pdf => b"%PDF-1.7 fake".to_vec(),
            };
            self.requests
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(request);
            Ok(bytes)
        }
    }

    #[test]
    fn browser_renderer_never_persists_profiles() {
        let tool = moltis_config::schema::BrowserConfig {
            persist_profile: true,
            enabled: false,
            ..Default::default()
        };
        let renderer = BrowserRenderer::new(&tool, "canvas".into());
        assert!(renderer.config.enabled);
        assert!(!renderer.config.persist_profile);
        assert!(renderer.config.profile_dir.is_none());
    }

    #[test]
    fn labels() {
        assert_eq!(format_label(RenderFormat::Png), "png");
        assert_eq!(format_label(RenderFormat::Pdf), "pdf");
    }
}
