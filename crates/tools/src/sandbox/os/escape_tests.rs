//! Real escape attempts against the OS sandbox of the host platform.
//!
//! On macOS Seatbelt must be usable (it ships with the OS); on Linux the tests
//! skip when bubblewrap is missing or user namespaces are disabled.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    net::TcpListener,
    path::{Path, PathBuf},
    time::Duration,
};

use {
    super::{OsSandbox, OsSandboxDirs, OsSandboxKind},
    crate::{
        exec::{ExecOpts, ExecResult},
        sandbox::types::{
            NetworkPolicy, Sandbox, SandboxConfig, SandboxId, SandboxScope, WorkspaceMount,
        },
    },
};

struct Fixture {
    _root: tempfile::TempDir,
    data: PathBuf,
    user_home: PathBuf,
    outside: PathBuf,
}

fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(root.path()).unwrap();
    let data = base.join("data");
    let user_home = base.join("userhome");
    let outside = base.join("outside");
    for dir in [&data, &user_home.join(".ssh"), &outside] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(data.join("notes.md"), "workspace note").unwrap();
    std::fs::write(data.join("credentials.json"), "{\"secret\":1}").unwrap();
    std::fs::write(data.join("antre.db"), "SQLITE-SECRET").unwrap();
    std::fs::write(user_home.join(".ssh/id_ed25519"), "PRIVATE-KEY").unwrap();
    std::fs::write(user_home.join("diary.txt"), "personal").unwrap();
    std::fs::write(outside.join("existing.txt"), "untouched").unwrap();
    Fixture {
        _root: root,
        data,
        user_home,
        outside,
    }
}

fn detect() -> Option<OsSandboxKind> {
    let kind = OsSandboxKind::detect();
    #[cfg(target_os = "macos")]
    assert!(kind.is_some(), "Seatbelt must be available on macOS");
    if kind.is_none() {
        eprintln!("skipping OS sandbox escape test: no usable OS sandbox on this host");
    }
    kind
}

fn sandbox(fx: &Fixture, kind: OsSandboxKind, config: SandboxConfig) -> OsSandbox {
    OsSandbox::with_dirs(config, kind, OsSandboxDirs {
        data_dir: fx.data.clone(),
        config_dir: Some(fx.user_home.join(".config/antre")),
        user_home: Some(fx.user_home.clone()),
    })
}

fn id() -> SandboxId {
    SandboxId {
        scope: SandboxScope::Session,
        key: "escape-test".into(),
    }
}

async fn run(sandbox: &OsSandbox, command: &str) -> ExecResult {
    let opts = ExecOpts {
        timeout: Duration::from_secs(30),
        ..ExecOpts::default()
    };
    sandbox.ensure_ready(&id(), None).await.unwrap();
    sandbox.exec(&id(), command, &opts).await.unwrap()
}

fn q(path: &Path) -> String {
    crate::sandbox::file_system::shell_single_quote(&path.display().to_string())
}

#[tokio::test]
async fn sandboxed_command_runs_in_its_home() {
    let Some(kind) = detect() else {
        return;
    };
    let fx = fixture();
    let sb = sandbox(&fx, kind, SandboxConfig::default());
    let result = run(&sb, "echo ok > note.txt && cat note.txt && pwd").await;
    assert_eq!(result.exit_code, 0, "stderr: {}", result.stderr);
    assert!(result.stdout.starts_with("ok\n"));
    let home = sb.workspace_dir_for(&id()).await;
    assert_eq!(result.stdout.lines().nth(1), Some(home.as_str()));
}

#[tokio::test]
async fn cannot_write_outside_allowed_roots() {
    let Some(kind) = detect() else {
        return;
    };
    let fx = fixture();
    let sb = sandbox(&fx, kind, SandboxConfig::default());
    let target = fx.outside.join("pwned.txt");
    let existing = fx.outside.join("existing.txt");
    let create = run(&sb, &format!("echo x > {}", q(&target))).await;
    let append = run(&sb, &format!("echo x >> {}", q(&existing))).await;
    // `rm -f` exits 0 when the path is invisible (bubblewrap), so only the
    // file's survival is checked.
    run(&sb, &format!("rm -f {}", q(&existing))).await;
    assert_ne!(create.exit_code, 0);
    assert_ne!(append.exit_code, 0);
    assert!(!target.exists(), "write outside the sandbox succeeded");
    assert_eq!(std::fs::read_to_string(&existing).unwrap(), "untouched");
}

#[tokio::test]
async fn read_only_workspace_is_not_writable() {
    let Some(kind) = detect() else {
        return;
    };
    let fx = fixture();
    let sb = sandbox(&fx, kind, SandboxConfig {
        workspace_mount: WorkspaceMount::Ro,
        ..SandboxConfig::default()
    });
    let result = run(
        &sb,
        &format!(
            "cat {}; echo tamper > {}",
            q(&fx.data.join("notes.md")),
            q(&fx.data.join("notes.md"))
        ),
    )
    .await;
    assert!(
        result.stdout.contains("workspace note"),
        "stderr: {}",
        result.stderr
    );
    assert_eq!(
        std::fs::read_to_string(fx.data.join("notes.md")).unwrap(),
        "workspace note"
    );
}

#[tokio::test]
async fn writable_workspace_still_protects_secrets() {
    let Some(kind) = detect() else {
        return;
    };
    let fx = fixture();
    let sb = sandbox(&fx, kind, SandboxConfig {
        workspace_mount: WorkspaceMount::Rw,
        ..SandboxConfig::default()
    });
    let result = run(
        &sb,
        &format!(
            "echo new > {notes}; cat {creds}; cat {db}; echo x > {creds}",
            notes = q(&fx.data.join("notes.md")),
            creds = q(&fx.data.join("credentials.json")),
            db = q(&fx.data.join("antre.db")),
        ),
    )
    .await;
    assert_eq!(
        std::fs::read_to_string(fx.data.join("notes.md")).unwrap(),
        "new\n"
    );
    assert!(!result.stdout.contains("secret"), "credentials leaked");
    assert!(!result.stdout.contains("SQLITE-SECRET"), "database leaked");
    assert_eq!(
        std::fs::read_to_string(fx.data.join("credentials.json")).unwrap(),
        "{\"secret\":1}"
    );
}

#[tokio::test]
async fn writable_workspace_cannot_plant_host_hooks() {
    let Some(kind) = detect() else {
        return;
    };
    let fx = fixture();
    std::fs::create_dir_all(fx.data.join("hooks")).unwrap();
    let sb = sandbox(&fx, kind, SandboxConfig {
        workspace_mount: WorkspaceMount::Rw,
        ..SandboxConfig::default()
    });
    let hook = fx.data.join("hooks/evil.sh");
    let result = run(&sb, &format!("echo 'touch /tmp/pwned' > {}", q(&hook))).await;
    assert_ne!(result.exit_code, 0);
    assert!(!hook.exists(), "sandbox planted a host hook");
}

#[tokio::test]
async fn cannot_read_user_home_or_ssh_keys() {
    let Some(kind) = detect() else {
        return;
    };
    let fx = fixture();
    let sb = sandbox(&fx, kind, SandboxConfig::default());
    let result = run(
        &sb,
        &format!(
            "cat {key}; cat {diary}; ls {home}",
            key = q(&fx.user_home.join(".ssh/id_ed25519")),
            diary = q(&fx.user_home.join("diary.txt")),
            home = q(&fx.user_home),
        ),
    )
    .await;
    assert!(!result.stdout.contains("PRIVATE-KEY"));
    assert!(!result.stdout.contains("personal"));
    assert!(!result.stdout.contains("diary.txt"));

    // The real user's home is never in the policy either.
    if let Some(real_home) = antre_config::home_dir() {
        let result = run(&sb, &format!("ls -a {}", q(&real_home.join(".ssh")))).await;
        assert!(
            result.stdout.trim().is_empty(),
            "listed real ~/.ssh: {}",
            result.stdout
        );
    }
}

#[tokio::test]
async fn links_do_not_reach_hidden_files() {
    let Some(kind) = detect() else {
        return;
    };
    let fx = fixture();
    let sb = sandbox(&fx, kind, SandboxConfig::default());
    let key = fx.user_home.join(".ssh/id_ed25519");
    let result = run(
        &sb,
        &format!(
            "ln {key} hard 2>/dev/null; cat hard; ln -s {key} soft; cat soft",
            key = q(&key)
        ),
    )
    .await;
    assert!(
        !result.stdout.contains("PRIVATE-KEY"),
        "link escape: {}",
        result.stdout
    );
}

#[tokio::test]
async fn host_environment_is_not_inherited() {
    let Some(kind) = detect() else {
        return;
    };
    let fx = fixture();
    let sb = sandbox(&fx, kind, SandboxConfig::default());
    let result = run(&sb, "env").await;
    assert_eq!(result.exit_code, 0, "stderr: {}", result.stderr);
    // `cargo test` exports CARGO_* to the test process.
    assert!(!result.stdout.contains("CARGO_"));
    assert!(result.stdout.contains("ANTRE_SANDBOX=os"));
}

/// Prints CONNECTED when a TCP connection to `port` succeeds.
fn connect_command(port: u16) -> String {
    format!(
        "if command -v curl >/dev/null; then \
           curl --noproxy '*' -s -o /dev/null --max-time 5 http://127.0.0.1:{port}/ && echo CONNECTED; \
         else python3 -c 'import socket; socket.create_connection((\"127.0.0.1\", {port}), 5); print(\"CONNECTED\")'; fi"
    )
}

/// Minimal HTTP server answering 200 to every connection, for the lifetime
/// of the test process.
fn http_listener() -> (std::thread::JoinHandle<()>, u16) {
    use std::io::{Read, Write};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(b"HTTP/1.0 200 OK\r\nContent-Length: 0\r\n\r\n");
        }
    });
    (handle, port)
}

#[tokio::test]
async fn trusted_network_reaches_only_the_proxy_port() {
    let Some(kind) = detect() else {
        return;
    };
    let fx = fixture();
    let (_proxy, proxy_port) = http_listener();
    let (_other, other_port) = http_listener();
    let sb = sandbox(&fx, kind.clone(), SandboxConfig {
        network: NetworkPolicy::Trusted,
        ..SandboxConfig::default()
    })
    .with_proxy_port(proxy_port);

    let direct = run(&sb, &connect_command(other_port)).await;
    assert!(
        !direct.stdout.contains("CONNECTED"),
        "direct connection allowed"
    );

    let via_proxy = run(&sb, &connect_command(proxy_port)).await;
    match kind {
        // bubblewrap fails closed: the host loopback proxy is unreachable.
        OsSandboxKind::Bubblewrap { .. } => assert!(!via_proxy.stdout.contains("CONNECTED")),
        OsSandboxKind::Seatbelt => assert!(
            via_proxy.stdout.contains("CONNECTED"),
            "proxy port blocked: {} {}",
            via_proxy.stdout,
            via_proxy.stderr
        ),
    }
}

#[tokio::test]
async fn blocked_network_reaches_nothing() {
    let Some(kind) = detect() else {
        return;
    };
    let fx = fixture();
    let (_proxy, proxy_port) = http_listener();
    let sb = sandbox(&fx, kind, SandboxConfig {
        network: NetworkPolicy::Blocked,
        ..SandboxConfig::default()
    })
    .with_proxy_port(proxy_port);
    let result = run(&sb, &connect_command(proxy_port)).await;
    assert!(!result.stdout.contains("CONNECTED"));
}

#[tokio::test]
async fn cleanup_removes_session_temp() {
    let Some(kind) = detect() else {
        return;
    };
    let fx = fixture();
    let sb = sandbox(&fx, kind, SandboxConfig::default());
    let result = run(&sb, "echo t > \"$TMPDIR/t\" && cat \"$TMPDIR/t\"").await;
    assert_eq!(result.stdout, "t\n", "stderr: {}", result.stderr);
    sb.cleanup(&id()).await.unwrap();
    assert!(!fx.data.join("sandbox/os/escape-test").exists());
}

/// Usability guard: the policy must not break everyday developer tools that
/// exist on the host.
#[tokio::test]
async fn common_tools_still_work() {
    let Some(kind) = detect() else {
        return;
    };
    let fx = fixture();
    let sb = sandbox(&fx, kind, SandboxConfig::default());
    for (probe, command) in [
        (
            "git",
            "git init -q repo && cd repo && git -c user.email=a@b -c user.name=t commit -q --allow-empty -m init && git log --oneline | wc -l",
        ),
        (
            "python3",
            "python3 -c 'import json, tempfile; print(json.dumps(1))'",
        ),
        (
            "tar",
            "mkdir -p d && echo x > d/f && tar -cf a.tar d && tar -tf a.tar | head -1",
        ),
    ] {
        let host_has_tool = std::process::Command::new("sh")
            .args(["-c", &format!("command -v {probe}")])
            .output()
            .is_ok_and(|output| output.status.success());
        if !host_has_tool {
            continue;
        }
        let result = run(&sb, command).await;
        assert_eq!(
            result.exit_code, 0,
            "{probe} failed under the sandbox: {} {}",
            result.stdout, result.stderr
        );
    }
}
