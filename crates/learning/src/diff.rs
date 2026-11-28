//! Unified diffs for the review panel.

use similar::TextDiff;

/// A unified diff between two versions of a file, labelled with its path.
#[must_use]
pub fn unified(path: &str, before: &str, after: &str) -> String {
    TextDiff::from_lines(before, after)
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{path}"), &format!("b/{path}"))
        .to_string()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn diff_shows_changed_lines() {
        let diff = unified("MEMORY.md", "a\nb\n", "a\nc\n");
        assert!(diff.contains("--- a/MEMORY.md"));
        assert!(diff.contains("+++ b/MEMORY.md"));
        assert!(diff.contains("-b\n"));
        assert!(diff.contains("+c\n"));
        assert!(unified("x", "same\n", "same\n").is_empty());
    }
}
