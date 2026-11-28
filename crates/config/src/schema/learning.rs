//! `[learning]`: the background learning loop (review, curator, ledger).

use serde::{Deserialize, Serialize};

/// Background learning loop settings. Off by default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LearningConfig {
    /// Master switch for the review pass and the curator.
    pub enabled: bool,
    /// Stage learned changes for review instead of writing them. The
    /// `learning.pending.*` RPC methods approve or reject staged changes.
    pub write_approval: bool,
    /// Model for the review and consolidation passes, as a registry id
    /// (`provider::model`). Unset picks the catalog's utility model for the
    /// session's provider, then the session's own model.
    pub model: Option<String>,
    pub review: LearningReviewConfig,
    pub curator: LearningCuratorConfig,
}

impl Default for LearningConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            write_approval: true,
            model: None,
            review: LearningReviewConfig::default(),
            curator: LearningCuratorConfig::default(),
        }
    }
}

/// When and what the post-conversation review proposes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LearningReviewConfig {
    /// Run the review automatically after runs. `learning.review.run` still
    /// works when this is off.
    pub enabled: bool,
    /// Review after this many user turns since the last review.
    pub every_user_turns: u32,
    /// Or after this many tool calls since the last review.
    pub every_tool_calls: u32,
    /// Most changes a single review may propose.
    pub max_changes: u32,
    /// Propose skill patches.
    pub skills: bool,
    /// Propose memory notes.
    pub memory: bool,
}

impl Default for LearningReviewConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            every_user_turns: 10,
            every_tool_calls: 10,
            max_changes: 3,
            skills: true,
            memory: true,
        }
    }
}

/// Aging, archiving and consolidation of agent-managed skills.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LearningCuratorConfig {
    pub enabled: bool,
    /// Hours between two curator runs. The first run waits one interval.
    pub interval_hours: u32,
    /// Only run after this long without chat activity.
    pub min_idle_hours: f64,
    /// Mark a personal skill stale after this many days without use.
    pub stale_after_days: u32,
    /// Archive it after this many days without use. Archives are restorable.
    pub archive_after_days: u32,
    /// Ask the learning model to propose merging overlapping skills.
    pub consolidate: bool,
}

impl Default for LearningCuratorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_hours: 24 * 7,
            min_idle_hours: 2.0,
            stale_after_days: 14,
            archive_after_days: 30,
            consolidate: false,
        }
    }
}
