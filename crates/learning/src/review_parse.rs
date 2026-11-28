//! Parse the review model's JSON answer into proposals.

use serde::Deserialize;

use crate::{
    change::{Change, Origin, Proposal},
    error::{Error, Result},
    target::Target,
};

#[derive(Debug, Deserialize)]
struct ChangeSpec {
    target: Target,
    #[serde(flatten)]
    change: Change,
    #[serde(default)]
    reason: String,
}

#[derive(Debug, Default, Deserialize)]
struct Answer {
    #[serde(default)]
    changes: Vec<serde_json::Value>,
    #[serde(default)]
    overlaps: Option<String>,
}

/// What a review answer contained.
#[derive(Debug, Default)]
pub struct ParsedAnswer {
    pub proposals: Vec<Proposal>,
    /// Entries that did not parse, with why.
    pub malformed: Vec<String>,
    /// The model's note about overlapping skills, for the curator.
    pub overlaps: Option<String>,
}

/// The JSON object in a model answer: models wrap it in prose or code
/// fences despite the instructions.
fn json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end > start).then(|| &text[start..=end])
}

/// Parse an answer. Each change is parsed on its own so one malformed entry
/// does not discard the rest.
pub fn parse_answer(text: &str, origin: &Origin) -> Result<ParsedAnswer> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(ParsedAnswer::default());
    }
    let object = json_object(trimmed)
        .ok_or_else(|| Error::Response("no JSON object in the answer".into()))?;
    let answer: Answer =
        serde_json::from_str(object).map_err(|error| Error::Response(error.to_string()))?;
    let mut parsed = ParsedAnswer {
        overlaps: answer.overlaps.filter(|note| !note.trim().is_empty()),
        ..ParsedAnswer::default()
    };
    for value in answer.changes {
        match serde_json::from_value::<ChangeSpec>(value) {
            Ok(spec) => parsed.proposals.push(Proposal::new(
                origin.clone(),
                spec.target,
                spec.change,
                spec.reason.trim(),
            )),
            Err(error) => parsed.malformed.push(error.to_string()),
        }
    }
    Ok(parsed)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn origin() -> Origin {
        Origin::Review {
            session_key: "main".into(),
        }
    }

    #[test]
    fn parses_fenced_answers_and_skips_bad_entries() {
        let text = r#"Here you go:
```json
{"changes": [
  {"target": {"kind": "skill", "name": "release-process", "file": "SKILL.md"}, "op": "replace", "old": "a", "new": "b", "reason": "user corrected"},
  {"target": {"kind": "memory", "file": "MEMORY.md"}, "op": "append", "text": "- x", "reason": "fact"},
  {"target": {"kind": "skill", "name": "release-process"}, "op": "create", "content": "---\nname: r\n---\n"},
  {"target": {"kind": "memory", "file": "MEMORY.md"}, "op": "explode"},
  {"op": "append", "text": "no target"}
], "overlaps": "release-process and shipping overlap"}
```"#;
        let parsed = parse_answer(text, &origin()).unwrap();
        assert_eq!(parsed.proposals.len(), 3);
        assert_eq!(parsed.malformed.len(), 2);
        assert_eq!(parsed.proposals[0].reason, "user corrected");
        assert_eq!(parsed.proposals[1].change, Change::Append {
            text: "- x".into()
        });
        assert_eq!(parsed.proposals[2].target, Target::skill("release-process"));
        assert!(parsed.overlaps.unwrap().contains("overlap"));
    }

    #[test]
    fn empty_and_nothing_answers() {
        assert!(parse_answer("", &origin()).unwrap().proposals.is_empty());
        assert!(
            parse_answer(r#"{"changes": []}"#, &origin())
                .unwrap()
                .proposals
                .is_empty()
        );
        assert!(parse_answer("{}", &origin()).unwrap().overlaps.is_none());
        assert!(matches!(
            parse_answer("Nothing to save.", &origin()),
            Err(Error::Response(_))
        ));
        assert!(matches!(
            parse_answer("{not json}", &origin()),
            Err(Error::Response(_))
        ));
    }
}
