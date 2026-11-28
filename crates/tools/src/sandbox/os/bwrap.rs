//! Linux bubblewrap argument generation.
//!
//! The sandbox starts from an empty root and only sees what is bound in:
//! system roots read-only, the workspace per its mount mode, its own home and
//! temp read-write. Paths keep their host location so host working
//! directories stay valid inside the sandbox.

use std::path::{Path, PathBuf};

use super::policy::{OsNetworkAccess, OsSandboxPolicy};

/// `PATH` given to sandboxed commands on Linux.
pub const LINUX_SANDBOX_PATH: &str = "/usr/local/bin:/usr/bin:/bin:/usr/local/sbin:/usr/sbin:/sbin";

/// Candidate read-only system roots on Linux. Missing entries are dropped at
/// runtime; symlinked ones (merged `/usr`) are recreated as symlinks.
#[must_use]
pub fn linux_system_read_roots() -> Vec<PathBuf> {
    [
        "/usr",
        "/bin",
        "/sbin",
        "/lib",
        "/lib32",
        "/lib64",
        "/libx32",
        "/etc",
        "/opt",
        "/nix/store",
        "/run/current-system",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect()
}

/// How a system root exists on the host, discovered at runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostEntry {
    Dir,
    File,
    Symlink(PathBuf),
    Missing,
}

/// Filesystem facts the generator needs, injected so generation stays pure.
pub trait HostFs {
    fn entry(&self, path: &Path) -> HostEntry;
}

/// Build `bwrap` arguments for `command`, run from `cwd`.
///
/// `extra_denied` lists existing files found by scanning
/// `policy.denied_suffix_roots` (databases); they are masked like
/// `policy.denied`.
#[must_use]
pub fn build_args(
    policy: &OsSandboxPolicy,
    host: &dyn HostFs,
    extra_denied: &[PathBuf],
    cwd: &Path,
    command: &[String],
) -> Vec<String> {
    let mut args: Vec<String> = [
        "--new-session",
        "--die-with-parent",
        "--unshare-user",
        "--unshare-pid",
        "--unshare-ipc",
        "--unshare-uts",
        "--unshare-cgroup-try",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    // The egress proxy listens on the host loopback, which a new network
    // namespace cannot reach; `Proxy` is downgraded to `Blocked` by the
    // backend before it gets here, so anything but `Open` unshares.
    if policy.network != OsNetworkAccess::Open {
        args.push("--unshare-net".into());
    }
    args.extend(["--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp"].map(String::from));

    let mut push = |flag: &str, paths: &[&Path]| {
        args.push(flag.to_string());
        args.extend(paths.iter().map(|path| path.display().to_string()));
    };

    for root in &policy.read_only {
        match host.entry(root) {
            HostEntry::Symlink(target) => push("--symlink", &[&target, root]),
            HostEntry::Dir | HostEntry::File => push("--ro-bind", &[root, root]),
            HostEntry::Missing => {},
        }
    }
    for root in &policy.read_write {
        match host.entry(root) {
            HostEntry::Dir | HostEntry::File => push("--bind", &[root, root]),
            // Writable roots are created by the backend before launch; a
            // symlink or missing entry here is refused rather than followed.
            HostEntry::Symlink(_) | HostEntry::Missing => {},
        }
    }
    let writable = |path: &Path| policy.read_write.iter().any(|root| path.starts_with(root));
    for nested in policy
        .read_only_inside_writable()
        .into_iter()
        .chain(policy.write_denied.iter().filter(|path| writable(path)))
    {
        if matches!(host.entry(nested), HostEntry::Dir | HostEntry::File) {
            push("--ro-bind", &[nested, nested]);
        }
    }
    for denied in policy.denied.iter().chain(extra_denied.iter()) {
        if !is_visible(policy, denied) {
            continue;
        }
        match host.entry(denied) {
            HostEntry::File => push("--ro-bind", &[Path::new("/dev/null"), denied]),
            HostEntry::Dir => {
                push("--tmpfs", &[denied]);
                push("--remount-ro", &[denied]);
            },
            // Symlinks inside a bound root resolve within the sandbox view;
            // their targets are only visible if bound, so nothing to mask.
            HostEntry::Symlink(_) | HostEntry::Missing => {},
        }
    }

    push("--chdir", &[cwd]);
    args.push("--".into());
    args.extend(command.iter().cloned());
    args
}

/// Only paths under a bound root need masking; anything else is already
/// absent from the sandbox's empty root.
fn is_visible(policy: &OsSandboxPolicy, path: &Path) -> bool {
    policy
        .read_only
        .iter()
        .chain(policy.read_write.iter())
        .any(|root| path.starts_with(root))
}

/// Find files matching the policy's denied suffixes, bounded in depth so a
/// large workspace cannot stall every command.
#[must_use]
pub fn scan_denied_suffixes(policy: &OsSandboxPolicy, max_depth: usize) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for root in &policy.denied_suffix_roots {
        if !is_visible(policy, root) {
            continue;
        }
        let mut stack = vec![(root.clone(), 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if file_type.is_dir() {
                    if depth < max_depth && path != policy.home && path != policy.tmp {
                        stack.push((path, depth + 1));
                    }
                } else if file_type.is_file()
                    && path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| {
                            policy
                                .denied_suffixes
                                .iter()
                                .any(|suffix| name.ends_with(suffix.as_str()))
                        })
                {
                    found.push(path);
                }
            }
        }
    }
    found.sort();
    found
}

/// [`HostFs`] backed by the real filesystem.
pub struct RealHostFs;

impl HostFs for RealHostFs {
    fn entry(&self, path: &Path) -> HostEntry {
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_symlink() => std::fs::read_link(path)
                .map(HostEntry::Symlink)
                .unwrap_or(HostEntry::Missing),
            Ok(meta) if meta.is_dir() => HostEntry::Dir,
            Ok(_) => HostEntry::File,
            Err(_) => HostEntry::Missing,
        }
    }
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
        std::collections::HashMap,
    };

    struct FakeFs(HashMap<PathBuf, HostEntry>);

    impl HostFs for FakeFs {
        fn entry(&self, path: &Path) -> HostEntry {
            self.0.get(path).cloned().unwrap_or(HostEntry::Missing)
        }
    }

    fn fake_fs() -> FakeFs {
        let mut map = HashMap::new();
        for dir in [
            "/usr",
            "/etc",
            "/home/u/.antre",
            "/home/u/.antre/files",
            "/home/u/.antre/sandbox/home/shared",
            "/home/u/.antre/sandbox/os/s1/tmp",
            "/home/u/.antre/logs",
            "/home/u/.antre/hooks",
        ] {
            map.insert(PathBuf::from(dir), HostEntry::Dir);
        }
        map.insert(PathBuf::from("/bin"), HostEntry::Symlink("usr/bin".into()));
        for file in ["/home/u/.antre/credentials.json", "/home/u/.antre/antre.db"] {
            map.insert(PathBuf::from(file), HostEntry::File);
        }
        FakeFs(map)
    }

    fn policy(network: OsNetworkAccess, workspace: WorkspaceMount) -> OsSandboxPolicy {
        OsSandboxPolicy::build(&OsPolicyInputs {
            data_dir: PathBuf::from("/home/u/.antre"),
            config_dir: Some(PathBuf::from("/home/u/.config/antre")),
            user_home: Some(PathBuf::from("/home/u")),
            home: PathBuf::from("/home/u/.antre/sandbox/home/shared"),
            tmp: PathBuf::from("/home/u/.antre/sandbox/os/s1/tmp"),
            workspace_mount: workspace,
            managed_files_dir: PathBuf::from("/home/u/.antre/files"),
            managed_files_mount: ManagedFilesMount::Ro,
            network,
            system_read_roots: vec![
                PathBuf::from("/usr"),
                PathBuf::from("/bin"),
                PathBuf::from("/etc"),
                PathBuf::from("/lib64"),
            ],
            extra_read: Vec::new(),
            extra_write: Vec::new(),
        })
        .unwrap()
    }

    fn args(policy: &OsSandboxPolicy) -> Vec<String> {
        build_args(
            policy,
            &fake_fs(),
            &[PathBuf::from("/home/u/.antre/antre.db")],
            Path::new("/home/u/.antre/sandbox/home/shared"),
            &["/bin/sh".into(), "-c".into(), "true".into()],
        )
    }

    fn has_seq(args: &[String], seq: &[&str]) -> bool {
        args.windows(seq.len())
            .any(|window| window.iter().zip(seq).all(|(a, b)| a == b))
    }

    #[test]
    fn isolates_namespaces_and_blocks_network() {
        let args = args(&policy(OsNetworkAccess::Blocked, WorkspaceMount::Ro));
        for flag in [
            "--unshare-user",
            "--unshare-pid",
            "--unshare-net",
            "--die-with-parent",
        ] {
            assert!(args.iter().any(|arg| arg == flag), "missing {flag}");
        }
        assert!(has_seq(&args, &["--tmpfs", "/tmp"]));
    }

    #[test]
    fn proxy_mode_still_unshares_network() {
        let args = args(&policy(
            OsNetworkAccess::Proxy { port: 18791 },
            WorkspaceMount::Ro,
        ));
        assert!(args.iter().any(|arg| arg == "--unshare-net"));
    }

    #[test]
    fn open_network_shares_network() {
        let args = args(&policy(OsNetworkAccess::Open, WorkspaceMount::Ro));
        assert!(!args.iter().any(|arg| arg == "--unshare-net"));
    }

    #[test]
    fn binds_system_ro_and_sandbox_dirs_rw() {
        let args = args(&policy(OsNetworkAccess::Blocked, WorkspaceMount::Ro));
        assert!(has_seq(&args, &["--ro-bind", "/usr", "/usr"]));
        assert!(has_seq(&args, &["--symlink", "usr/bin", "/bin"]));
        assert!(has_seq(&args, &[
            "--ro-bind",
            "/home/u/.antre",
            "/home/u/.antre"
        ]));
        assert!(has_seq(&args, &[
            "--bind",
            "/home/u/.antre/sandbox/home/shared",
            "/home/u/.antre/sandbox/home/shared"
        ]));
        assert!(!args.iter().any(|arg| arg == "/lib64"));
        assert!(!has_seq(&args, &[
            "--bind",
            "/home/u/.antre",
            "/home/u/.antre"
        ]));
    }

    #[test]
    fn masks_secrets_after_binds() {
        let args = args(&policy(OsNetworkAccess::Blocked, WorkspaceMount::Rw));
        let bind = args
            .windows(2)
            .position(|w| w[0] == "--bind" && w[1] == "/home/u/.antre")
            .unwrap();
        let mask = args
            .windows(3)
            .position(|w| {
                w[0] == "--ro-bind"
                    && w[1] == "/dev/null"
                    && w[2] == "/home/u/.antre/credentials.json"
            })
            .unwrap();
        assert!(mask > bind);
        assert!(has_seq(&args, &[
            "--ro-bind",
            "/dev/null",
            "/home/u/.antre/antre.db"
        ]));
        assert!(has_seq(&args, &["--tmpfs", "/home/u/.antre/logs"]));
        assert!(has_seq(&args, &["--remount-ro", "/home/u/.antre/logs"]));
        // Managed Files stays read-only inside the writable workspace.
        assert!(has_seq(&args, &[
            "--ro-bind",
            "/home/u/.antre/files",
            "/home/u/.antre/files"
        ]));
        // Hooks run on the host: read-only even in a writable workspace.
        assert!(has_seq(&args, &[
            "--ro-bind",
            "/home/u/.antre/hooks",
            "/home/u/.antre/hooks"
        ]));
        // Paths outside every bound root are never mentioned.
        assert!(!args.iter().any(|arg| arg.contains(".ssh")));
    }

    #[test]
    fn command_follows_double_dash() {
        let args = args(&policy(OsNetworkAccess::Blocked, WorkspaceMount::Ro));
        let dashdash = args.iter().position(|arg| arg == "--").unwrap();
        assert_eq!(&args[dashdash + 1..], ["/bin/sh", "-c", "true"]);
        assert!(has_seq(&args, &[
            "--chdir",
            "/home/u/.antre/sandbox/home/shared"
        ]));
    }

    #[test]
    fn scan_finds_databases_but_skips_sandbox_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let home = data.join("sandbox/home/shared");
        let tmp = data.join("sandbox/os/s1/tmp");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::create_dir_all(data.join("agents/main")).unwrap();
        std::fs::write(data.join("antre.db"), b"").unwrap();
        std::fs::write(data.join("agents/main/memory.db-wal"), b"").unwrap();
        std::fs::write(data.join("notes.md"), b"").unwrap();
        std::fs::write(home.join("own.db"), b"").unwrap();
        let policy = OsSandboxPolicy::build(&OsPolicyInputs {
            data_dir: data.clone(),
            config_dir: None,
            user_home: None,
            home: home.clone(),
            tmp,
            workspace_mount: WorkspaceMount::Ro,
            managed_files_dir: data.join("files"),
            managed_files_mount: ManagedFilesMount::Ro,
            network: OsNetworkAccess::Blocked,
            system_read_roots: Vec::new(),
            extra_read: Vec::new(),
            extra_write: Vec::new(),
        })
        .unwrap();
        let found = scan_denied_suffixes(&policy, 4);
        assert_eq!(found, vec![
            data.join("agents/main/memory.db-wal"),
            data.join("antre.db")
        ]);
    }

    #[test]
    fn real_host_fs_reports_entries() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        std::fs::write(&file, b"").unwrap();
        assert_eq!(RealHostFs.entry(dir.path()), HostEntry::Dir);
        assert_eq!(RealHostFs.entry(&file), HostEntry::File);
        assert_eq!(
            RealHostFs.entry(&dir.path().join("none")),
            HostEntry::Missing
        );
        #[cfg(unix)]
        {
            let link = dir.path().join("l");
            std::os::unix::fs::symlink("f", &link).unwrap();
            assert_eq!(RealHostFs.entry(&link), HostEntry::Symlink("f".into()));
        }
    }
}
