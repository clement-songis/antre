//! The Tauri application: one window, pointed at a daemon or a bench page.

use std::sync::Mutex;

use tauri::{Manager, RunEvent, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::{
    bench::{self, BenchOutput},
    cli::Mode,
    connect::{self, Connection},
};

const WINDOW: &str = "main";

/// Keeps the sidecar alive for the app's lifetime.
#[derive(Default)]
struct Daemon(Mutex<Option<Connection>>);

pub fn run(mode: Mode) -> anyhow::Result<()> {
    let app = match mode {
        Mode::Bench { page, out } => tauri::Builder::default()
            .manage(BenchOutput(out))
            .invoke_handler(tauri::generate_handler![
                bench::bench_report,
                bench::bench_log
            ])
            .setup(move |app| {
                WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::App(page.path().into()))
                    .title(format!("Antre bench: {}", page.path()))
                    .inner_size(1280.0, 900.0)
                    // An occluded or unfocused WKWebView throttles timers and
                    // requestAnimationFrame, which would skew frame timings.
                    .focused(true)
                    .always_on_top(true)
                    .build()?
                    .set_focus()?;
                Ok(())
            }),
        mode => tauri::Builder::default()
            .manage(Daemon::default())
            .setup(move |app| {
                let window =
                    WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::App("index.html".into()))
                        .title("Antre")
                        .inner_size(1280.0, 860.0)
                        .build()?;
                let app_data = app.path().app_data_dir()?;
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(exit_on_signal(handle.clone()));
                tauri::async_runtime::spawn(async move {
                    let reload = window.clone();
                    let result = connect::connect(mode, app_data, move || {
                        if let Ok(url) = reload.url() {
                            let _ = reload.navigate(url);
                        }
                    })
                    .await;
                    show(&window, result, &handle);
                });
                Ok(())
            }),
    }
    .build(tauri::generate_context!())?;

    app.run(|handle, event| {
        if let RunEvent::Exit = event
            && let Some(daemon) = handle.try_state::<Daemon>()
            && let Some(connection) = daemon.0.lock().unwrap_or_else(|e| e.into_inner()).take()
            && let Some(sidecar) = connection.sidecar
        {
            // The run loop is not inside the async runtime, so blocking here
            // is allowed; the child must be gone before the process exits.
            tauri::async_runtime::block_on(sidecar.shutdown());
        }
    });
    Ok(())
}

/// SIGTERM/SIGINT end the process without `RunEvent::Exit`, which would
/// orphan the sidecar; turn them into a normal exit instead.
async fn exit_on_signal(handle: tauri::AppHandle) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let (Ok(mut term), Ok(mut interrupt)) = (
            signal(SignalKind::terminate()),
            signal(SignalKind::interrupt()),
        ) else {
            return;
        };
        tokio::select! {
            _ = term.recv() => {},
            _ = interrupt.recv() => {},
        }
    }
    #[cfg(not(unix))]
    if tokio::signal::ctrl_c().await.is_err() {
        return;
    }
    tracing::info!("signal received, quitting");
    handle.exit(0);
}

fn show(window: &WebviewWindow, result: anyhow::Result<Connection>, handle: &tauri::AppHandle) {
    match result {
        Ok(connection) => {
            tracing::info!(url = %connection.url, kind = connection.kind, "showing daemon");
            if let Err(error) = window.navigate(connection.url.clone()) {
                tracing::error!(%error, "navigation failed");
            }
            if let Some(daemon) = handle.try_state::<Daemon>() {
                *daemon.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(connection);
            }
        },
        Err(error) => {
            tracing::error!(error = format!("{error:#}"), "no daemon");
            let message = serde_json::to_string(&format!("{error:#}")).unwrap_or_default();
            let _ = window.eval(format!("window.showError && window.showError({message})"));
        },
    }
}
