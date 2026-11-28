//! What a learned change may touch: one file of a personal skill, or one
//! memory file. Anything else is refused before it reaches the disk.

use std::{
    fmt,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    error::{Error, Result},
    paths::LearningPaths,
};

/// The skill entry point.
pub const SKILL_FILE: &str = "SKILL.md";
/// Prompt-visible memory: environment facts and conventions.
pub const MEMORY_FILE: &str = "MEMORY.md";
/// Searchable note for user preferences. `USER.md` is a managed profile
/// surface written from settings, so the loop keeps out of it.
pub const USER_NOTES_FILE: &str = "memory/user-preferences.md";

const SUPPORT_DIRS: [&str; 3] = ["references", "templates", "scripts"];
const MAX_FILE_NAME: usize = 64;

/// A file the learning loop may change.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    /// `skills/<name>/<file>`: `SKILL.md` or `references|templates|scripts/<file>`.
    Skill {
        name: String,
        #[serde(default = "default_skill_file")]
        file: String,
    },
    /// `MEMORY.md` or `memory/<name>.md`.
    Memory { file: String },
}

fn default_skill_file() -> String {
    SKILL_FILE.to_string()
}

impl Target {
    #[must_use]
    pub fn skill(name: impl Into<String>) -> Self {
        Self::Skill {
            name: name.into(),
            file: default_skill_file(),
        }
    }

    #[must_use]
    pub fn memory(file: impl Into<String>) -> Self {
        Self::Memory { file: file.into() }
    }

    /// The skill this target belongs to, if any.
    #[must_use]
    pub fn skill_name(&self) -> Option<&str> {
        match self {
            Self::Skill { name, .. } => Some(name),
            Self::Memory { .. } => None,
        }
    }

    #[must_use]
    pub fn is_skill_entry(&self) -> bool {
        matches!(self, Self::Skill { file, .. } if file == SKILL_FILE)
    }

    /// Check the shape of the target without touching the disk.
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Skill { name, file } => {
                validate_skill_name(name)?;
                validate_skill_file(file)
            },
            Self::Memory { file } => validate_memory_file(file),
        }
    }

    /// Path relative to `data_dir`, for display and the ledger.
    pub fn relative_path(&self) -> Result<PathBuf> {
        self.validate()?;
        Ok(match self {
            Self::Skill { name, file } => Path::new("skills").join(name).join(file),
            Self::Memory { file } => PathBuf::from(file),
        })
    }

    /// Absolute path of the file.
    pub fn resolve(&self, paths: &LearningPaths) -> Result<PathBuf> {
        Ok(paths.data_dir().join(self.relative_path()?))
    }

    /// Directory of the skill, for whole-skill operations.
    pub fn skill_dir(&self, paths: &LearningPaths) -> Result<Option<PathBuf>> {
        self.validate()?;
        Ok(self.skill_name().map(|name| paths.skills_dir().join(name)))
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Skill { name, file } => write!(f, "skills/{name}/{file}"),
            Self::Memory { file } => f.write_str(file),
        }
    }
}

/// Personal skill directories use plain names: no `ns:` prefixes.
pub fn validate_skill_name(name: &str) -> Result<()> {
    if antre_skills::parse::validate_name(name) && !name.contains(':') {
        return Ok(());
    }
    Err(Error::InvalidTarget(format!(
        "skill name '{name}' must be 1-64 lowercase letters, digits or hyphens"
    )))
}

fn validate_file_name(name: &str, allowed_ext: &[&str]) -> bool {
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return false;
    };
    !stem.is_empty()
        && name.len() <= MAX_FILE_NAME
        && allowed_ext.contains(&ext)
        && stem
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        && !stem.starts_with('.')
}

fn validate_skill_file(file: &str) -> Result<()> {
    if file == SKILL_FILE {
        return Ok(());
    }
    let path = Path::new(file);
    let parts: Vec<_> = path.components().collect();
    let valid = match parts.as_slice() {
        [Component::Normal(dir), Component::Normal(name)] => {
            let dir = dir.to_string_lossy();
            let name = name.to_string_lossy();
            SUPPORT_DIRS.contains(&dir.as_ref())
                && validate_file_name(&name, &[
                    "md", "txt", "json", "yaml", "yml", "toml", "sh", "py", "js", "ts",
                ])
        },
        _ => false,
    };
    if valid && !file.contains('\\') {
        return Ok(());
    }
    Err(Error::InvalidTarget(format!(
        "skill file '{file}' must be SKILL.md or references|templates|scripts/<name>.<ext>"
    )))
}

fn validate_memory_file(file: &str) -> Result<()> {
    if file == MEMORY_FILE {
        return Ok(());
    }
    let valid = file
        .strip_prefix("memory/")
        .is_some_and(|name| !name.contains('/') && validate_file_name(name, &["md"]));
    if valid {
        return Ok(());
    }
    Err(Error::InvalidTarget(format!(
        "memory file '{file}' must be MEMORY.md or memory/<name>.md"
    )))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn valid_targets_resolve_under_data_dir() {
        let paths = LearningPaths::new("/data");
        assert_eq!(
            Target::skill("git-flow").resolve(&paths).unwrap(),
            PathBuf::from("/data/skills/git-flow/SKILL.md")
        );
        let support = Target::Skill {
            name: "git-flow".into(),
            file: "references/rebase.md".into(),
        };
        assert_eq!(
            support.resolve(&paths).unwrap(),
            PathBuf::from("/data/skills/git-flow/references/rebase.md")
        );
        assert_eq!(
            Target::memory(USER_NOTES_FILE).resolve(&paths).unwrap(),
            PathBuf::from("/data/memory/user-preferences.md")
        );
        assert_eq!(Target::memory(MEMORY_FILE).to_string(), "MEMORY.md");
    }

    #[test]
    fn escapes_and_odd_names_are_refused() {
        for target in [
            Target::skill("../etc"),
            Target::skill("ns:skill"),
            Target::skill("Bad"),
            Target::Skill {
                name: "ok".into(),
                file: "../SKILL.md".into(),
            },
            Target::Skill {
                name: "ok".into(),
                file: "references/../../x.md".into(),
            },
            Target::Skill {
                name: "ok".into(),
                file: "/etc/passwd".into(),
            },
            Target::Skill {
                name: "ok".into(),
                file: "notes/x.md".into(),
            },
            Target::Skill {
                name: "ok".into(),
                file: "references/deep/x.md".into(),
            },
            Target::Skill {
                name: "ok".into(),
                file: "references/x.exe".into(),
            },
            Target::Skill {
                name: "ok".into(),
                file: "references/.hidden.md".into(),
            },
            Target::memory("USER.md"),
            Target::memory("memory/a/b.md"),
            Target::memory("memory/../MEMORY.md"),
            Target::memory("memory/x.txt"),
        ] {
            assert!(target.validate().is_err(), "{target:?} should be refused");
        }
    }

    #[test]
    fn serde_shape_is_tagged() {
        let target: Target = serde_json::from_str(r#"{"kind":"skill","name":"git-flow"}"#).unwrap();
        assert_eq!(target, Target::skill("git-flow"));
        let memory: Target =
            serde_json::from_str(r#"{"kind":"memory","file":"MEMORY.md"}"#).unwrap();
        assert!(memory.skill_name().is_none());
        assert!(target.is_skill_entry());
    }
}
