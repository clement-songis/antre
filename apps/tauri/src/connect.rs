//! Resolve the daemon the window shows: attach, spawn, embed or go remote.

use std::path::PathBuf;

use {anyhow::Context, url::Url};

use crate::{
    cli::{Mode, SidecarOptions},
    probe,
    sidecar::{self, IsolatedDirs, Sidecar, SidecarEvent},
};

/// The daemon the window is pointed at, and what keeps it alive.
pub struct Connection {
    pub url: Url,
    pub kind: &'static str,
    pub sidecar: Option<Sidecar>,
}

/// `app_data` is the shell's own data directory (isolated sidecars live
/// there). `on_event` hears about sidecar crash recoveries.
pub async fn connect(
    mode: Mode,
    app_data: PathBuf,
    on_event: impl Fn(SidecarEvent) + Send + 'static,
) -> anyhow::Result<Connection> {
    match mode {
        // The default is a sidecar on the user's own config. A daemon
        // already running on that config is reused: two daemons cannot share
        // one database.
        Mode::Auto => {
            if let Some(url) = running_local_daemon().await {
                return Ok(Connection {
                    url,
                    kind: "local daemon",
                    sidecar: None,
                });
            }
            spawn_sidecar(SidecarOptions::default(), &app_data, on_event).await
        },
        Mode::Sidecar(options) => spawn_sidecar(options, &app_data, on_event).await,
        Mode::Remote { url, api_key } => {
            let health = probe::health(&url)
                .await
                .with_context(|| format!("{url} does not answer /health"))?;
            tracing::info!(%url, version = health.version, "remote daemon reachable");
            // With a key, open the UI already signed in; without one (or with
            // a non-admin key) the user logs in once in the WebView.
            let target = match &api_key {
                None => url,
                Some(api_key) => match probe::session_ticket(&url, api_key).await? {
                    probe::SessionTicket::Redeem(redeem) => redeem,
                    probe::SessionTicket::NotAdmin => {
                        tracing::warn!(
                            "the API key lacks operator.admin; log in in the window instead"
                        );
                        url
                    },
                    probe::SessionTicket::Rejected => {
                        anyhow::bail!("{url} rejected the API key")
                    },
                },
            };
            Ok(Connection {
                url: target,
                kind: "remote daemon",
                sidecar: None,
            })
        },
        Mode::Embedded { port } => embedded(port, &app_data).await,
        Mode::Bench { .. } => anyhow::bail!("bench mode has no daemon"),
    }
}

/// The daemon configured in `~/.moltis/moltis.toml`, when it answers over
/// plain HTTP. A TLS daemon is skipped: its self-signed certificate is not
/// trusted by the system WebView unless the local CA was installed.
async fn running_local_daemon() -> Option<Url> {
    let local = moltis_client::local::local_daemon()?;
    if local.endpoint.is_tls() {
        tracing::info!("local daemon uses TLS; starting a sidecar instead");
        return None;
    }
    let url = local.endpoint.base_url().clone();
    probe::health(&url).await.ok().map(|_| url)
}

async fn spawn_sidecar(
    options: SidecarOptions,
    app_data: &std::path::Path,
    on_event: impl Fn(SidecarEvent) + Send + 'static,
) -> anyhow::Result<Connection> {
    let beside_exe = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf));
    let binary = sidecar::find_moltis(
        options.binary.as_deref(),
        beside_exe.as_deref(),
        std::env::var_os("MOLTIS_BIN"),
        std::env::var_os("PATH"),
    )
    .context("no `moltis` binary found (pass --moltis PATH or set MOLTIS_BIN)")?;
    let dirs = options.isolated.then(|| isolated_dirs(app_data));
    let sidecar = sidecar::start(binary, dirs, on_event).await?;
    Ok(Connection {
        url: sidecar.base.clone(),
        kind: "sidecar daemon",
        sidecar: Some(sidecar),
    })
}

fn isolated_dirs(app_data: &std::path::Path) -> IsolatedDirs {
    IsolatedDirs {
        config: app_data.join("daemon").join("config"),
        data: app_data.join("daemon").join("data"),
    }
}

#[cfg(feature = "embedded")]
async fn embedded(port: Option<u16>, app_data: &std::path::Path) -> anyhow::Result<Connection> {
    let url = crate::embedded::start(port, isolated_dirs(app_data)).await?;
    Ok(Connection {
        url,
        kind: "embedded gateway",
        sidecar: None,
    })
}

#[cfg(not(feature = "embedded"))]
async fn embedded(_port: Option<u16>, _app_data: &std::path::Path) -> anyhow::Result<Connection> {
    anyhow::bail!("this build has no embedded gateway (rebuild with --features embedded)")
}
