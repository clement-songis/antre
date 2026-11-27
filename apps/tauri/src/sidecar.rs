//! The `moltis` daemon as a supervised child process of the shell.
//!
//! The child listens on a free loopback port without TLS (the system WebView
//! would reject the daemon's self-signed certificate, and loopback traffic
//! never leaves the machine). It is restarted with backoff when it crashes
//! and killed when the shell exits.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::Duration,
};

use {
    anyhow::Context,
    tokio::{process::Command, sync::watch},
    url::Url,
};

use crate::probe;

// Generous: a cold debug daemon migrates databases and probes runtimes.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_RESTARTS: u32 = 5;

/// Directories for an isolated daemon (not the user's `~/.moltis`).
#[derive(Debug, Clone)]
pub struct IsolatedDirs {
    pub config: PathBuf,
    pub data: PathBuf,
}

/// A running, supervised daemon. Dropping it stops the daemon.
pub struct Sidecar {
    pub base: Url,
    stop: watch::Sender<bool>,
    supervisor: Option<tokio::task::JoinHandle<()>>,
}

impl Sidecar {
    /// Stop the daemon and wait until the child process is gone.
    pub async fn shutdown(mut self) {
        let _ = self.stop.send(true);
        if let Some(supervisor) = self.supervisor.take() {
            let _ = supervisor.await;
        }
    }
}

impl Drop for Sidecar {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
    }
}

/// Find an executable `moltis`: `explicit` first, then next to the shell
/// (a packaged app ships it there), then `MOLTIS_BIN`, then `PATH`.
#[must_use]
pub fn find_moltis(
    explicit: Option<&Path>,
    beside_exe: Option<&Path>,
    moltis_bin: Option<OsString>,
    path: Option<OsString>,
) -> Option<PathBuf> {
    if let Some(explicit) = explicit {
        return is_executable(explicit).then(|| explicit.to_path_buf());
    }
    let name = if cfg!(windows) {
        "moltis.exe"
    } else {
        "moltis"
    };
    beside_exe
        .map(|dir| dir.join(name))
        .into_iter()
        .chain(moltis_bin.map(PathBuf::from))
        .chain(
            path.iter()
                .flat_map(std::env::split_paths)
                .map(|dir| dir.join(name)),
        )
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        metadata.is_file()
    }
}

/// A loopback port nothing listens on right now. The daemon binds it a
/// moment later; a race with another process is possible but unlikely.
pub fn free_port() -> std::io::Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

/// Arguments for `moltis` (global flags, then the `gateway` subcommand).
#[must_use]
pub fn gateway_args(port: u16, dirs: Option<&IsolatedDirs>) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![
        "--bind".into(),
        "127.0.0.1".into(),
        "--port".into(),
        port.to_string().into(),
        "--no-tls".into(),
        "--no-open-browser".into(),
    ];
    if let Some(dirs) = dirs {
        args.extend([
            "--config-dir".into(),
            dirs.config.clone().into_os_string(),
            "--data-dir".into(),
            dirs.data.clone().into_os_string(),
        ]);
    }
    args.push("gateway".into());
    args
}

/// What the supervisor reports after the initial start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidecarEvent {
    /// The daemon crashed and is healthy again.
    Restarted,
    /// The daemon crashed too often; it stays down.
    GaveUp,
}

/// Start the daemon and wait until it answers `/health`. `on_event` reports
/// crash recoveries and the supervisor giving up.
pub async fn start(
    binary: PathBuf,
    dirs: Option<IsolatedDirs>,
    on_event: impl Fn(SidecarEvent) + Send + 'static,
) -> anyhow::Result<Sidecar> {
    if let Some(dirs) = &dirs {
        std::fs::create_dir_all(&dirs.config)?;
        std::fs::create_dir_all(&dirs.data)?;
    }
    let port = free_port()?;
    let base = Url::parse(&format!("http://127.0.0.1:{port}/"))?;
    let args = gateway_args(port, dirs.as_ref());
    let spawn = move || {
        Command::new(&binary)
            .args(&args)
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("starting {}", binary.display()))
    };

    let mut child = spawn()?;
    let health = wait_ready(&mut child, &base).await?;
    tracing::info!(%base, version = health.version, protocol = health.protocol, "sidecar daemon ready");

    let (stop, mut stopped) = watch::channel(false);
    let supervised_base = base.clone();
    let supervisor = tokio::spawn(async move {
        let mut restarts = 0;
        loop {
            tokio::select! {
                _ = stopped.changed() => {
                    let _ = child.kill().await;
                    return;
                },
                status = child.wait() => {
                    tracing::warn!(?status, restarts, "sidecar daemon exited");
                    if restarts >= MAX_RESTARTS {
                        tracing::error!("sidecar daemon keeps crashing; giving up");
                        on_event(SidecarEvent::GaveUp);
                        return;
                    }
                    restarts += 1;
                    tokio::time::sleep(Duration::from_millis(250 << restarts)).await;
                    child = match spawn() {
                        Ok(child) => child,
                        Err(error) => {
                            tracing::error!(%error, "could not restart the sidecar daemon");
                            on_event(SidecarEvent::GaveUp);
                            return;
                        },
                    };
                    match wait_ready(&mut child, &supervised_base).await {
                        Ok(_) => on_event(SidecarEvent::Restarted),
                        Err(error) => tracing::warn!(%error, "restarted sidecar daemon is not healthy"),
                    }
                },
            }
        }
    });
    Ok(Sidecar {
        base,
        stop,
        supervisor: Some(supervisor),
    })
}

/// Wait for `/health`, failing fast when the child exits first.
async fn wait_ready(
    child: &mut tokio::process::Child,
    base: &Url,
) -> anyhow::Result<probe::Health> {
    tokio::select! {
        status = child.wait() => anyhow::bail!("moltis exited during startup ({})", status?),
        health = probe::wait_until_healthy(base, STARTUP_TIMEOUT) => health,
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn executable(dir: &Path, name: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn prefers_explicit_then_beside_exe_then_env_then_path() {
        let beside = tempfile::tempdir().unwrap();
        let on_path = tempfile::tempdir().unwrap();
        let path_binary = executable(on_path.path(), "moltis");
        let path_env = Some(on_path.path().as_os_str().to_owned());

        assert_eq!(
            find_moltis(None, Some(beside.path()), None, path_env.clone()),
            Some(path_binary.clone())
        );

        let env_binary = executable(beside.path(), "custom-moltis");
        assert_eq!(
            find_moltis(
                None,
                None,
                Some(env_binary.clone().into_os_string()),
                path_env.clone()
            ),
            Some(env_binary)
        );

        let beside_binary = executable(beside.path(), "moltis");
        assert_eq!(
            find_moltis(None, Some(beside.path()), None, path_env.clone()),
            Some(beside_binary.clone())
        );
        assert_eq!(
            find_moltis(Some(&path_binary), Some(beside.path()), None, path_env),
            Some(path_binary)
        );
    }

    #[cfg(unix)]
    #[test]
    fn ignores_non_executable_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("moltis"), "").unwrap();
        assert_eq!(
            find_moltis(None, Some(dir.path()), None, None),
            None,
            "a non-executable file must not be picked"
        );
        assert_eq!(
            find_moltis(Some(&dir.path().join("moltis")), None, None, None),
            None
        );
    }

    #[test]
    fn gateway_args_bind_loopback_without_tls() {
        let args = gateway_args(4242, None);
        let args: Vec<_> = args.iter().map(|arg| arg.to_string_lossy()).collect();
        assert_eq!(args, [
            "--bind",
            "127.0.0.1",
            "--port",
            "4242",
            "--no-tls",
            "--no-open-browser",
            "gateway"
        ]);
    }

    #[test]
    fn gateway_args_pass_isolated_dirs_before_the_subcommand() {
        let dirs = IsolatedDirs {
            config: "/c".into(),
            data: "/d".into(),
        };
        let args = gateway_args(1, Some(&dirs));
        let args: Vec<_> = args.iter().map(|arg| arg.to_string_lossy()).collect();
        assert_eq!(&args[6..], [
            "--config-dir",
            "/c",
            "--data-dir",
            "/d",
            "gateway"
        ]);
    }

    #[test]
    fn free_port_is_bindable() {
        let port = free_port().unwrap();
        assert!(std::net::TcpListener::bind(("127.0.0.1", port)).is_ok());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn start_fails_fast_when_the_binary_exits() {
        let dir = tempfile::tempdir().unwrap();
        let binary = executable(dir.path(), "moltis");
        let result =
            tokio::time::timeout(Duration::from_secs(10), start(binary, None, |_| {})).await;
        let error = result.unwrap().err().unwrap();
        assert!(
            error.to_string().contains("exited during startup"),
            "{error}"
        );
    }
}
