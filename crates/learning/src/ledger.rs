//! Append-only mutation ledger. Every applied change writes one JSON line
//! with the before and after hashes of the file; the contents live in the
//! blob store, so any single entry can be rolled back while the entries
//! around it stay in place.

use std::{
    io::Write,
    path::{Path, PathBuf},
};

use {
    serde::{Deserialize, Serialize},
    time::{OffsetDateTime, macros::format_description},
};

use crate::{
    blobs::{BlobStore, sha256_hex},
    change::{Change, Proposal},
    error::{Error, Result},
    macros::log_warn,
    paths::{LearningPaths, read_optional, write_atomic},
    target::{SKILL_FILE, Target},
};

/// Who made a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Actor {
    /// The post-conversation review.
    Review,
    /// The curator (aging or consolidation).
    Curator,
    /// A person, through `learning.*` (rollbacks).
    User,
}

/// What a ledger entry did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Create,
    Replace,
    Append,
    Rewrite,
    Archive,
    Rollback,
}

impl From<&Change> for Action {
    fn from(change: &Change) -> Self {
        match change {
            Change::Create { .. } => Self::Create,
            Change::Replace { .. } => Self::Replace,
            Change::Append { .. } => Self::Append,
            Change::Rewrite { .. } => Self::Rewrite,
            Change::Archive => Self::Archive,
        }
    }
}

/// One line of `learning/ledger.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerEntry {
    pub id: String,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    pub actor: Actor,
    pub action: Action,
    pub target: Target,
    /// sha256 of the file before; `None` when it did not exist. For an
    /// archive, the skill's `SKILL.md`.
    pub before: Option<String>,
    /// sha256 of the file after; `None` when it no longer exists.
    pub after: Option<String>,
    /// Where an archived skill went, relative to `data_dir`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal_id: Option<String>,
    /// Applied after someone approved the staged proposal.
    #[serde(default)]
    pub approved: bool,
    pub reason: String,
    /// For a rollback: the entry it undid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reverts: Option<String>,
}

/// The ledger and the files it guards.
#[derive(Debug, Clone)]
pub struct Ledger {
    paths: LearningPaths,
    blobs: BlobStore,
}

fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

fn read_text(path: &Path) -> Result<Option<String>> {
    read_optional(path)?
        .map(|bytes| {
            String::from_utf8(bytes)
                .map_err(|_| Error::Conflict(format!("{} is not UTF-8 text", path.display())))
        })
        .transpose()
}

impl Ledger {
    #[must_use]
    pub fn new(paths: LearningPaths) -> Self {
        let blobs = BlobStore::new(paths.blobs());
        Self { paths, blobs }
    }

    #[must_use]
    pub fn blobs(&self) -> &BlobStore {
        &self.blobs
    }

    /// Hash of a target's current content, `None` when absent.
    pub fn current_sha(&self, target: &Target) -> Result<Option<String>> {
        Ok(read_optional(&target.resolve(&self.paths)?)?.map(|bytes| sha256_hex(&bytes)))
    }

    /// Current text of a target, `None` when absent.
    pub fn current_text(&self, target: &Target) -> Result<Option<String>> {
        read_text(&target.resolve(&self.paths)?)
    }

    fn append(&self, entry: &LedgerEntry) -> Result<()> {
        let path = self.paths.ledger();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut line = serde_json::to_vec(entry)?;
        line.push(b'\n');
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        file.write_all(&line)?;
        file.sync_data()?;
        Ok(())
    }

    /// All entries, oldest first. Unreadable lines are skipped.
    pub fn entries(&self) -> Result<Vec<LedgerEntry>> {
        let Some(bytes) = read_optional(&self.paths.ledger())? else {
            return Ok(Vec::new());
        };
        Ok(bytes
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.iter().all(u8::is_ascii_whitespace))
            .filter_map(|line| {
                serde_json::from_slice(line)
                    .inspect_err(
                        |_error| log_warn!(error = %_error, "skipping unreadable ledger line"),
                    )
                    .ok()
            })
            .collect())
    }

    /// One entry by id.
    pub fn get(&self, id: &str) -> Result<LedgerEntry> {
        self.entries()?
            .into_iter()
            .find(|entry| entry.id == id)
            .ok_or_else(|| Error::NotFound(format!("ledger entry {id}")))
    }

    /// Apply a proposal and record it. Refuses when the target changed since
    /// the proposal's `base_sha256`.
    pub fn apply(&self, proposal: &Proposal, actor: Actor, approved: bool) -> Result<LedgerEntry> {
        let target = &proposal.target;
        target.validate()?;
        if matches!(proposal.change, Change::Archive) {
            return self.archive(proposal, actor, approved);
        }
        let path = target.resolve(&self.paths)?;
        let current = read_text(&path)?;
        let before = current.as_deref().map(|text| sha256_hex(text.as_bytes()));
        if proposal.base_sha256 != before {
            return Err(Error::StaleTarget(target.to_string()));
        }
        if !target.is_skill_entry()
            && let Some(dir) = target.skill_dir(&self.paths)?
            && !dir.join(SKILL_FILE).exists()
        {
            return Err(Error::Conflict(format!(
                "skill '{}' does not exist",
                target.skill_name().unwrap_or_default()
            )));
        }
        let next = proposal.change.apply(current.as_deref())?;
        if let Some(text) = &current {
            self.blobs.put(text.as_bytes())?;
        }
        let after = self.blobs.put(next.as_bytes())?;
        write_atomic(&path, next.as_bytes())?;
        let entry = LedgerEntry {
            id: new_id(),
            at: OffsetDateTime::now_utc(),
            actor,
            action: Action::from(&proposal.change),
            target: target.clone(),
            before,
            after: Some(after),
            archived_to: None,
            proposal_id: Some(proposal.id.clone()),
            approved,
            reason: proposal.reason.clone(),
            reverts: None,
        };
        self.append(&entry)?;
        Ok(entry)
    }

    fn archive(&self, proposal: &Proposal, actor: Actor, approved: bool) -> Result<LedgerEntry> {
        let target = &proposal.target;
        let (Some(name), Some(dir)) = (target.skill_name(), target.skill_dir(&self.paths)?) else {
            return Err(Error::InvalidTarget("only skills can be archived".into()));
        };
        let skill_md = read_text(&dir.join(SKILL_FILE))?
            .ok_or_else(|| Error::NotFound(format!("skill '{name}'")))?;
        let before = self.blobs.put(skill_md.as_bytes())?;
        if proposal
            .base_sha256
            .as_ref()
            .is_some_and(|base| *base != before)
        {
            return Err(Error::StaleTarget(target.to_string()));
        }
        let stamp = OffsetDateTime::now_utc()
            .format(format_description!(
                "[year][month][day][hour][minute][second]"
            ))
            .unwrap_or_default();
        let id = new_id();
        let archived = self
            .paths
            .archive()
            .join(format!("{name}-{stamp}-{}", &id[..8]));
        std::fs::create_dir_all(self.paths.archive())?;
        std::fs::rename(&dir, &archived)?;
        let archived_to = archived
            .strip_prefix(self.paths.data_dir())
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        let entry = LedgerEntry {
            id,
            at: OffsetDateTime::now_utc(),
            actor,
            action: Action::Archive,
            target: target.clone(),
            before: Some(before),
            after: None,
            archived_to: Some(archived_to),
            proposal_id: Some(proposal.id.clone()),
            approved,
            reason: proposal.reason.clone(),
            reverts: None,
        };
        self.append(&entry)?;
        Ok(entry)
    }

    /// Id of the rollback that undid `id`, if any.
    pub fn reverted_by(&self, id: &str) -> Result<Option<String>> {
        Ok(self
            .entries()?
            .into_iter()
            .find(|entry| entry.reverts.as_deref() == Some(id))
            .map(|entry| entry.id))
    }

    /// Undo one entry. Refused when the file changed after that entry, so a
    /// rollback never discards a later edit.
    pub fn rollback(&self, id: &str, reason: &str) -> Result<LedgerEntry> {
        let entry = self.get(id)?;
        if entry.action == Action::Rollback {
            return Err(Error::Conflict(
                "a rollback cannot be rolled back; approve the change again instead".into(),
            ));
        }
        if let Some(by) = self.reverted_by(id)? {
            return Err(Error::Conflict(format!("already rolled back by {by}")));
        }
        let (before, after) = if entry.action == Action::Archive {
            self.restore_archive(&entry)?
        } else {
            self.restore_file(&entry)?
        };
        let rollback = LedgerEntry {
            id: new_id(),
            at: OffsetDateTime::now_utc(),
            actor: Actor::User,
            action: Action::Rollback,
            target: entry.target.clone(),
            before,
            after,
            archived_to: None,
            proposal_id: entry.proposal_id.clone(),
            approved: true,
            reason: reason.to_string(),
            reverts: Some(entry.id.clone()),
        };
        self.append(&rollback)?;
        Ok(rollback)
    }

    fn restore_file(&self, entry: &LedgerEntry) -> Result<(Option<String>, Option<String>)> {
        let path = entry.target.resolve(&self.paths)?;
        let current = self.current_sha(&entry.target)?;
        if current != entry.after {
            return Err(Error::Conflict(format!(
                "{} changed after this entry; roll back the later entries first",
                entry.target
            )));
        }
        match &entry.before {
            Some(sha) => {
                let bytes = self.blobs.get(sha)?;
                write_atomic(&path, &bytes)?;
            },
            None => {
                std::fs::remove_file(&path)?;
                remove_empty_skill_dir(&entry.target, &self.paths);
            },
        }
        Ok((current, entry.before.clone()))
    }

    fn restore_archive(&self, entry: &LedgerEntry) -> Result<(Option<String>, Option<String>)> {
        let Some(dir) = entry.target.skill_dir(&self.paths)? else {
            return Err(Error::InvalidTarget("archive entry without a skill".into()));
        };
        if dir.exists() {
            return Err(Error::Conflict(format!(
                "a skill already exists at {}",
                entry.target
            )));
        }
        let archived = entry
            .archived_to
            .as_deref()
            .map(|relative| self.paths.data_dir().join(relative))
            .filter(|path| path.starts_with(self.paths.archive()))
            .ok_or_else(|| Error::NotFound("archived skill location".into()))?;
        if !archived.exists() {
            return Err(Error::NotFound(format!("{}", archived.display())));
        }
        if let Some(parent) = dir.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(&archived, &dir)?;
        Ok((None, entry.before.clone()))
    }
}

fn remove_empty_skill_dir(target: &Target, paths: &LearningPaths) {
    let Ok(Some(dir)) = target.skill_dir(paths) else {
        return;
    };
    let mut current: Option<PathBuf> = target
        .resolve(paths)
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));
    while let Some(path) = current {
        if !path.starts_with(&dir) || std::fs::remove_dir(&path).is_err() {
            break;
        }
        current = path.parent().map(Path::to_path_buf);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use {super::*, crate::change::Origin};

    fn setup() -> (tempfile::TempDir, Ledger, LearningPaths) {
        let tmp = tempfile::tempdir().unwrap();
        let paths = LearningPaths::new(tmp.path());
        (tmp, Ledger::new(paths.clone()), paths)
    }

    fn proposal(ledger: &Ledger, target: Target, change: Change) -> Proposal {
        let mut proposal = Proposal::new(Origin::Curator, target, change, "test");
        proposal.base_sha256 = ledger.current_sha(&proposal.target).unwrap();
        proposal
    }

    const SKILL: &str = "---\nname: git-flow\ndescription: Git flow\n---\nRebase, never merge.\n";

    #[test]
    fn create_patch_and_roll_back_one_edit() {
        let (_tmp, ledger, paths) = setup();
        let create = ledger
            .apply(
                &proposal(&ledger, Target::skill("git-flow"), Change::Create {
                    content: SKILL.into(),
                }),
                Actor::Review,
                false,
            )
            .unwrap();
        assert_eq!(create.before, None);
        let patch = ledger
            .apply(
                &proposal(&ledger, Target::skill("git-flow"), Change::Replace {
                    old: "never merge".into(),
                    new: "never merge; push with -u".into(),
                }),
                Actor::Review,
                true,
            )
            .unwrap();
        let memory = ledger
            .apply(
                &proposal(&ledger, Target::memory("MEMORY.md"), Change::Append {
                    text: "- CI runs on nix".into(),
                }),
                Actor::Review,
                false,
            )
            .unwrap();
        let skill_path = paths.skills_dir().join("git-flow/SKILL.md");

        // Undo only the patch: the create and the memory note stay.
        let undo = ledger.rollback(&patch.id, "too specific").unwrap();
        assert_eq!(undo.reverts.as_deref(), Some(patch.id.as_str()));
        assert_eq!(std::fs::read_to_string(&skill_path).unwrap(), SKILL);
        assert!(paths.data_dir().join("MEMORY.md").exists());
        assert!(ledger.rollback(&patch.id, "again").is_err());
        assert!(ledger.rollback(&undo.id, "redo").is_err());
        assert_eq!(ledger.reverted_by(&patch.id).unwrap(), Some(undo.id));

        // Undoing the create removes the file and the empty directory.
        ledger.rollback(&create.id, "unwanted").unwrap();
        assert!(!paths.skills_dir().join("git-flow").exists());
        assert_eq!(ledger.entries().unwrap().len(), 5);
        assert_eq!(ledger.get(&memory.id).unwrap().action, Action::Append);
    }

    #[test]
    fn rollback_refuses_to_discard_a_later_edit() {
        let (_tmp, ledger, paths) = setup();
        let first = ledger
            .apply(
                &proposal(&ledger, Target::memory("MEMORY.md"), Change::Append {
                    text: "- one".into(),
                }),
                Actor::Review,
                false,
            )
            .unwrap();
        std::fs::write(paths.data_dir().join("MEMORY.md"), "hand edit\n").unwrap();
        let error = ledger.rollback(&first.id, "x").unwrap_err();
        assert!(matches!(error, Error::Conflict(_)), "{error}");
    }

    #[test]
    fn stale_proposals_are_refused() {
        let (_tmp, ledger, paths) = setup();
        let stale = proposal(&ledger, Target::memory("MEMORY.md"), Change::Append {
            text: "- x".into(),
        });
        std::fs::write(paths.data_dir().join("MEMORY.md"), "changed\n").unwrap();
        assert!(matches!(
            ledger.apply(&stale, Actor::Review, false),
            Err(Error::StaleTarget(_))
        ));
        assert!(ledger.entries().unwrap().is_empty());
    }

    #[test]
    fn support_files_need_an_existing_skill() {
        let (_tmp, ledger, _paths) = setup();
        let target = Target::Skill {
            name: "ghost".into(),
            file: "references/x.md".into(),
        };
        let error = ledger
            .apply(
                &proposal(&ledger, target, Change::Create {
                    content: "x".into(),
                }),
                Actor::Review,
                false,
            )
            .unwrap_err();
        assert!(matches!(error, Error::Conflict(_)));
    }

    #[test]
    fn archive_and_restore_a_whole_skill() {
        let (_tmp, ledger, paths) = setup();
        let dir = paths.skills_dir().join("old-skill");
        std::fs::create_dir_all(dir.join("references")).unwrap();
        std::fs::write(dir.join("SKILL.md"), SKILL).unwrap();
        std::fs::write(dir.join("references/a.md"), "a").unwrap();
        let archived = ledger
            .apply(
                &Proposal::new(
                    Origin::Curator,
                    Target::skill("old-skill"),
                    Change::Archive,
                    "unused for 30 days",
                ),
                Actor::Curator,
                false,
            )
            .unwrap();
        assert!(!dir.exists());
        let location = paths
            .data_dir()
            .join(archived.archived_to.as_deref().unwrap());
        assert!(location.join("references/a.md").exists());
        ledger.rollback(&archived.id, "still needed").unwrap();
        assert!(dir.join("references/a.md").exists());
        assert!(!location.exists());
    }

    #[test]
    fn corrupt_lines_are_skipped() {
        let (_tmp, ledger, paths) = setup();
        ledger
            .apply(
                &proposal(&ledger, Target::memory("MEMORY.md"), Change::Append {
                    text: "- a".into(),
                }),
                Actor::Review,
                false,
            )
            .unwrap();
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(paths.ledger())
            .unwrap();
        file.write_all(b"{not json\n\n").unwrap();
        assert_eq!(ledger.entries().unwrap().len(), 1);
        assert!(matches!(ledger.get("nope"), Err(Error::NotFound(_))));
    }
}
