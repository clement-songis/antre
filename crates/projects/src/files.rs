//! Fuzzy file search inside a project directory, for `@file` mentions.

use std::path::Path;

use ignore::WalkBuilder;

/// Most files looked at per search, so a huge tree cannot stall a request.
const MAX_WALKED: usize = 50_000;

/// Score `candidate` against `query` (both lowercased): `None` when the
/// query is not a subsequence. Lower is better: consecutive matches,
/// matches in the file name and short paths win.
fn score(candidate: &str, query: &str) -> Option<usize> {
    if query.is_empty() {
        return Some(candidate.len());
    }
    let name_start = candidate.rfind('/').map_or(0, |i| i + 1);
    let mut chars = candidate.char_indices();
    let mut penalty = 0usize;
    let mut last: Option<usize> = None;
    for q in query.chars() {
        let (index, _) = chars.by_ref().find(|(_, c)| *c == q)?;
        if let Some(previous) = last {
            penalty += index - previous - 1;
        } else {
            penalty += index;
        }
        if index < name_start {
            penalty += 2;
        }
        last = Some(index);
    }
    let name_bonus = if candidate[name_start..].contains(query) {
        0
    } else {
        50
    };
    Some(penalty * 4 + name_bonus + candidate.len())
}

/// Files under `root` (gitignore rules respected, hidden files skipped)
/// whose relative path fuzzily matches `query`, best first.
#[must_use]
pub fn search_files(root: &Path, query: &str, limit: usize) -> Vec<String> {
    let query = query.trim().to_lowercase();
    let mut scored: Vec<(usize, String)> = WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .require_git(false)
        .build()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .take(MAX_WALKED)
        .filter_map(|entry| {
            let relative = entry
                .path()
                .strip_prefix(root)
                .ok()?
                .to_string_lossy()
                .replace('\\', "/");
            let rank = score(&relative.to_lowercase(), &query)?;
            Some((rank, relative))
        })
        .collect();
    scored.sort();
    scored
        .into_iter()
        .take(limit)
        .map(|(_, path)| path)
        .collect()
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for path in [
            "src/main.rs",
            "src/chat/handler.rs",
            "docs/chat.md",
            "target/debug/main.rs",
            ".hidden/secret.rs",
        ] {
            let full = dir.path().join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, "x").unwrap();
        }
        std::fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
        dir
    }

    #[test]
    fn finds_files_by_subsequence_and_prefers_name_matches() {
        let dir = tree();
        let results = search_files(dir.path(), "chat", 10);
        assert_eq!(results.first().map(String::as_str), Some("docs/chat.md"));
        assert!(results.contains(&"src/chat/handler.rs".to_string()));
        assert_eq!(search_files(dir.path(), "smr", 10), vec![
            "src/main.rs".to_string()
        ]);
    }

    #[test]
    fn skips_ignored_and_hidden_files_and_honours_the_limit() {
        let dir = tree();
        let all = search_files(dir.path(), "", 100);
        assert!(!all.iter().any(|path| path.starts_with("target/")));
        assert!(!all.iter().any(|path| path.starts_with(".hidden/")));
        assert_eq!(search_files(dir.path(), "", 2).len(), 2);
    }

    #[test]
    fn no_match_returns_nothing() {
        let dir = tree();
        assert!(search_files(dir.path(), "zzz", 10).is_empty());
    }
}
