//! [`OsSandbox`]: runs commands on the host under a kernel-enforced policy
//! (Seatbelt on macOS, bubblewrap on Linux) instead of inside a container.

use std::path::{Path, PathBuf};

use {async_trait::async_trait, tracing::debug};

#[cfg(target_os = "linux")]
use super::bwrap::{RealHostFs, build_args, linux_system_read_roots, scan_denied_suffixes};
#[cfg(target_os = "macos")]
use super::seatbelt::{SANDBOX_EXEC_PATH, build_profile, macos_system_read_roots};
use {
    super::policy::{OsNetworkAccess, OsPolicyInputs, OsSandboxPolicy, canonicalize_lossy},
    crate::{
        error::{Error, Result},
        exec::{ExecOpts, ExecResult},
        sandbox::{
            platform::ulimit_wrapped_command,
            types::{
                HomePersistence, Sandbox, SandboxConfig, SandboxId, sanitize_path_component,
                truncate_output_for_display,
            },
        },
    },
};

/// Which kernel mechanism enforces the policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OsSandboxKind {
    /// macOS `sandbox-exec` with a generated SBPL profile.
    Seatbelt,
    /// Linux bubblewrap (user + mount + pid namespaces).
    Bubblewrap { bwrap: PathBuf },
}

impl OsSandboxKind {
    #[must_use]
    pub fn backend_name(&self) -> &'static str {
        match self {
            Self::Seatbelt => "seatbelt",
            Self::Bubblewrap { .. } => "bubblewrap",
        }
    }

    /// Network access this mechanism can actually enforce for `requested`.
    /// A bubblewrap sandbox lives in its own network namespace and cannot
    /// reach the egress proxy on the host loopback, so proxy mode fails
    /// closed to no network.
    #[must_use]
    pub fn effective_network(&self, requested: OsNetworkAccess) -> OsNetworkAccess {
        match (self, requested) {
            (Self::Bubblewrap { .. }, OsNetworkAccess::Proxy { .. }) => OsNetworkAccess::Blocked,
            (_, other) => other,
        }
    }

    /// Operator-facing description of what this backend protects.
    #[must_use]
    pub fn protection_summary(&self, requested: OsNetworkAccess) -> String {
        let mechanism = match self {
            Self::Seatbelt => "macOS Seatbelt",
            Self::Bubblewrap { .. } => "Linux bubblewrap",
        };
        let network = match (requested, self.effective_network(requested)) {
            (_, OsNetworkAccess::Open) => "network unrestricted".to_string(),
            (_, OsNetworkAccess::Proxy { port }) => {
                format!("network only through the egress proxy on 127.0.0.1:{port}")
            },
            (OsNetworkAccess::Proxy { .. }, OsNetworkAccess::Blocked) => {
                "network blocked (the egress proxy is not reachable from bubblewrap yet)"
                    .to_string()
            },
            (_, OsNetworkAccess::Blocked) => "network blocked".to_string(),
        };
        format!(
            "{mechanism}: system read-only, writes limited to the sandbox home, temp and \
             writable workspace, user home and Moltis secrets hidden, {network}"
        )
    }

    /// Detect a usable OS sandbox on this host by running a trivial command
    /// under it. Fails when the mechanism is missing or cannot nest (Moltis
    /// already running inside a sandbox, user namespaces disabled).
    #[must_use]
    pub fn detect() -> Option<Self> {
        #[cfg(target_os = "macos")]
        {
            if !Path::new(SANDBOX_EXEC_PATH).is_file() {
                debug!("seatbelt unavailable: {SANDBOX_EXEC_PATH} missing");
                return None;
            }
            let ok = std::process::Command::new(SANDBOX_EXEC_PATH)
                .args(["-p", "(version 1)(allow default)", "/usr/bin/true"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
            if !ok {
                debug!("seatbelt unavailable: sandbox-exec probe failed");
                return None;
            }
            Some(Self::Seatbelt)
        }
        #[cfg(target_os = "linux")]
        {
            let bwrap = find_bwrap()?;
            let ok = std::process::Command::new(&bwrap)
                .args([
                    "--ro-bind",
                    "/",
                    "/",
                    "--proc",
                    "/proc",
                    "--dev",
                    "/dev",
                    "--unshare-user",
                    "--unshare-pid",
                    "--unshare-net",
                    "--die-with-parent",
                    "--",
                    "true",
                ])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
            if !ok {
                debug!(bwrap = %bwrap.display(), "bubblewrap probe failed (user namespaces disabled?)");
                return None;
            }
            Some(Self::Bubblewrap { bwrap })
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            None
        }
    }
}

#[cfg(target_os = "linux")]
fn find_bwrap() -> Option<PathBuf> {
    // Fixed system locations first: a `bwrap` earlier in PATH could be a shim.
    [
        "/usr/bin/bwrap",
        "/usr/local/bin/bwrap",
        "/run/current-system/sw/bin/bwrap",
    ]
    .into_iter()
    .map(PathBuf::from)
    .find(|path| path.is_file())
    .or_else(|| which::which("bwrap").ok())
}

/// Keep only roots that exist. Seatbelt needs them resolved; bubblewrap
/// keeps symlinks (merged `/usr`) so they can be recreated inside.
#[must_use]
pub fn existing_roots(candidates: Vec<PathBuf>, resolve: bool) -> Vec<PathBuf> {
    candidates
        .into_iter()
        .filter(|path| std::fs::symlink_metadata(path).is_ok())
        .map(|path| {
            if resolve {
                canonicalize_lossy(&path)
            } else {
                path
            }
        })
        .collect()
}

/// Environment keys never forwarded to the launcher: they would run code in
/// `bwrap`/`sandbox-exec` before the policy applies.
fn is_loader_env(key: &str) -> bool {
    key.starts_with("LD_") || key.starts_with("DYLD_")
}

/// Host directories the policy is built from.
#[derive(Debug, Clone)]
pub struct OsSandboxDirs {
    pub data_dir: PathBuf,
    pub config_dir: Option<PathBuf>,
    pub user_home: Option<PathBuf>,
}

impl OsSandboxDirs {
    /// The running Moltis instance's directories.
    #[must_use]
    pub fn from_moltis_config() -> Self {
        Self {
            data_dir: moltis_config::data_dir(),
            config_dir: moltis_config::config_dir(),
            user_home: moltis_config::home_dir(),
        }
    }
}

/// OS-level sandbox backend.
pub struct OsSandbox {
    config: SandboxConfig,
    kind: OsSandboxKind,
    proxy_port: u16,
    dirs: OsSandboxDirs,
}

impl OsSandbox {
    #[must_use]
    pub fn new(config: SandboxConfig, kind: OsSandboxKind) -> Self {
        Self::with_dirs(config, kind, OsSandboxDirs::from_moltis_config())
    }

    #[must_use]
    pub fn with_dirs(config: SandboxConfig, kind: OsSandboxKind, dirs: OsSandboxDirs) -> Self {
        Self {
            proxy_port: config.proxy_port,
            config,
            kind,
            dirs,
        }
    }

    /// Port of the egress proxy reachable in `trusted` network mode.
    #[must_use]
    pub fn with_proxy_port(mut self, port: u16) -> Self {
        self.proxy_port = port;
        self
    }

    #[must_use]
    pub fn kind(&self) -> &OsSandboxKind {
        &self.kind
    }

    fn requested_network(&self) -> OsNetworkAccess {
        OsNetworkAccess::from_policy(&self.config.network, self.proxy_port)
    }

    #[must_use]
    pub fn protection_summary(&self) -> String {
        self.kind.protection_summary(self.requested_network())
    }

    fn session_dir(&self, id: &SandboxId) -> PathBuf {
        self.dirs
            .data_dir
            .join("sandbox")
            .join("os")
            .join(sanitize_path_component(&id.key))
    }

    fn home_dir(&self, id: &SandboxId) -> PathBuf {
        let base = self.dirs.data_dir.join("sandbox").join("home");
        match self.config.home_persistence {
            HomePersistence::Off => self.session_dir(id).join("home"),
            HomePersistence::Session => base.join("session").join(sanitize_path_component(&id.key)),
            HomePersistence::Shared => match self.config.shared_home_dir.as_ref() {
                Some(path) if path.is_absolute() => path.clone(),
                Some(path) if !path.as_os_str().is_empty() => self.dirs.data_dir.join(path),
                _ => base.join("shared"),
            },
        }
    }

    fn resolve_extra(&self, paths: &[PathBuf]) -> Vec<PathBuf> {
        let data_dir = &self.dirs.data_dir;
        paths
            .iter()
            .map(|path| {
                let absolute = if path.is_absolute() {
                    path.clone()
                } else {
                    data_dir.join(path)
                };
                canonicalize_lossy(&absolute)
            })
            .collect()
    }

    fn system_roots(&self) -> Vec<PathBuf> {
        match self.kind {
            #[cfg(target_os = "macos")]
            OsSandboxKind::Seatbelt => existing_roots(macos_system_read_roots(), true),
            #[cfg(target_os = "linux")]
            OsSandboxKind::Bubblewrap { .. } => existing_roots(linux_system_read_roots(), false),
            #[allow(unreachable_patterns)]
            _ => Vec::new(),
        }
    }

    /// Create the per-session directories and build the policy for `id`.
    pub(crate) fn prepare_policy(&self, id: &SandboxId) -> Result<OsSandboxPolicy> {
        let home = self.home_dir(id);
        let tmp = self.session_dir(id).join("tmp");
        std::fs::create_dir_all(&home)?;
        std::fs::create_dir_all(&tmp)?;
        let data_dir = canonicalize_lossy(&self.dirs.data_dir);
        OsSandboxPolicy::build(&OsPolicyInputs {
            managed_files_dir: data_dir.join("files"),
            data_dir,
            config_dir: self.dirs.config_dir.as_deref().map(canonicalize_lossy),
            user_home: self.dirs.user_home.as_deref().map(canonicalize_lossy),
            home: canonicalize_lossy(&home),
            tmp: canonicalize_lossy(&tmp),
            workspace_mount: self.config.workspace_mount.clone(),
            managed_files_mount: self.config.managed_files_mount,
            network: self.kind.effective_network(self.requested_network()),
            system_read_roots: self.system_roots(),
            extra_read: self.resolve_extra(&self.config.os_read_paths),
            extra_write: self.resolve_extra(&self.config.os_write_paths),
        })
    }

    fn working_dir(policy: &OsSandboxPolicy, requested: Option<&Path>) -> PathBuf {
        requested
            .filter(|dir| dir.is_dir())
            .map(canonicalize_lossy)
            .filter(|dir| policy.allows_read(dir))
            .unwrap_or_else(|| policy.home.clone())
    }

    /// Build the launcher command (program + args) for `command`.
    #[cfg_attr(not(target_os = "linux"), allow(unused_variables))]
    pub(crate) fn launcher(
        &self,
        policy: &OsSandboxPolicy,
        cwd: &Path,
        command: &str,
    ) -> Result<(PathBuf, Vec<String>)> {
        let wrapped = ulimit_wrapped_command(&self.config.resource_limits, None, command);
        let shell = vec!["/bin/sh".to_string(), "-c".to_string(), wrapped];
        match &self.kind {
            #[cfg(target_os = "macos")]
            OsSandboxKind::Seatbelt => Ok((
                PathBuf::from(SANDBOX_EXEC_PATH),
                build_profile(policy).sandbox_exec_args(&shell),
            )),
            #[cfg(target_os = "linux")]
            OsSandboxKind::Bubblewrap { bwrap } => {
                let databases = scan_denied_suffixes(policy, 4);
                Ok((
                    bwrap.clone(),
                    build_args(policy, &RealHostFs, &databases, cwd, &shell),
                ))
            },
            #[allow(unreachable_patterns)]
            other => {
                let _ = (policy, shell);
                Err(Error::message(format!(
                    "{} sandbox is not supported on this platform",
                    other.backend_name()
                )))
            },
        }
    }
}

#[async_trait]
impl Sandbox for OsSandbox {
    fn backend_name(&self) -> &'static str {
        self.kind.backend_name()
    }

    fn provides_fs_isolation(&self) -> bool {
        true
    }

    fn runs_on_host(&self) -> bool {
        true
    }

    fn exposes_managed_files(&self) -> bool {
        false
    }

    async fn workspace_dir_for(&self, id: &SandboxId) -> String {
        canonicalize_lossy(&self.home_dir(id)).display().to_string()
    }

    async fn ensure_ready(&self, id: &SandboxId, _image_override: Option<&str>) -> Result<()> {
        self.prepare_policy(id).map(|_| ())
    }

    async fn exec(&self, id: &SandboxId, command: &str, opts: &ExecOpts) -> Result<ExecResult> {
        let policy = self.prepare_policy(id)?;
        let cwd = Self::working_dir(&policy, opts.working_dir.as_deref());
        let (program, args) = self.launcher(&policy, &cwd, command)?;

        let mut cmd = tokio::process::Command::new(&program);
        cmd.args(&args)
            .env_clear()
            .current_dir(&cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let path_var = match self.kind {
            OsSandboxKind::Seatbelt => super::seatbelt::MACOS_SANDBOX_PATH,
            OsSandboxKind::Bubblewrap { .. } => super::bwrap::LINUX_SANDBOX_PATH,
        };
        for (key, value) in policy.base_env(path_var) {
            cmd.env(key, value);
        }
        for (key, value) in opts.env.iter().filter(|(key, _)| !is_loader_env(key)) {
            cmd.env(key, value);
        }

        debug!(
            backend = self.backend_name(),
            sandbox_id = %id,
            cwd = %cwd.display(),
            "os sandbox exec"
        );
        let child = cmd.spawn().map_err(|error| {
            Error::message(format!(
                "{} sandbox failed to start {}: {error}",
                self.backend_name(),
                program.display()
            ))
        })?;
        match tokio::time::timeout(opts.timeout, child.wait_with_output()).await {
            Ok(Ok(output)) => {
                let mut stdout = String::from_utf8_lossy(&output.stdout).into_owned();
                let mut stderr = String::from_utf8_lossy(&output.stderr).into_owned();
                truncate_output_for_display(&mut stdout, opts.max_output_bytes);
                truncate_output_for_display(&mut stderr, opts.max_output_bytes);
                Ok(ExecResult {
                    stdout,
                    stderr,
                    exit_code: output.status.code().unwrap_or(-1),
                })
            },
            Ok(Err(error)) => Err(Error::message(format!(
                "{} sandbox exec failed: {error}",
                self.backend_name()
            ))),
            Err(_) => Err(Error::message(format!(
                "{} sandbox exec timed out after {}s",
                self.backend_name(),
                opts.timeout.as_secs()
            ))),
        }
    }

    async fn cleanup(&self, id: &SandboxId) -> Result<()> {
        let dir = self.session_dir(id);
        match tokio::fs::remove_dir_all(&dir).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn bubblewrap() -> OsSandboxKind {
        OsSandboxKind::Bubblewrap {
            bwrap: PathBuf::from("/usr/bin/bwrap"),
        }
    }

    #[test]
    fn os_sandbox_routes_through_the_configured_proxy_port() {
        let config = SandboxConfig {
            network: moltis_network_filter::NetworkPolicy::Trusted,
            proxy_port: 45678,
            ..Default::default()
        };
        let dirs = OsSandboxDirs {
            data_dir: PathBuf::from("/tmp/moltis-data"),
            config_dir: None,
            user_home: None,
        };
        let sandbox = OsSandbox::with_dirs(config, OsSandboxKind::Seatbelt, dirs);
        assert_eq!(sandbox.requested_network(), OsNetworkAccess::Proxy {
            port: 45678
        });
    }

    #[test]
    fn backend_names() {
        assert_eq!(OsSandboxKind::Seatbelt.backend_name(), "seatbelt");
        assert_eq!(bubblewrap().backend_name(), "bubblewrap");
    }

    #[test]
    fn bubblewrap_proxy_fails_closed() {
        let proxy = OsNetworkAccess::Proxy { port: 18791 };
        assert_eq!(
            bubblewrap().effective_network(proxy),
            OsNetworkAccess::Blocked
        );
        assert_eq!(OsSandboxKind::Seatbelt.effective_network(proxy), proxy);
        assert_eq!(
            bubblewrap().effective_network(OsNetworkAccess::Open),
            OsNetworkAccess::Open
        );
    }

    #[test]
    fn protection_summary_names_network_mode() {
        let proxy = OsNetworkAccess::Proxy { port: 18791 };
        assert!(
            OsSandboxKind::Seatbelt
                .protection_summary(proxy)
                .contains("egress proxy on 127.0.0.1:18791")
        );
        assert!(
            bubblewrap()
                .protection_summary(proxy)
                .contains("network blocked (the egress proxy is not reachable")
        );
        assert!(
            OsSandboxKind::Seatbelt
                .protection_summary(OsNetworkAccess::Blocked)
                .ends_with("network blocked")
        );
    }

    #[test]
    fn loader_env_is_filtered() {
        assert!(is_loader_env("LD_PRELOAD"));
        assert!(is_loader_env("DYLD_INSERT_LIBRARIES"));
        assert!(!is_loader_env("OPENAI_API_KEY"));
    }

    #[test]
    fn existing_roots_drops_missing() {
        let dir = tempfile::tempdir().unwrap();
        let roots = existing_roots(
            vec![dir.path().to_path_buf(), dir.path().join("missing")],
            true,
        );
        assert_eq!(roots, vec![std::fs::canonicalize(dir.path()).unwrap()]);
    }

    #[test]
    fn working_dir_falls_back_to_home() {
        let dir = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(dir.path()).unwrap();
        let policy = OsSandboxPolicy::build(&OsPolicyInputs {
            data_dir: base.join("data"),
            config_dir: None,
            user_home: None,
            home: base.join("data/home"),
            tmp: base.join("data/tmp"),
            workspace_mount: crate::sandbox::types::WorkspaceMount::Ro,
            managed_files_dir: base.join("data/files"),
            managed_files_mount: crate::sandbox::types::ManagedFilesMount::Ro,
            network: OsNetworkAccess::Blocked,
            system_read_roots: Vec::new(),
            extra_read: Vec::new(),
            extra_write: Vec::new(),
        })
        .unwrap();
        std::fs::create_dir_all(base.join("data/notes")).unwrap();
        std::fs::create_dir_all(base.join("outside")).unwrap();
        assert_eq!(
            OsSandbox::working_dir(&policy, Some(&base.join("data/notes"))),
            base.join("data/notes")
        );
        assert_eq!(
            OsSandbox::working_dir(&policy, Some(&base.join("outside"))),
            base.join("data/home")
        );
        assert_eq!(
            OsSandbox::working_dir(&policy, Some(Path::new("/home/sandbox/nope"))),
            base.join("data/home")
        );
        assert_eq!(
            OsSandbox::working_dir(&policy, None),
            base.join("data/home")
        );
    }
}
