//! Antre shell spike: a Tauri 2 window around the Moltis web UI.
//! See `plans/research/tauri-spike.md`.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod bench;
mod cli;
mod connect;
mod desktop;
#[cfg(feature = "embedded")]
mod embedded;
mod probe;
mod sidecar;

use std::process::ExitCode;

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("ANTRE_SHELL_LOG")
                .unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let result =
        cli::parse(std::env::args().skip(1), |name| std::env::var(name).ok()).and_then(app::run);
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:#}");
            ExitCode::FAILURE
        },
    }
}
