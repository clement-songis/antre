use super::*;

#[test]
fn mods_section_is_known_and_defaults_are_sane() {
    let toml = r#"
[mods]
enabled = true
fuel = 1000
memory_mb = 8
timeout_ms = 500
epoch_interval_ms = 10
max_output_kb = 64
pin_shell_hooks = false
"#;
    let result = validate_toml_str(toml);
    assert!(
        !result
            .diagnostics
            .iter()
            .any(|d| d.path.starts_with("mods")),
        "{:?}",
        result.diagnostics
    );
    let defaults = crate::schema::ModsConfig::default();
    assert!(defaults.enabled && defaults.pin_shell_hooks);
    assert!(defaults.epoch_interval_ms < defaults.timeout_ms);
}

#[test]
fn mods_unknown_key_is_reported() {
    let result = validate_toml_str("[mods]\nfuell = 1\n");
    assert!(result.diagnostics.iter().any(|d| d.path == "mods.fuell"));
}

#[test]
fn mods_zero_limits_warn() {
    let result = validate_toml_str("[mods]\nfuel = 0\nepoch_interval_ms = 900\ntimeout_ms = 100\n");
    let paths: Vec<&str> = result
        .diagnostics
        .iter()
        .filter(|d| d.category == "mods")
        .map(|d| d.path.as_str())
        .collect();
    assert!(paths.contains(&"mods.fuel"), "{paths:?}");
    assert!(paths.contains(&"mods.epoch_interval_ms"), "{paths:?}");
}
