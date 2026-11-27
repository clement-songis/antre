//! Design systems as skills.
//!
//! A design system is a skill directory that holds a `DESIGN.md` (guidance
//! injected into the agent's context) and a `tokens.css` (copied into each
//! design that uses it). Directories named `design-system-<name>` are found
//! by `<name>`; any other skill with both files is found by its directory
//! name.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::{Error, Result};

pub const GUIDE_FILE: &str = "DESIGN.md";
pub const TOKENS_FILE: &str = "tokens.css";
const SKILL_FILE: &str = "SKILL.md";
const PREFIX: &str = "design-system-";
/// Largest DESIGN.md / tokens.css accepted.
const MAX_FILE_BYTES: u64 = 256 * 1024;

struct Starter {
    name: &'static str,
    skill: &'static str,
    guide: &'static str,
    tokens: &'static str,
}

const STARTERS: &[Starter] = &[
    Starter {
        name: "minimal",
        skill: include_str!("../design-systems/minimal/SKILL.md"),
        guide: include_str!("../design-systems/minimal/DESIGN.md"),
        tokens: include_str!("../design-systems/minimal/tokens.css"),
    },
    Starter {
        name: "editorial",
        skill: include_str!("../design-systems/editorial/SKILL.md"),
        guide: include_str!("../design-systems/editorial/DESIGN.md"),
        tokens: include_str!("../design-systems/editorial/tokens.css"),
    },
    Starter {
        name: "midnight",
        skill: include_str!("../design-systems/midnight/SKILL.md"),
        guide: include_str!("../design-systems/midnight/DESIGN.md"),
        tokens: include_str!("../design-systems/midnight/tokens.css"),
    },
];

/// Summary of an available design system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignSystemInfo {
    pub name: String,
    pub skill: String,
    pub description: Option<String>,
}

/// A design system's files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesignSystem {
    pub info: DesignSystemInfo,
    pub guide: String,
    pub tokens: String,
}

/// Write the starter systems into `skills_dir`, never overwriting files the
/// user may have edited.
pub fn seed_starters(skills_dir: &Path) -> Result<usize> {
    let mut written = 0;
    for starter in STARTERS {
        let dir = skills_dir.join(format!("{PREFIX}{}", starter.name));
        std::fs::create_dir_all(&dir)?;
        for (file, content) in [
            (SKILL_FILE, starter.skill),
            (GUIDE_FILE, starter.guide),
            (TOKENS_FILE, starter.tokens),
        ] {
            let path = dir.join(file);
            if path.exists() {
                continue;
            }
            std::fs::write(path, content)?;
            written += 1;
        }
    }
    Ok(written)
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn public_name(dir_name: &str) -> &str {
    dir_name.strip_prefix(PREFIX).unwrap_or(dir_name)
}

fn read_small(path: &Path) -> Option<String> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

fn description(skill_md: Option<&str>) -> Option<String> {
    let body = skill_md?.strip_prefix("---")?;
    let (front, _) = body.split_once("\n---")?;
    front.lines().find_map(|line| {
        line.strip_prefix("description:")
            .map(|value| value.trim().trim_matches('"').to_string())
            .filter(|value| !value.is_empty())
    })
}

fn load_dir(dir: &Path) -> Option<DesignSystem> {
    let dir_name = dir.file_name()?.to_str()?;
    if !valid_name(dir_name) {
        return None;
    }
    let guide = read_small(&dir.join(GUIDE_FILE))?;
    let tokens = read_small(&dir.join(TOKENS_FILE))?;
    let skill_md = read_small(&dir.join(SKILL_FILE));
    Some(DesignSystem {
        info: DesignSystemInfo {
            name: public_name(dir_name).to_string(),
            skill: dir_name.to_string(),
            description: description(skill_md.as_deref()),
        },
        guide,
        tokens,
    })
}

/// Lists and resolves design systems under a skills directory.
#[derive(Debug, Clone)]
pub struct DesignSystemCatalog {
    skills_dir: PathBuf,
}

impl DesignSystemCatalog {
    #[must_use]
    pub fn new(skills_dir: PathBuf) -> Self {
        Self { skills_dir }
    }

    /// Available design systems, sorted by name.
    pub fn list(&self) -> Vec<DesignSystemInfo> {
        let Ok(entries) = std::fs::read_dir(&self.skills_dir) else {
            return Vec::new();
        };
        let mut found: Vec<_> = entries
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .filter_map(|entry| load_dir(&entry.path()))
            .map(|system| system.info)
            .collect();
        found.sort_by(|left, right| left.name.cmp(&right.name));
        found.dedup_by(|later, earlier| later.name == earlier.name);
        found
    }

    /// Resolve `name` (`minimal` or `design-system-minimal`, or any skill
    /// directory with both files).
    pub fn get(&self, name: &str) -> Result<DesignSystem> {
        let name = name.trim();
        if !valid_name(name) {
            return Err(Error::Invalid(format!(
                "invalid design system name '{name}'"
            )));
        }
        let candidates = [format!("{PREFIX}{}", public_name(name)), name.to_string()];
        candidates
            .iter()
            .find_map(|dir_name| {
                let dir = self.skills_dir.join(dir_name);
                std::fs::symlink_metadata(&dir)
                    .is_ok_and(|meta| meta.is_dir())
                    .then(|| load_dir(&dir))
                    .flatten()
            })
            .ok_or_else(|| {
                let known: Vec<_> = self.list().into_iter().map(|info| info.name).collect();
                Error::Invalid(format!(
                    "unknown design system '{name}' (available: {})",
                    if known.is_empty() {
                        "none".to_string()
                    } else {
                        known.join(", ")
                    }
                ))
            })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn starters_are_complete() {
        assert_eq!(STARTERS.len(), 3);
        for starter in STARTERS {
            assert!(
                starter
                    .skill
                    .contains(&format!("name: {PREFIX}{}", starter.name))
            );
            assert!(
                starter.guide.contains("data-mid"),
                "{} guide mentions data-mid",
                starter.name
            );
            assert!(starter.tokens.contains(":root"));
            assert!(
                !starter.tokens.contains("http"),
                "tokens must not load remote assets"
            );
        }
    }

    #[test]
    fn seed_then_list_and_resolve() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(seed_starters(tmp.path()).unwrap(), 9);
        assert_eq!(
            seed_starters(tmp.path()).unwrap(),
            0,
            "seeding is idempotent"
        );

        let catalog = DesignSystemCatalog::new(tmp.path().to_path_buf());
        let names: Vec<_> = catalog.list().into_iter().map(|info| info.name).collect();
        assert_eq!(names, vec!["editorial", "midnight", "minimal"]);

        let minimal = catalog.get("minimal").unwrap();
        assert_eq!(minimal.info.skill, "design-system-minimal");
        assert!(
            minimal
                .info
                .description
                .unwrap()
                .starts_with("Neutral grayscale")
        );
        assert_eq!(
            catalog.get("design-system-minimal").unwrap().tokens,
            minimal.tokens
        );
    }

    #[test]
    fn seeding_keeps_user_edits() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("design-system-minimal");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(TOKENS_FILE), ":root{--mine:1}").unwrap();
        seed_starters(tmp.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join(TOKENS_FILE)).unwrap(),
            ":root{--mine:1}"
        );
    }

    #[test]
    fn custom_skill_with_both_files_counts() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("acme-brand");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(GUIDE_FILE), "# Acme").unwrap();
        std::fs::write(dir.join(TOKENS_FILE), ":root{}").unwrap();
        std::fs::create_dir_all(tmp.path().join("plain-skill")).unwrap();
        std::fs::write(tmp.path().join("plain-skill/SKILL.md"), "---\nname: x\n---").unwrap();

        let catalog = DesignSystemCatalog::new(tmp.path().to_path_buf());
        assert_eq!(catalog.list().len(), 1);
        assert_eq!(catalog.get("acme-brand").unwrap().guide, "# Acme");
        assert!(catalog.get("plain-skill").is_err());
    }

    #[test]
    fn rejects_traversal_and_unknown_names() {
        let tmp = tempfile::tempdir().unwrap();
        seed_starters(tmp.path()).unwrap();
        let catalog = DesignSystemCatalog::new(tmp.path().join("."));
        for bad in ["../minimal", "", "a/b", "minimal/../x"] {
            assert!(catalog.get(bad).is_err(), "{bad}");
        }
        let err = catalog.get("brutalist").unwrap_err().to_string();
        assert!(
            err.contains("available: editorial, midnight, minimal"),
            "{err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_files_are_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("skills/linked");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(tmp.path().join("secret"), "secret").unwrap();
        std::os::unix::fs::symlink(tmp.path().join("secret"), dir.join(GUIDE_FILE)).unwrap();
        std::fs::write(dir.join(TOKENS_FILE), ":root{}").unwrap();
        let catalog = DesignSystemCatalog::new(tmp.path().join("skills"));
        assert!(catalog.get("linked").is_err());
    }
}
