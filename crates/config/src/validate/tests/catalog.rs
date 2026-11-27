use super::*;

#[test]
fn catalog_section_is_known_and_defaults_are_clean() {
    let result = validate_toml_str(
        r#"
[catalog]
refresh = true
refresh_urls = ["https://models.dev/api.json", "https://mirror.example/models.json.gz"]
refresh_interval_hours = 24
"#,
    );
    assert!(
        !result
            .diagnostics
            .iter()
            .any(|d| d.path.starts_with("catalog")),
        "unexpected catalog diagnostics: {:?}",
        result.diagnostics
    );
    let defaults = MoltisConfig::default();
    assert!(defaults.catalog.refresh);
    assert_eq!(defaults.catalog.refresh_urls.len(), 2);
}

#[test]
fn catalog_rejects_non_http_urls() {
    let result = validate_toml_str(
        r#"
[catalog]
refresh_urls = ["file:///etc/passwd"]
"#,
    );
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| { d.severity == Severity::Error && d.path == "catalog.refresh_urls[0]" })
    );
}

#[test]
fn catalog_warns_on_empty_urls_and_zero_interval() {
    let result = validate_toml_str(
        r#"
[catalog]
refresh_urls = []
refresh_interval_hours = 0
"#,
    );
    let paths: Vec<&str> = result.diagnostics.iter().map(|d| d.path.as_str()).collect();
    assert!(paths.contains(&"catalog.refresh_urls"));
    assert!(paths.contains(&"catalog.refresh_interval_hours"));
}

#[test]
fn catalog_unknown_key_is_flagged() {
    let result = validate_toml_str("[catalog]\nrefresh_url = \"https://x\"\n");
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.category == "unknown-field" && d.path == "catalog.refresh_url")
    );
}

#[test]
fn quota_wait_section_is_known_and_validated() {
    let ok = validate_toml_str("[chat.quota_wait]\nenabled = true\nmax_wait_hours = 12\n");
    assert!(
        !ok.diagnostics
            .iter()
            .any(|d| d.path.starts_with("chat.quota_wait"))
    );
    let bad = validate_toml_str("[chat.quota_wait]\nmax_wait_hours = 0\n");
    assert!(
        bad.diagnostics
            .iter()
            .any(|d| d.path == "chat.quota_wait.max_wait_hours")
    );
    let defaults = MoltisConfig::default();
    assert!(defaults.chat.quota_wait.enabled);
    assert!((defaults.chat.quota_wait.max_wait_hours - 6.0).abs() < f64::EPSILON);
}
