//! Vendored scripts served at `/_vendor/<name>` by the render origin.
//!
//! Built-in files are compiled into the binary (see `vendor/README.md`).
//! Operators can add more by dropping `.js`/`.css` files into
//! `data_dir/canvas/vendor/`; built-ins win on name clashes.

use std::path::{Path, PathBuf};

use {
    base64::{Engine, engine::general_purpose::STANDARD},
    serde::Serialize,
    sha2::{Digest, Sha384},
};

struct BuiltIn {
    name: &'static str,
    description: &'static str,
    bytes: &'static [u8],
}

const BUILT_INS: &[BuiltIn] = &[
    BuiltIn {
        name: "preact.js",
        description: "Preact 10.29.1 UMD build, exposes window.preact",
        bytes: include_bytes!("../vendor/preact.js"),
    },
    BuiltIn {
        name: "preact-hooks.js",
        description: "Preact hooks 10.29.1 UMD build, exposes window.preactHooks",
        bytes: include_bytes!("../vendor/preact-hooks.js"),
    },
];

/// Public description of a vendored file, for prompts and the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VendorEntry {
    pub name: String,
    pub path: String,
    pub description: String,
    /// Subresource Integrity value (`sha384-...`).
    pub integrity: String,
}

/// A vendored file ready to serve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VendorFile {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
}

/// Registry of vendored files.
#[derive(Debug, Clone)]
pub struct Vendor {
    extra_dir: Option<PathBuf>,
}

fn integrity(bytes: &[u8]) -> String {
    format!("sha384-{}", STANDARD.encode(Sha384::digest(bytes)))
}

fn content_type(name: &str) -> Option<&'static str> {
    if name.ends_with(".js") || name.ends_with(".mjs") {
        Some("text/javascript; charset=utf-8")
    } else if name.ends_with(".css") {
        Some("text/css; charset=utf-8")
    } else {
        None
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
        && content_type(name).is_some()
}

impl Vendor {
    #[must_use]
    pub fn new(extra_dir: Option<PathBuf>) -> Self {
        Self { extra_dir }
    }

    /// Look up a vendored file by name. Names are flat: no directories.
    pub async fn get(&self, name: &str) -> Option<VendorFile> {
        let vendor = self.clone();
        let name = name.to_string();
        tokio::task::spawn_blocking(move || vendor.get_blocking(&name))
            .await
            .ok()
            .flatten()
    }

    /// Blocking variant of [`Vendor::get`], for exporters already running on
    /// a blocking thread.
    #[must_use]
    pub fn get_blocking(&self, name: &str) -> Option<VendorFile> {
        if !valid_name(name) {
            return None;
        }
        let content_type = content_type(name)?;
        if let Some(built_in) = BUILT_INS.iter().find(|entry| entry.name == name) {
            return Some(VendorFile {
                bytes: built_in.bytes.to_vec(),
                content_type,
            });
        }
        let dir = self.extra_dir.as_deref()?;
        let path = crate::paths::resolve_in_dir(dir, name).ok()?;
        let bytes = std::fs::read(path).ok()?;
        Some(VendorFile {
            bytes,
            content_type,
        })
    }

    /// All vendored files with their integrity hashes.
    pub async fn list(&self) -> Vec<VendorEntry> {
        let mut entries: Vec<VendorEntry> = BUILT_INS
            .iter()
            .map(|entry| VendorEntry {
                name: entry.name.to_string(),
                path: format!("{}{}", crate::csp::VENDOR_PREFIX, entry.name),
                description: entry.description.to_string(),
                integrity: integrity(entry.bytes),
            })
            .collect();
        if let Some(dir) = self.extra_dir.as_deref() {
            entries.extend(list_extra(dir, &entries).await);
        }
        entries
    }
}

async fn list_extra(dir: &Path, built_ins: &[VendorEntry]) -> Vec<VendorEntry> {
    let mut extra = Vec::new();
    let Ok(mut reader) = tokio::fs::read_dir(dir).await else {
        return extra;
    };
    while let Ok(Some(entry)) = reader.next_entry().await {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !valid_name(&name) || built_ins.iter().any(|known| known.name == name) {
            continue;
        }
        let Ok(path) = crate::paths::resolve_in_dir(dir, &name) else {
            continue;
        };
        let Ok(bytes) = tokio::fs::read(path).await else {
            continue;
        };
        extra.push(VendorEntry {
            path: format!("{}{name}", crate::csp::VENDOR_PREFIX),
            description: "operator-provided".into(),
            integrity: integrity(&bytes),
            name,
        });
    }
    extra.sort_by(|left, right| left.name.cmp(&right.name));
    extra
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn serves_built_ins_with_integrity() {
        let vendor = Vendor::new(None);
        let file = vendor.get("preact.js").await.unwrap();
        assert!(file.content_type.starts_with("text/javascript"));
        assert!(!file.bytes.is_empty());
        assert!(
            !String::from_utf8_lossy(&file.bytes).contains("sourceMappingURL"),
            "source maps are not vendored"
        );
        let listed = vendor.list().await;
        assert_eq!(listed.len(), BUILT_INS.len());
        assert!(
            listed
                .iter()
                .all(|entry| entry.integrity.starts_with("sha384-"))
        );
        assert_eq!(listed[0].path, "/_vendor/preact.js");
    }

    #[tokio::test]
    async fn rejects_bad_names() {
        let vendor = Vendor::new(None);
        for name in [
            "",
            "../preact.js",
            ".hidden.js",
            "a/b.js",
            "x.html",
            "x.svg",
        ] {
            assert!(vendor.get(name).await.is_none(), "{name}");
        }
    }

    #[tokio::test]
    async fn serves_operator_files_but_not_traversal_or_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("vendor");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("tailwind.js"), "console.log(1)").unwrap();
        std::fs::write(dir.join("preact.js"), "shadow").unwrap();
        std::fs::write(dir.join("notes.txt"), "x").unwrap();
        std::fs::write(tmp.path().join("secret.js"), "secret").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(tmp.path().join("secret.js"), dir.join("leak.js")).unwrap();

        let vendor = Vendor::new(Some(dir));
        assert_eq!(
            vendor.get("tailwind.js").await.unwrap().bytes,
            b"console.log(1)"
        );
        assert_ne!(vendor.get("preact.js").await.unwrap().bytes, b"shadow");
        assert!(vendor.get("leak.js").await.is_none());
        let names: Vec<_> = vendor.list().await.into_iter().map(|e| e.name).collect();
        assert!(names.contains(&"tailwind.js".to_string()));
        assert!(!names.contains(&"notes.txt".to_string()));
        assert!(!names.contains(&"leak.js".to_string()));
        assert_eq!(names.iter().filter(|n| *n == "preact.js").count(), 1);
    }
}
