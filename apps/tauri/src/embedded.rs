//! The gateway running inside the shell process (`embedded` feature).

use std::time::Duration;

use url::Url;

use crate::{
    probe,
    sidecar::{IsolatedDirs, free_port},
};

/// Start the gateway on a loopback port in this process and wait until it
/// answers. It runs on the shell's own directories, never `~/.moltis`, so it
/// cannot fight a running user service over the same database.
pub async fn start(port: Option<u16>, dirs: IsolatedDirs) -> anyhow::Result<Url> {
    let port = match port {
        Some(port) => port,
        None => free_port()?,
    };
    std::fs::create_dir_all(&dirs.config)?;
    std::fs::create_dir_all(&dirs.data)?;
    moltis_config::set_config_dir(dirs.config.clone());
    moltis_config::set_data_dir(dirs.data.clone());
    moltis_config::initialize_config();
    let url = Url::parse(&format!("http://127.0.0.1:{port}/"))?;
    // `start_gateway`'s future is not `Send`, so it cannot go on the shell's
    // runtime with `tokio::spawn`; it gets a thread and a runtime of its own.
    std::thread::Builder::new()
        .name("embedded-gateway".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    tracing::error!(%error, "no runtime for the embedded gateway");
                    return;
                },
            };
            let launch = moltis_httpd::server::LaunchOptions {
                open_browser: Some(moltis_config::OpenBrowser::Never),
                status: None,
            };
            let result = runtime.block_on(moltis_httpd::server::start_gateway(
                "127.0.0.1",
                port,
                true,
                None,
                Some(dirs.config),
                Some(dirs.data),
                None,
                Some(moltis_web::web_routes),
                launch,
            ));
            if let Err(error) = result {
                tracing::error!(%error, "embedded gateway stopped");
            }
        })?;
    probe::wait_until_healthy(&url, Duration::from_secs(60)).await?;
    Ok(url)
}
