//! macOS Seatbelt (`sandbox-exec`) profile generation.
//!
//! Paths reach the profile only as `-D` parameters, never interpolated into
//! the profile text, so a path cannot inject SBPL. The one exception is the
//! database-suffix deny, which needs a regex; [`OsSandboxPolicy::build`]
//! already rejects quotes and control characters and the path is
//! regex-escaped.

use std::path::{Path, PathBuf};

use super::policy::{OsNetworkAccess, OsSandboxPolicy};

/// Only the system copy is trusted; a `sandbox-exec` earlier in `PATH` could
/// be a no-op shim.
pub const SANDBOX_EXEC_PATH: &str = "/usr/bin/sandbox-exec";

pub(crate) const SEATBELT_BASE_POLICY: &str = include_str!("seatbelt_base.sbpl");

/// `PATH` given to sandboxed commands on macOS.
pub const MACOS_SANDBOX_PATH: &str =
    "/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";

/// Read-only system roots on macOS, as resolved paths. Missing entries are
/// dropped at runtime by [`existing_roots`](super::backend::existing_roots).
#[must_use]
pub fn macos_system_read_roots() -> Vec<PathBuf> {
    [
        "/usr",
        "/bin",
        "/sbin",
        "/System",
        "/Library/Apple",
        "/Library/Preferences",
        "/Library/Developer/CommandLineTools",
        "/Library/Frameworks",
        "/private/etc",
        "/private/var/select",
        "/private/var/db/timezone",
        "/opt/homebrew",
        "/opt/local",
        "/nix/store",
        "/Applications/Xcode.app",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect()
}

/// A generated profile plus the `-D` parameters it references.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeatbeltProfile {
    pub profile: String,
    pub params: Vec<(String, PathBuf)>,
}

impl SeatbeltProfile {
    /// Arguments for `/usr/bin/sandbox-exec`, ending with `command`.
    #[must_use]
    pub fn sandbox_exec_args(&self, command: &[String]) -> Vec<String> {
        let mut args = vec!["-p".to_string(), self.profile.clone()];
        args.extend(
            self.params
                .iter()
                .map(|(key, value)| format!("-D{key}={}", value.display())),
        );
        args.push("--".to_string());
        args.extend(command.iter().cloned());
        args
    }
}

struct ParamSet {
    params: Vec<(String, PathBuf)>,
}

impl ParamSet {
    fn add(&mut self, prefix: &str, path: &Path) -> String {
        let key = format!("{prefix}_{}", self.params.len());
        self.params.push((key.clone(), path.to_path_buf()));
        key
    }
}

fn subpaths(keys: &[String]) -> String {
    keys.iter()
        .map(|key| format!("(subpath (param \"{key}\"))"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Build the per-session profile. Paths in `policy` must already be resolved
/// (no symlinks): Seatbelt matches on the resolved vnode path.
#[must_use]
pub fn build_profile(policy: &OsSandboxPolicy) -> SeatbeltProfile {
    let mut params = ParamSet { params: Vec::new() };
    let mut sections = vec![SEATBELT_BASE_POLICY.to_string()];

    let ro_keys: Vec<String> = policy
        .read_only
        .iter()
        .map(|path| params.add("RO", path))
        .collect();
    let rw_keys: Vec<String> = policy
        .read_write
        .iter()
        .map(|path| params.add("RW", path))
        .collect();

    if !ro_keys.is_empty() {
        sections.push(format!(
            "; read-only roots\n(allow file-read* file-map-executable {})",
            subpaths(&ro_keys)
        ));
    }
    if !rw_keys.is_empty() {
        sections.push(format!(
            "; writable roots\n(allow file-read* file-write* file-map-executable {})",
            subpaths(&rw_keys)
        ));
    }

    // Programs stat every ancestor of their cwd (getcwd, realpath); grant
    // metadata only, never contents or listings.
    let ancestor_rules: Vec<String> = ro_keys
        .iter()
        .chain(rw_keys.iter())
        .map(|key| format!("(path-ancestors (param \"{key}\"))"))
        .collect();
    if !ancestor_rules.is_empty() {
        sections.push(format!(
            "(allow file-read-metadata {})",
            ancestor_rules.join(" ")
        ));
    }

    // Unix sockets only inside the sandbox's own home and temp (tmux, ssh
    // control masters, language servers). Host sockets (ssh-agent,
    // docker.sock) stay unreachable.
    let ipc_keys = [
        params.add("IPC", &policy.home),
        params.add("IPC", &policy.tmp),
    ];
    for key in &ipc_keys {
        sections.push(format!(
            "(allow network-bind (local unix-socket (subpath (param \"{key}\"))))\n\
             (allow network-outbound (remote unix-socket (subpath (param \"{key}\"))))"
        ));
    }

    sections.push(network_section(policy.network));

    // Read-only roots nested in a writable root (managed Files `ro` inside a
    // `rw` workspace): later rules win, so re-deny writes after the grant.
    let nested_ro: Vec<String> = policy
        .read_only_inside_writable()
        .into_iter()
        .map(|path| params.add("NESTED_RO", path))
        .collect();
    if !nested_ro.is_empty() {
        sections.push(format!("(deny file-write* {})", subpaths(&nested_ro)));
    }

    let write_deny_keys: Vec<String> = policy
        .write_denied
        .iter()
        .map(|path| params.add("NO_WRITE", path))
        .collect();
    if !write_deny_keys.is_empty() {
        sections.push(format!(
            "; loaded or executed by Antre on the host\n(deny file-write* {})",
            subpaths(&write_deny_keys)
        ));
    }

    // Denies go last so no earlier grant can reopen them.
    let deny_keys: Vec<String> = policy
        .denied
        .iter()
        .map(|path| params.add("DENY", path))
        .collect();
    if !deny_keys.is_empty() {
        sections.push(format!(
            "; secrets and credential stores\n(deny file-read* file-write* {})",
            subpaths(&deny_keys)
        ));
    }
    if let Some(regex) = suffix_deny_regex(policy) {
        sections.push(format!(
            "(deny file-read* file-write* (regex #\"{regex}\"))"
        ));
    }

    SeatbeltProfile {
        profile: sections.join("\n"),
        params: params.params,
    }
}

fn network_section(network: OsNetworkAccess) -> String {
    const TLS_TRUST: &str = "(allow mach-lookup (global-name \"com.apple.TrustEvaluationAgent\") \
                             (global-name \"com.apple.ocspd\"))";
    match network {
        OsNetworkAccess::Blocked => "; network: blocked".to_string(),
        OsNetworkAccess::Proxy { port } => format!(
            "; network: egress proxy only\n\
             (allow network-outbound (remote ip \"localhost:{port}\"))\n{TLS_TRUST}"
        ),
        OsNetworkAccess::Open => format!(
            "; network: open\n\
             (allow network-outbound (remote ip \"*:*\"))\n\
             (allow network-inbound (local ip \"*:*\"))\n\
             (allow network-bind (local ip \"*:*\"))\n\
             (allow network-outbound (literal \"/private/var/run/mDNSResponder\"))\n\
             (allow system-socket (require-all (socket-domain AF_SYSTEM) (socket-protocol 2)))\n\
             (allow mach-lookup (global-name \"com.apple.SystemConfiguration.DNSConfiguration\") \
             (global-name \"com.apple.SystemConfiguration.configd\") \
             (global-name \"com.apple.networkd\"))\n{TLS_TRUST}"
        ),
    }
}

fn suffix_deny_regex(policy: &OsSandboxPolicy) -> Option<String> {
    if policy.denied_suffix_roots.is_empty() || policy.denied_suffixes.is_empty() {
        return None;
    }
    let roots = policy
        .denied_suffix_roots
        .iter()
        .map(|root| regex_escape(&root.display().to_string()))
        .collect::<Vec<_>>()
        .join("|");
    let suffixes = policy
        .denied_suffixes
        .iter()
        .map(|suffix| regex_escape(suffix))
        .collect::<Vec<_>>()
        .join("|");
    Some(format!("^({roots})/(.*/)?[^/]*({suffixes})$"))
}

/// Escape POSIX extended-regex metacharacters, plus `\` and `"` for the SBPL
/// `#"..."` string literal.
pub(crate) fn regex_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(
            ch,
            '\\' | '.'
                | '^'
                | '$'
                | '|'
                | '?'
                | '*'
                | '+'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '"'
        ) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use {
        super::*,
        crate::sandbox::{
            os::policy::OsPolicyInputs,
            types::{ManagedFilesMount, WorkspaceMount},
        },
    };

    fn policy(network: OsNetworkAccess, workspace: WorkspaceMount) -> OsSandboxPolicy {
        OsSandboxPolicy::build(&OsPolicyInputs {
            data_dir: PathBuf::from("/Users/u/.antre"),
            config_dir: Some(PathBuf::from("/Users/u/.config/antre")),
            user_home: Some(PathBuf::from("/Users/u")),
            home: PathBuf::from("/Users/u/.antre/sandbox/home/shared"),
            tmp: PathBuf::from("/Users/u/.antre/sandbox/os/s1/tmp"),
            workspace_mount: workspace,
            managed_files_dir: PathBuf::from("/Users/u/.antre/files"),
            managed_files_mount: ManagedFilesMount::Ro,
            network,
            system_read_roots: vec![PathBuf::from("/usr"), PathBuf::from("/bin")],
            extra_read: Vec::new(),
            extra_write: Vec::new(),
        })
        .unwrap()
    }

    fn param<'a>(profile: &'a SeatbeltProfile, path: &str) -> &'a str {
        profile
            .params
            .iter()
            .find(|(_, value)| value == Path::new(path))
            .map(|(key, _)| key.as_str())
            .unwrap_or_else(|| panic!("no param for {path}"))
    }

    #[test]
    fn profile_is_deny_by_default() {
        let profile = build_profile(&policy(OsNetworkAccess::Blocked, WorkspaceMount::Ro));
        assert!(profile.profile.starts_with("(version 1)"));
        assert!(profile.profile.contains("(deny default)"));
        assert!(!profile.profile.contains("(allow default)"));
        assert!(!profile.profile.contains("(allow file-read*)\n"));
        assert!(!profile.profile.contains("(allow network*"));
    }

    #[test]
    fn paths_are_passed_as_params_only() {
        let profile = build_profile(&policy(OsNetworkAccess::Blocked, WorkspaceMount::Ro));
        let without_regex: String = profile
            .profile
            .lines()
            .filter(|line| !line.contains("(regex #\"^(/Users"))
            .collect();
        assert!(!without_regex.contains("/Users/u"));
        let ro = param(&profile, "/Users/u/.antre");
        assert!(ro.starts_with("RO_"));
        let rw = param(&profile, "/Users/u/.antre/sandbox/home/shared");
        assert!(rw.starts_with("RW_"));
        assert!(profile.profile.contains(&format!(
            "(allow file-read* file-write* file-map-executable (subpath (param \"{rw}\"))"
        )));
    }

    #[test]
    fn secrets_are_denied_after_grants() {
        let profile = build_profile(&policy(OsNetworkAccess::Blocked, WorkspaceMount::Rw));
        let deny = param(&profile, "/Users/u/.ssh");
        let deny_pos = profile
            .profile
            .find(&format!("(param \"{deny}\")"))
            .unwrap();
        let grant_pos = profile.profile.find("; writable roots").unwrap();
        assert!(deny_pos > grant_pos);
        param(&profile, "/Users/u/.antre/credentials.json");
        param(&profile, "/Users/u/.config/antre");
        assert!(profile.profile.contains(
            "(regex #\"^(/Users/u/\\.antre)/(.*/)?[^/]*(\\.db|\\.db-wal|\\.db-shm|\\.db-journal|\\.sqlite|\\.sqlite3)$\")"
        ));
    }

    #[test]
    fn nested_read_only_files_are_redenied_for_write() {
        let profile = build_profile(&policy(OsNetworkAccess::Blocked, WorkspaceMount::Rw));
        let key = profile
            .params
            .iter()
            .find(|(key, value)| {
                key.starts_with("NESTED_RO") && value == Path::new("/Users/u/.antre/files")
            })
            .map(|(key, _)| key.clone())
            .unwrap();
        assert!(
            profile
                .profile
                .contains(&format!("(deny file-write* (subpath (param \"{key}\")))"))
        );
    }

    #[test]
    fn host_executed_entries_are_write_denied() {
        let profile = build_profile(&policy(OsNetworkAccess::Blocked, WorkspaceMount::Rw));
        let key = param(&profile, "/Users/u/.antre/hooks");
        assert!(key.starts_with("NO_WRITE_"));
        let deny = profile
            .profile
            .find(&format!("(deny file-write* (subpath (param \"{key}\"))"))
            .unwrap();
        assert!(deny > profile.profile.find("; writable roots").unwrap());
    }

    #[test]
    fn network_blocked_has_no_ip_rules() {
        let profile = build_profile(&policy(OsNetworkAccess::Blocked, WorkspaceMount::Ro));
        assert!(!profile.profile.contains("remote ip"));
        assert!(!profile.profile.contains("local ip"));
    }

    #[test]
    fn network_proxy_allows_only_proxy_port() {
        let profile = build_profile(&policy(
            OsNetworkAccess::Proxy { port: 18791 },
            WorkspaceMount::Ro,
        ));
        assert!(
            profile
                .profile
                .contains("(allow network-outbound (remote ip \"localhost:18791\"))")
        );
        assert!(!profile.profile.contains("\"*:*\""));
        assert!(!profile.profile.contains("mDNSResponder"));
    }

    #[test]
    fn network_open_allows_ip_but_not_host_unix_sockets() {
        let profile = build_profile(&policy(OsNetworkAccess::Open, WorkspaceMount::Ro));
        assert!(profile.profile.contains("(remote ip \"*:*\")"));
        assert!(!profile.profile.contains("(allow network-outbound)\n"));
        assert!(!profile.profile.contains("(remote unix-socket)"));
    }

    #[test]
    fn unix_sockets_limited_to_sandbox_dirs() {
        let profile = build_profile(&policy(OsNetworkAccess::Blocked, WorkspaceMount::Ro));
        let ipc: Vec<&(String, PathBuf)> = profile
            .params
            .iter()
            .filter(|(key, _)| key.starts_with("IPC_"))
            .collect();
        assert_eq!(ipc.len(), 2);
        assert!(
            profile
                .profile
                .contains("(allow network-bind (local unix-socket (subpath (param \"IPC_")
        );
    }

    #[test]
    fn sandbox_exec_args_end_with_command() {
        let profile = build_profile(&policy(OsNetworkAccess::Blocked, WorkspaceMount::Ro));
        let args = profile.sandbox_exec_args(&["/bin/sh".into(), "-c".into(), "echo hi".into()]);
        assert_eq!(args[0], "-p");
        let dashdash = args.iter().position(|arg| arg == "--").unwrap();
        assert_eq!(&args[dashdash + 1..], ["/bin/sh", "-c", "echo hi"]);
        assert!(args[2..dashdash].iter().all(|arg| arg.starts_with("-D")));
    }

    #[test]
    fn regex_escape_handles_metacharacters() {
        assert_eq!(regex_escape("/a.b(c)+d"), "/a\\.b\\(c\\)\\+d");
        assert_eq!(regex_escape("q\"x"), "q\\\"x");
    }
}
