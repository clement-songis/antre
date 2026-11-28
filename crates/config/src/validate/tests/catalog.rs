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

#[test]
fn static_custom_provider_fields_validate_cleanly() {
    let result = validate_toml_str(
        r#"
[providers.custom-my-vllm]
base_url = "http://gpu-box:8000/v1"
catalog = "live+static"
models_file = "/nix/store/x-greenpt-models.json"
headers = { "User-Agent" = "claude-code/2.1.280" }
probe_context_window = true
models = [
  "kimi-k3",
  { id = "qwen3.8-27b", display_name = "Qwen 3.8 27B", context_window = 131072, maxTokens = 16384, reasoning = ["low", "high"] },
]
"#,
    );
    assert!(
        !result
            .diagnostics
            .iter()
            .any(|d| d.path.starts_with("providers.custom-my-vllm")),
        "unexpected diagnostics: {:?}",
        result.diagnostics
    );
    let config: MoltisConfig = toml::from_str(
        r#"
[providers.custom-my-vllm]
catalog = "static"
models = ["a", { id = "b", contextWindow = 4096 }]
"#,
    )
    .unwrap();
    let entry = &config.providers.providers["custom-my-vllm"];
    assert_eq!(entry.catalog, moltis_model_catalog::CatalogMode::Static);
    assert_eq!(entry.models[0], "a");
    assert_eq!(
        entry.models[1].spec().and_then(|s| s.context_window),
        Some(4096)
    );
}

#[test]
fn delivery_section_is_known_and_validated() {
    let ok = validate_toml_str(
        "[chat.delivery]\nmax_attempts = 5\ninitial_retry_secs = 1\nmax_retry_secs = 60\nattempt_timeout_secs = 10\nretention_days = 3\nannounce_background_tasks = false\n",
    );
    assert!(
        !ok.diagnostics
            .iter()
            .any(|d| d.path.starts_with("chat.delivery"))
    );
    let bad = validate_toml_str(
        "[chat.delivery]\nmax_attempts = 0\ninitial_retry_secs = 0\nmax_retry_secs = 0\nattempt_timeout_secs = 0\n",
    );
    for field in ["max_attempts", "initial_retry_secs", "attempt_timeout_secs"] {
        assert!(
            bad.diagnostics
                .iter()
                .any(|d| d.path == format!("chat.delivery.{field}")),
            "missing warning for {field}"
        );
    }
    let ceiling =
        validate_toml_str("[chat.delivery]\ninitial_retry_secs = 10\nmax_retry_secs = 5\n");
    assert!(
        ceiling
            .diagnostics
            .iter()
            .any(|d| d.path == "chat.delivery.max_retry_secs")
    );
    let defaults = MoltisConfig::default();
    assert_eq!(defaults.chat.delivery.max_attempts, 8);
    assert!(defaults.chat.delivery.announce_background_tasks);
}
