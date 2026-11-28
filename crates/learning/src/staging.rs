//! Write-approval staging: proposals wait as one JSON file each under
//! `learning/pending/` until someone approves or rejects them. Rejections
//! are remembered so the review does not propose the same thing again.

use std::path::PathBuf;

use {
    serde::{Deserialize, Serialize},
    time::OffsetDateTime,
};

use crate::{
    change::Proposal,
    error::{Error, Result},
    macros::log_warn,
    paths::{LearningPaths, read_optional, write_atomic},
    target::Target,
};

const REJECTION_SUMMARY_CHARS: usize = 240;

/// A rejected proposal, kept as a hint for later reviews.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rejection {
    pub proposal_id: String,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    pub target: Target,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// The staging area.
#[derive(Debug, Clone)]
pub struct Staging {
    paths: LearningPaths,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

impl Staging {
    #[must_use]
    pub fn new(paths: LearningPaths) -> Self {
        Self { paths }
    }

    fn path(&self, id: &str) -> Result<PathBuf> {
        if !valid_id(id) {
            return Err(Error::NotFound(format!("pending change {id}")));
        }
        Ok(self.paths.pending().join(format!("{id}.json")))
    }

    /// Stage a proposal. Returns `false` when the same edit is already pending.
    pub fn stage(&self, proposal: &Proposal) -> Result<bool> {
        if self
            .list()?
            .iter()
            .any(|pending| pending.same_edit(proposal))
        {
            return Ok(false);
        }
        write_atomic(
            &self.path(&proposal.id)?,
            &serde_json::to_vec_pretty(proposal)?,
        )?;
        Ok(true)
    }

    /// Pending proposals, oldest first. Unreadable files are skipped.
    pub fn list(&self) -> Result<Vec<Proposal>> {
        let dir = self.paths.pending();
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut proposals: Vec<Proposal> = entries
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .filter_map(|path| {
                let bytes = std::fs::read(&path).ok()?;
                serde_json::from_slice(&bytes)
                    .inspect_err(|_error| log_warn!(error = %_error, path = %path.display(), "skipping unreadable pending change"))
                    .ok()
            })
            .collect();
        proposals.sort_by_key(|proposal| proposal.created_at);
        Ok(proposals)
    }

    pub fn get(&self, id: &str) -> Result<Proposal> {
        let bytes = read_optional(&self.path(id)?)?
            .ok_or_else(|| Error::NotFound(format!("pending change {id}")))?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// Drop a proposal once it has been applied.
    pub fn remove(&self, id: &str) -> Result<()> {
        match std::fs::remove_file(self.path(id)?) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Err(Error::NotFound(format!("pending change {id}")))
            },
            Err(error) => Err(error.into()),
        }
    }

    /// Reject a proposal and remember it.
    pub fn reject(&self, id: &str, note: Option<String>) -> Result<Rejection> {
        let proposal = self.get(id)?;
        let summary: String = format!(
            "{} {}: {}",
            proposal.change.verb(),
            proposal.target,
            proposal.change.added_text().replace('\n', " ")
        )
        .chars()
        .take(REJECTION_SUMMARY_CHARS)
        .collect();
        let rejection = Rejection {
            proposal_id: proposal.id.clone(),
            at: OffsetDateTime::now_utc(),
            target: proposal.target,
            summary,
            note,
        };
        let mut line = serde_json::to_vec(&rejection)?;
        line.push(b'\n');
        let path = self.paths.rejected();
        let mut existing = read_optional(&path)?.unwrap_or_default();
        existing.extend_from_slice(&line);
        write_atomic(&path, &existing)?;
        self.remove(id)?;
        Ok(rejection)
    }

    /// The most recent rejections, newest last.
    pub fn recent_rejections(&self, limit: usize) -> Result<Vec<Rejection>> {
        let Some(bytes) = read_optional(&self.paths.rejected())? else {
            return Ok(Vec::new());
        };
        let all: Vec<Rejection> = bytes
            .split(|byte| *byte == b'\n')
            .filter_map(|line| serde_json::from_slice(line).ok())
            .collect();
        let skip = all.len().saturating_sub(limit);
        Ok(all.into_iter().skip(skip).collect())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use {
        super::*,
        crate::change::{Change, Origin},
    };

    fn proposal(text: &str) -> Proposal {
        Proposal::new(
            Origin::Review {
                session_key: "main".into(),
            },
            Target::memory("MEMORY.md"),
            Change::Append { text: text.into() },
            "why",
        )
    }

    #[test]
    fn stage_dedupes_lists_and_removes() {
        let tmp = tempfile::tempdir().unwrap();
        let staging = Staging::new(LearningPaths::new(tmp.path()));
        let first = proposal("- a");
        assert!(staging.stage(&first).unwrap());
        assert!(!staging.stage(&proposal("- a")).unwrap());
        assert!(staging.stage(&proposal("- b")).unwrap());
        assert_eq!(staging.list().unwrap().len(), 2);
        assert_eq!(staging.get(&first.id).unwrap(), first);
        staging.remove(&first.id).unwrap();
        assert!(matches!(staging.remove(&first.id), Err(Error::NotFound(_))));
        assert!(matches!(staging.get("../../x"), Err(Error::NotFound(_))));
    }

    #[test]
    fn rejections_are_remembered() {
        let tmp = tempfile::tempdir().unwrap();
        let staging = Staging::new(LearningPaths::new(tmp.path()));
        for text in ["- one", "- two", "- three"] {
            let pending = proposal(text);
            staging.stage(&pending).unwrap();
            staging.reject(&pending.id, Some("noise".into())).unwrap();
        }
        assert!(staging.list().unwrap().is_empty());
        let recent = staging.recent_rejections(2).unwrap();
        assert_eq!(recent.len(), 2);
        assert!(recent[1].summary.contains("- three"));
        assert_eq!(recent[0].note.as_deref(), Some("noise"));
    }

    #[test]
    fn unreadable_pending_files_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = LearningPaths::new(tmp.path());
        std::fs::create_dir_all(paths.pending()).unwrap();
        std::fs::write(paths.pending().join("bad.json"), "{").unwrap();
        std::fs::write(paths.pending().join("note.txt"), "x").unwrap();
        assert!(Staging::new(paths).list().unwrap().is_empty());
    }
}
