//! Content-addressed snapshots: every version the ledger references is
//! stored once under its sha256.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::{
    error::{Error, Result},
    paths::{read_optional, write_atomic},
};

/// Hex sha256 of some bytes.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Blob store rooted at `learning/blobs`.
#[derive(Debug, Clone)]
pub struct BlobStore {
    root: PathBuf,
}

impl BlobStore {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn path(&self, sha: &str) -> Result<PathBuf> {
        if sha.len() != 64 || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(Error::NotFound(format!("blob {sha}")));
        }
        Ok(self.root.join(&sha[..2]).join(sha))
    }

    /// Store bytes and return their hash. Existing blobs are left as is.
    pub fn put(&self, bytes: &[u8]) -> Result<String> {
        let sha = sha256_hex(bytes);
        let path = self.path(&sha)?;
        if !path.exists() {
            write_atomic(&path, bytes)?;
        }
        Ok(sha)
    }

    /// Read a blob back, checking its hash.
    pub fn get(&self, sha: &str) -> Result<Vec<u8>> {
        let path = self.path(sha)?;
        let bytes = read_optional(&path)?.ok_or_else(|| Error::NotFound(format!("blob {sha}")))?;
        if sha256_hex(&bytes) != sha {
            return Err(Error::Conflict(format!("blob {sha} is corrupt")));
        }
        Ok(bytes)
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn put_get_dedupes_and_verifies() {
        let tmp = tempfile::tempdir().unwrap();
        let store = BlobStore::new(tmp.path());
        let sha = store.put(b"hello").unwrap();
        assert_eq!(sha, sha256_hex(b"hello"));
        assert_eq!(store.put(b"hello").unwrap(), sha);
        assert_eq!(store.get(&sha).unwrap(), b"hello");
        std::fs::write(tmp.path().join(&sha[..2]).join(&sha), b"tampered").unwrap();
        assert!(matches!(store.get(&sha), Err(Error::Conflict(_))));
        assert!(matches!(store.get("../etc"), Err(Error::NotFound(_))));
        assert!(matches!(
            store.get(&"0".repeat(64)),
            Err(Error::NotFound(_))
        ));
    }
}
