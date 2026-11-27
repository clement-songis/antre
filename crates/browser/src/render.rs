//! One-shot rendering of an HTML document to PNG or PDF.
//!
//! Each render launches its own browser session and closes it afterwards, so
//! untrusted documents never share a profile, cookies or storage with the
//! interactive browser tool or with each other.

use std::time::Duration;

use {
    chromiumoxide::cdp::browser_protocol::{
        emulation::SetDeviceMetricsOverrideParams,
        page::{CaptureScreenshotFormat, PrintToPdfParams},
    },
    serde::{Deserialize, Serialize},
    tokio::time::timeout,
    tracing::{info, warn},
};

use crate::{error::Error, manager::BrowserManager, types::BrowserPreference};

/// Largest viewport edge accepted for a render, in CSS pixels.
pub const MAX_RENDER_DIMENSION: u32 = 8192;

/// Output format of [`BrowserManager::render_html`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RenderFormat {
    Png,
    Pdf,
}

/// A self-contained HTML document to render.
///
/// The document is loaded with `Page.setDocumentContent`, so it must not need
/// the network: callers inline their assets and add a CSP `<meta>` tag.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderRequest {
    pub html: String,
    pub format: RenderFormat,
    pub width: u32,
    pub height: u32,
    /// PNG only: capture the whole scrollable page instead of the viewport.
    pub full_page: bool,
    /// PDF only: landscape orientation.
    pub landscape: bool,
    /// Run the browser in a container instead of on the host.
    pub sandbox: bool,
    pub timeout_ms: u64,
}

impl Default for RenderRequest {
    fn default() -> Self {
        Self {
            html: String::new(),
            format: RenderFormat::Png,
            width: 1280,
            height: 800,
            full_page: true,
            landscape: false,
            sandbox: false,
            timeout_ms: 30_000,
        }
    }
}

impl RenderRequest {
    fn validate(&self) -> Result<(), Error> {
        let valid = |edge: u32| (1..=MAX_RENDER_DIMENSION).contains(&edge);
        if !valid(self.width) || !valid(self.height) {
            return Err(Error::InvalidAction(format!(
                "render viewport must be between 1 and {MAX_RENDER_DIMENSION} pixels, got {}x{}",
                self.width, self.height
            )));
        }
        if self.html.trim().is_empty() {
            return Err(Error::InvalidAction("render document is empty".into()));
        }
        Ok(())
    }
}

impl BrowserManager {
    /// Render `request.html` in a fresh, throwaway browser session.
    pub async fn render_html(&self, request: RenderRequest) -> Result<Vec<u8>, Error> {
        request.validate()?;
        if !self.config.enabled {
            return Err(Error::InvalidAction("browser support is disabled".into()));
        }
        let limit = Duration::from_millis(request.timeout_ms.max(1));
        let session_id = self
            .pool
            .get_or_create(None, request.sandbox, Some(BrowserPreference::Auto))
            .await?;
        let outcome = timeout(limit, self.render_in_session(&session_id, &request)).await;
        if let Err(error) = self.pool.close_session(&session_id).await {
            warn!(session_id, %error, "failed to close render session");
        }
        match outcome {
            Ok(result) => result,
            Err(_) => Err(Error::Timeout(format!(
                "render did not finish within {}ms",
                request.timeout_ms
            ))),
        }
    }

    async fn render_in_session(
        &self,
        session_id: &str,
        request: &RenderRequest,
    ) -> Result<Vec<u8>, Error> {
        if let Some(kind) = self.pool.browser_kind(session_id).await
            && !kind.supports_screenshots()
        {
            return Err(Error::ScreenshotFailed(format!(
                "{kind} cannot render pages to images or PDF"
            )));
        }
        let page = self.pool.get_page(session_id).await?;
        let viewport = SetDeviceMetricsOverrideParams::builder()
            .width(request.width)
            .height(request.height)
            .device_scale_factor(self.config.device_scale_factor)
            .mobile(false)
            .build()
            .map_err(|e| Error::Cdp(format!("invalid viewport params: {e}")))?;
        page.execute(viewport)
            .await
            .map_err(|e| Error::Cdp(e.to_string()))?;
        page.set_content(&request.html)
            .await
            .map_err(|e| Error::NavigationFailed(e.to_string()))?;
        // Web fonts and layout settle after load; a best-effort wait avoids
        // capturing fallback fonts.
        let _ = page
            .evaluate("document.fonts ? document.fonts.ready.then(() => true) : true")
            .await;

        let bytes = match request.format {
            RenderFormat::Png => page
                .screenshot(
                    chromiumoxide::page::ScreenshotParams::builder()
                        .format(CaptureScreenshotFormat::Png)
                        .full_page(request.full_page)
                        .build(),
                )
                .await
                .map_err(|e| Error::ScreenshotFailed(e.to_string()))?,
            RenderFormat::Pdf => page
                .pdf(
                    PrintToPdfParams::builder()
                        .print_background(true)
                        .landscape(request.landscape)
                        .prefer_css_page_size(true)
                        .build(),
                )
                .await
                .map_err(|e| Error::PdfFailed(e.to_string()))?,
        };

        #[cfg(feature = "metrics")]
        moltis_metrics::counter!(moltis_metrics::browser::SCREENSHOTS_TOTAL).increment(1);

        info!(
            session_id,
            format = ?request.format,
            bytes = bytes.len(),
            "rendered html document"
        );
        Ok(bytes)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn request(width: u32, height: u32, html: &str) -> RenderRequest {
        RenderRequest {
            html: html.into(),
            width,
            height,
            ..RenderRequest::default()
        }
    }

    #[test]
    fn rejects_out_of_range_viewport() {
        assert!(request(0, 800, "<p>x</p>").validate().is_err());
        assert!(
            request(MAX_RENDER_DIMENSION + 1, 800, "<p>x</p>")
                .validate()
                .is_err()
        );
        assert!(request(1280, 800, "<p>x</p>").validate().is_ok());
    }

    #[test]
    fn rejects_empty_document() {
        assert!(request(1280, 800, "   ").validate().is_err());
    }

    #[test]
    fn default_request_is_a_full_page_png() {
        let request = RenderRequest::default();
        assert_eq!(request.format, RenderFormat::Png);
        assert!(request.full_page);
        assert!(!request.sandbox);
    }

    #[test]
    fn format_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&RenderFormat::Pdf).unwrap(),
            "\"pdf\""
        );
    }
}
