//! OS-level sandbox backends: Seatbelt (macOS) and bubblewrap (Linux).
//!
//! Lighter than containers: commands run on the host under a kernel-enforced
//! policy built from the same rules on both platforms (see `policy`).

pub mod backend;
pub mod bwrap;
pub mod policy;
pub mod seatbelt;

#[cfg(test)]
mod escape_tests;

pub use {
    backend::{OsSandbox, OsSandboxDirs, OsSandboxKind},
    policy::{OsNetworkAccess, OsSandboxPolicy},
};
