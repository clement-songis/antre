//! Publishing designs through the gateway share store.
//!
//! A published design is an immutable single-file snapshot of one version,
//! served only on the render origin (never the app origin) at `/p/<id>`,
//! private by default with an access key in the URL.

use {
    async_trait::async_trait,
    serde::{Deserialize, Serialize},
};

use crate::{
    CanvasService, DesignId, Error, Result,
    events::{CanvasEvent, Visibility},
    service::CommitContext,
};

/// Marker stored in snapshots so other share consumers can tell them apart.
pub const SNAPSHOT_KIND: &str = "canvas-design";

/// What the share store keeps for a published design.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishedSnapshot {
    pub kind: String,
    pub design_id: DesignId,
    pub version: u32,
    pub title: String,
    pub html: String,
}

/// A share created by [`ShareBackend::publish`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedShare {
    pub share_id: String,
    pub access_key: Option<String>,
}

/// Storage for published designs. The gateway implements it on top of its
/// session share store (same access-key model, revocation and view counts).
#[async_trait]
pub trait ShareBackend: Send + Sync {
    /// Store a snapshot, replacing any active share of the same design.
    async fn publish(
        &self,
        design_id: &DesignId,
        visibility: Visibility,
        snapshot_json: String,
        version: u32,
    ) -> anyhow::Result<CreatedShare>;

    /// Snapshot JSON of an active design share when access is granted.
    async fn resolve(
        &self,
        share_id: &str,
        access_key: Option<&str>,
    ) -> anyhow::Result<Option<String>>;

    /// Revoke a design share. Returns the design it belonged to, or `None`
    /// when the id is unknown or not a design share.
    async fn unpublish(&self, share_id: &str) -> anyhow::Result<Option<DesignId>>;
}

/// Result of [`CanvasService::publish`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Published {
    pub share_id: String,
    pub url: String,
    pub version: u32,
    pub visibility: Visibility,
    /// Shown once; private links are unusable without it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_key: Option<String>,
}

fn share_backend(service: &CanvasService) -> Result<&dyn ShareBackend> {
    service
        .shares
        .as_deref()
        .ok_or(Error::Unavailable("design publishing"))
}

/// Share ids are UUIDs; anything else is refused before reaching storage.
#[must_use]
pub fn valid_share_id(share_id: &str) -> bool {
    uuid::Uuid::parse_str(share_id).is_ok() && share_id.len() == 36
}

impl CanvasService {
    /// Publish `version` (default: the live directory, versioned first).
    pub async fn publish(
        &self,
        id: &DesignId,
        version: Option<u32>,
        visibility: Visibility,
        session_key: Option<String>,
        request_host: Option<&str>,
    ) -> Result<Published> {
        let backend = share_backend(self)?;
        let design = self.store().get(id).await?;
        let version = match version {
            Some(number) => self.store().version(id, number).await?.version,
            None => {
                let context = CommitContext {
                    reason: "publish".into(),
                    session_key,
                    run_id: None,
                };
                match self.commit(id, &context).await? {
                    Some(recorded) => recorded.version,
                    None => self.store().get(id).await?.current_version,
                }
            },
        };
        let snapshot = PublishedSnapshot {
            kind: SNAPSHOT_KIND.into(),
            design_id: id.clone(),
            version,
            title: design.title,
            html: self.single_file(id, Some(version)).await?,
        };
        let created = backend
            .publish(id, visibility, serde_json::to_string(&snapshot)?, version)
            .await
            .map_err(|error| Error::message(format!("publish failed: {error}")))?;
        let origin = self
            .origin(request_host)
            .ok_or(Error::Unavailable("canvas render origin"))?;
        let url = match created.access_key.as_deref() {
            Some(key) => format!("{origin}/p/{}?k={key}", created.share_id),
            None => format!("{origin}/p/{}", created.share_id),
        };
        self.emit(CanvasEvent::Published {
            design_id: id.clone(),
            share_id: created.share_id.clone(),
            version,
            visibility,
        });
        Ok(Published {
            share_id: created.share_id,
            url,
            version,
            visibility,
            access_key: created.access_key,
        })
    }

    /// Revoke a published link.
    pub async fn unpublish(&self, share_id: &str) -> Result<bool> {
        if !valid_share_id(share_id) {
            return Err(Error::Invalid("invalid share id".into()));
        }
        let design_id = share_backend(self)?
            .unpublish(share_id)
            .await
            .map_err(|error| Error::message(format!("unpublish failed: {error}")))?;
        let Some(design_id) = design_id else {
            return Ok(false);
        };
        self.emit(CanvasEvent::Unpublished {
            design_id: Some(design_id),
            share_id: share_id.to_string(),
        });
        Ok(true)
    }

    /// Snapshot of a published design, when the link and key are valid.
    pub async fn published(
        &self,
        share_id: &str,
        access_key: Option<&str>,
    ) -> Result<Option<PublishedSnapshot>> {
        if !valid_share_id(share_id) {
            return Ok(None);
        }
        let Some(json) = share_backend(self)?
            .resolve(share_id, access_key)
            .await
            .map_err(|error| Error::message(format!("share lookup failed: {error}")))?
        else {
            return Ok(None);
        };
        let snapshot: PublishedSnapshot = serde_json::from_str(&json)?;
        Ok((snapshot.kind == SNAPSHOT_KIND).then_some(snapshot))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{collections::HashMap, sync::Mutex};

    use super::*;

    struct Entry {
        design_id: DesignId,
        snapshot: String,
        access_key: Option<String>,
        revoked: bool,
    }

    /// In-memory share store with the gateway's semantics.
    #[derive(Default)]
    pub(crate) struct MemoryShares {
        entries: Mutex<HashMap<String, Entry>>,
    }

    #[async_trait]
    impl ShareBackend for MemoryShares {
        async fn publish(
            &self,
            design_id: &DesignId,
            visibility: Visibility,
            snapshot_json: String,
            _version: u32,
        ) -> anyhow::Result<CreatedShare> {
            let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            for entry in entries.values_mut() {
                if &entry.design_id == design_id {
                    entry.revoked = true;
                }
            }
            let share_id = uuid::Uuid::new_v4().to_string();
            let access_key = (visibility == Visibility::Private)
                .then(|| uuid::Uuid::new_v4().simple().to_string());
            entries.insert(share_id.clone(), Entry {
                design_id: design_id.clone(),
                snapshot: snapshot_json,
                access_key: access_key.clone(),
                revoked: false,
            });
            Ok(CreatedShare {
                share_id,
                access_key,
            })
        }

        async fn resolve(
            &self,
            share_id: &str,
            access_key: Option<&str>,
        ) -> anyhow::Result<Option<String>> {
            let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            Ok(entries.get(share_id).and_then(|entry| {
                let allowed = !entry.revoked
                    && entry
                        .access_key
                        .as_deref()
                        .is_none_or(|key| Some(key) == access_key);
                allowed.then(|| entry.snapshot.clone())
            }))
        }

        async fn unpublish(&self, share_id: &str) -> anyhow::Result<Option<DesignId>> {
            let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            Ok(entries.get_mut(share_id).map(|entry| {
                entry.revoked = true;
                entry.design_id.clone()
            }))
        }
    }

    #[test]
    fn share_ids_must_be_uuids() {
        assert!(valid_share_id("67e55044-10b1-426f-9247-bb680e5fe0c8"));
        assert!(!valid_share_id("67e5504410b1426f9247bb680e5fe0c8"));
        assert!(!valid_share_id("../x"));
        assert!(!valid_share_id(""));
    }
}
