//! Staged documents ("frames"): single HTML pages the web UI shows inline
//! through the render origin, such as MCP App `ui://` resources and HTML
//! snippets from replies. They live in memory only, behind a signed link,
//! and are evicted oldest first.

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
};

use {
    serde::{Deserialize, Serialize},
    time::OffsetDateTime,
};

use crate::{DesignId, Error, Result, csp::SANDBOX_FLAGS, service::PreviewLink};

/// Most frames kept at once.
pub const MAX_FRAMES: usize = 128;
/// Most bytes of HTML kept across all frames.
pub const MAX_FRAMES_BYTES: usize = 64 * 1024 * 1024;
/// Most declared network origins accepted per frame.
const MAX_DECLARED_ORIGINS: usize = 32;

/// What a frame holds; decides the bridge and the network policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FrameKind {
    /// HTML written by the agent in a reply. Gets the preview bridge and no
    /// network.
    Artifact,
    /// An MCP App view. Talks to the host with its own JSON-RPC protocol, so
    /// no bridge; may reach the origins its server declared.
    McpApp,
}

/// Network origins a frame may reach, as declared by an MCP App
/// (`_meta.ui.csp`). Only bare `https://` origins are kept.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FrameNetwork {
    pub connect_domains: Vec<String>,
    pub resource_domains: Vec<String>,
}

impl FrameNetwork {
    fn sanitized(self) -> Self {
        let keep = |origins: Vec<String>| {
            origins
                .into_iter()
                .map(|origin| origin.trim_end_matches('/').to_string())
                .filter(|origin| antre_config::schema::is_https_origin(origin))
                .take(MAX_DECLARED_ORIGINS)
                .collect()
        };
        Self {
            connect_domains: keep(self.connect_domains),
            resource_domains: keep(self.resource_domains),
        }
    }
}

/// A document to stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageFrame {
    pub kind: FrameKind,
    pub html: String,
    pub network: FrameNetwork,
}

/// A staged document.
#[derive(Debug)]
pub struct Frame {
    pub kind: FrameKind,
    pub html: String,
    pub network: FrameNetwork,
    pub expires_at: i64,
}

#[derive(Default)]
struct Entries {
    frames: HashMap<DesignId, Arc<Frame>>,
    order: VecDeque<DesignId>,
    bytes: usize,
}

impl Entries {
    fn evict_oldest(&mut self) {
        if let Some(id) = self.order.pop_front()
            && let Some(frame) = self.frames.remove(&id)
        {
            self.bytes = self.bytes.saturating_sub(frame.html.len());
        }
    }

    fn purge_expired(&mut self, now: i64) {
        let expired: Vec<DesignId> = self
            .frames
            .iter()
            .filter(|(_, frame)| frame.expires_at < now)
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            if let Some(frame) = self.frames.remove(&id) {
                self.bytes = self.bytes.saturating_sub(frame.html.len());
            }
            self.order.retain(|known| known != &id);
        }
    }
}

/// In-memory store of staged documents.
#[derive(Default)]
pub struct FrameStore {
    entries: Mutex<Entries>,
}

impl FrameStore {
    /// Keep `frame` until `expires_at`, evicting the oldest frames past the
    /// count or byte budget. Returns its id.
    pub fn insert(&self, frame: StageFrame, expires_at: i64, max_bytes: usize) -> Result<DesignId> {
        if frame.html.trim().is_empty() {
            return Err(Error::Invalid("empty document".into()));
        }
        let limit = max_bytes.min(MAX_FRAMES_BYTES);
        if frame.html.len() > limit {
            return Err(Error::TooLarge {
                path: "document".into(),
                bytes: frame.html.len() as u64,
                limit: limit as u64,
            });
        }
        let id = DesignId::generate();
        let size = frame.html.len();
        let stored = Arc::new(Frame {
            kind: frame.kind,
            html: frame.html,
            network: frame.network.sanitized(),
            expires_at,
        });
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.purge_expired(OffsetDateTime::now_utc().unix_timestamp());
        while !entries.order.is_empty()
            && (entries.order.len() >= MAX_FRAMES || entries.bytes + size > MAX_FRAMES_BYTES)
        {
            entries.evict_oldest();
        }
        entries.frames.insert(id.clone(), stored);
        entries.order.push_back(id.clone());
        entries.bytes += size;
        Ok(id)
    }

    /// A frame that has not expired.
    #[must_use]
    pub fn get(&self, id: &DesignId) -> Option<Arc<Frame>> {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries
            .frames
            .get(id)
            .filter(|frame| frame.expires_at >= now)
            .cloned()
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .frames
            .len()
    }
}

impl crate::CanvasService {
    /// Stage a single HTML document and return a render-origin link to it.
    pub fn stage_frame(
        &self,
        frame: StageFrame,
        request_host: Option<&str>,
    ) -> Result<PreviewLink> {
        let origin = self
            .origin(request_host)
            .ok_or(Error::Unavailable("canvas render origin"))?;
        let max_bytes = usize::try_from(self.settings.max_file_bytes).unwrap_or(usize::MAX);
        let ttl = self.settings.token_ttl_secs;
        let expires_at = OffsetDateTime::now_utc()
            .unix_timestamp()
            .saturating_add(i64::try_from(ttl).unwrap_or(i64::MAX));
        let id = self.frames.insert(frame, expires_at, max_bytes)?;
        let token = self.signer.sign_frame(&id, ttl);
        Ok(PreviewLink {
            url: format!("{origin}/f/{token}/"),
            origin,
            expires_at,
            sandbox: SANDBOX_FLAGS,
            version: None,
        })
    }

    /// `frame-src` sources the app page needs to frame the render origin:
    /// `public_url` when set, else the render port on any host (preview links
    /// reuse the host the browser used to reach the app).
    #[must_use]
    pub fn frame_sources(&self) -> Vec<String> {
        if let Some(url) = self.settings.public_url.clone() {
            return vec![url];
        }
        self.render_port()
            .map(|port| vec![format!("http://*:{port}"), format!("https://*:{port}")])
            .unwrap_or_default()
    }

    /// The staged document a frame token grants access to.
    pub(crate) fn frame_for_token(&self, token: &str) -> Result<Arc<Frame>> {
        let claims = self.signer.verify_frame(token)?;
        self.frames
            .get(&claims.frame_id)
            .ok_or_else(|| Error::NotFound("document".into()))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn stage(html: &str) -> StageFrame {
        StageFrame {
            kind: FrameKind::Artifact,
            html: html.into(),
            network: FrameNetwork::default(),
        }
    }

    fn far_future() -> i64 {
        OffsetDateTime::now_utc().unix_timestamp() + 3600
    }

    #[test]
    fn stores_and_expires_frames() {
        let store = FrameStore::default();
        let id = store.insert(stage("<p>a</p>"), far_future(), 1024).unwrap();
        assert_eq!(store.get(&id).unwrap().html, "<p>a</p>");
        let old = store.insert(stage("<p>b</p>"), 10, 1024).unwrap();
        assert!(store.get(&old).is_none(), "expired frames are hidden");
        store.insert(stage("<p>c</p>"), far_future(), 1024).unwrap();
        assert_eq!(store.len(), 2, "expired frames are purged on insert");
    }

    #[test]
    fn rejects_empty_and_oversized_documents() {
        let store = FrameStore::default();
        assert!(matches!(
            store.insert(stage("  \n"), far_future(), 1024),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            store.insert(stage(&"x".repeat(2048)), far_future(), 1024),
            Err(Error::TooLarge { .. })
        ));
    }

    #[test]
    fn evicts_oldest_past_the_count_budget() {
        let store = FrameStore::default();
        let first = store.insert(stage("<p>0</p>"), far_future(), 1024).unwrap();
        let mut last = first.clone();
        for index in 1..=MAX_FRAMES {
            last = store
                .insert(stage(&format!("<p>{index}</p>")), far_future(), 1024)
                .unwrap();
        }
        assert_eq!(store.len(), MAX_FRAMES);
        assert!(store.get(&first).is_none());
        assert!(store.get(&last).is_some());
    }

    #[test]
    fn declared_network_keeps_only_https_origins() {
        let network = FrameNetwork {
            connect_domains: vec![
                "https://api.example.com/".into(),
                "http://plain.example.com".into(),
                "https://*.example.com".into(),
            ],
            resource_domains: vec!["https://cdn.example.com".into(), "'self'".into()],
        }
        .sanitized();
        assert_eq!(network.connect_domains, vec!["https://api.example.com"]);
        assert_eq!(network.resource_domains, vec!["https://cdn.example.com"]);
    }

    #[test]
    fn frame_kind_uses_kebab_case() {
        assert_eq!(
            serde_json::to_string(&FrameKind::McpApp).unwrap(),
            "\"mcp-app\""
        );
        assert_eq!(
            serde_json::from_str::<FrameKind>("\"artifact\"").unwrap(),
            FrameKind::Artifact
        );
    }
}
