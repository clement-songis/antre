//! Background learning loop: after conversations a cheap model reviews the
//! chat and proposes skill and memory changes; a curator ages and archives
//! personal skills. Every applied change is recorded in an append-only
//! ledger with content-addressed snapshots, so each one can be rolled back
//! on its own. With write approval on, changes wait in a staging area.
//! See `docs/src/learning.md`.

mod macros;

pub mod blobs;
pub mod change;
pub mod curator;
pub mod curator_consolidate;
pub mod diff;
pub mod error;
pub mod guard;
pub mod ledger;
pub mod model;
pub mod paths;
pub mod review;
pub mod review_digest;
pub mod review_parse;
pub mod review_prompt;
pub mod service;
pub mod staging;
pub mod target;

pub use {
    change::{Change, Origin, Proposal},
    error::{Error, Result},
    ledger::{Action, Actor, LedgerEntry},
    model::{ReviewModel, ReviewRequest},
    paths::LearningPaths,
    service::LearningService,
    target::Target,
};
