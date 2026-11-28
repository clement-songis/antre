//! Design canvas: design artifacts, their versions, exports, and the
//! sandboxed render origin that serves them.
//!
//! A design is a directory under `data_dir/designs/<id>/` (entry
//! `index.html`), with metadata and an append-only version list in SQLite.
//! The agent edits it with the regular file tools; [`hook`] versions it once
//! per turn. See `docs/src/canvas.md`.

pub mod bridge;
pub mod comments;
pub mod csp;
pub mod design_systems;
pub mod error;
pub mod events;
pub mod export;
pub mod frames;
pub mod hook;
pub mod paths;
pub mod publish;
pub mod render;
pub mod server;
pub mod service;
pub mod store;
pub mod token;
pub mod tools;
pub mod vendor;
pub mod versions;

pub use {
    comments::{Comment, NewComment},
    error::{Error, Result},
    events::{CanvasEvent, CanvasEventSink, Visibility},
    frames::{FrameKind, FrameNetwork, StageFrame},
    paths::DesignId,
    publish::{CreatedShare, PublishedSnapshot, ShareBackend},
    render::{BrowserRenderer, DesignRenderer},
    service::{CanvasService, CanvasServiceParts, CanvasSettings, ExportFormat, Viewport},
    store::{Design, DesignFilter, DesignStore, DesignVersion},
};

/// Run the canvas migrations on the shared `antre.db` pool.
pub async fn run_migrations(pool: &sqlx::SqlitePool) -> Result<()> {
    sqlx::migrate!("./migrations")
        .set_ignore_missing(true)
        .run(pool)
        .await?;
    Ok(())
}
