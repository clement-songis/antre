//! When a session is due for review, and what the review reads from disk.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use {
    antre_config::LearningReviewConfig,
    serde::{Deserialize, Serialize},
    time::OffsetDateTime,
};

use crate::{
    paths::LearningPaths,
    review_digest::{Turn, activity},
    review_prompt::{MemoryBrief, SkillBrief},
    target::{MEMORY_FILE, SKILL_FILE, USER_NOTES_FILE},
};

const MEMORY_BRIEF_CHARS: usize = 6_000;
const LOADED_SKILL_CHARS: usize = 16_000;

/// Review progress of one session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionReview {
    /// History length at the last review.
    pub reviewed_turns: usize,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
}

/// Persisted review progress (`learning/review-state.json`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewState {
    #[serde(default)]
    pub sessions: HashMap<String, SessionReview>,
}

impl ReviewState {
    /// Turns added since the last review. A history shorter than the mark
    /// was reset or compacted, so all of it counts.
    #[must_use]
    pub fn unreviewed<'a>(&self, session_key: &str, turns: &'a [Turn]) -> &'a [Turn] {
        let mark = self
            .sessions
            .get(session_key)
            .map_or(0, |review| review.reviewed_turns);
        if mark > turns.len() {
            return turns;
        }
        &turns[mark..]
    }

    pub fn mark(&mut self, session_key: &str, turns: usize, now: OffsetDateTime) {
        self.sessions
            .insert(session_key.to_string(), SessionReview {
                reviewed_turns: turns,
                at: now,
            });
    }
}

/// Whether the unreviewed part of a session crosses a threshold.
#[must_use]
pub fn is_due(config: &LearningReviewConfig, unreviewed: &[Turn]) -> bool {
    let (users, tools) = activity(unreviewed);
    let users_due = config.every_user_turns > 0 && users >= config.every_user_turns as usize;
    let tools_due = config.every_tool_calls > 0 && tools >= config.every_tool_calls as usize;
    users_due || tools_due
}

fn truncate(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((cut, _)) => format!("{}\n…(truncated)", &text[..cut]),
        None => text.to_string(),
    }
}

/// A personal skill on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonalSkill {
    pub name: String,
    pub description: String,
    pub skill_md: String,
}

/// Personal skills under `skills/`, sorted by name. Unparseable ones are
/// skipped.
#[must_use]
pub fn personal_skills(paths: &LearningPaths) -> Vec<PersonalSkill> {
    let Ok(entries) = std::fs::read_dir(paths.skills_dir()) else {
        return Vec::new();
    };
    let mut skills: Vec<PersonalSkill> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let dir = entry.path();
            let name = dir.file_name()?.to_string_lossy().into_owned();
            let skill_md = std::fs::read_to_string(dir.join(SKILL_FILE)).ok()?;
            let meta = antre_skills::parse::parse_metadata(&skill_md, &dir).ok()?;
            (meta.name == name).then_some(PersonalSkill {
                name,
                description: meta.description,
                skill_md,
            })
        })
        .collect();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// Skill briefs for the review: every personal skill in the index, full
/// text for the ones loaded this session.
#[must_use]
pub fn skill_briefs(skills: &[PersonalSkill], loaded: &HashSet<String>) -> Vec<SkillBrief> {
    skills
        .iter()
        .map(|skill| SkillBrief {
            name: skill.name.clone(),
            description: skill.description.clone(),
            body: loaded
                .contains(&skill.name)
                .then(|| truncate(&skill.skill_md, LOADED_SKILL_CHARS)),
        })
        .collect()
}

fn read_brief(data_dir: &Path, file: &str) -> Option<MemoryBrief> {
    let content = std::fs::read_to_string(data_dir.join(file)).ok()?;
    Some(MemoryBrief {
        file: file.to_string(),
        content: truncate(&content, MEMORY_BRIEF_CHARS),
    })
}

/// The memory files the review may append to.
#[must_use]
pub fn memory_briefs(paths: &LearningPaths) -> Vec<MemoryBrief> {
    [MEMORY_FILE, USER_NOTES_FILE]
        .iter()
        .filter_map(|file| read_brief(paths.data_dir(), file))
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use {super::*, time::macros::datetime};

    fn turns(users: usize, tools_per_turn: usize) -> Vec<Turn> {
        (0..users)
            .flat_map(|index| {
                [
                    Turn::user(format!("q{index}")),
                    Turn::assistant("a", vec!["exec".into(); tools_per_turn]),
                ]
            })
            .collect()
    }

    #[test]
    fn thresholds() {
        let config = LearningReviewConfig::default();
        assert!(!is_due(&config, &turns(9, 0)));
        assert!(is_due(&config, &turns(10, 0)));
        assert!(is_due(&config, &turns(2, 5)));
        let users_only = LearningReviewConfig {
            every_tool_calls: 0,
            ..config.clone()
        };
        assert!(!is_due(&users_only, &turns(2, 50)));
        let never = LearningReviewConfig {
            every_tool_calls: 0,
            every_user_turns: 0,
            ..config
        };
        assert!(!is_due(&never, &turns(100, 100)));
    }

    #[test]
    fn marks_track_unreviewed_turns() {
        let history = turns(4, 0);
        let mut state = ReviewState::default();
        assert_eq!(state.unreviewed("main", &history).len(), 8);
        state.mark("main", 6, datetime!(2026-11-28 0:00 UTC));
        assert_eq!(state.unreviewed("main", &history).len(), 2);
        assert_eq!(state.unreviewed("main", &history[..3]).len(), 3);
        assert_eq!(state.unreviewed("other", &history).len(), 8);
    }

    #[test]
    fn reads_personal_skills_and_memory() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = LearningPaths::new(tmp.path());
        let write = |name: &str, frontmatter_name: &str| {
            let dir = paths.skills_dir().join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("SKILL.md"),
                format!("---\nname: {frontmatter_name}\ndescription: {name} desc\n---\nbody"),
            )
            .unwrap();
        };
        write("beta", "beta");
        write("alpha", "alpha");
        write("mismatch", "other");
        std::fs::create_dir_all(paths.skills_dir().join("empty")).unwrap();
        std::fs::write(tmp.path().join("MEMORY.md"), "- fact").unwrap();

        let skills = personal_skills(&paths);
        let names: Vec<&str> = skills.iter().map(|skill| skill.name.as_str()).collect();
        assert_eq!(names, ["alpha", "beta"]);
        let briefs = skill_briefs(&skills, &HashSet::from(["beta".to_string()]));
        assert!(briefs[0].body.is_none());
        assert!(briefs[1].body.as_deref().unwrap().contains("body"));
        let memory = memory_briefs(&paths);
        assert_eq!(memory.len(), 1);
        assert_eq!(memory[0].file, "MEMORY.md");
        assert!(personal_skills(&LearningPaths::new(tmp.path().join("none"))).is_empty());
    }

    #[test]
    fn truncation_marks_cut() {
        assert_eq!(truncate("abc", 5), "abc");
        assert!(truncate("abcdef", 3).ends_with("…(truncated)"));
    }
}
