//! Provider registry: model registration, lookup, discovery, and lifecycle.

mod catalog_bridge;
mod core;
pub mod registration;
#[cfg(test)]
mod tests;

pub use self::{
    catalog_bridge::{ModelCatalogDetails, model_catalog_details, set_live_cache_dir},
    core::*,
};
