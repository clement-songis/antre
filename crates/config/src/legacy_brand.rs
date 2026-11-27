//! Compatibility with installs made under the Moltis name.
//!
//! `scripts/rebrand.py` skips this file: it is where the old names live.
//! See `docs/src/legacy-migration.md`.

use std::{
    collections::HashSet,
    ffi::OsString,
    fmt,
    path::{Path, PathBuf},
};

pub const LEGACY_ENV_PREFIX: &str = "MOLTIS_";
pub const ENV_PREFIX: &str = "ANTRE_";

/// Config file names read when no `antre.*` file exists.
pub const LEGACY_CONFIG_FILENAMES: &[&str] =
    &["moltis.toml", "moltis.yaml", "moltis.yml", "moltis.json"];

/// Main database entry in export archives made by Moltis.
pub const LEGACY_ARCHIVE_DB_PATH: &str = "db/moltis.db";

/// Keychain items of the desktop app were named `moltis-desktop://<server id>`.
pub const LEGACY_DESKTOP_KEYCHAIN_SCHEME: &str = "moltis-desktop://";

/// A user service installed under the Moltis name.
#[derive(Debug, Clone, Copy)]
pub struct LegacyService {
    pub launchd_label: &'static str,
    pub systemd_unit: &'static str,
}

/// `moltis service install`.
pub const LEGACY_GATEWAY_SERVICE: LegacyService = LegacyService {
    launchd_label: "org.moltis.gateway",
    systemd_unit: "moltis.service",
};

/// `moltis node install`.
pub const LEGACY_NODE_SERVICE: LegacyService = LegacyService {
    launchd_label: "org.moltis.node",
    systemd_unit: "moltis-node.service",
};

/// Written in a migrated directory, records where its content came from.
pub const MIGRATION_MARKER: &str = ".migrated-from-moltis";

/// Data files renamed in place, relative to the data directory.
const LEGACY_DATA_FILES: &[(&str, &str)] = &[
    ("moltis.db", "antre.db"),
    ("moltis.db-wal", "antre.db-wal"),
    ("moltis.db-shm", "antre.db-shm"),
    ("docs/moltis", "docs/antre"),
];

/// `~/.moltis`, the legacy default data directory.
#[must_use]
pub fn legacy_data_dir(home: &Path) -> PathBuf {
    home.join(".moltis")
}

/// `~/.config/moltis`, the legacy default config directory.
#[must_use]
pub fn legacy_config_dir(home: &Path) -> PathBuf {
    home.join(".config").join("moltis")
}

/// Picks the current default directory, or the legacy one while only the
/// legacy one exists (before migration, or when migration failed).
#[must_use]
pub fn resolve_default_dir(current: PathBuf, legacy: PathBuf) -> PathBuf {
    if !current.exists() && legacy.is_dir() {
        legacy
    } else {
        current
    }
}

#[must_use]
pub fn is_legacy_config_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| LEGACY_CONFIG_FILENAMES.contains(&name))
}

/// A `MOLTIS_*` variable to copy to its `ANTRE_*` name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvPromotion {
    pub legacy: String,
    pub current: String,
    pub value: OsString,
}

/// Lists the `MOLTIS_*` variables whose `ANTRE_*` counterpart is unset.
/// An `ANTRE_*` variable that is already set always wins.
#[must_use]
pub fn legacy_env_promotions<I>(vars: I) -> Vec<EnvPromotion>
where
    I: IntoIterator<Item = (OsString, OsString)>,
{
    let vars: Vec<(OsString, OsString)> = vars.into_iter().collect();
    let present: HashSet<&OsString> = vars.iter().map(|(key, _)| key).collect();
    let mut promotions: Vec<EnvPromotion> = vars
        .iter()
        .filter_map(|(key, value)| {
            let rest = key.to_str()?.strip_prefix(LEGACY_ENV_PREFIX)?;
            if rest.is_empty() {
                return None;
            }
            let current = format!("{ENV_PREFIX}{rest}");
            if present.contains(&OsString::from(&current)) {
                return None;
            }
            Some(EnvPromotion {
                legacy: key.to_str()?.to_owned(),
                current,
                value: value.clone(),
            })
        })
        .collect();
    promotions.sort_by(|a, b| a.legacy.cmp(&b.legacy));
    promotions
}

/// One thing [`migrate_legacy_install`] did or found.
#[derive(Debug)]
pub enum MigrationStep {
    /// A legacy default directory was moved to its new location.
    MovedDir {
        from: PathBuf,
        to: PathBuf,
    },
    /// The legacy directory stays in use; its content is untouched.
    DirMoveFailed {
        from: PathBuf,
        to: PathBuf,
        error: String,
    },
    /// Moved, but the compatibility symlink at the old path is missing.
    SymlinkFailed {
        at: PathBuf,
        error: String,
    },
    RenamedFile {
        from: PathBuf,
        to: PathBuf,
    },
    FileRenameFailed {
        from: PathBuf,
        to: PathBuf,
        error: String,
    },
    /// The config file still has its legacy name; it is read as is.
    LegacyConfigFile(PathBuf),
}

impl MigrationStep {
    #[must_use]
    pub fn is_failure(&self) -> bool {
        matches!(
            self,
            Self::DirMoveFailed { .. } | Self::SymlinkFailed { .. } | Self::FileRenameFailed { .. }
        )
    }
}

impl fmt::Display for MigrationStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MovedDir { from, to } => write!(
                f,
                "moved {} to {} (a symlink stays at the old path)",
                from.display(),
                to.display()
            ),
            Self::DirMoveFailed { from, to, error } => write!(
                f,
                "could not move {} to {}: {error}; still using {}",
                from.display(),
                to.display(),
                from.display()
            ),
            Self::SymlinkFailed { at, error } => {
                write!(f, "could not leave a symlink at {}: {error}", at.display())
            },
            Self::RenamedFile { from, to } => {
                write!(f, "renamed {} to {}", from.display(), to.display())
            },
            Self::FileRenameFailed { from, to, error } => write!(
                f,
                "could not rename {} to {}: {error}",
                from.display(),
                to.display()
            ),
            Self::LegacyConfigFile(path) => write!(
                f,
                "reading legacy config file {}; rename it to antre.{}",
                path.display(),
                path.extension()
                    .and_then(|ext| ext.to_str())
                    .unwrap_or("toml")
            ),
        }
    }
}

/// Moves the legacy default directories to the new ones and renames legacy
/// data files. Run once at startup, after directory overrides are applied
/// and before anything opens a file in them.
///
/// A directory is moved only when it is the one in use: no override points
/// elsewhere and the new directory does not exist yet. Nothing is deleted
/// or overwritten, and a symlink at the old path keeps tools and configs
/// that still name it working.
pub fn migrate_legacy_install() -> Vec<MigrationStep> {
    let mut steps = Vec::new();
    if let Some(home) = crate::home_dir() {
        let pairs = [
            (
                crate::config_dir(),
                legacy_config_dir(&home),
                home.join(".config").join("antre"),
            ),
            (
                Some(crate::data_dir()),
                legacy_data_dir(&home),
                home.join(".antre"),
            ),
        ];
        for (resolved, legacy, current) in pairs {
            if resolved.as_deref() == Some(legacy.as_path()) {
                steps.extend(move_dir(&legacy, &current));
            }
        }
    }
    steps.extend(rename_legacy_files(&crate::data_dir()));
    if let Some(path) = crate::loader::find_config_file()
        && is_legacy_config_file(&path)
    {
        steps.push(MigrationStep::LegacyConfigFile(path));
    }
    steps
}

/// Moves `legacy` to `current` when only `legacy` exists.
pub fn move_dir(legacy: &Path, current: &Path) -> Vec<MigrationStep> {
    let legacy_is_real_dir = std::fs::symlink_metadata(legacy).is_ok_and(|meta| meta.is_dir());
    if !legacy_is_real_dir || std::fs::symlink_metadata(current).is_ok() {
        return Vec::new();
    }
    let failed = |error: std::io::Error| MigrationStep::DirMoveFailed {
        from: legacy.to_path_buf(),
        to: current.to_path_buf(),
        error: error.to_string(),
    };
    if let Some(parent) = current.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        return vec![failed(error)];
    }
    // A rename never copies: across filesystems it fails and the legacy
    // directory stays in use, rather than risking a half-copied tree.
    if let Err(error) = std::fs::rename(legacy, current) {
        return vec![failed(error)];
    }
    let mut steps = vec![MigrationStep::MovedDir {
        from: legacy.to_path_buf(),
        to: current.to_path_buf(),
    }];
    append_marker(current, &format!("moved from {}", legacy.display()));
    if let Err(error) = symlink_dir(current, legacy) {
        steps.push(MigrationStep::SymlinkFailed {
            at: legacy.to_path_buf(),
            error: error.to_string(),
        });
    }
    steps
}

/// Renames legacy data files in `data_dir` whose new name is free.
pub fn rename_legacy_files(data_dir: &Path) -> Vec<MigrationStep> {
    let steps: Vec<MigrationStep> = LEGACY_DATA_FILES
        .iter()
        .filter_map(|(old, new)| {
            let from = data_dir.join(old);
            let to = data_dir.join(new);
            if std::fs::symlink_metadata(&from).is_err() || std::fs::symlink_metadata(&to).is_ok() {
                return None;
            }
            Some(match std::fs::rename(&from, &to) {
                Ok(()) => MigrationStep::RenamedFile { from, to },
                Err(error) => MigrationStep::FileRenameFailed {
                    from,
                    to,
                    error: error.to_string(),
                },
            })
        })
        .collect();
    for step in &steps {
        if let MigrationStep::RenamedFile { from, to } = step {
            append_marker(
                data_dir,
                &format!("renamed {} to {}", from.display(), to.display()),
            );
        }
    }
    steps
}

/// Service files of `service` present under `home`: the launchd agent on
/// macOS, the systemd user unit elsewhere.
#[must_use]
pub fn legacy_service_files(home: &Path, service: &LegacyService) -> Vec<PathBuf> {
    let candidates = if cfg!(target_os = "macos") {
        home.join("Library")
            .join("LaunchAgents")
            .join(format!("{}.plist", service.launchd_label))
    } else {
        home.join(".config")
            .join("systemd")
            .join("user")
            .join(service.systemd_unit)
    };
    [candidates]
        .into_iter()
        .filter(|path| path.exists())
        .collect()
}

/// Stops and removes `service` when it is installed, so it does not run
/// next to its Antre replacement. Returns the files removed. Stopping is
/// best effort; a file that cannot be removed is left and not returned.
pub fn remove_legacy_service(service: &LegacyService) -> Vec<PathBuf> {
    use std::process::Command;

    let Some(home) = crate::home_dir() else {
        return Vec::new();
    };
    let files = legacy_service_files(&home, service);
    if files.is_empty() {
        return files;
    }
    if cfg!(target_os = "macos") {
        for plist in &files {
            let _ = Command::new("launchctl").arg("unload").arg(plist).output();
        }
    } else {
        for args in [["stop", service.systemd_unit], [
            "disable",
            service.systemd_unit,
        ]] {
            let _ = Command::new("systemctl").arg("--user").args(args).output();
        }
    }
    let removed: Vec<PathBuf> = files
        .into_iter()
        .filter(|path| std::fs::remove_file(path).is_ok())
        .collect();
    if !cfg!(target_os = "macos") && !removed.is_empty() {
        let _ = Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .output();
    }
    removed
}

fn append_marker(dir: &Path, line: &str) {
    use std::io::Write;

    let written = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(MIGRATION_MARKER))
        .and_then(|mut file| writeln!(file, "{line}"));
    if let Err(error) = written {
        tracing::debug!(%error, dir = %dir.display(), "could not write migration marker");
    }
}

#[cfg(unix)]
fn symlink_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn symlink_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
#[path = "legacy_brand/tests.rs"]
mod tests;
