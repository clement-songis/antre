//! Where the learning loop keeps its files, all under `data_dir`.

use std::path::{Path, PathBuf};

/// File layout of the learning loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearningPaths {
    data_dir: PathBuf,
}

impl LearningPaths {
    #[must_use]
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
        }
    }

    #[must_use]
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Personal skills, the only skills the loop may touch.
    #[must_use]
    pub fn skills_dir(&self) -> PathBuf {
        self.data_dir.join("skills")
    }

    #[must_use]
    pub fn root(&self) -> PathBuf {
        self.data_dir.join("learning")
    }

    #[must_use]
    pub fn ledger(&self) -> PathBuf {
        self.root().join("ledger.jsonl")
    }

    #[must_use]
    pub fn blobs(&self) -> PathBuf {
        self.root().join("blobs")
    }

    #[must_use]
    pub fn pending(&self) -> PathBuf {
        self.root().join("pending")
    }

    #[must_use]
    pub fn rejected(&self) -> PathBuf {
        self.root().join("rejected.jsonl")
    }

    /// Archived skills live outside `skills/` so discovery never sees them.
    #[must_use]
    pub fn archive(&self) -> PathBuf {
        self.root().join("archive")
    }

    #[must_use]
    pub fn curator_state(&self) -> PathBuf {
        self.root().join("curator.json")
    }

    #[must_use]
    pub fn review_state(&self) -> PathBuf {
        self.root().join("review-state.json")
    }
}

/// Write a file through a temporary sibling and a rename, so readers never
/// see a half-written file.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(
        ".{file_name}.{}.tmp",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Read a file, `None` when it does not exist.
pub(crate) fn read_optional(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn layout_is_under_data_dir() {
        let paths = LearningPaths::new("/data");
        assert_eq!(paths.ledger(), PathBuf::from("/data/learning/ledger.jsonl"));
        assert_eq!(paths.skills_dir(), PathBuf::from("/data/skills"));
        assert_eq!(paths.archive(), PathBuf::from("/data/learning/archive"));
    }

    #[test]
    fn atomic_write_creates_parents_and_replaces() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("a/b/c.txt");
        write_atomic(&path, b"one").unwrap();
        write_atomic(&path, b"two").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
        assert_eq!(read_optional(&tmp.path().join("missing")).unwrap(), None);
    }
}
