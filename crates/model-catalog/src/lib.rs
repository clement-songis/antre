//! Model catalog: per-model metadata (kind, limits, modalities, capabilities,
//! cost, lifecycle) merged from user overrides, runtime probes, live provider
//! discovery, models.dev and id heuristics. See `docs/src/model-catalog.md`.

mod macros;

pub mod catalog;
pub mod diff;
pub mod error;
pub mod global;
pub mod live_cache;
pub mod models_dev;
pub mod names;
pub mod providers;
pub mod refresh;
pub mod roles;
pub mod snapshot;
pub mod types;

pub use {
    catalog::{ModelCatalog, ResolvedModel, Source},
    error::{Error, Result},
    names::{PrettyName, prettify_id},
    types::{
        Cost, Modality, ModelKind, ModelSpec, ModelStatus, RateLimitTz, Reasoning, ReasoningLevel,
    },
};
