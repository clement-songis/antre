//! Platform-neutral filesystem and network policy shared by the OS sandbox
//! backends (Seatbelt on macOS, bubblewrap on Linux).
//!
//! The policy is deny-by-default: a sandboxed command may only read the
//! system roots needed to run programs, the Moltis workspace and its own
//! home/temp directories, and may only write to its home, its temp directory
//! and (when configured `rw`) the workspace and managed Files.

use std::path::{Component, Path, PathBuf};

use crate::{
    error::{Error, Result},
    sandbox::types::{ManagedFilesMount, NetworkPolicy, WorkspaceMount},
};

/// Files directly under the Moltis data directory that hold secrets or
/// private state. They stay unreadable even when the workspace is mounted.
pub(crate) const DENIED_DATA_DIR_ENTRIES: &[&str] = &[
    "credentials.json",
    "provider_keys.json",
    "oauth_tokens.json",
    "oauth_providers.json",
    "mcp_oauth_registrations.json",
    "mcp-servers.json",
    "moltis.toml",
    ".env",
    "logs",
    "moltis.log",
    "kimi_device_id",
    "browser",
];

/// Entries under the data directory that Moltis loads or executes on the
/// host (hooks, plugins, service scripts). Readable, but never writable from
/// the sandbox even with `workspace_mount = "rw"`: writing them would run
/// code outside the sandbox.
pub(crate) const READ_ONLY_DATA_DIR_ENTRIES: &[&str] = &[
    "hooks",
    "disabled_hooks.json",
    "plugins",
    "plugins-manifest.json",
    "installed-skills",
    "bundled-skills",
    "skills-manifest.json",
    "moltis-service-supervisor.sh",
    "moltis.pid",
    "moltis-service.pid",
    "moltis-service.stop",
];

/// SQLite databases anywhere under the data directory (sessions, auth,
/// memory) are denied by suffix.
pub(crate) const DENIED_DATA_DIR_SUFFIXES: &[&str] = &[
    ".db",
    ".db-wal",
    ".db-shm",
    ".db-journal",
    ".sqlite",
    ".sqlite3",
];

/// Credential stores under `$HOME` that stay denied even when an operator
/// adds `$HOME` (or a parent) to the extra read/write paths.
pub(crate) const DENIED_HOME_ENTRIES: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".aws",
    ".azure",
    ".kube",
    ".docker",
    ".netrc",
    ".npmrc",
    ".pypirc",
    ".git-credentials",
    ".config/gcloud",
    ".config/gh",
    ".config/moltis",
    ".moltis",
    "Library/Keychains",
    "Library/Cookies",
];

/// Network reachability granted to the sandboxed command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsNetworkAccess {
    /// No network at all (loopback included).
    Blocked,
    /// Only the local egress proxy on `127.0.0.1:<port>` is reachable.
    Proxy { port: u16 },
    /// Unrestricted network.
    Open,
}

impl OsNetworkAccess {
    #[must_use]
    pub fn from_policy(policy: &NetworkPolicy, proxy_port: u16) -> Self {
        match policy {
            NetworkPolicy::Blocked => Self::Blocked,
            NetworkPolicy::Trusted => Self::Proxy { port: proxy_port },
            NetworkPolicy::Bypass => Self::Open,
        }
    }

    #[must_use]
    pub fn proxy_url(&self) -> Option<String> {
        match self {
            Self::Proxy { port } => Some(format!("http://127.0.0.1:{port}")),
            Self::Blocked | Self::Open => None,
        }
    }
}

/// Inputs for building a policy. Kept free of global lookups so policy
/// generation is unit-testable on every platform.
#[derive(Debug, Clone)]
pub struct OsPolicyInputs {
    pub data_dir: PathBuf,
    pub config_dir: Option<PathBuf>,
    pub user_home: Option<PathBuf>,
    pub home: PathBuf,
    pub tmp: PathBuf,
    pub workspace_mount: WorkspaceMount,
    pub managed_files_dir: PathBuf,
    pub managed_files_mount: ManagedFilesMount,
    pub network: OsNetworkAccess,
    pub system_read_roots: Vec<PathBuf>,
    pub extra_read: Vec<PathBuf>,
    pub extra_write: Vec<PathBuf>,
}

/// Resolved policy: every path is absolute and normalized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OsSandboxPolicy {
    /// Roots readable (and executable) but not writable.
    pub read_only: Vec<PathBuf>,
    /// Roots readable and writable.
    pub read_write: Vec<PathBuf>,
    /// Exact paths (files or directories) denied for read and write, even
    /// inside an allowed root.
    pub denied: Vec<PathBuf>,
    /// Exact paths readable where their root allows it, but never writable.
    pub write_denied: Vec<PathBuf>,
    /// Directories under which files ending in one of `denied_suffixes`
    /// are denied.
    pub denied_suffix_roots: Vec<PathBuf>,
    pub denied_suffixes: Vec<String>,
    pub home: PathBuf,
    pub tmp: PathBuf,
    pub network: OsNetworkAccess,
}

impl OsSandboxPolicy {
    /// Build the policy from inputs. Fails closed on relative paths or paths
    /// containing characters that cannot be expressed safely in a profile.
    pub fn build(inputs: &OsPolicyInputs) -> Result<Self> {
        let mut read_only = Vec::new();
        let mut read_write = Vec::new();
        let mut denied = Vec::new();

        for root in &inputs.system_read_roots {
            read_only.push(checked_path(root)?);
        }

        let data_dir = checked_path(&inputs.data_dir)?;
        match inputs.workspace_mount {
            WorkspaceMount::Ro => read_only.push(data_dir.clone()),
            WorkspaceMount::Rw => read_write.push(data_dir.clone()),
            WorkspaceMount::None => {},
        }

        let files_dir = checked_path(&inputs.managed_files_dir)?;
        match inputs.managed_files_mount {
            ManagedFilesMount::Ro => read_only.push(files_dir),
            ManagedFilesMount::Rw => read_write.push(files_dir),
            ManagedFilesMount::None => denied.push(files_dir),
        }

        for path in &inputs.extra_read {
            read_only.push(checked_path(path)?);
        }
        for path in &inputs.extra_write {
            read_write.push(checked_path(path)?);
        }

        let home = checked_path(&inputs.home)?;
        let tmp = checked_path(&inputs.tmp)?;
        read_write.push(home.clone());
        read_write.push(tmp.clone());

        denied.extend(
            DENIED_DATA_DIR_ENTRIES
                .iter()
                .map(|entry| data_dir.join(entry)),
        );
        if let Some(config_dir) = &inputs.config_dir {
            denied.push(checked_path(config_dir)?);
        }
        if let Some(user_home) = &inputs.user_home {
            let user_home = checked_path(user_home)?;
            denied.extend(
                DENIED_HOME_ENTRIES
                    .iter()
                    .map(|entry| user_home.join(entry)),
            );
        }
        // A deny on an ancestor of the workspace or of the sandbox's own
        // directories (config dir == data dir, data dir == `~/.moltis`) would
        // hide them entirely; the per-entry data dir denies cover that case.
        let protected = [&data_dir, &home, &tmp];
        denied.retain(|path| !protected.iter().any(|kept| kept.starts_with(path)));

        dedup_paths(&mut read_only);
        dedup_paths(&mut read_write);
        dedup_paths(&mut denied);
        // A root listed as writable does not need a read-only entry too.
        read_only.retain(|path| !read_write.contains(path));

        let write_denied = READ_ONLY_DATA_DIR_ENTRIES
            .iter()
            .map(|entry| data_dir.join(entry))
            .collect();

        Ok(Self {
            read_only,
            read_write,
            denied,
            write_denied,
            denied_suffix_roots: vec![data_dir],
            denied_suffixes: DENIED_DATA_DIR_SUFFIXES
                .iter()
                .map(|suffix| (*suffix).to_string())
                .collect(),
            home,
            tmp,
            network: inputs.network,
        })
    }

    fn is_denied(&self, path: &Path) -> bool {
        if self.denied.iter().any(|denied| path.starts_with(denied)) {
            return true;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return false;
        };
        self.denied_suffix_roots
            .iter()
            .any(|root| path.starts_with(root))
            && self
                .denied_suffixes
                .iter()
                .any(|suffix| name.ends_with(suffix.as_str()))
    }

    /// Host-side mirror of the kernel policy, used for diagnostics and tests.
    /// The kernel sandbox stays the enforcement point.
    #[must_use]
    pub fn allows_read(&self, path: &Path) -> bool {
        let path = normalize(path);
        if self.is_denied(&path) {
            return false;
        }
        self.read_only
            .iter()
            .chain(self.read_write.iter())
            .any(|root| path.starts_with(root))
    }

    #[must_use]
    pub fn allows_write(&self, path: &Path) -> bool {
        let path = normalize(path);
        if self.is_denied(&path)
            || self
                .write_denied
                .iter()
                .any(|denied| path.starts_with(denied))
        {
            return false;
        }
        // A read-only root nested inside a writable one (managed Files `ro`
        // inside a `rw` workspace) wins: the most specific root decides.
        let best_rw = longest_prefix(&self.read_write, &path);
        let best_ro = longest_prefix(&self.read_only, &path);
        match (best_rw, best_ro) {
            (Some(rw), Some(ro)) => rw > ro,
            (Some(_), None) => true,
            _ => false,
        }
    }

    /// Read-only roots nested inside a writable root; profiles must re-deny
    /// writes there after granting the writable root.
    #[must_use]
    pub fn read_only_inside_writable(&self) -> Vec<&PathBuf> {
        self.read_only
            .iter()
            .filter(|ro| {
                self.read_write
                    .iter()
                    .any(|rw| ro.starts_with(rw) && *ro != rw)
            })
            .collect()
    }

    /// Minimal environment for the sandboxed command. The host environment is
    /// never inherited.
    #[must_use]
    pub fn base_env(&self, path_var: &str) -> Vec<(String, String)> {
        let mut env = vec![
            ("PATH".to_string(), path_var.to_string()),
            ("HOME".to_string(), self.home.display().to_string()),
            ("TMPDIR".to_string(), self.tmp.display().to_string()),
            ("LANG".to_string(), "C.UTF-8".to_string()),
            ("MOLTIS_SANDBOX".to_string(), "os".to_string()),
        ];
        if let Some(proxy) = self.network.proxy_url() {
            for key in ["HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy"] {
                env.push((key.to_string(), proxy.clone()));
            }
            for key in ["ALL_PROXY", "all_proxy"] {
                env.push((key.to_string(), proxy.clone()));
            }
            for key in ["NO_PROXY", "no_proxy"] {
                env.push((key.to_string(), "localhost,127.0.0.1,::1".to_string()));
            }
        }
        env
    }
}

fn longest_prefix(roots: &[PathBuf], path: &Path) -> Option<usize> {
    roots
        .iter()
        .filter(|root| path.starts_with(root))
        .map(|root| root.components().count())
        .max()
}

fn dedup_paths(paths: &mut Vec<PathBuf>) {
    let mut seen = std::collections::HashSet::new();
    paths.retain(|path| seen.insert(path.clone()));
}

/// Reject relative paths and characters (quotes, control characters) that
/// could break out of a profile string or a regex. Paths are normalized
/// lexically; symlinks are resolved separately by [`canonicalize_lossy`].
pub(crate) fn checked_path(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        return Err(Error::message(format!(
            "os sandbox: path '{}' must be absolute",
            path.display()
        )));
    }
    let Some(text) = path.to_str() else {
        return Err(Error::message(format!(
            "os sandbox: path '{}' is not valid UTF-8",
            path.display()
        )));
    };
    if text.chars().any(|ch| ch == '"' || ch.is_control()) {
        return Err(Error::message(format!(
            "os sandbox: path '{text}' contains a quote or control character"
        )));
    }
    Ok(normalize(path))
}

/// Resolve `.` and `..` without touching the filesystem.
pub(crate) fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            },
            Component::CurDir => {},
            other => out.push(other),
        }
    }
    out
}

/// Resolve symlinks for the longest existing prefix of `path` and append the
/// remaining components. Seatbelt matches on resolved paths (`/tmp` is
/// `/private/tmp`, `/var` is `/private/var`), so every root must be resolved
/// before it reaches a profile.
pub(crate) fn canonicalize_lossy(path: &Path) -> PathBuf {
    let normalized = normalize(path);
    let mut existing = normalized.clone();
    let mut rest = Vec::new();
    loop {
        if let Ok(resolved) = std::fs::canonicalize(&existing) {
            let mut out = resolved;
            for component in rest.iter().rev() {
                out.push(component);
            }
            return out;
        }
        match (
            existing.file_name().map(ToOwned::to_owned),
            existing.parent(),
        ) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                existing = parent.to_path_buf();
            },
            _ => return normalized,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn inputs() -> OsPolicyInputs {
        OsPolicyInputs {
            data_dir: PathBuf::from("/data"),
            config_dir: Some(PathBuf::from("/home/u/.config/moltis")),
            user_home: Some(PathBuf::from("/home/u")),
            home: PathBuf::from("/data/sandbox/home/shared"),
            tmp: PathBuf::from("/data/sandbox/os/s1/tmp"),
            workspace_mount: WorkspaceMount::Ro,
            managed_files_dir: PathBuf::from("/data/files"),
            managed_files_mount: ManagedFilesMount::Ro,
            network: OsNetworkAccess::Proxy { port: 18791 },
            system_read_roots: vec![PathBuf::from("/usr"), PathBuf::from("/bin")],
            extra_read: Vec::new(),
            extra_write: Vec::new(),
        }
    }

    #[test]
    fn workspace_ro_is_readable_not_writable() {
        let policy = OsSandboxPolicy::build(&inputs()).unwrap();
        assert!(policy.allows_read(Path::new("/data/notes/a.md")));
        assert!(!policy.allows_write(Path::new("/data/notes/a.md")));
        assert!(policy.allows_write(Path::new("/data/sandbox/home/shared/x")));
        assert!(policy.allows_write(Path::new("/data/sandbox/os/s1/tmp/x")));
    }

    #[test]
    fn workspace_rw_is_writable() {
        let policy = OsSandboxPolicy::build(&OsPolicyInputs {
            workspace_mount: WorkspaceMount::Rw,
            ..inputs()
        })
        .unwrap();
        assert!(policy.allows_write(Path::new("/data/notes/a.md")));
        // Managed Files stays read-only inside the writable workspace.
        assert!(!policy.allows_write(Path::new("/data/files/doc.pdf")));
        assert!(policy.allows_read(Path::new("/data/files/doc.pdf")));
        assert_eq!(policy.read_only_inside_writable(), vec![&PathBuf::from(
            "/data/files"
        )]);
    }

    #[test]
    fn workspace_none_hides_data_dir() {
        let policy = OsSandboxPolicy::build(&OsPolicyInputs {
            workspace_mount: WorkspaceMount::None,
            managed_files_mount: ManagedFilesMount::None,
            ..inputs()
        })
        .unwrap();
        assert!(!policy.allows_read(Path::new("/data/notes/a.md")));
        assert!(!policy.allows_read(Path::new("/data/files/doc.pdf")));
        assert!(policy.allows_read(Path::new("/data/sandbox/home/shared/x")));
    }

    #[test]
    fn secrets_in_data_dir_are_denied() {
        let policy = OsSandboxPolicy::build(&OsPolicyInputs {
            workspace_mount: WorkspaceMount::Rw,
            ..inputs()
        })
        .unwrap();
        for path in [
            "/data/credentials.json",
            "/data/moltis.db",
            "/data/moltis.db-wal",
            "/data/agents/main/memory.db",
            "/data/oauth_tokens.json",
            "/data/logs/commands.log",
        ] {
            assert!(!policy.allows_read(Path::new(path)), "{path} readable");
            assert!(!policy.allows_write(Path::new(path)), "{path} writable");
        }
    }

    #[test]
    fn host_executed_entries_stay_read_only_in_rw_workspace() {
        let policy = OsSandboxPolicy::build(&OsPolicyInputs {
            workspace_mount: WorkspaceMount::Rw,
            ..inputs()
        })
        .unwrap();
        for path in [
            "/data/hooks/pre.sh",
            "/data/plugins/x/plugin.toml",
            "/data/plugins-manifest.json",
            "/data/moltis-service-supervisor.sh",
        ] {
            assert!(policy.allows_read(Path::new(path)), "{path} unreadable");
            assert!(!policy.allows_write(Path::new(path)), "{path} writable");
        }
        assert!(policy.allows_write(Path::new("/data/notes/today.md")));
    }

    #[test]
    fn user_home_and_its_secrets_are_denied() {
        let policy = OsSandboxPolicy::build(&inputs()).unwrap();
        assert!(!policy.allows_read(Path::new("/home/u/.ssh/id_ed25519")));
        assert!(!policy.allows_read(Path::new("/home/u/Documents/a.txt")));
        assert!(!policy.allows_write(Path::new("/home/u/x")));
        assert!(!policy.allows_write(Path::new("/etc/passwd")));
        assert!(!policy.allows_write(Path::new("/usr/bin/env")));
        assert!(policy.allows_read(Path::new("/usr/bin/env")));
    }

    #[test]
    fn extra_home_read_keeps_credential_denies() {
        let policy = OsSandboxPolicy::build(&OsPolicyInputs {
            extra_read: vec![PathBuf::from("/home/u")],
            ..inputs()
        })
        .unwrap();
        assert!(policy.allows_read(Path::new("/home/u/Documents/a.txt")));
        assert!(!policy.allows_read(Path::new("/home/u/.ssh/id_ed25519")));
        assert!(!policy.allows_read(Path::new("/home/u/.config/moltis/credentials.json")));
        assert!(!policy.allows_read(Path::new("/home/u/.aws/credentials")));
    }

    #[test]
    fn traversal_does_not_escape() {
        let policy = OsSandboxPolicy::build(&inputs()).unwrap();
        assert!(!policy.allows_write(Path::new(
            "/data/sandbox/os/s1/tmp/../../../../home/u/.bashrc"
        )));
    }

    #[test]
    fn rejects_relative_and_quoted_paths() {
        assert!(
            OsSandboxPolicy::build(&OsPolicyInputs {
                home: PathBuf::from("relative/home"),
                ..inputs()
            })
            .is_err()
        );
        assert!(
            OsSandboxPolicy::build(&OsPolicyInputs {
                extra_read: vec![PathBuf::from("/tmp/a\"b")],
                ..inputs()
            })
            .is_err()
        );
        assert!(
            OsSandboxPolicy::build(&OsPolicyInputs {
                extra_write: vec![PathBuf::from("/tmp/a\nb")],
                ..inputs()
            })
            .is_err()
        );
    }

    #[test]
    fn deny_never_shadows_sandbox_dirs() {
        // Config dir == data dir (container deployments) must not deny the
        // sandbox home that lives inside it.
        let policy = OsSandboxPolicy::build(&OsPolicyInputs {
            config_dir: Some(PathBuf::from("/data")),
            ..inputs()
        })
        .unwrap();
        assert!(policy.allows_write(Path::new("/data/sandbox/home/shared/x")));
        assert!(policy.allows_read(Path::new("/data/notes/a.md")));
        assert!(!policy.allows_read(Path::new("/data/credentials.json")));
        assert!(!policy.denied.contains(&PathBuf::from("/data")));
    }

    #[test]
    fn default_data_dir_under_home_stays_readable() {
        // `data_dir()` defaults to `~/.moltis`, which is also a home deny entry.
        let policy = OsSandboxPolicy::build(&OsPolicyInputs {
            data_dir: PathBuf::from("/home/u/.moltis"),
            managed_files_dir: PathBuf::from("/home/u/.moltis/files"),
            home: PathBuf::from("/home/u/.moltis/sandbox/home/shared"),
            tmp: PathBuf::from("/home/u/.moltis/sandbox/os/s1/tmp"),
            ..inputs()
        })
        .unwrap();
        assert!(policy.allows_read(Path::new("/home/u/.moltis/MEMORY.md")));
        assert!(!policy.allows_read(Path::new("/home/u/.moltis/moltis.db")));
        assert!(!policy.allows_read(Path::new("/home/u/.ssh/id_ed25519")));
    }

    #[test]
    fn env_is_minimal_and_routes_through_proxy() {
        let policy = OsSandboxPolicy::build(&inputs()).unwrap();
        let env = policy.base_env("/usr/bin:/bin");
        let get = |key: &str| env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
        assert_eq!(get("HOME"), Some("/data/sandbox/home/shared"));
        assert_eq!(get("TMPDIR"), Some("/data/sandbox/os/s1/tmp"));
        assert_eq!(get("HTTPS_PROXY"), Some("http://127.0.0.1:18791"));
        assert_eq!(get("PATH"), Some("/usr/bin:/bin"));

        let blocked = OsSandboxPolicy::build(&OsPolicyInputs {
            network: OsNetworkAccess::Blocked,
            ..inputs()
        })
        .unwrap();
        assert!(
            blocked
                .base_env("/usr/bin")
                .iter()
                .all(|(k, _)| !k.to_ascii_lowercase().contains("proxy"))
        );
    }

    #[test]
    fn network_access_from_policy() {
        assert_eq!(
            OsNetworkAccess::from_policy(&NetworkPolicy::Blocked, 1),
            OsNetworkAccess::Blocked
        );
        assert_eq!(
            OsNetworkAccess::from_policy(&NetworkPolicy::Trusted, 1),
            OsNetworkAccess::Proxy { port: 1 }
        );
        assert_eq!(
            OsNetworkAccess::from_policy(&NetworkPolicy::Bypass, 1),
            OsNetworkAccess::Open
        );
    }

    #[test]
    fn canonicalize_lossy_resolves_existing_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let resolved = canonicalize_lossy(&dir.path().join("missing/child"));
        let base = std::fs::canonicalize(dir.path()).unwrap();
        assert_eq!(resolved, base.join("missing").join("child"));
    }
}
