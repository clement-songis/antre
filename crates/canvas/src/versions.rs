//! Design versions backed by directory checkpoints.
//!
//! Versions reuse [`antre_tools::checkpoints::CheckpointManager`] with a
//! dedicated base directory, so they are never pruned by the general
//! checkpoint cleanup.

use std::path::{Path, PathBuf};

use {
    antre_tools::checkpoints::CheckpointManager,
    sha2::{Digest, Sha256},
};

use crate::{Error, Result, paths::list_files};

/// Snapshot storage for design directories.
#[derive(Debug, Clone)]
pub struct VersionStorage {
    checkpoints: CheckpointManager,
}

impl VersionStorage {
    #[must_use]
    pub fn new(base_dir: PathBuf) -> Self {
        Self {
            checkpoints: CheckpointManager::with_base_dir(base_dir),
        }
    }

    /// Copy `dir` into a new checkpoint and return its id.
    pub async fn snapshot(&self, dir: &Path, reason: &str) -> Result<String> {
        Ok(self.checkpoints.checkpoint_path(dir, reason).await?.id)
    }

    /// Directory holding the files of a snapshot.
    pub async fn snapshot_dir(&self, checkpoint_id: &str) -> Result<PathBuf> {
        self.checkpoints
            .snapshot_path(checkpoint_id)
            .await?
            .ok_or_else(|| Error::Checkpoint(format!("checkpoint {checkpoint_id} has no files")))
    }

    /// Replace the design directory with the snapshot's content.
    pub async fn restore(&self, checkpoint_id: &str) -> Result<()> {
        self.checkpoints.restore(checkpoint_id).await?;
        Ok(())
    }
}

/// Hash of every served file of a design (paths and content), used to skip
/// versions that would not change anything.
pub async fn content_hash(dir: &Path) -> Result<String> {
    let dir = dir.to_path_buf();
    tokio::task::spawn_blocking(move || content_hash_blocking(&dir))
        .await
        .map_err(|error| Error::message(format!("hash task failed: {error}")))?
}

fn content_hash_blocking(dir: &Path) -> Result<String> {
    let mut hasher = Sha256::new();
    for file in list_files(dir)? {
        let bytes = std::fs::read(dir.join(&file.path))?;
        hasher.update((file.path.len() as u64).to_le_bytes());
        hasher.update(file.path.as_bytes());
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hash_tracks_content_and_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::write(dir.join("index.html"), "<p>a</p>").unwrap();
        let first = content_hash(dir).await.unwrap();
        assert_eq!(first, content_hash(dir).await.unwrap());

        std::fs::write(dir.join("index.html"), "<p>b</p>").unwrap();
        let second = content_hash(dir).await.unwrap();
        assert_ne!(first, second);

        std::fs::rename(dir.join("index.html"), dir.join("main.html")).unwrap();
        assert_ne!(second, content_hash(dir).await.unwrap());

        std::fs::write(dir.join(".hidden"), "ignored").unwrap();
        let before = content_hash(dir).await.unwrap();
        std::fs::write(dir.join(".hidden"), "still ignored").unwrap();
        assert_eq!(before, content_hash(dir).await.unwrap());
    }

    #[tokio::test]
    async fn snapshot_and_restore_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let storage = VersionStorage::new(tmp.path().join("versions"));
        let dir = tmp.path().join("design");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.html"), "v1").unwrap();

        let id = storage.snapshot(&dir, "v1").await.unwrap();
        std::fs::write(dir.join("index.html"), "v2").unwrap();
        std::fs::write(dir.join("extra.css"), "x").unwrap();
        let snapshot = storage.snapshot_dir(&id).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(snapshot.join("index.html")).unwrap(),
            "v1"
        );

        storage.restore(&id).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("index.html")).unwrap(),
            "v1"
        );
        assert!(!dir.join("extra.css").exists());
    }
}
