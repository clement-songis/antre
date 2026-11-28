//! Mod SDK runtime (server side).
//!
//! A mod is a folder with an `antre-mod.toml` manifest and, optionally, a
//! WASM component implementing the `antre:mods/plugin` world
//! (`wit/mod/antre-mod.wit`). This crate validates manifests, computes the
//! capability set and its consent hash, installs packages into a
//! content-addressed store, runs components under fuel/epoch/memory limits and
//! exposes their tools and hooks through a [`registry::ContributionRegistry`]
//! that core features use too. Design: `plans/2026-11-28-plan-mods-sdk.md`.

pub mod adapters;
pub mod capability;
pub mod error;
pub mod grant;
pub mod hook_pins;
#[cfg(feature = "wasm")]
pub mod host;
pub mod id;
pub mod manifest;
mod metrics_support;
pub mod package;
pub mod registry;
#[cfg(feature = "wasm")]
pub mod service;
pub mod state;
#[cfg(all(test, feature = "wasm"))]
mod test_fixtures;

pub use {
    capability::{Capability, CapabilitySet, ConsentHash, Risk},
    error::{Error, Result},
    grant::{ConsentState, Grant, GrantOrigin, InstallMode},
    id::{ContributionId, LocalId, ModId, Owner},
    manifest::ModManifest,
    registry::{Contribution, ContributionRegistry, Registration},
};
