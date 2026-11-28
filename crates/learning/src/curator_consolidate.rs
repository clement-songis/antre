//! Optional consolidation pass: the learning model merges overlapping
//! narrow skills into class-level umbrellas. A merge becomes a group of
//! proposals: rewrite (or create) the umbrella, archive the absorbed skills.

use serde::Deserialize;

use crate::{
    blobs::sha256_hex,
    change::{Change, Origin, Proposal},
    curator::SkillState,
    error::{Error, Result},
    target::Target,
};

const MAX_MERGES: usize = 3;

pub const CONSOLIDATION_SYSTEM_PROMPT: &str = r#"You curate an AI agent's personal skill library. The goal is a library of class-level skills: each SKILL.md carries the always-on procedure and rules for one class of task, with a small set of topical references. Hundreds of narrow skills that each capture one session are a failure of the library, because the agent finds skills by their descriptions.

Find groups of skills that cover the same class of task and merge each group into one umbrella. Merging means distilling: absorbed content becomes rules (an imperative plus one clause of why), the same lesson stated twice becomes one rule, and incident narration, issue numbers, dates and quotes are dropped. Keep every distinct, still-useful rule.

Rules:
- Only merge skills listed below. Pinned or locked skills are not listed.
- Never delete: absorbed skills are archived and stay restorable.
- The umbrella is either one of the listed skills (keep its name) or a new class-level name.
- The umbrella's SKILL.md starts with frontmatter: name and a one-line description naming the class of task.
- At most 3 merges. Propose none when the library is already well shaped.

Answer with one JSON object and nothing else:
{"merges": [{"into": "<umbrella>", "absorb": ["<skill>", ...], "skill_md": "---\nname: <umbrella>\ndescription: ...\n---\n...", "reason": "<one line>"}]}"#;

/// A skill offered to the consolidation pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub name: String,
    pub description: String,
    pub state: SkillState,
    pub body: String,
}

/// The user message listing candidates.
#[must_use]
pub fn user_message(candidates: &[Candidate]) -> String {
    let mut out = String::from("# Personal skills\n");
    for candidate in candidates {
        out.push_str(&format!(
            "\n## {} ({:?}, {} chars)\n{}\n",
            candidate.name,
            candidate.state,
            candidate.body.len(),
            candidate.body.trim()
        ));
    }
    out.push_str("\nAnswer with the JSON object only.");
    out
}

#[derive(Debug, Deserialize)]
struct Merge {
    into: String,
    #[serde(default)]
    absorb: Vec<String>,
    skill_md: String,
    #[serde(default)]
    reason: String,
}

#[derive(Debug, Default, Deserialize)]
struct Answer {
    #[serde(default)]
    merges: Vec<Merge>,
}

/// Turn the model's merges into grouped proposals. Merges naming skills
/// outside the candidate list are dropped.
pub fn parse_merges(text: &str, candidates: &[Candidate]) -> Result<Vec<Proposal>> {
    let start = text.find('{');
    let end = text.rfind('}');
    let (Some(start), Some(end)) = (start, end) else {
        return Err(Error::Response("no JSON object in the answer".into()));
    };
    if end <= start {
        return Err(Error::Response("no JSON object in the answer".into()));
    }
    let answer: Answer = serde_json::from_str(&text[start..=end])
        .map_err(|error| Error::Response(error.to_string()))?;
    let known = |name: &str| candidates.iter().find(|candidate| candidate.name == name);
    let mut proposals = Vec::new();
    for merge in answer.merges.into_iter().take(MAX_MERGES) {
        let absorbed: Vec<&Candidate> = merge
            .absorb
            .iter()
            .filter(|name| **name != merge.into)
            .filter_map(|name| known(name))
            .collect();
        if absorbed.is_empty()
            || absorbed.len()
                != merge
                    .absorb
                    .iter()
                    .filter(|name| **name != merge.into)
                    .count()
        {
            continue;
        }
        let group = uuid::Uuid::new_v4().simple().to_string();
        let reason = if merge.reason.trim().is_empty() {
            format!("merge into {}", merge.into)
        } else {
            merge.reason.trim().to_string()
        };
        let mut umbrella = match known(&merge.into) {
            Some(existing) => {
                let mut proposal = Proposal::new(
                    Origin::Consolidation,
                    Target::skill(&merge.into),
                    Change::Rewrite {
                        content: merge.skill_md.clone(),
                    },
                    reason.clone(),
                );
                proposal.base_sha256 = Some(sha256_hex(existing.body.as_bytes()));
                proposal
            },
            None => Proposal::new(
                Origin::Consolidation,
                Target::skill(&merge.into),
                Change::Create {
                    content: merge.skill_md.clone(),
                },
                reason.clone(),
            ),
        };
        umbrella.group = Some(group.clone());
        proposals.push(umbrella);
        for candidate in absorbed {
            let mut archive = Proposal::new(
                Origin::Consolidation,
                Target::skill(&candidate.name),
                Change::Archive,
                format!("absorbed into {}", merge.into),
            );
            archive.base_sha256 = Some(sha256_hex(candidate.body.as_bytes()));
            archive.group = Some(group.clone());
            proposals.push(archive);
        }
    }
    Ok(proposals)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn candidate(name: &str) -> Candidate {
        Candidate {
            name: name.into(),
            description: format!("{name} things"),
            state: SkillState::Active,
            body: format!("---\nname: {name}\ndescription: x\n---\nbody"),
        }
    }

    #[test]
    fn merges_become_grouped_proposals() {
        let candidates = [
            candidate("git-rebase"),
            candidate("git-push"),
            candidate("docker"),
        ];
        let text = r#"{"merges": [
            {"into": "git-workflow", "absorb": ["git-rebase", "git-push"], "skill_md": "---\nname: git-workflow\ndescription: Git\n---\nrules", "reason": "same class"},
            {"into": "docker", "absorb": ["docker", "ghost"], "skill_md": "x"}
        ]}"#;
        let proposals = parse_merges(text, &candidates).unwrap();
        assert_eq!(proposals.len(), 3);
        assert!(matches!(proposals[0].change, Change::Create { .. }));
        assert!(proposals.iter().all(|p| p.group == proposals[0].group));
        assert_eq!(proposals[1].change, Change::Archive);
        assert_eq!(proposals[2].target, Target::skill("git-push"));
        assert_eq!(
            proposals[1].base_sha256.as_deref(),
            Some(sha256_hex(candidates[0].body.as_bytes()).as_str())
        );
    }

    #[test]
    fn merging_into_an_existing_skill_rewrites_it() {
        let candidates = [candidate("git-workflow"), candidate("git-push")];
        let text = r#"{"merges": [{"into": "git-workflow", "absorb": ["git-workflow", "git-push"], "skill_md": "new"}]}"#;
        let proposals = parse_merges(text, &candidates).unwrap();
        assert_eq!(proposals.len(), 2);
        assert!(matches!(proposals[0].change, Change::Rewrite { .. }));
        assert_eq!(proposals[0].reason, "merge into git-workflow");
    }

    #[test]
    fn bad_answers() {
        assert!(parse_merges("none", &[]).is_err());
        assert!(parse_merges("} {", &[]).is_err());
        assert!(parse_merges(r#"{"merges": []}"#, &[]).unwrap().is_empty());
    }

    #[test]
    fn message_lists_candidates() {
        let message = user_message(&[candidate("git-push")]);
        assert!(message.contains("## git-push (Active"));
        assert!(CONSOLIDATION_SYSTEM_PROMPT.contains("Never delete"));
    }
}
