//! Staging a mod package from a local folder.
//!
//! The source folder is copied into a private staging directory first and
//! everything (manifest, component, content hash) is read from the copy, so a
//! source that changes during install cannot smuggle in unvalidated files.
//! Symlinks, special files and paths leaving the package are rejected.

use std::{
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use {
    sha2::{Digest, Sha256},
    walkdir::WalkDir,
};

use crate::{
    error::{Error, Result},
    manifest::{MANIFEST_FILE, ModManifest},
};

const MAX_MANIFEST_BYTES: u64 = 64 * 1024;

/// Size limits applied while copying a package.
#[derive(Debug, Clone, Copy)]
pub struct PackageLimits {
    pub max_files: usize,
    pub max_total_bytes: u64,
    pub max_file_bytes: u64,
}

impl Default for PackageLimits {
    fn default() -> Self {
        Self {
            max_files: 1_000,
            max_total_bytes: 64 * 1024 * 1024,
            max_file_bytes: 32 * 1024 * 1024,
        }
    }
}

/// A validated package copy waiting to be moved into the store.
#[derive(Debug)]
pub struct StagedPackage {
    pub dir: PathBuf,
    pub content_hash: String,
    pub manifest: ModManifest,
    pub component_bytes: Option<Vec<u8>>,
}

fn reject(message: impl Into<String>) -> Error {
    Error::Package(message.into())
}

fn unique_suffix() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    format!(
        "{}-{nanos}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

fn relative_utf8(path: &Path) -> Result<String> {
    if !path
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(reject(format!(
            "path `{}` leaves the package",
            path.display()
        )));
    }
    let text = path
        .to_str()
        .ok_or_else(|| reject(format!("path `{}` is not UTF-8", path.display())))?;
    Ok(text.replace('\\', "/"))
}

/// Copy `source` into a fresh directory under `staging_root`, validate the
/// manifest and component, and hash the copied tree.
pub fn stage_from_dir(
    source: &Path,
    staging_root: &Path,
    limits: PackageLimits,
) -> Result<StagedPackage> {
    let source_meta = fs::symlink_metadata(source)?;
    if source_meta.file_type().is_symlink() || !source_meta.is_dir() {
        return Err(reject("the package path must be a real directory"));
    }
    fs::create_dir_all(staging_root)?;
    let staged = staging_root.join(format!("stage-{}", unique_suffix()));
    fs::create_dir(&staged)?;
    match copy_and_validate(source, &staged, limits) {
        Ok((manifest, component_bytes)) => {
            let content_hash = hash_tree(&staged)?;
            Ok(StagedPackage {
                dir: staged,
                content_hash,
                manifest,
                component_bytes,
            })
        },
        Err(error) => {
            let _ = fs::remove_dir_all(&staged);
            Err(error)
        },
    }
}

fn copy_and_validate(
    source: &Path,
    staged: &Path,
    limits: PackageLimits,
) -> Result<(ModManifest, Option<Vec<u8>>)> {
    let mut files = 0usize;
    let mut total = 0u64;
    let walker = WalkDir::new(source)
        .follow_links(false)
        .min_depth(1)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|entry| entry.file_name() != ".git");
    for entry in walker {
        let entry = entry.map_err(|error| reject(format!("cannot read package: {error}")))?;
        let relative = entry
            .path()
            .strip_prefix(source)
            .map_err(|_| reject("walk escaped the package"))?;
        let relative_text = relative_utf8(relative)?;
        let file_type = entry.file_type();
        let target = staged.join(relative);
        if file_type.is_symlink() {
            return Err(reject(format!("symlinks are not allowed: {relative_text}")));
        }
        if file_type.is_dir() {
            fs::create_dir_all(&target)?;
            continue;
        }
        if !file_type.is_file() {
            return Err(reject(format!(
                "special files are not allowed: {relative_text}"
            )));
        }
        files += 1;
        if files > limits.max_files {
            return Err(reject(format!("more than {} files", limits.max_files)));
        }
        let size = entry
            .metadata()
            .map_err(|error| reject(format!("cannot stat {relative_text}: {error}")))?
            .len();
        if size > limits.max_file_bytes {
            return Err(reject(format!(
                "{relative_text} is larger than the file limit"
            )));
        }
        total = total.saturating_add(size);
        if total > limits.max_total_bytes {
            return Err(reject("package is larger than the size limit"));
        }
        copy_bounded(entry.path(), &target, limits.max_file_bytes)?;
    }

    let manifest_path = staged.join(MANIFEST_FILE);
    let manifest_meta = fs::symlink_metadata(&manifest_path)
        .map_err(|_| reject(format!("missing {MANIFEST_FILE}")))?;
    if !manifest_meta.is_file() || manifest_meta.len() > MAX_MANIFEST_BYTES {
        return Err(reject(format!(
            "{MANIFEST_FILE} must be a file under 64 KiB"
        )));
    }
    let manifest = ModManifest::parse(&fs::read_to_string(&manifest_path)?)?;

    let component_bytes = match &manifest.component {
        Some(relative) => {
            let path = staged.join(relative);
            let meta = fs::symlink_metadata(&path)
                .map_err(|_| reject(format!("component `{}` is missing", relative.display())))?;
            if !meta.is_file() {
                return Err(reject("component must be a regular file"));
            }
            Some(fs::read(&path)?)
        },
        None => None,
    };
    Ok((manifest, component_bytes))
}

/// Copy at most `limit` bytes; a file that grew past the limit since it was
/// measured is rejected instead of truncated.
fn copy_bounded(from: &Path, to: &Path, limit: u64) -> Result<()> {
    let file = fs::File::open(from)?;
    let mut reader = file.take(limit + 1);
    let mut buffer = Vec::new();
    reader.read_to_end(&mut buffer)?;
    if buffer.len() as u64 > limit {
        return Err(reject(format!("{} grew during install", from.display())));
    }
    fs::write(to, buffer)?;
    Ok(())
}

/// SHA-256 over every regular file (sorted relative path, length, bytes).
pub fn hash_tree(root: &Path) -> Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(b"antre-mod-package-v1\n");
    for entry in WalkDir::new(root)
        .follow_links(false)
        .min_depth(1)
        .sort_by_file_name()
    {
        let entry = entry.map_err(|error| reject(format!("cannot hash package: {error}")))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(root)
            .map_err(|_| reject("walk escaped the package"))?;
        let relative_text = relative_utf8(relative)?;
        let bytes = fs::read(entry.path())?;
        hasher.update(relative_text.as_bytes());
        hasher.update([0]);
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    Ok(hex::encode(hasher.finalize()))
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str =
        "schema = 1\nid = \"acme.theme\"\nversion = \"1.0.0\"\nname = \"Theme\"\nsdk = \"0.1\"\n";

    fn package(dir: &Path) {
        fs::write(dir.join(MANIFEST_FILE), MANIFEST).unwrap();
        fs::create_dir_all(dir.join("themes")).unwrap();
        fs::write(dir.join("themes/dusk.json"), "{}").unwrap();
    }

    #[test]
    fn stages_a_valid_package() {
        let source = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        package(source.path());
        fs::create_dir_all(source.path().join(".git")).unwrap();
        fs::write(source.path().join(".git/HEAD"), "ref").unwrap();
        let staged =
            stage_from_dir(source.path(), staging.path(), PackageLimits::default()).unwrap();
        assert_eq!(staged.manifest.id.to_string(), "acme.theme");
        assert!(staged.dir.join("themes/dusk.json").is_file());
        assert!(!staged.dir.join(".git").exists());
        assert_eq!(staged.content_hash.len(), 64);
        assert_eq!(staged.content_hash, hash_tree(&staged.dir).unwrap());
    }

    #[test]
    fn hash_changes_with_content() {
        let source = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        package(source.path());
        let first =
            stage_from_dir(source.path(), staging.path(), PackageLimits::default()).unwrap();
        fs::write(source.path().join("themes/dusk.json"), "{\"a\":1}").unwrap();
        let second =
            stage_from_dir(source.path(), staging.path(), PackageLimits::default()).unwrap();
        assert_ne!(first.content_hash, second.content_hash);
        assert_ne!(first.dir, second.dir);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks_and_cleans_up() {
        let source = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        package(source.path());
        std::os::unix::fs::symlink("/etc/passwd", source.path().join("passwd")).unwrap();
        let err = stage_from_dir(source.path(), staging.path(), PackageLimits::default())
            .unwrap_err()
            .to_string();
        assert!(err.contains("symlinks are not allowed"), "{err}");
        assert_eq!(fs::read_dir(staging.path()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_package_root() {
        let source = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        package(source.path());
        let link = staging.path().join("link");
        std::os::unix::fs::symlink(source.path(), &link).unwrap();
        assert!(
            stage_from_dir(&link, &staging.path().join("s"), PackageLimits::default()).is_err()
        );
    }

    #[test]
    fn enforces_limits() {
        let source = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        package(source.path());
        fs::write(source.path().join("big.bin"), vec![0u8; 2048]).unwrap();
        let tight = PackageLimits {
            max_files: 10,
            max_total_bytes: 1_000_000,
            max_file_bytes: 1024,
        };
        assert!(stage_from_dir(source.path(), staging.path(), tight).is_err());
        let few = PackageLimits {
            max_files: 1,
            ..PackageLimits::default()
        };
        assert!(stage_from_dir(source.path(), staging.path(), few).is_err());
        let small_total = PackageLimits {
            max_total_bytes: 100,
            ..PackageLimits::default()
        };
        assert!(stage_from_dir(source.path(), staging.path(), small_total).is_err());
    }

    #[test]
    fn requires_manifest_and_component() {
        let source = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        assert!(stage_from_dir(source.path(), staging.path(), PackageLimits::default()).is_err());
        fs::write(
            source.path().join(MANIFEST_FILE),
            format!("{MANIFEST}\n[server]\ncomponent = \"server/x.wasm\"\n"),
        )
        .unwrap();
        let err = stage_from_dir(source.path(), staging.path(), PackageLimits::default())
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("component `server/x.wasm` is missing"),
            "{err}"
        );
    }

    #[test]
    fn rejects_file_as_package() {
        let source = tempfile::tempdir().unwrap();
        let file = source.path().join("file");
        fs::write(&file, "x").unwrap();
        assert!(stage_from_dir(&file, source.path(), PackageLimits::default()).is_err());
    }
}
