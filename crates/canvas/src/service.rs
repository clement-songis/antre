//! High-level design operations shared by agent tools, RPC and REST.

use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

use {
    antre_browser::{RenderFormat, RenderRequest},
    serde::Serialize,
    tracing::{debug, info, warn},
};

use crate::{
    Design, DesignId, DesignStore, DesignVersion, Error, Result,
    csp::SANDBOX_FLAGS,
    design_systems::{DesignSystem, DesignSystemCatalog, DesignSystemInfo, TOKENS_FILE},
    events::{CanvasEvent, CanvasEventSink},
    export::{ENTRY_FILE, single_file_html, slug, with_offline_csp, zip_design},
    paths::{DesignFile, DesignsLocation, list_files, locate, resolve_in_dir},
    publish::ShareBackend,
    render::{DesignRenderer, format_label},
    store::{DesignFilter, NewDesign, NewVersion},
    token::{TokenClaims, TokenSigner},
    vendor::{Vendor, VendorEntry},
    versions::{VersionStorage, content_hash},
};

/// Runtime settings derived from `[canvas]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanvasSettings {
    pub token_ttl_secs: u64,
    pub public_url: Option<String>,
    pub script_sources: Vec<String>,
    pub allow_eval: bool,
    pub max_file_bytes: u64,
    pub render_sandbox: bool,
    pub render_timeout_secs: u64,
}

impl From<&antre_config::schema::CanvasConfig> for CanvasSettings {
    fn from(config: &antre_config::schema::CanvasConfig) -> Self {
        Self {
            token_ttl_secs: config.token_ttl_secs.clamp(60, 7 * 24 * 3600),
            public_url: config
                .public_url
                .as_deref()
                .filter(|url| antre_config::schema::is_http_origin(url))
                .map(|url| url.trim_end_matches('/').to_string()),
            script_sources: config
                .script_sources
                .iter()
                .filter(|source| antre_config::schema::is_https_origin(source))
                .cloned()
                .collect(),
            allow_eval: config.allow_eval,
            max_file_bytes: config.max_file_bytes.max(1024),
            render_sandbox: config.render_sandbox,
            render_timeout_secs: config.render_timeout_secs.clamp(1, 600),
        }
    }
}

impl Default for CanvasSettings {
    fn default() -> Self {
        Self::from(&antre_config::schema::CanvasConfig::default())
    }
}

/// Everything a [`CanvasService`] needs.
pub struct CanvasServiceParts {
    pub data_dir: PathBuf,
    pub pool: sqlx::SqlitePool,
    pub settings: CanvasSettings,
    pub renderer: Option<Arc<dyn DesignRenderer>>,
    pub shares: Option<Arc<dyn ShareBackend>>,
}

/// Request to create a design.
#[derive(Debug, Clone, Default)]
pub struct CreateDesign {
    pub title: String,
    pub html: Option<String>,
    pub design_system: Option<String>,
    pub session_key: Option<String>,
    pub project_id: Option<String>,
    pub run_id: Option<String>,
}

/// Result of [`CanvasService::create`].
#[derive(Debug, Clone)]
pub struct CreatedDesign {
    pub design: Design,
    pub dir: PathBuf,
    pub version: DesignVersion,
    pub design_system: Option<DesignSystem>,
}

/// A design with its files and recent versions.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignDetails {
    pub design: Design,
    pub dir: PathBuf,
    pub entry: PathBuf,
    pub files: Vec<DesignFile>,
    pub versions: Vec<DesignVersion>,
}

/// Who and why a version is recorded.
#[derive(Debug, Clone, Default)]
pub struct CommitContext {
    pub reason: String,
    pub session_key: Option<String>,
    pub run_id: Option<String>,
}

/// A signed link to the render origin.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewLink {
    pub url: String,
    pub origin: String,
    pub expires_at: i64,
    /// Value for the preview iframe's `sandbox` attribute.
    pub sandbox: &'static str,
    pub version: Option<u32>,
}

/// Viewport for PNG/PDF renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
    pub full_page: bool,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 800,
            full_page: true,
        }
    }
}

/// Export formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Html,
    Zip,
    Png,
    Pdf,
}

impl std::str::FromStr for ExportFormat {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "html" => Ok(Self::Html),
            "zip" => Ok(Self::Zip),
            "png" => Ok(Self::Png),
            "pdf" => Ok(Self::Pdf),
            other => Err(Error::Invalid(format!(
                "unknown export format '{other}' (expected html, zip, png or pdf)"
            ))),
        }
    }
}

impl ExportFormat {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Html => "html",
            Self::Zip => "zip",
            Self::Png => "png",
            Self::Pdf => "pdf",
        }
    }
}

/// Bytes of an export, ready to download.
#[derive(Debug, Clone)]
pub struct Export {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
    pub file_name: String,
}

/// A file served by the render origin.
#[derive(Debug, Clone)]
pub struct ServedFile {
    pub bytes: Vec<u8>,
    pub content_type: String,
    pub is_html: bool,
}

/// Public settings for the web UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasInfo {
    pub origin: Option<String>,
    pub sandbox: &'static str,
    pub bridge_protocol: &'static str,
    pub vendor: Vec<VendorEntry>,
    pub design_systems: Vec<DesignSystemInfo>,
    pub token_ttl_secs: u64,
}

/// Version of the postMessage contract documented in `docs/src/canvas.md`.
pub const BRIDGE_PROTOCOL: &str = "antre.canvas/1";

#[derive(Default)]
struct TurnActivity {
    run_id: Option<String>,
    designs: BTreeSet<DesignId>,
}

/// Designs touched by one agent turn, to version when the turn ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnDesigns {
    pub session_key: String,
    pub run_id: Option<String>,
    pub designs: Vec<DesignId>,
}

/// The canvas service.
pub struct CanvasService {
    pub(crate) settings: CanvasSettings,
    designs_root: PathBuf,
    store: DesignStore,
    versions: VersionStorage,
    pub(crate) signer: TokenSigner,
    pub(crate) vendor: Vendor,
    pub(crate) frames: crate::frames::FrameStore,
    pub(crate) comment_store: crate::comments::CommentStore,
    catalog: DesignSystemCatalog,
    renderer: Option<Arc<dyn DesignRenderer>>,
    pub(crate) shares: Option<Arc<dyn ShareBackend>>,
    events: OnceLock<Arc<dyn CanvasEventSink>>,
    render_address: OnceLock<std::net::SocketAddr>,
    activity: Mutex<HashMap<String, TurnActivity>>,
    /// Content hash of the latest version per design, guarded so concurrent
    /// commits of the same design cannot interleave.
    commits: tokio::sync::Mutex<HashMap<DesignId, String>>,
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn starter_html(title: &str, with_tokens: bool) -> String {
    let tokens = if with_tokens {
        format!("\n  <link rel=\"stylesheet\" href=\"{TOKENS_FILE}\">")
    } else {
        String::new()
    };
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n  <meta charset=\"utf-8\">\n  \
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n  \
         <title>{}</title>{tokens}\n</head>\n<body data-mid=\"page\">\n</body>\n</html>\n",
        escape_html(title)
    )
}

/// Link `tokens.css` from model-supplied HTML that does not reference it.
fn ensure_tokens_link(html: &str) -> String {
    if html.contains(TOKENS_FILE) {
        return html.to_string();
    }
    let link = format!("<link rel=\"stylesheet\" href=\"{TOKENS_FILE}\">");
    let lower = html.to_ascii_lowercase();
    match lower.find("<head") {
        Some(start) => match lower[start..].find('>') {
            Some(end) => {
                let at = start + end + 1;
                format!("{}{link}{}", &html[..at], &html[at..])
            },
            None => format!("{link}{html}"),
        },
        None => format!("{link}{html}"),
    }
}

impl CanvasService {
    pub fn new(parts: CanvasServiceParts) -> Result<Self> {
        let designs_root = parts.data_dir.join("designs");
        std::fs::create_dir_all(&designs_root)?;
        let designs_root = std::fs::canonicalize(designs_root)?;
        let vendor_dir = parts.data_dir.join("canvas").join("vendor");
        Ok(Self {
            settings: parts.settings,
            designs_root,
            comment_store: crate::comments::CommentStore::new(parts.pool.clone()),
            store: DesignStore::new(parts.pool),
            frames: crate::frames::FrameStore::default(),
            versions: VersionStorage::new(parts.data_dir.join("design-versions")),
            signer: TokenSigner::random(),
            vendor: Vendor::new(Some(vendor_dir)),
            catalog: DesignSystemCatalog::new(parts.data_dir.join("skills")),
            renderer: parts.renderer,
            shares: parts.shares,
            events: OnceLock::new(),
            render_address: OnceLock::new(),
            activity: Mutex::new(HashMap::new()),
            commits: tokio::sync::Mutex::new(HashMap::new()),
        })
    }

    /// Install the event sink (once; later calls are ignored).
    pub fn set_event_sink(&self, sink: Arc<dyn CanvasEventSink>) {
        let _ = self.events.set(sink);
    }

    /// Record the address the render server is bound to.
    pub fn set_render_address(&self, address: std::net::SocketAddr) {
        let _ = self.render_address.set(address);
    }

    /// Port of the bound render server.
    #[must_use]
    pub fn render_port(&self) -> Option<u16> {
        self.render_address.get().map(std::net::SocketAddr::port)
    }

    /// Origin preview links point to: `public_url` when set, else the host
    /// the client used to reach the app (so LAN and loopback both work) on
    /// the render port, else the bound address.
    #[must_use]
    pub fn origin(&self, request_host: Option<&str>) -> Option<String> {
        if let Some(url) = self.settings.public_url.clone() {
            return Some(url);
        }
        let address = self.render_address.get()?;
        if let Some(host) = request_host.and_then(crate::csp::host_without_port) {
            return Some(format!("http://{host}:{}", address.port()));
        }
        let ip = address.ip();
        let host = if ip.is_unspecified() {
            "127.0.0.1".to_string()
        } else if ip.is_ipv6() {
            format!("[{ip}]")
        } else {
            ip.to_string()
        };
        Some(format!("http://{host}:{}", address.port()))
    }

    pub(crate) fn emit(&self, event: CanvasEvent) {
        if let Some(sink) = self.events.get() {
            sink.emit(event);
        }
    }

    #[must_use]
    pub fn designs_root(&self) -> &Path {
        &self.designs_root
    }

    #[must_use]
    pub fn store(&self) -> &DesignStore {
        &self.store
    }

    #[must_use]
    pub fn catalog(&self) -> &DesignSystemCatalog {
        &self.catalog
    }

    #[must_use]
    pub fn design_dir(&self, id: &DesignId) -> PathBuf {
        self.designs_root.join(id.as_str())
    }

    /// Classify a path the agent is about to write.
    #[must_use]
    pub fn locate(&self, path: &Path) -> DesignsLocation {
        locate(&self.designs_root, path)
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, request), fields(title = %request.title)))]
    pub async fn create(&self, request: CreateDesign) -> Result<CreatedDesign> {
        let design_system = request
            .design_system
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .map(|name| self.catalog.get(name))
            .transpose()?;
        let title = crate::store::clean_title(&request.title)?;
        let html = match request
            .html
            .as_deref()
            .filter(|html| !html.trim().is_empty())
        {
            Some(html) if design_system.is_some() => ensure_tokens_link(html),
            Some(html) => html.to_string(),
            None => starter_html(&title, design_system.is_some()),
        };
        if html.len() as u64 > self.settings.max_file_bytes {
            return Err(Error::TooLarge {
                path: ENTRY_FILE.into(),
                bytes: html.len() as u64,
                limit: self.settings.max_file_bytes,
            });
        }

        let id = DesignId::generate();
        let dir = self.design_dir(&id);
        tokio::fs::create_dir(&dir).await?;
        let created = self
            .populate(&id, &dir, &title, &html, design_system.as_ref(), &request)
            .await;
        let (design, version) = match created {
            Ok(created) => created,
            Err(error) => {
                if let Err(cleanup) = tokio::fs::remove_dir_all(&dir).await {
                    warn!(design_id = %id, %cleanup, "failed to remove half-created design");
                }
                return Err(error);
            },
        };
        if let Some(session_key) = request.session_key.as_deref() {
            self.touch(session_key, request.run_id.as_deref(), &id);
        }
        #[cfg(feature = "metrics")]
        antre_metrics::counter!(antre_metrics::canvas::DESIGNS_CREATED_TOTAL).increment(1);
        info!(design_id = %id, "design created");
        self.emit(CanvasEvent::DesignCreated {
            design: design.clone(),
        });
        self.emit(CanvasEvent::VersionCreated {
            design_id: id,
            version: version.clone(),
            reverted_from: None,
        });
        Ok(CreatedDesign {
            design,
            dir,
            version,
            design_system,
        })
    }

    async fn populate(
        &self,
        id: &DesignId,
        dir: &Path,
        title: &str,
        html: &str,
        design_system: Option<&DesignSystem>,
        request: &CreateDesign,
    ) -> Result<(Design, DesignVersion)> {
        tokio::fs::write(dir.join(ENTRY_FILE), html).await?;
        if let Some(system) = design_system {
            tokio::fs::write(dir.join(TOKENS_FILE), &system.tokens).await?;
        }
        self.store
            .insert(id, NewDesign {
                title: title.to_string(),
                session_key: request.session_key.clone(),
                project_id: request.project_id.clone(),
                design_system: design_system.map(|system| system.info.name.clone()),
            })
            .await?;
        let context = CommitContext {
            reason: "created".into(),
            session_key: request.session_key.clone(),
            run_id: request.run_id.clone(),
        };
        let version = self
            .record_version(id, &context)
            .await?
            .ok_or_else(|| Error::message("new design produced no version"))?;
        Ok((self.store.get(id).await?, version))
    }

    /// Metadata, files and the latest versions of a design.
    pub async fn open(&self, id: &DesignId) -> Result<DesignDetails> {
        let design = self.store.get(id).await?;
        let dir = self.design_dir(id);
        let listing_dir = dir.clone();
        let files = tokio::task::spawn_blocking(move || list_files(&listing_dir))
            .await
            .map_err(|error| Error::message(format!("listing task failed: {error}")))??;
        let versions = self.store.versions(id, 20).await?;
        Ok(DesignDetails {
            entry: dir.join(ENTRY_FILE),
            design,
            dir,
            files,
            versions,
        })
    }

    pub async fn list(&self, filter: &DesignFilter) -> Result<Vec<Design>> {
        self.store.list(filter).await
    }

    pub async fn versions(&self, id: &DesignId, limit: u32) -> Result<Vec<DesignVersion>> {
        self.store.get(id).await?;
        self.store.versions(id, limit).await
    }

    pub async fn rename(&self, id: &DesignId, title: &str) -> Result<Design> {
        self.store.rename(id, title).await
    }

    /// Snapshot the design if its files changed since the latest version.
    /// Returns `None` when nothing changed.
    pub async fn commit(
        &self,
        id: &DesignId,
        context: &CommitContext,
    ) -> Result<Option<DesignVersion>> {
        let version = self.record_version(id, context).await?;
        if let Some(ref version) = version {
            self.emit(CanvasEvent::VersionCreated {
                design_id: id.clone(),
                version: version.clone(),
                reverted_from: None,
            });
        }
        Ok(version)
    }

    async fn record_version(
        &self,
        id: &DesignId,
        context: &CommitContext,
    ) -> Result<Option<DesignVersion>> {
        let mut latest_hashes = self.commits.lock().await;
        let dir = self.design_dir(id);
        let hash = content_hash(&dir).await?;
        let latest = match latest_hashes.get(id) {
            Some(hash) => Some(hash.clone()),
            None => self
                .store
                .latest_version(id)
                .await?
                .map(|version| version.content_hash),
        };
        if latest.as_deref() == Some(hash.as_str()) {
            debug!(design_id = %id, "design unchanged, no new version");
            return Ok(None);
        }
        let scan_dir = dir.clone();
        let special =
            tokio::task::spawn_blocking(move || crate::paths::first_special_entry(&scan_dir))
                .await
                .map_err(|error| Error::message(format!("scan task failed: {error}")))?;
        if let Some(entry) = special {
            return Err(Error::Invalid(format!(
                "design contains '{entry}', which is not a regular file; designs may only \
                 contain regular files and folders (remove it to resume versioning)"
            )));
        }
        let checkpoint_id = self.versions.snapshot(&dir, &context.reason).await?;
        let version = self
            .store
            .append_version(id, NewVersion {
                checkpoint_id,
                content_hash: hash.clone(),
                reason: context.reason.chars().take(200).collect(),
                session_key: context.session_key.clone(),
                run_id: context.run_id.clone(),
            })
            .await?;
        latest_hashes.insert(id.clone(), hash);
        #[cfg(feature = "metrics")]
        antre_metrics::counter!(antre_metrics::canvas::VERSIONS_CREATED_TOTAL).increment(1);
        info!(design_id = %id, version = version.version, reason = %version.reason, "design version recorded");
        Ok(Some(version))
    }

    /// Restore `version` into the live directory and record it as a new
    /// version (history stays append-only).
    pub async fn revert(
        &self,
        id: &DesignId,
        version: u32,
        session_key: Option<String>,
    ) -> Result<DesignVersion> {
        let target = self.store.version(id, version).await?;
        {
            // Hold the commit lock so no turn-end snapshot runs mid-restore.
            let mut latest_hashes = self.commits.lock().await;
            self.versions.restore(&target.checkpoint_id).await?;
            latest_hashes.remove(id);
        }
        let context = CommitContext {
            reason: format!("revert to v{version}"),
            session_key,
            run_id: None,
        };
        match self.record_version(id, &context).await? {
            Some(recorded) => {
                self.emit(CanvasEvent::VersionCreated {
                    design_id: id.clone(),
                    version: recorded.clone(),
                    reverted_from: Some(version),
                });
                Ok(recorded)
            },
            None => self
                .store
                .latest_version(id)
                .await?
                .ok_or_else(|| Error::message("design has no versions")),
        }
    }

    /// Directory to read: the live design or a version snapshot.
    pub async fn source_dir(&self, id: &DesignId, version: Option<u32>) -> Result<PathBuf> {
        match version {
            None => {
                if !self.store.exists(id).await? {
                    return Err(Error::NotFound(id.to_string()));
                }
                Ok(self.design_dir(id))
            },
            Some(number) => {
                let version = self.store.version(id, number).await?;
                self.versions.snapshot_dir(&version.checkpoint_id).await
            },
        }
    }

    /// Signed render-origin link for a design (live or pinned version).
    pub async fn preview_link(
        &self,
        id: &DesignId,
        version: Option<u32>,
        request_host: Option<&str>,
    ) -> Result<PreviewLink> {
        if let Some(number) = version {
            self.store.version(id, number).await?;
        } else if !self.store.exists(id).await? {
            return Err(Error::NotFound(id.to_string()));
        }
        let origin = self
            .origin(request_host)
            .ok_or(Error::Unavailable("canvas render origin"))?;
        let token = self.signer.sign(id, version, self.settings.token_ttl_secs);
        let claims = self.signer.verify(&token)?;
        Ok(PreviewLink {
            url: format!("{origin}/d/{token}/"),
            origin,
            expires_at: claims.expires_at,
            sandbox: SANDBOX_FLAGS,
            version,
        })
    }

    pub(crate) fn verify_token(&self, token: &str) -> Result<TokenClaims> {
        self.signer.verify(token)
    }

    /// Read a file for the render origin.
    pub async fn serve_file(&self, claims: &TokenClaims, relative: &str) -> Result<ServedFile> {
        let dir = self.source_dir(&claims.design_id, claims.version).await?;
        let relative = relative.to_string();
        let limit = self.settings.max_file_bytes;
        tokio::task::spawn_blocking(move || {
            let path = resolve_in_dir(&dir, &relative)?;
            let bytes = std::fs::metadata(&path)?.len();
            if bytes > limit {
                return Err(Error::TooLarge {
                    path: relative,
                    bytes,
                    limit,
                });
            }
            let mime = mime_guess::from_path(&path).first_or_octet_stream();
            let is_html = mime.essence_str() == "text/html";
            let content_type = if mime.type_() == "text" || mime.essence_str() == "image/svg+xml" {
                format!("{}; charset=utf-8", mime.essence_str())
            } else {
                mime.essence_str().to_string()
            };
            Ok(ServedFile {
                bytes: std::fs::read(path)?,
                content_type,
                is_html,
            })
        })
        .await
        .map_err(|error| Error::message(format!("read task failed: {error}")))?
    }

    /// Single-file HTML of a design version (or the live directory).
    pub async fn single_file(&self, id: &DesignId, version: Option<u32>) -> Result<String> {
        let dir = self.source_dir(id, version).await?;
        let vendor = self.vendor.clone();
        let limit = self.settings.max_file_bytes;
        tokio::task::spawn_blocking(move || single_file_html(&dir, &vendor, limit))
            .await
            .map_err(|error| Error::message(format!("export task failed: {error}")))?
    }

    /// Render a design to PNG or PDF through the headless browser.
    pub async fn render(
        &self,
        id: &DesignId,
        version: Option<u32>,
        format: RenderFormat,
        viewport: Viewport,
    ) -> Result<Vec<u8>> {
        let renderer = self
            .renderer
            .as_ref()
            .ok_or(Error::Unavailable("headless browser rendering"))?;
        let html = with_offline_csp(&self.single_file(id, version).await?)?;
        renderer
            .render(RenderRequest {
                html,
                format,
                width: viewport.width,
                height: viewport.height,
                full_page: viewport.full_page,
                landscape: false,
                sandbox: self.settings.render_sandbox,
                timeout_ms: self.settings.render_timeout_secs.saturating_mul(1000),
            })
            .await
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub async fn export(
        &self,
        id: &DesignId,
        version: Option<u32>,
        format: ExportFormat,
        viewport: Viewport,
    ) -> Result<Export> {
        let design = self.store.get(id).await?;
        let base = slug(&design.title);
        let suffix = version.map_or_else(String::new, |v| format!("-v{v}"));
        let export = match format {
            ExportFormat::Html => Export {
                bytes: self.single_file(id, version).await?.into_bytes(),
                content_type: "text/html; charset=utf-8",
                file_name: format!("{base}{suffix}.html"),
            },
            ExportFormat::Zip => {
                let dir = self.source_dir(id, version).await?;
                let folder = format!("{base}{suffix}");
                let limit = self.settings.max_file_bytes;
                let bytes = tokio::task::spawn_blocking(move || zip_design(&dir, &folder, limit))
                    .await
                    .map_err(|error| Error::message(format!("zip task failed: {error}")))??;
                Export {
                    bytes,
                    content_type: "application/zip",
                    file_name: format!("{base}{suffix}.zip"),
                }
            },
            ExportFormat::Png | ExportFormat::Pdf => {
                let render_format = if format == ExportFormat::Png {
                    RenderFormat::Png
                } else {
                    RenderFormat::Pdf
                };
                Export {
                    bytes: self.render(id, version, render_format, viewport).await?,
                    content_type: if format == ExportFormat::Png {
                        "image/png"
                    } else {
                        "application/pdf"
                    },
                    file_name: format!("{base}{suffix}.{}", format_label(render_format)),
                }
            },
        };
        #[cfg(feature = "metrics")]
        antre_metrics::counter!(antre_metrics::canvas::EXPORTS_TOTAL, "format" => format.label())
            .increment(1);
        Ok(export)
    }

    /// Settings the web UI needs to frame previews.
    pub async fn info(&self, request_host: Option<&str>) -> CanvasInfo {
        let catalog = self.catalog.clone();
        let design_systems = tokio::task::spawn_blocking(move || catalog.list())
            .await
            .unwrap_or_default();
        CanvasInfo {
            origin: self.origin(request_host),
            sandbox: SANDBOX_FLAGS,
            bridge_protocol: BRIDGE_PROTOCOL,
            vendor: self.vendor.list().await,
            design_systems,
            token_ttl_secs: self.settings.token_ttl_secs,
        }
    }

    /// Note that a turn touched a design. When the session moved to a new
    /// run, the previous run's designs are returned so they can be versioned.
    pub fn touch(
        &self,
        session_key: &str,
        run_id: Option<&str>,
        id: &DesignId,
    ) -> Option<TurnDesigns> {
        let mut activity = self.activity.lock().unwrap_or_else(|e| e.into_inner());
        let entry = activity.entry(session_key.to_string()).or_default();
        let mut flushed = None;
        if let Some(run_id) = run_id {
            if entry
                .run_id
                .as_deref()
                .is_some_and(|current| current != run_id)
                && !entry.designs.is_empty()
            {
                flushed = Some(TurnDesigns {
                    session_key: session_key.to_string(),
                    run_id: entry.run_id.take(),
                    designs: std::mem::take(&mut entry.designs).into_iter().collect(),
                });
            }
            entry.run_id = Some(run_id.to_string());
        }
        entry.designs.insert(id.clone());
        flushed
    }

    /// Designs touched by the session's current turn, clearing the record.
    pub fn end_turn(&self, session_key: &str) -> Option<TurnDesigns> {
        let mut activity = self.activity.lock().unwrap_or_else(|e| e.into_inner());
        let entry = activity.remove(session_key)?;
        (!entry.designs.is_empty()).then(|| TurnDesigns {
            session_key: session_key.to_string(),
            run_id: entry.run_id,
            designs: entry.designs.into_iter().collect(),
        })
    }

    /// Version every design a finished turn touched.
    pub async fn commit_turn(&self, turn: TurnDesigns) {
        for id in turn.designs {
            let context = CommitContext {
                reason: "agent turn".into(),
                session_key: Some(turn.session_key.clone()),
                run_id: turn.run_id.clone(),
            };
            if let Err(error) = self.commit(&id, &context).await {
                warn!(design_id = %id, %error, "failed to version design at end of turn");
            }
        }
    }
}

#[cfg(test)]
#[path = "service_tests.rs"]
pub(crate) mod tests;
