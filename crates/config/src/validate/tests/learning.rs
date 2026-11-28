use super::*;

fn learning_paths(toml: &str) -> Vec<(Severity, String)> {
    validate_toml_str(toml)
        .diagnostics
        .into_iter()
        .filter(|d| d.path.starts_with("learning"))
        .map(|d| (d.severity, d.path))
        .collect()
}

#[test]
fn learning_section_is_known_and_off_by_default() {
    let paths = learning_paths(
        r#"
[learning]
enabled = true
write_approval = true
model = "anthropic::claude-haiku-5-5"

[learning.review]
enabled = true
every_user_turns = 8
every_tool_calls = 12
max_changes = 2
skills = true
memory = false

[learning.curator]
enabled = true
interval_hours = 48
min_idle_hours = 1.5
stale_after_days = 10
archive_after_days = 40
consolidate = true
"#,
    );
    assert!(
        paths.is_empty(),
        "unexpected learning diagnostics: {paths:?}"
    );
    let defaults = AntreConfig::default().learning;
    assert!(!defaults.enabled);
    assert!(defaults.write_approval);
    assert_eq!(defaults.curator.stale_after_days, 14);
    assert_eq!(defaults.curator.archive_after_days, 30);
    assert!(!defaults.curator.consolidate);
}

#[test]
fn learning_typo_is_an_unknown_field() {
    let result = validate_toml_str("[learning.review]\nevery_user_turn = 3\n");
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.category == "unknown-field" && d.path == "learning.review.every_user_turn")
    );
}

#[test]
fn learning_rejects_zero_day_thresholds() {
    let paths = learning_paths(
        "[learning.curator]\nstale_after_days = 0\narchive_after_days = 0\nmin_idle_hours = -1\n",
    );
    for path in [
        "learning.curator.stale_after_days",
        "learning.curator.archive_after_days",
        "learning.curator.min_idle_hours",
    ] {
        assert!(
            paths.contains(&(Severity::Error, path.to_string())),
            "{path} missing from {paths:?}"
        );
    }
}

#[test]
fn learning_warns_on_inverted_or_inert_settings() {
    let paths = learning_paths(
        r#"
[learning]
enabled = true
write_approval = false
model = " "

[learning.review]
every_user_turns = 0
every_tool_calls = 0
max_changes = 0
skills = false
memory = false

[learning.curator]
interval_hours = 0
stale_after_days = 30
archive_after_days = 20
"#,
    );
    for (severity, path) in [
        (Severity::Warning, "learning.curator.archive_after_days"),
        (Severity::Warning, "learning.curator.interval_hours"),
        (Severity::Warning, "learning.review"),
        (Severity::Warning, "learning.review.max_changes"),
        (Severity::Info, "learning.write_approval"),
        (Severity::Warning, "learning.model"),
    ] {
        assert!(
            paths.contains(&(severity, path.to_string())),
            "{path} missing from {paths:?}"
        );
    }
}
