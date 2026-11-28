#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use {
    antre_agents::tool_registry::AgentTool, antre_skills::discover::SkillDiscoverer,
    async_trait::async_trait, serde_json::json,
};

use {super::*, crate::checkpoints::CheckpointManager};

/// Re-export for the read test that checks the constant matches the skills crate.
const SIDECAR_SUBDIRS: &[&str] = antre_skills::SIDECAR_SUBDIRS;

#[path = "crud_write.rs"]
mod crud_write;
#[path = "read.rs"]
mod read;
