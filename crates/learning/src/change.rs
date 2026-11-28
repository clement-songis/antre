//! A proposed change and how it turns the current file into the next one.

use {
    serde::{Deserialize, Serialize},
    time::OffsetDateTime,
};

use crate::{
    error::{Error, Result},
    target::Target,
};

/// One edit to one target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Change {
    /// New file; refused when the file exists.
    Create { content: String },
    /// Replace one exact occurrence of `old` with `new`.
    Replace { old: String, new: String },
    /// Append a block at the end of the file, creating it when absent.
    Append { text: String },
    /// Replace the whole content of an existing file (consolidation).
    Rewrite { content: String },
    /// Move a whole personal skill to the archive. Restorable.
    Archive,
}

impl Change {
    /// Short verb for logs and the ledger.
    #[must_use]
    pub fn verb(&self) -> &'static str {
        match self {
            Self::Create { .. } => "create",
            Self::Replace { .. } => "replace",
            Self::Append { .. } => "append",
            Self::Rewrite { .. } => "rewrite",
            Self::Archive => "archive",
        }
    }

    /// The text this change adds, for the guard.
    #[must_use]
    pub fn added_text(&self) -> &str {
        match self {
            Self::Create { content } | Self::Rewrite { content } => content,
            Self::Replace { new, .. } => new,
            Self::Append { text } => text,
            Self::Archive => "",
        }
    }

    /// Compute the next content of a file from its current content. Not
    /// meaningful for [`Change::Archive`], which moves a directory.
    pub fn apply(&self, current: Option<&str>) -> Result<String> {
        match (self, current) {
            (Self::Create { content }, None) => Ok(content.clone()),
            (Self::Create { .. }, Some(_)) => {
                Err(Error::Conflict("the file already exists".into()))
            },
            (Self::Replace { old, new }, Some(text)) => replace_once(text, old, new),
            (Self::Append { text }, current) => Ok(append_block(current.unwrap_or_default(), text)),
            (Self::Rewrite { content }, Some(_)) => Ok(content.clone()),
            (Self::Replace { .. } | Self::Rewrite { .. }, None) => {
                Err(Error::Conflict("the file does not exist".into()))
            },
            (Self::Archive, _) => Err(Error::Conflict(
                "archive moves a skill directory; it has no file content".into(),
            )),
        }
    }
}

fn replace_once(text: &str, old: &str, new: &str) -> Result<String> {
    if old.is_empty() {
        return Err(Error::Conflict("the text to replace is empty".into()));
    }
    match text.matches(old).count() {
        1 => Ok(text.replacen(old, new, 1)),
        0 => {
            // Models often copy text with normalized line endings.
            let crlf = old.replace('\n', "\r\n");
            if old != crlf && text.matches(crlf.as_str()).count() == 1 {
                return Ok(text.replacen(crlf.as_str(), &new.replace('\n', "\r\n"), 1));
            }
            Err(Error::Conflict("the text to replace was not found".into()))
        },
        count => Err(Error::Conflict(format!(
            "the text to replace occurs {count} times; it must be unique"
        ))),
    }
}

fn append_block(current: &str, text: &str) -> String {
    let block = text.trim_matches('\n');
    if current.trim().is_empty() {
        return format!("{block}\n");
    }
    let separator = if current.ends_with("\n\n") {
        ""
    } else if current.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    format!("{current}{separator}{block}\n")
}

/// Who produced a proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Origin {
    /// Post-conversation review of a session.
    Review { session_key: String },
    /// Deterministic aging by the curator.
    Curator,
    /// The curator's model-driven consolidation pass.
    Consolidation,
}

/// A change waiting to be applied, staged or refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposal {
    pub id: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    pub origin: Origin,
    pub target: Target,
    pub change: Change,
    /// One line on why, shown in the review panel.
    pub reason: String,
    /// sha256 of the target when the proposal was made; `None` when absent.
    /// Approval refuses a proposal whose target changed since.
    #[serde(default)]
    pub base_sha256: Option<String>,
    /// Proposals that only make sense together (a merge and its archives).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

impl Proposal {
    #[must_use]
    pub fn new(origin: Origin, target: Target, change: Change, reason: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().simple().to_string(),
            created_at: OffsetDateTime::now_utc(),
            origin,
            target,
            change,
            reason: reason.into(),
            base_sha256: None,
            group: None,
        }
    }

    /// Same target and same edit, ignoring ids and timestamps.
    #[must_use]
    pub fn same_edit(&self, other: &Self) -> bool {
        self.target == other.target && self.change == other.change
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn create_refuses_existing_files() {
        let change = Change::Create {
            content: "a".into(),
        };
        assert_eq!(change.apply(None).unwrap(), "a");
        assert!(change.apply(Some("x")).is_err());
    }

    #[test]
    fn replace_needs_one_exact_match() {
        let change = Change::Replace {
            old: "two".into(),
            new: "2".into(),
        };
        assert_eq!(change.apply(Some("one two three")).unwrap(), "one 2 three");
        assert!(change.apply(Some("one three")).is_err());
        assert!(change.apply(Some("two two")).is_err());
        assert!(change.apply(None).is_err());
        let empty = Change::Replace {
            old: String::new(),
            new: "x".into(),
        };
        assert!(empty.apply(Some("abc")).is_err());
    }

    #[test]
    fn replace_tolerates_crlf_files() {
        let change = Change::Replace {
            old: "a\nb".into(),
            new: "a\nc".into(),
        };
        assert_eq!(
            change.apply(Some("x\r\na\r\nb\r\n")).unwrap(),
            "x\r\na\r\nc\r\n"
        );
    }

    #[test]
    fn append_separates_blocks() {
        let change = Change::Append {
            text: "\n- new fact\n".into(),
        };
        assert_eq!(change.apply(None).unwrap(), "- new fact\n");
        assert_eq!(
            change.apply(Some("# Notes")).unwrap(),
            "# Notes\n\n- new fact\n"
        );
        assert_eq!(
            change.apply(Some("# Notes\n")).unwrap(),
            "# Notes\n\n- new fact\n"
        );
        assert_eq!(
            change.apply(Some("# Notes\n\n")).unwrap(),
            "# Notes\n\n- new fact\n"
        );
    }

    #[test]
    fn rewrite_and_archive() {
        let rewrite = Change::Rewrite {
            content: "new".into(),
        };
        assert_eq!(rewrite.apply(Some("old")).unwrap(), "new");
        assert!(rewrite.apply(None).is_err());
        assert!(Change::Archive.apply(Some("x")).is_err());
        assert_eq!(Change::Archive.added_text(), "");
        assert_eq!(Change::Archive.verb(), "archive");
    }

    #[test]
    fn proposal_round_trips_and_compares_edits() {
        let proposal = Proposal::new(
            Origin::Review {
                session_key: "main".into(),
            },
            Target::memory("MEMORY.md"),
            Change::Append { text: "x".into() },
            "why",
        );
        let json = serde_json::to_string(&proposal).unwrap();
        let back: Proposal = serde_json::from_str(&json).unwrap();
        assert_eq!(back, proposal);
        let mut other = proposal.clone();
        other.id = "other".into();
        assert!(proposal.same_edit(&other));
        assert!(json.contains(r#""op":"append""#));
    }
}
