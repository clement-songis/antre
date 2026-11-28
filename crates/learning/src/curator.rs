//! The curator: ages personal skills by their last use. Active skills turn
//! stale, stale skills get archived (restorable from the ledger), and a
//! skill used again turns active. Pinned and locked skills are left alone.
//! The model-driven merge pass lives in [`crate::curator_consolidate`].

use std::collections::{BTreeMap, HashMap};

use {
    moltis_config::LearningCuratorConfig,
    moltis_skills::usage::SkillUsageEntry,
    serde::{Deserialize, Serialize},
    time::{Duration, OffsetDateTime},
};

/// Lifecycle of a personal skill.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillState {
    #[default]
    Active,
    Stale,
    Archived,
}

/// What the curator remembers about one skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillRecord {
    #[serde(default)]
    pub state: SkillState,
    /// When the curator first saw the skill: the inactivity clock of a skill
    /// without usage data starts here, not at the epoch.
    #[serde(with = "time::serde::rfc3339")]
    pub first_seen: OffsetDateTime,
    /// The curator skips pinned skills.
    #[serde(default)]
    pub pinned: bool,
    /// Neither the review nor the curator touch locked skills.
    #[serde(default)]
    pub locked: bool,
}

impl SkillRecord {
    fn new(now: OffsetDateTime) -> Self {
        Self {
            state: SkillState::Active,
            first_seen: now,
            pinned: false,
            locked: false,
        }
    }

    fn has_flags(&self) -> bool {
        self.pinned || self.locked
    }
}

/// Persisted curator state (`learning/curator.json`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CuratorState {
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub last_run_at: Option<OffsetDateTime>,
    #[serde(default)]
    pub skills: BTreeMap<String, SkillRecord>,
}

impl CuratorState {
    /// Flags for a skill, creating its record when missing.
    pub fn record_mut(&mut self, name: &str, now: OffsetDateTime) -> &mut SkillRecord {
        self.skills
            .entry(name.to_string())
            .or_insert_with(|| SkillRecord::new(now))
    }

    /// Names of locked skills.
    #[must_use]
    pub fn locked(&self) -> std::collections::HashSet<String> {
        self.skills
            .iter()
            .filter(|(_, record)| record.locked)
            .map(|(name, _)| name.clone())
            .collect()
    }
}

/// Usage of one personal skill on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillActivity {
    pub name: String,
    pub last_used: Option<OffsetDateTime>,
    pub created: Option<OffsetDateTime>,
    pub uses: u64,
}

fn from_unix_ms(ms: u64) -> Option<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000).ok()
}

/// Build activity for the personal skills on disk from the usage store.
#[must_use]
pub fn activity_from_usage(
    names: &[String],
    usage: &HashMap<String, SkillUsageEntry>,
) -> Vec<SkillActivity> {
    names
        .iter()
        .map(|name| {
            let entry = usage.get(name);
            let last_used = entry
                .and_then(|entry| entry.last_read_at.max(entry.last_write_at))
                .and_then(from_unix_ms);
            SkillActivity {
                name: name.clone(),
                last_used,
                created: entry.and_then(|entry| from_unix_ms(entry.created_at)),
                uses: entry.map_or(0, |entry| entry.read_count + entry.write_count),
            }
        })
        .collect()
}

/// What one curator pass decided.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CuratorPlan {
    pub checked: usize,
    pub seeded: Vec<String>,
    pub marked_stale: Vec<String>,
    pub reactivated: Vec<String>,
    /// Skills to archive; the service applies or stages these.
    pub archive: Vec<String>,
}

/// Whether the curator should run now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Due {
    /// First observation: remember now and wait one interval, so a fresh
    /// install never changes the library on its first tick.
    Seed,
    Wait,
    Run,
}

/// Decide whether a scheduled run is due.
#[must_use]
pub fn due(
    now: OffsetDateTime,
    state: &CuratorState,
    config: &LearningCuratorConfig,
    idle_for: Duration,
) -> Due {
    let Some(last) = state.last_run_at else {
        return Due::Seed;
    };
    let interval = Duration::hours(i64::from(config.interval_hours.max(1)));
    let min_idle = Duration::seconds_f64(config.min_idle_hours.max(0.0) * 3_600.0);
    if now - last >= interval && idle_for >= min_idle {
        Due::Run
    } else {
        Due::Wait
    }
}

/// Plan the deterministic transitions and update `state` for everything but
/// archiving, which goes through the ledger.
pub fn plan(
    now: OffsetDateTime,
    state: &mut CuratorState,
    activity: &[SkillActivity],
    config: &LearningCuratorConfig,
) -> CuratorPlan {
    let stale_days = i64::from(config.stale_after_days.max(1));
    let archive_days = i64::from(config.archive_after_days.max(1));
    let stale_cutoff = now - Duration::days(stale_days);
    let archive_cutoff = now - Duration::days(archive_days);
    let mut result = CuratorPlan {
        checked: activity.len(),
        ..CuratorPlan::default()
    };

    // Forget skills that left the disk, unless the user flagged them.
    state.skills.retain(|name, record| {
        record.has_flags()
            || record.state == SkillState::Archived
            || activity.iter().any(|skill| skill.name == *name)
    });

    for skill in activity {
        let Some(record) = state.skills.get_mut(&skill.name) else {
            state
                .skills
                .insert(skill.name.clone(), SkillRecord::new(now));
            result.seeded.push(skill.name.clone());
            continue;
        };
        if record.has_flags() {
            continue;
        }
        if record.state == SkillState::Archived {
            // Back on disk after an archive: restored or recreated.
            record.state = SkillState::Active;
            result.reactivated.push(skill.name.clone());
            continue;
        }
        // The clock never starts before the curator first saw the skill, so
        // enabling the curator does not archive an old library at once.
        let anchor = skill
            .last_used
            .or(skill.created)
            .map_or(record.first_seen, |at| at.max(record.first_seen));
        if anchor > stale_cutoff {
            if record.state == SkillState::Stale {
                record.state = SkillState::Active;
                result.reactivated.push(skill.name.clone());
            }
        } else if anchor <= archive_cutoff {
            result.archive.push(skill.name.clone());
        } else if record.state == SkillState::Active {
            record.state = SkillState::Stale;
            result.marked_stale.push(skill.name.clone());
        }
    }
    result
}

/// Mark a skill archived after the ledger moved it.
pub fn mark_archived(state: &mut CuratorState, name: &str, now: OffsetDateTime) {
    state.record_mut(name, now).state = SkillState::Archived;
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use {super::*, time::macros::datetime};

    const NOW: OffsetDateTime = datetime!(2026-11-28 12:00 UTC);

    fn config() -> LearningCuratorConfig {
        LearningCuratorConfig::default()
    }

    fn skill(name: &str, days_ago: Option<i64>, uses: u64) -> SkillActivity {
        SkillActivity {
            name: name.into(),
            last_used: days_ago.map(|days| NOW - Duration::days(days)),
            created: Some(NOW - Duration::days(400)),
            uses,
        }
    }

    fn seeded(names: &[&str], days_ago: i64) -> CuratorState {
        let mut state = CuratorState::default();
        for name in names {
            state.skills.insert(
                (*name).into(),
                SkillRecord::new(NOW - Duration::days(days_ago)),
            );
        }
        state
    }

    #[test]
    fn first_sight_seeds_without_changes() {
        let mut state = CuratorState::default();
        let plan = plan(NOW, &mut state, &[skill("old", Some(100), 5)], &config());
        assert_eq!(plan.seeded, ["old"]);
        assert!(plan.archive.is_empty());
        assert_eq!(state.skills["old"].first_seen, NOW);
    }

    #[test]
    fn ages_by_last_use() {
        let mut state = seeded(&["fresh", "idle", "gone-quiet", "unused"], 200);
        let plan = plan(
            NOW,
            &mut state,
            &[
                skill("fresh", Some(2), 9),
                skill("idle", Some(20), 3),
                skill("gone-quiet", Some(45), 3),
                skill("unused", None, 0),
            ],
            &config(),
        );
        assert_eq!(plan.marked_stale, ["idle"]);
        assert_eq!(plan.archive, ["gone-quiet", "unused"]);
        assert_eq!(state.skills["idle"].state, SkillState::Stale);
        assert_eq!(state.skills["fresh"].state, SkillState::Active);
    }

    #[test]
    fn old_skills_age_from_first_sight() {
        let mut state = seeded(&["legacy"], 5);
        let plan = plan(NOW, &mut state, &[skill("legacy", Some(300), 2)], &config());
        assert!(plan.archive.is_empty() && plan.marked_stale.is_empty());
    }

    #[test]
    fn never_used_young_skills_are_kept() {
        let mut state = seeded(&["new-skill"], 3);
        let mut young = skill("new-skill", None, 0);
        young.created = None;
        let plan = plan(NOW, &mut state, &[young], &config());
        assert!(plan.archive.is_empty() && plan.marked_stale.is_empty());
    }

    #[test]
    fn use_after_stale_reactivates() {
        let mut state = seeded(&["back"], 200);
        state.skills.get_mut("back").unwrap().state = SkillState::Stale;
        let plan = plan(NOW, &mut state, &[skill("back", Some(1), 4)], &config());
        assert_eq!(plan.reactivated, ["back"]);
        assert_eq!(state.skills["back"].state, SkillState::Active);
    }

    #[test]
    fn flags_and_archive_records() {
        let mut state = seeded(&["pinned", "locked", "restored", "deleted"], 200);
        state.skills.get_mut("pinned").unwrap().pinned = true;
        state.skills.get_mut("locked").unwrap().locked = true;
        mark_archived(&mut state, "restored", NOW);
        let plan = plan(
            NOW,
            &mut state,
            &[
                skill("pinned", Some(300), 1),
                skill("locked", Some(300), 1),
                skill("restored", Some(300), 1),
            ],
            &config(),
        );
        assert!(plan.archive.is_empty());
        assert_eq!(plan.reactivated, ["restored"]);
        assert!(!state.skills.contains_key("deleted"));
        assert_eq!(state.locked(), ["locked".to_string()].into());
    }

    #[test]
    fn schedule_seeds_then_waits_for_interval_and_idle() {
        let mut state = CuratorState::default();
        let config = config();
        assert_eq!(due(NOW, &state, &config, Duration::hours(5)), Due::Seed);
        state.last_run_at = Some(NOW - Duration::days(3));
        assert_eq!(due(NOW, &state, &config, Duration::hours(5)), Due::Wait);
        state.last_run_at = Some(NOW - Duration::days(8));
        assert_eq!(due(NOW, &state, &config, Duration::minutes(30)), Due::Wait);
        assert_eq!(due(NOW, &state, &config, Duration::hours(3)), Due::Run);
    }

    #[test]
    fn usage_entries_become_activity() {
        let usage = HashMap::from([("a".to_string(), SkillUsageEntry {
            read_count: 2,
            write_count: 1,
            last_read_at: Some(1_700_000_000_000),
            last_write_at: Some(1_700_000_100_000),
            created_at: 1_600_000_000_000,
        })]);
        let activity = activity_from_usage(&["a".into(), "b".into()], &usage);
        assert_eq!(activity[0].uses, 3);
        assert_eq!(
            activity[0].last_used,
            OffsetDateTime::from_unix_timestamp(1_700_000_100).ok()
        );
        assert_eq!(activity[1].uses, 0);
        assert!(activity[1].last_used.is_none());
    }
}
