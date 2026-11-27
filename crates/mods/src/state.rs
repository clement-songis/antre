//! Persisted mod state (`<mods dir>/state.json`): installed mods, grants,
//! pending updates and shell-hook pins. Written atomically.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use {
    serde::{Deserialize, Serialize},
    time::OffsetDateTime,
};

use crate::{
    capability::ConsentHash,
    error::Result,
    grant::{Grant, GrantOrigin},
    id::ModId,
};

const STATE_SCHEMA: u32 = 1;

/// Where an installed mod came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModSource {
    LocalPath { path: String },
}

/// A newer version waiting for consent because it widens capabilities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingUpdate {
    pub version: String,
    pub content_hash: String,
    pub consent_hash: ConsentHash,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledMod {
    pub version: String,
    pub content_hash: String,
    pub source: ModSource,
    pub enabled: bool,
    #[serde(default)]
    pub grant: Option<Grant>,
    #[serde(default)]
    pub pending_update: Option<PendingUpdate>,
    #[serde(with = "time::serde::rfc3339")]
    pub installed_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// Content pin for a filesystem shell hook (`HOOK.md` folder).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookPin {
    pub content_hash: String,
    pub path: String,
    pub origin: GrantOrigin,
    #[serde(with = "time::serde::rfc3339")]
    pub pinned_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModsState {
    pub schema: u32,
    #[serde(default)]
    pub mods: BTreeMap<ModId, InstalledMod>,
    #[serde(default)]
    pub hook_pins: BTreeMap<String, HookPin>,
    /// Set once existing shell hooks have been pinned as migrated.
    #[serde(default)]
    pub hooks_migrated: bool,
}

impl Default for ModsState {
    fn default() -> Self {
        Self {
            schema: STATE_SCHEMA,
            mods: BTreeMap::new(),
            hook_pins: BTreeMap::new(),
            hooks_migrated: false,
        }
    }
}

/// Reads and writes the state file.
#[derive(Debug, Clone)]
pub struct StateFile {
    path: PathBuf,
}

impl StateFile {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<ModsState> {
        match fs::read_to_string(&self.path) {
            Ok(text) => Ok(serde_json::from_str(&text)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(ModsState::default()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn save(&self, state: &ModsState) -> Result<()> {
        write_atomic(&self.path, &serde_json::to_vec_pretty(state)?)
    }
}

/// Write via a sibling temp file and rename, so readers never see a torn file.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use {super::*, crate::capability::CapabilitySet};

    #[test]
    fn missing_file_loads_default() {
        let dir = tempfile::tempdir().unwrap();
        let file = StateFile::new(dir.path().join("state.json"));
        let state = file.load().unwrap();
        assert!(state.mods.is_empty());
        assert!(!state.hooks_migrated);
    }

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let file = StateFile::new(dir.path().join("nested/state.json"));
        let mut state = ModsState::default();
        let now = OffsetDateTime::now_utc();
        state
            .mods
            .insert(ModId::parse("acme.echo").unwrap(), InstalledMod {
                version: "0.1.0".into(),
                content_hash: "abc".into(),
                source: ModSource::LocalPath {
                    path: "/tmp/echo".into(),
                },
                enabled: false,
                grant: Some(Grant::new(CapabilitySet::new(), GrantOrigin::Interactive)),
                pending_update: None,
                installed_at: now,
                updated_at: now,
            });
        state.hooks_migrated = true;
        file.save(&state).unwrap();
        let back = file.load().unwrap();
        assert_eq!(back.mods.len(), 1);
        assert!(back.hooks_migrated);
        assert!(!dir.path().join("nested/state.tmp").exists());
    }

    #[test]
    fn corrupt_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        fs::write(&path, "{not json").unwrap();
        assert!(StateFile::new(path).load().is_err());
    }
}
