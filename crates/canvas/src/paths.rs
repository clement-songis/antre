//! Design ids and path scoping.
//!
//! Every file the canvas reads on behalf of a request goes through
//! [`resolve_in_dir`]: the relative path is checked component by component,
//! then canonicalized and required to stay inside the design directory, so
//! `..`, absolute paths and symlinks pointing outside are all refused.

use std::{
    fmt,
    path::{Component, Path, PathBuf},
    str::FromStr,
};

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Upper bound on the number of files listed or exported for one design.
pub const MAX_DESIGN_FILES: usize = 2_000;

/// Identifier of a design: 32 lowercase hex characters (a simple UUID).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DesignId(String);

impl DesignId {
    #[must_use]
    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().simple().to_string())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for DesignId {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        let valid = value.len() == 32
            && value
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
        if valid {
            Ok(Self(value.to_string()))
        } else {
            Err(Error::InvalidId(value.chars().take(64).collect()))
        }
    }
}

impl TryFrom<String> for DesignId {
    type Error = Error;

    fn try_from(value: String) -> Result<Self> {
        value.parse()
    }
}

impl From<DesignId> for String {
    fn from(id: DesignId) -> Self {
        id.0
    }
}

impl fmt::Display for DesignId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Validate a request-supplied relative path without touching the disk.
///
/// Only plain components are accepted; hidden segments (leading `.`) are
/// refused so dotfiles in a design are never served.
pub fn validate_relative(path: &str) -> Result<PathBuf> {
    let reject = |reason| Error::InvalidPath {
        path: path.chars().take(200).collect(),
        reason,
    };
    if path.is_empty() {
        return Err(reject("empty path"));
    }
    if path.contains('\0') || path.contains('\\') {
        return Err(reject("forbidden character"));
    }
    let relative = Path::new(path);
    let mut clean = PathBuf::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => {
                let part = part.to_str().ok_or_else(|| reject("not valid UTF-8"))?;
                if part.starts_with('.') {
                    return Err(reject("hidden path segment"));
                }
                clean.push(part);
            },
            Component::ParentDir => return Err(reject("parent directory segment")),
            Component::CurDir => return Err(reject("current directory segment")),
            Component::RootDir | Component::Prefix(_) => return Err(reject("absolute path")),
        }
    }
    if clean.as_os_str().is_empty() {
        return Err(reject("empty path"));
    }
    Ok(clean)
}

/// Resolve `relative` inside `dir` and return the canonical path of an
/// existing regular file that is guaranteed to live under `dir`.
pub fn resolve_in_dir(dir: &Path, relative: &str) -> Result<PathBuf> {
    let clean = validate_relative(relative)?;
    let root = std::fs::canonicalize(dir)?;
    let candidate = std::fs::canonicalize(root.join(&clean))?;
    if !candidate.starts_with(&root) {
        return Err(Error::InvalidPath {
            path: relative.chars().take(200).collect(),
            reason: "resolves outside the design directory",
        });
    }
    if !candidate.is_file() {
        return Err(Error::InvalidPath {
            path: relative.chars().take(200).collect(),
            reason: "not a regular file",
        });
    }
    Ok(candidate)
}

/// A file inside a design, with its path relative to the design directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesignFile {
    pub path: String,
    pub bytes: u64,
}

/// List the regular files of a design directory, sorted by path.
///
/// Symlinks and hidden entries are skipped, matching what the render origin
/// is willing to serve.
pub fn list_files(dir: &Path) -> Result<Vec<DesignFile>> {
    let mut files = Vec::new();
    let walker = walkdir::WalkDir::new(dir)
        .follow_links(false)
        .min_depth(1)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|entry| !entry.file_name().to_string_lossy().starts_with('.'));
    for entry in walker {
        let entry =
            entry.map_err(|error| Error::message(format!("cannot list design: {error}")))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(dir) else {
            continue;
        };
        let Some(relative) = relative.to_str() else {
            continue;
        };
        if files.len() >= MAX_DESIGN_FILES {
            return Err(Error::Invalid(format!(
                "design has more than {MAX_DESIGN_FILES} files"
            )));
        }
        let bytes = entry
            .metadata()
            .map_err(|error| Error::message(format!("cannot stat design file: {error}")))?
            .len();
        files.push(DesignFile {
            path: relative.replace(std::path::MAIN_SEPARATOR, "/"),
            bytes,
        });
    }
    Ok(files)
}

/// First entry of `dir` that is neither a regular file nor a directory
/// (symlink, socket, fifo), as a relative path. Checkpoints refuse those.
pub fn first_special_entry(dir: &Path) -> Option<String> {
    walkdir::WalkDir::new(dir)
        .follow_links(false)
        .min_depth(1)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .find(|entry| {
            let kind = entry.file_type();
            !kind.is_file() && !kind.is_dir()
        })
        .and_then(|entry| {
            entry
                .path()
                .strip_prefix(dir)
                .ok()
                .map(|relative| relative.to_string_lossy().into_owned())
        })
}

/// Where a filesystem path sits relative to the designs root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesignsLocation {
    /// Not under the designs root at all.
    Outside,
    /// Under the root but not inside a design directory (the root itself, a
    /// stray file, or a directory whose name is not a design id).
    Stray,
    /// Inside the directory of this design.
    Design(DesignId),
}

/// Classify an absolute path (which may not exist yet) against the canonical
/// designs root. The path's deepest existing ancestor is canonicalized so
/// symlinked parents cannot disguise the real location.
#[must_use]
pub fn locate(designs_root: &Path, path: &Path) -> DesignsLocation {
    let Some(resolved) = canonicalize_lenient(path) else {
        return DesignsLocation::Outside;
    };
    let Ok(rest) = resolved.strip_prefix(designs_root) else {
        return DesignsLocation::Outside;
    };
    let mut components = rest.components();
    let Some(Component::Normal(first)) = components.next() else {
        return DesignsLocation::Stray;
    };
    if components.next().is_none() {
        // A file directly in the root, or a design directory itself.
        return match first.to_str().and_then(|name| name.parse().ok()) {
            Some(id) if resolved.is_dir() => DesignsLocation::Design(id),
            _ => DesignsLocation::Stray,
        };
    }
    match first.to_str().and_then(|name| name.parse().ok()) {
        Some(id) => DesignsLocation::Design(id),
        None => DesignsLocation::Stray,
    }
}

fn canonicalize_lenient(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut existing = path;
    let mut missing = Vec::new();
    loop {
        if let Ok(canonical) = std::fs::canonicalize(existing) {
            let mut resolved = canonical;
            for part in missing.iter().rev() {
                resolved.push(part);
            }
            return Some(resolved);
        }
        let name = existing.file_name()?;
        if name == ".." {
            return None;
        }
        missing.push(name.to_os_string());
        existing = existing.parent()?;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn design_id_accepts_only_simple_uuids() {
        let id = DesignId::generate();
        assert_eq!(id.as_str().parse::<DesignId>().unwrap(), id);
        for bad in [
            "",
            "../etc",
            "ABCDEF00000000000000000000000000",
            "0123456789abcdef0123456789abcdeg",
            "0123456789abcdef0123456789abcde",
            "0123456789abcdef0123456789abcdef0",
        ] {
            assert!(bad.parse::<DesignId>().is_err(), "{bad} should be rejected");
        }
    }

    #[test]
    fn design_id_serde_validates() {
        let id = DesignId::generate();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(serde_json::from_str::<DesignId>(&json).unwrap(), id);
        assert!(serde_json::from_str::<DesignId>("\"../../x\"").is_err());
    }

    #[test]
    fn validate_relative_rejects_traversal_and_hidden_segments() {
        for bad in [
            "",
            "../secret",
            "a/../../b",
            "/etc/passwd",
            "./index.html",
            ".env",
            "assets/.git/config",
            "a\\..\\b",
            "a\0b",
        ] {
            assert!(
                validate_relative(bad).is_err(),
                "{bad:?} should be rejected"
            );
        }
        assert_eq!(
            validate_relative("assets/logo.svg").unwrap(),
            PathBuf::from("assets/logo.svg")
        );
        assert_eq!(
            validate_relative("index.html/").unwrap(),
            PathBuf::from("index.html")
        );
    }

    #[test]
    fn resolve_in_dir_returns_files_inside_only() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("design");
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(dir.join("index.html"), "<p>hi</p>").unwrap();
        std::fs::write(dir.join("assets/app.css"), "p{}").unwrap();
        std::fs::write(tmp.path().join("secret.txt"), "secret").unwrap();

        let found = resolve_in_dir(&dir, "assets/app.css").unwrap();
        assert!(found.ends_with("assets/app.css"));
        assert!(
            resolve_in_dir(&dir, "assets").is_err(),
            "directories are not served"
        );
        assert!(
            resolve_in_dir(&dir, "missing.html")
                .unwrap_err()
                .is_not_found()
        );
        assert!(resolve_in_dir(&dir, "../secret.txt").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn resolve_in_dir_refuses_symlink_escape() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("design");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(tmp.path().join("secret.txt"), "secret").unwrap();
        std::os::unix::fs::symlink(tmp.path().join("secret.txt"), dir.join("leak.txt")).unwrap();
        std::os::unix::fs::symlink(tmp.path(), dir.join("up")).unwrap();

        let err = resolve_in_dir(&dir, "leak.txt").unwrap_err();
        assert!(err.to_string().contains("outside"), "{err}");
        assert!(resolve_in_dir(&dir, "up/secret.txt").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn list_files_skips_symlinks_and_hidden_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("design");
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::create_dir_all(dir.join(".cache")).unwrap();
        std::fs::write(dir.join("index.html"), "<p>hi</p>").unwrap();
        std::fs::write(dir.join("assets/a.css"), "a{}").unwrap();
        std::fs::write(dir.join(".cache/x"), "x").unwrap();
        std::fs::write(dir.join(".env"), "x").unwrap();
        std::fs::write(tmp.path().join("secret.txt"), "secret").unwrap();
        std::os::unix::fs::symlink(tmp.path().join("secret.txt"), dir.join("leak.txt")).unwrap();

        let files = list_files(&dir).unwrap();
        let paths: Vec<_> = files.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(paths, vec!["assets/a.css", "index.html"]);
        assert_eq!(files[1].bytes, 9);
    }

    #[cfg(unix)]
    #[test]
    fn first_special_entry_finds_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("design");
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::write(dir.join("a/x.css"), "x").unwrap();
        assert_eq!(first_special_entry(&dir), None);
        std::os::unix::fs::symlink("/etc/hosts", dir.join("a/link")).unwrap();
        assert_eq!(first_special_entry(&dir).as_deref(), Some("a/link"));
    }

    #[test]
    fn locate_classifies_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("designs");
        let id = DesignId::generate();
        std::fs::create_dir_all(root.join(id.as_str())).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();

        assert_eq!(
            locate(&root, &root.join(id.as_str()).join("index.html")),
            DesignsLocation::Design(id.clone())
        );
        assert_eq!(
            locate(&root, &root.join(id.as_str()).join("new/dir/file.css")),
            DesignsLocation::Design(id.clone())
        );
        assert_eq!(
            locate(&root, &root.join(id.as_str())),
            DesignsLocation::Design(id.clone())
        );
        assert_eq!(
            locate(&root, &root.join("stray.html")),
            DesignsLocation::Stray
        );
        assert_eq!(
            locate(&root, &root.join("not-an-id/index.html")),
            DesignsLocation::Stray
        );
        assert_eq!(locate(&root, &root), DesignsLocation::Stray);
        assert_eq!(
            locate(&root, &tmp.path().join("elsewhere.txt")),
            DesignsLocation::Outside
        );
        assert_eq!(
            locate(&root, Path::new("relative/path")),
            DesignsLocation::Outside
        );
        assert_eq!(
            locate(&root, &root.join(id.as_str()).join("../../escape.txt")),
            DesignsLocation::Outside
        );
    }
}
