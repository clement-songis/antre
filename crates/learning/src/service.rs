//! The learning service: runs reviews and curator passes, routes proposals
//! through the guard into the staging area or the ledger, and answers the
//! `learning.*` RPC methods.

use std::{collections::HashSet, path::Path, time::Instant};

use {
    antre_config::{AgentMemoryWriteMode, LearningConfig},
    serde::Serialize,
    time::{Duration, OffsetDateTime},
    tokio::sync::Mutex,
};

#[cfg(feature = "metrics")]
use antre_metrics::{counter, histogram, labels, learning as metrics};

use crate::{
    blobs::sha256_hex,
    change::{Change, Origin, Proposal},
    curator::{self, CuratorPlan, CuratorState, Due, SkillActivity, SkillState},
    curator_consolidate::{self, CONSOLIDATION_SYSTEM_PROMPT, Candidate},
    diff,
    error::{Error, Result},
    guard::{self, GuardContext, Verdict},
    ledger::{Action, Actor, Ledger, LedgerEntry},
    macros::{log_debug, log_info, log_warn},
    model::{ReviewModel, ReviewRequest},
    paths::{LearningPaths, read_optional, write_atomic},
    review::{self, ReviewState, personal_skills},
    review_digest::Turn,
    review_parse::parse_answer,
    review_prompt::{self, ReviewContext},
    staging::Staging,
    target::Target,
};

const REJECTION_HINTS: usize = 10;
const CONSOLIDATION_BUDGET_CHARS: usize = 60_000;

/// Outcome of one proposal.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Outcome {
    Applied { entry: LedgerEntry },
    Staged { proposal: Proposal },
    Duplicate { target: Target },
    Refused { target: Target, reason: String },
    Failed { target: Target, reason: String },
}

impl Outcome {
    /// Short label for metrics and logs.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Applied { .. } => "applied",
            Self::Staged { .. } => "staged",
            Self::Duplicate { .. } => "duplicate",
            Self::Refused { .. } => "refused",
            Self::Failed { .. } => "failed",
        }
    }
}

/// What a review pass did.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ReviewReport {
    pub session_key: String,
    pub model: String,
    pub outcomes: Vec<Outcome>,
    /// Entries of the answer that did not parse.
    pub malformed: Vec<String>,
    pub overlaps: Option<String>,
}

/// What a curator pass did.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct CuratorReport {
    pub plan: CuratorPlan,
    pub outcomes: Vec<Outcome>,
    pub dry_run: bool,
}

/// A staged proposal with what it would do now.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PendingView {
    pub proposal: Proposal,
    /// The target changed since the proposal was made.
    pub stale: bool,
    pub diff: String,
    /// Why the change cannot apply as is, if it cannot.
    pub problem: Option<String>,
}

/// A ledger entry and the rollback that undid it, if any.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LedgerView {
    pub entry: LedgerEntry,
    pub reverted_by: Option<String>,
}

/// Overview for a future "learned changes" panel.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Status {
    pub enabled: bool,
    pub write_approval: bool,
    pub review_enabled: bool,
    pub curator_enabled: bool,
    pub pending: usize,
    pub ledger_entries: usize,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_curator_run: Option<OffsetDateTime>,
    pub stale_skills: Vec<String>,
    pub pinned_skills: Vec<String>,
    pub locked_skills: Vec<String>,
}

/// Mutable state behind the service lock: every write goes through it, so
/// the ledger, the staging area and the state files never interleave.
struct Inner {
    ledger: Ledger,
    staging: Staging,
    curator: CuratorState,
    reviews: ReviewState,
}

/// The learning loop of one data directory.
pub struct LearningService {
    paths: LearningPaths,
    config: LearningConfig,
    memory_mode: AgentMemoryWriteMode,
    inner: Mutex<Inner>,
}

fn load_json<T: serde::de::DeserializeOwned + Default>(path: &Path) -> T {
    let Ok(Some(bytes)) = read_optional(path) else {
        return T::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_else(|_error| {
        log_warn!(error = %_error, path = %path.display(), "learning state unreadable; starting fresh");
        T::default()
    })
}

fn save_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    write_atomic(path, &serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

fn actor_for(origin: &Origin) -> Actor {
    match origin {
        Origin::Review { .. } => Actor::Review,
        Origin::Curator | Origin::Consolidation => Actor::Curator,
    }
}

#[cfg(feature = "metrics")]
fn record_outcomes(outcomes: &[Outcome]) {
    for outcome in outcomes {
        counter!(metrics::PROPOSALS_TOTAL, labels::KIND => outcome.kind()).increment(1);
    }
}

#[cfg(not(feature = "metrics"))]
fn record_outcomes(_outcomes: &[Outcome]) {}

impl LearningService {
    #[must_use]
    pub fn new(
        data_dir: impl Into<std::path::PathBuf>,
        config: LearningConfig,
        memory_mode: AgentMemoryWriteMode,
    ) -> Self {
        let paths = LearningPaths::new(data_dir);
        let inner = Inner {
            ledger: Ledger::new(paths.clone()),
            staging: Staging::new(paths.clone()),
            curator: load_json(&paths.curator_state()),
            reviews: load_json(&paths.review_state()),
        };
        Self {
            paths,
            config,
            memory_mode,
            inner: Mutex::new(inner),
        }
    }

    #[must_use]
    pub fn config(&self) -> &LearningConfig {
        &self.config
    }

    #[must_use]
    pub fn paths(&self) -> &LearningPaths {
        &self.paths
    }

    /// Guard settings from the config, the memory write mode and the locks.
    fn guard_context(&self, curator: &CuratorState, reserved: HashSet<String>) -> GuardContext {
        let (memory_file, memory_notes) = match self.memory_mode {
            AgentMemoryWriteMode::Hybrid => (true, true),
            AgentMemoryWriteMode::PromptOnly => (true, false),
            AgentMemoryWriteMode::SearchOnly => (false, true),
            AgentMemoryWriteMode::Off => (false, false),
        };
        let memory = self.config.review.memory;
        GuardContext {
            allow_skills: self.config.review.skills,
            allow_memory_file: memory && memory_file,
            allow_memory_notes: memory && memory_notes,
            locked: curator.locked(),
            reserved,
        }
    }

    fn base_sha(ledger: &Ledger, proposal: &Proposal) -> Result<Option<String>> {
        if matches!(proposal.change, Change::Archive) {
            let entry = match &proposal.target {
                Target::Skill { name, .. } => Target::skill(name),
                Target::Memory { .. } => proposal.target.clone(),
            };
            return ledger.current_sha(&entry);
        }
        ledger.current_sha(&proposal.target)
    }

    fn submit_locked(
        &self,
        inner: &mut Inner,
        proposals: Vec<Proposal>,
        reserved: &HashSet<String>,
    ) -> Vec<Outcome> {
        let ctx = self.guard_context(&inner.curator, reserved.clone());
        let mut outcomes = Vec::with_capacity(proposals.len());
        for mut proposal in proposals {
            let target = proposal.target.clone();
            let current = match &proposal.change {
                Change::Archive => None,
                _ => inner.ledger.current_text(&target).ok().flatten(),
            };
            match guard::check(&proposal, current.as_deref(), &ctx) {
                Verdict::Accept => {},
                Verdict::Duplicate => {
                    outcomes.push(Outcome::Duplicate { target });
                    continue;
                },
                Verdict::Refuse(reason) => {
                    log_debug!(%target, %reason, "learning guard refused a proposal");
                    outcomes.push(Outcome::Refused { target, reason });
                    continue;
                },
            }
            if proposal.base_sha256.is_none() {
                match Self::base_sha(&inner.ledger, &proposal) {
                    Ok(sha) => proposal.base_sha256 = sha,
                    Err(error) => {
                        outcomes.push(Outcome::Failed {
                            target,
                            reason: error.to_string(),
                        });
                        continue;
                    },
                }
            }
            let outcome = if self.config.write_approval {
                match inner.staging.stage(&proposal) {
                    Ok(true) => Outcome::Staged { proposal },
                    Ok(false) => Outcome::Duplicate { target },
                    Err(error) => Outcome::Failed {
                        target,
                        reason: error.to_string(),
                    },
                }
            } else {
                match self.apply_locked(inner, &proposal, false) {
                    Ok(entry) => Outcome::Applied { entry },
                    Err(error) => Outcome::Failed {
                        target,
                        reason: error.to_string(),
                    },
                }
            };
            outcomes.push(outcome);
        }
        record_outcomes(&outcomes);
        outcomes
    }

    fn apply_locked(
        &self,
        inner: &mut Inner,
        proposal: &Proposal,
        approved: bool,
    ) -> Result<LedgerEntry> {
        let entry = inner
            .ledger
            .apply(proposal, actor_for(&proposal.origin), approved)?;
        if entry.action == Action::Archive
            && let Some(name) = entry.target.skill_name()
        {
            curator::mark_archived(&mut inner.curator, name, entry.at);
            save_json(&self.paths.curator_state(), &inner.curator)?;
        }
        log_info!(target = %entry.target, action = ?entry.action, approved, "learning change applied");
        Ok(entry)
    }

    /// Route proposals through the guard, then stage or apply them.
    pub async fn submit(
        &self,
        proposals: Vec<Proposal>,
        reserved: &HashSet<String>,
    ) -> Vec<Outcome> {
        let mut inner = self.inner.lock().await;
        self.submit_locked(&mut inner, proposals, reserved)
    }

    /// Whether a session's unreviewed turns cross a review threshold.
    pub async fn review_due(&self, session_key: &str, turns: &[Turn]) -> bool {
        if !self.config.enabled || !self.config.review.enabled {
            return false;
        }
        let inner = self.inner.lock().await;
        review::is_due(
            &self.config.review,
            inner.reviews.unreviewed(session_key, turns),
        )
    }

    /// Build the review request for a session.
    pub async fn review_request(
        &self,
        turns: &[Turn],
        loaded_skills: &HashSet<String>,
        budget_chars: usize,
    ) -> ReviewRequest {
        let rejected = {
            let inner = self.inner.lock().await;
            inner
                .staging
                .recent_rejections(REJECTION_HINTS)
                .unwrap_or_default()
                .into_iter()
                .map(|rejection| rejection.summary)
                .collect()
        };
        let context = ReviewContext {
            skills: review::skill_briefs(&personal_skills(&self.paths), loaded_skills),
            memory: review::memory_briefs(&self.paths),
            rejected,
            max_changes: self.config.review.max_changes as usize,
            allow_skills: self.config.review.skills,
            allow_memory: self.config.review.memory,
        };
        ReviewRequest {
            system: review_prompt::system_prompt(),
            user: review_prompt::user_message(&context, turns, budget_chars),
        }
    }

    /// Review a session and route what the model proposes.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(session = session_key, model = model.id())))]
    pub async fn run_review(
        &self,
        model: &dyn ReviewModel,
        session_key: &str,
        turns: &[Turn],
        loaded_skills: &HashSet<String>,
        reserved: &HashSet<String>,
        budget_chars: usize,
    ) -> Result<ReviewReport> {
        let started = Instant::now();
        let request = self
            .review_request(turns, loaded_skills, budget_chars)
            .await;
        let answer = model.complete(&request).await.map_err(Error::Model);
        #[cfg(feature = "metrics")]
        {
            let success = if answer.is_ok() {
                "true"
            } else {
                "false"
            };
            counter!(metrics::REVIEWS_TOTAL, labels::SUCCESS => success).increment(1);
            histogram!(metrics::REVIEW_DURATION_SECONDS).record(started.elapsed().as_secs_f64());
        }
        #[cfg(not(feature = "metrics"))]
        let _ = started;
        let origin = Origin::Review {
            session_key: session_key.to_string(),
        };
        let parsed = parse_answer(&answer?, &origin)?;
        let mut inner = self.inner.lock().await;
        inner
            .reviews
            .mark(session_key, turns.len(), OffsetDateTime::now_utc());
        save_json(&self.paths.review_state(), &inner.reviews)?;
        let max = self.config.review.max_changes as usize;
        let proposals = parsed.proposals.into_iter().take(max).collect();
        let outcomes = self.submit_locked(&mut inner, proposals, reserved);
        log_info!(
            session = session_key,
            changes = outcomes.len(),
            malformed = parsed.malformed.len(),
            "learning review finished"
        );
        Ok(ReviewReport {
            session_key: session_key.to_string(),
            model: model.id().to_string(),
            outcomes,
            malformed: parsed.malformed,
            overlaps: parsed.overlaps,
        })
    }

    /// Staged proposals with their diff against the current files.
    pub async fn pending(&self) -> Result<Vec<PendingView>> {
        let inner = self.inner.lock().await;
        let proposals = inner.staging.list()?;
        Ok(proposals
            .into_iter()
            .map(|proposal| self.pending_view(&inner.ledger, proposal))
            .collect())
    }

    /// One staged proposal.
    pub async fn pending_get(&self, id: &str) -> Result<PendingView> {
        let inner = self.inner.lock().await;
        let proposal = inner.staging.get(id)?;
        Ok(self.pending_view(&inner.ledger, proposal))
    }

    fn pending_view(&self, ledger: &Ledger, proposal: Proposal) -> PendingView {
        let current_sha = Self::base_sha(ledger, &proposal).ok().flatten();
        let stale = current_sha != proposal.base_sha256;
        let path = proposal.target.to_string();
        let (diff, problem) = match &proposal.change {
            Change::Archive => (format!("archive {path}\n"), None),
            change => {
                let current = ledger.current_text(&proposal.target).ok().flatten();
                match change.apply(current.as_deref()) {
                    Ok(next) => (
                        diff::unified(&path, current.as_deref().unwrap_or_default(), &next),
                        None,
                    ),
                    Err(error) => (String::new(), Some(error.to_string())),
                }
            },
        };
        PendingView {
            proposal,
            stale,
            diff,
            problem,
        }
    }

    /// Apply a staged proposal.
    pub async fn approve(&self, id: &str) -> Result<LedgerEntry> {
        let mut inner = self.inner.lock().await;
        let proposal = inner.staging.get(id)?;
        if let Some(name) = proposal.target.skill_name()
            && inner.curator.locked().contains(name)
        {
            return Err(Error::Refused(format!("skill '{name}' is locked")));
        }
        let entry = self.apply_locked(&mut inner, &proposal, true)?;
        inner.staging.remove(id)?;
        #[cfg(feature = "metrics")]
        counter!(metrics::DECISIONS_TOTAL, labels::OPERATION => "approve").increment(1);
        Ok(entry)
    }

    /// Reject a staged proposal; later reviews see it as a hint.
    pub async fn reject(&self, id: &str, note: Option<String>) -> Result<()> {
        let inner = self.inner.lock().await;
        inner.staging.reject(id, note)?;
        #[cfg(feature = "metrics")]
        counter!(metrics::DECISIONS_TOTAL, labels::OPERATION => "reject").increment(1);
        Ok(())
    }

    /// Ledger entries, newest first, optionally for one skill.
    pub async fn ledger(&self, skill: Option<&str>, limit: usize) -> Result<Vec<LedgerView>> {
        let inner = self.inner.lock().await;
        let entries = inner.ledger.entries()?;
        let reverted: std::collections::HashMap<&str, &str> = entries
            .iter()
            .filter_map(|entry| entry.reverts.as_deref().map(|id| (id, entry.id.as_str())))
            .collect();
        Ok(entries
            .iter()
            .rev()
            .filter(|entry| skill.is_none_or(|name| entry.target.skill_name() == Some(name)))
            .take(limit)
            .map(|entry| LedgerView {
                entry: entry.clone(),
                reverted_by: reverted.get(entry.id.as_str()).map(|id| (*id).to_string()),
            })
            .collect())
    }

    /// Diff recorded by one ledger entry.
    pub async fn ledger_diff(&self, id: &str) -> Result<String> {
        let inner = self.inner.lock().await;
        let entry = inner.ledger.get(id)?;
        let read = |sha: &Option<String>| -> Result<String> {
            match sha {
                Some(sha) => {
                    Ok(String::from_utf8_lossy(&inner.ledger.blobs().get(sha)?).into_owned())
                },
                None => Ok(String::new()),
            }
        };
        if entry.action == Action::Archive {
            return Ok(format!(
                "archive {} -> {}\n",
                entry.target,
                entry.archived_to.unwrap_or_default()
            ));
        }
        Ok(diff::unified(
            &entry.target.to_string(),
            &read(&entry.before)?,
            &read(&entry.after)?,
        ))
    }

    /// Undo one ledger entry.
    pub async fn rollback(&self, id: &str, reason: &str) -> Result<LedgerEntry> {
        let mut inner = self.inner.lock().await;
        let undone = inner.ledger.get(id)?;
        let entry = inner.ledger.rollback(id, reason)?;
        if undone.action == Action::Archive
            && let Some(name) = undone.target.skill_name()
        {
            inner
                .curator
                .record_mut(name, OffsetDateTime::now_utc())
                .state = SkillState::Active;
            save_json(&self.paths.curator_state(), &inner.curator)?;
        }
        #[cfg(feature = "metrics")]
        counter!(metrics::ROLLBACKS_TOTAL).increment(1);
        log_info!(reverts = id, target = %entry.target, "learning change rolled back");
        Ok(entry)
    }

    /// Pin (curator skips) or lock (review and curator skip) a skill.
    pub async fn set_flags(
        &self,
        name: &str,
        pinned: Option<bool>,
        locked: Option<bool>,
    ) -> Result<()> {
        crate::target::validate_skill_name(name)?;
        let mut inner = self.inner.lock().await;
        let record = inner.curator.record_mut(name, OffsetDateTime::now_utc());
        if let Some(pinned) = pinned {
            record.pinned = pinned;
        }
        if let Some(locked) = locked {
            record.locked = locked;
        }
        save_json(&self.paths.curator_state(), &inner.curator)
    }

    /// Scheduled curator tick: seeds, waits or runs the aging pass.
    pub async fn curator_tick(
        &self,
        activity: &[SkillActivity],
        idle_for: Duration,
        reserved: &HashSet<String>,
    ) -> Result<Option<CuratorReport>> {
        if !self.config.enabled || !self.config.curator.enabled {
            return Ok(None);
        }
        let now = OffsetDateTime::now_utc();
        let decision = {
            let inner = self.inner.lock().await;
            curator::due(now, &inner.curator, &self.config.curator, idle_for)
        };
        match decision {
            Due::Wait => Ok(None),
            Due::Seed => {
                let mut inner = self.inner.lock().await;
                inner.curator.last_run_at = Some(now);
                save_json(&self.paths.curator_state(), &inner.curator)?;
                log_info!("learning curator seeded; first run after one interval");
                Ok(None)
            },
            Due::Run => self.run_curator(activity, reserved, false).await.map(Some),
        }
    }

    /// Run the aging pass now. A dry run plans without changing anything.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(dry_run)))]
    pub async fn run_curator(
        &self,
        activity: &[SkillActivity],
        reserved: &HashSet<String>,
        dry_run: bool,
    ) -> Result<CuratorReport> {
        let now = OffsetDateTime::now_utc();
        let mut inner = self.inner.lock().await;
        let mut state = inner.curator.clone();
        let plan = curator::plan(now, &mut state, activity, &self.config.curator);
        if dry_run {
            return Ok(CuratorReport {
                plan,
                outcomes: Vec::new(),
                dry_run,
            });
        }
        state.last_run_at = Some(now);
        inner.curator = state;
        save_json(&self.paths.curator_state(), &inner.curator)?;
        let days = self.config.curator.archive_after_days;
        let proposals = plan
            .archive
            .iter()
            .map(|name| {
                Proposal::new(
                    Origin::Curator,
                    Target::skill(name),
                    Change::Archive,
                    format!("unused for {days} days"),
                )
            })
            .collect();
        let outcomes = self.submit_locked(&mut inner, proposals, reserved);
        #[cfg(feature = "metrics")]
        {
            counter!(metrics::CURATOR_RUNS_TOTAL, labels::VARIANT => "aging").increment(1);
            for (kind, count) in [
                ("stale", plan.marked_stale.len()),
                ("reactivated", plan.reactivated.len()),
                ("archive", plan.archive.len()),
            ] {
                counter!(metrics::CURATOR_TRANSITIONS_TOTAL, labels::KIND => kind)
                    .increment(count as u64);
            }
        }
        log_info!(
            stale = plan.marked_stale.len(),
            archive = plan.archive.len(),
            reactivated = plan.reactivated.len(),
            "learning curator pass finished"
        );
        Ok(CuratorReport {
            plan,
            outcomes,
            dry_run,
        })
    }

    /// Skills the consolidation pass may merge: personal, not pinned or
    /// locked, within a total size budget.
    pub async fn consolidation_candidates(&self) -> Vec<Candidate> {
        let inner = self.inner.lock().await;
        let mut budget = CONSOLIDATION_BUDGET_CHARS;
        personal_skills(&self.paths)
            .into_iter()
            .filter_map(|skill| {
                let record = inner.curator.skills.get(&skill.name);
                if record.is_some_and(|record| record.pinned || record.locked) {
                    return None;
                }
                budget = budget.checked_sub(skill.skill_md.len())?;
                Some(Candidate {
                    state: record.map_or(SkillState::Active, |record| record.state),
                    name: skill.name,
                    description: skill.description,
                    body: skill.skill_md,
                })
            })
            .collect()
    }

    /// Ask the model to merge overlapping skills.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(model = model.id())))]
    pub async fn run_consolidation(
        &self,
        model: &dyn ReviewModel,
        reserved: &HashSet<String>,
    ) -> Result<Vec<Outcome>> {
        let candidates = self.consolidation_candidates().await;
        if candidates.len() < 2 {
            return Ok(Vec::new());
        }
        let request = ReviewRequest {
            system: CONSOLIDATION_SYSTEM_PROMPT,
            user: curator_consolidate::user_message(&candidates),
        };
        let answer = model.complete(&request).await.map_err(Error::Model)?;
        let proposals = curator_consolidate::parse_merges(&answer, &candidates)?;
        #[cfg(feature = "metrics")]
        counter!(metrics::CURATOR_RUNS_TOTAL, labels::VARIANT => "consolidation").increment(1);
        Ok(self.submit(proposals, reserved).await)
    }

    /// Overview of the loop.
    pub async fn status(&self) -> Result<Status> {
        let inner = self.inner.lock().await;
        let names = |keep: fn(&curator::SkillRecord) -> bool| -> Vec<String> {
            inner
                .curator
                .skills
                .iter()
                .filter(|(_, record)| keep(record))
                .map(|(name, _)| name.clone())
                .collect()
        };
        Ok(Status {
            enabled: self.config.enabled,
            write_approval: self.config.write_approval,
            review_enabled: self.config.review.enabled,
            curator_enabled: self.config.curator.enabled,
            pending: inner.staging.list()?.len(),
            ledger_entries: inner.ledger.entries()?.len(),
            last_curator_run: inner.curator.last_run_at,
            stale_skills: names(|record| record.state == SkillState::Stale),
            pinned_skills: names(|record| record.pinned),
            locked_skills: names(|record| record.locked),
        })
    }

    /// Names of personal skills on disk.
    #[must_use]
    pub fn personal_skill_names(&self) -> Vec<String> {
        personal_skills(&self.paths)
            .into_iter()
            .map(|skill| skill.name)
            .collect()
    }

    /// sha256 of a personal skill's `SKILL.md`, for tests and callers that
    /// build proposals by hand.
    pub fn skill_sha(&self, name: &str) -> Result<Option<String>> {
        let path = Target::skill(name).resolve(&self.paths)?;
        Ok(read_optional(&path)?.map(|bytes| sha256_hex(&bytes)))
    }
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
