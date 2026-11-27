//! Provider registry: model registration, lookup, discovery, and lifecycle.

mod catalog_bridge;
mod core;
mod custom;
mod genai;
pub mod registration;
#[cfg(test)]
mod tests;

pub(crate) use self::catalog_bridge::record_catalog_mismatch;
pub use self::{
    catalog_bridge::{
        CatalogMismatch, ModelCatalogDetails, catalog_mismatch, model_catalog_details,
        run_context_probes, set_live_cache_dir,
    },
    core::*,
};
