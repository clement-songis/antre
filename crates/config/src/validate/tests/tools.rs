use super::*;

#[test]
fn sandbox_mode_off_warned() {
    let toml = r#"
[tools.exec.sandbox]
mode = "off"
"#;
    let result = validate_toml_str(toml);
    let warning = result
        .diagnostics
        .iter()
        .find(|d| d.path == "tools.exec.sandbox.mode");
    assert!(warning.is_some(), "expected warning for sandbox mode off");
}

#[test]
fn port_zero_info() {
    let toml = r#"
[server]
port = 0
"#;
    let result = validate_toml_str(toml);
    let info = result
        .diagnostics
        .iter()
        .find(|d| d.severity == Severity::Info && d.path == "server.port");
    assert!(info.is_some(), "expected info for port 0");
}

#[test]
fn unknown_sandbox_backend_warned() {
    let toml = r#"
[tools.exec.sandbox]
backend = "lxc"
"#;
    let result = validate_toml_str(toml);
    let warning = result
        .diagnostics
        .iter()
        .find(|d| d.path == "tools.exec.sandbox.backend");
    assert!(
        warning.is_some(),
        "expected warning for unknown sandbox backend"
    );
}

#[test]
fn podman_sandbox_backend_accepted() {
    let toml = r#"
[tools.exec.sandbox]
backend = "podman"
"#;
    let result = validate_toml_str(toml);
    let warning = result
        .diagnostics
        .iter()
        .find(|d| d.path == "tools.exec.sandbox.backend");
    assert!(
        warning.is_none(),
        "podman should be accepted as a valid sandbox backend"
    );
}

#[test]
fn podman_escape_hatch_fields_accepted_and_warned() {
    let toml = r#"
[tools.exec.sandbox]
backend = "podman"
allow_nested_podman = true
"#;
    let result = validate_toml_str(toml);

    assert!(
        result
            .diagnostics
            .iter()
            .all(|d| d.category != "unknown-field"),
        "podman escape hatch fields should be accepted"
    );
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.path == "tools.exec.sandbox.allow_nested_podman")
    );
    assert!(
        result
            .diagnostics
            .iter()
            .all(|d| d.severity != Severity::Error)
    );
}

#[cfg(not(target_os = "linux"))]
#[test]
fn host_podman_escape_hatch_rejected_off_linux() {
    let toml = r#"
[tools.exec.sandbox]
backend = "podman"
allow_host_podman = true
"#;
    let result = validate_toml_str(toml);

    assert!(result.diagnostics.iter().any(|d| {
        d.severity == Severity::Error && d.path == "tools.exec.sandbox.allow_host_podman"
    }));
}

#[cfg(target_os = "linux")]
#[test]
fn host_podman_escape_hatch_accepted_on_linux() {
    let toml = r#"
[tools.exec.sandbox]
backend = "podman"
allow_host_podman = true
"#;
    let result = validate_toml_str(toml);

    assert!(
        result
            .diagnostics
            .iter()
            .all(|d| d.severity != Severity::Error)
    );
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.path == "tools.exec.sandbox.allow_host_podman")
    );
}

#[test]
fn podman_escape_hatches_require_podman_backend() {
    let toml = r#"
[tools.exec.sandbox]
backend = "docker"
allow_nested_podman = true
"#;
    let result = validate_toml_str(toml);

    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| { d.severity == Severity::Error && d.path == "tools.exec.sandbox.backend" })
    );
}

#[test]
fn podman_escape_hatches_are_mutually_exclusive() {
    let toml = r#"
[tools.exec.sandbox]
backend = "podman"
allow_host_podman = true
allow_nested_podman = true
"#;
    let result = validate_toml_str(toml);

    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| { d.severity == Severity::Error && d.path == "tools.exec.sandbox" })
    );
}

#[test]
fn managed_files_mount_is_recognized_by_schema_validation() {
    let result = validate_toml_str(
        r#"
[tools.exec.sandbox]
managed_files_mount = "rw"
"#,
    );
    assert!(
        result
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.path != "tools.exec.sandbox.managed_files_mount"),
        "managed_files_mount should be a recognized typed field"
    );
}

#[test]
fn unknown_security_level_warned() {
    let toml = r#"
[tools.exec]
security_level = "paranoid"
"#;
    let result = validate_toml_str(toml);
    let warning = result
        .diagnostics
        .iter()
        .find(|d| d.path == "tools.exec.security_level");
    assert!(
        warning.is_some(),
        "expected warning for unknown security level"
    );
}

#[test]
fn ssh_exec_host_accepted() {
    let toml = r#"
[tools.exec]
host = "ssh"
ssh_target = "deploy@example"
"#;
    let result = validate_toml_str(toml);
    let warning = result
        .diagnostics
        .iter()
        .find(|d| d.path == "tools.exec.host");
    assert!(
        warning.is_none(),
        "ssh should be accepted as a valid exec host"
    );
}

#[test]
fn ssh_exec_host_without_target_warned() {
    let toml = r#"
[tools.exec]
host = "ssh"
"#;
    let result = validate_toml_str(toml);
    let warning = result
        .diagnostics
        .iter()
        .find(|d| d.path == "tools.exec.ssh_target");
    assert!(warning.is_some(), "expected warning for missing ssh target");
}

#[test]
fn browser_obscura_fields_accepted() {
    let toml = r#"
[tools.browser]
obscura_path = "/usr/local/bin/obscura"
obscura_stealth = false
"#;
    let result = validate_toml_str(toml);
    let unknown = result
        .diagnostics
        .iter()
        .find(|d| d.category == "unknown-field" && d.path == "tools.browser.obscura_path");
    assert!(
        unknown.is_none(),
        "obscura_path should be accepted as a browser config field, got: {:?}",
        result.diagnostics
    );
    let unknown = result
        .diagnostics
        .iter()
        .find(|d| d.category == "unknown-field" && d.path == "tools.browser.obscura_stealth");
    assert!(
        unknown.is_none(),
        "obscura_stealth should be accepted as a browser config field, got: {:?}",
        result.diagnostics
    );
}

#[test]
fn browser_lightpanda_path_accepted() {
    let toml = r#"
[tools.browser]
lightpanda_path = "/usr/local/bin/lightpanda"
"#;
    let result = validate_toml_str(toml);
    let unknown = result
        .diagnostics
        .iter()
        .find(|d| d.category == "unknown-field" && d.path == "tools.browser.lightpanda_path");
    assert!(
        unknown.is_none(),
        "lightpanda_path should be accepted as a browser config field, got: {:?}",
        result.diagnostics
    );
}

#[test]
fn keenable_search_provider_accepted_without_key() {
    let toml = r#"
[tools.web.search]
provider = "keenable"
"#;
    let result = validate_toml_str(toml);
    let search = result
        .diagnostics
        .iter()
        .find(|d| d.path.starts_with("tools.web.search"));
    assert!(
        !result.has_errors() && search.is_none(),
        "keenable should be a valid search provider without a key, got: {:?}",
        result.diagnostics
    );
}

#[test]
fn tools_agent_max_iterations_must_be_positive() {
    let toml = r#"
[tools]
agent_max_iterations = 0
"#;
    let result = validate_toml_str(toml);
    let invalid = result.diagnostics.iter().find(|d| {
        d.path == "tools.agent_max_iterations"
            && d.severity == Severity::Error
            && d.category == "invalid-value"
    });
    assert!(
        invalid.is_some(),
        "expected tools.agent_max_iterations invalid-value error, got: {:?}",
        result.diagnostics
    );
}

#[test]
fn mcp_request_timeout_must_be_positive() {
    let toml = r#"
[mcp]
request_timeout_secs = 0
"#;
    let result = validate_toml_str(toml);
    let invalid = result.diagnostics.iter().find(|d| {
        d.path == "mcp.request_timeout_secs"
            && d.severity == Severity::Error
            && d.category == "invalid-value"
    });
    assert!(
        invalid.is_some(),
        "expected mcp.request_timeout_secs invalid-value error, got: {:?}",
        result.diagnostics
    );
}

#[test]
fn mcp_server_request_timeout_override_must_be_positive() {
    let toml = r#"
[mcp.servers.memory]
command = "npx"
request_timeout_secs = 0
"#;
    let result = validate_toml_str(toml);
    let invalid = result.diagnostics.iter().find(|d| {
        d.path == "mcp.servers.memory.request_timeout_secs"
            && d.severity == Severity::Error
            && d.category == "invalid-value"
    });
    assert!(
        invalid.is_some(),
        "expected mcp server request_timeout_secs invalid-value error, got: {:?}",
        result.diagnostics
    );
}

#[test]
fn os_sandbox_backends_and_paths_accepted() {
    for backend in ["os", "seatbelt", "bubblewrap"] {
        let toml = format!(
            r#"
[tools.exec.sandbox]
backend = "{backend}"
os_read_paths = ["/opt/docs"]
os_write_paths = ["projects/demo"]
"#
        );
        let result = validate_toml_str(&toml);
        let flagged: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.path.starts_with("tools.exec.sandbox"))
            .collect();
        assert!(flagged.is_empty(), "{backend}: {flagged:?}");
    }
}

#[test]
fn mcp_trust_sampling_and_auto_mode_keys_are_known() {
    let toml = r#"
[tools]
registry_mode = "auto"
lazy_mcp_threshold = 10

[mcp]
elicitation_timeout_secs = 120
apps_enabled = true

[mcp.servers.notes]
command = "npx"
trust = "ask"

[mcp.servers.notes.sampling]
enabled = true
max_tokens_per_request = 512
max_requests_per_session = 5
max_tokens_per_session = 10000
model = "openai/gpt-5-mini"
"#;
    let result = validate_toml_str(toml);
    let problems: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.path.starts_with("mcp") || d.path.starts_with("tools"))
        .collect();
    assert!(problems.is_empty(), "unexpected diagnostics: {problems:?}");
}

#[test]
fn mcp_elicitation_timeout_must_be_positive() {
    let result = validate_toml_str("[mcp]\nelicitation_timeout_secs = 0\n");
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.path == "mcp.elicitation_timeout_secs" && d.severity == Severity::Error),
        "got: {:?}",
        result.diagnostics
    );
}

#[test]
fn mcp_sampling_warnings_for_untrusted_and_zero_limits() {
    let toml = r#"
[mcp.servers.scraper]
command = "npx"
trust = "untrusted"

[mcp.servers.scraper.sampling]
enabled = true
max_requests_per_session = 0
"#;
    let result = validate_toml_str(toml);
    let paths: Vec<&str> = result
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .map(|d| d.path.as_str())
        .collect();
    assert!(
        paths.contains(&"mcp.servers.scraper.sampling.enabled"),
        "{paths:?}"
    );
    assert!(
        paths.contains(&"mcp.servers.scraper.sampling.max_requests_per_session"),
        "{paths:?}"
    );
}

#[test]
fn mcp_disabled_sampling_produces_no_warning() {
    let toml = r#"
[mcp.servers.scraper]
command = "npx"
trust = "untrusted"
"#;
    let result = validate_toml_str(toml);
    assert!(
        !result
            .diagnostics
            .iter()
            .any(|d| d.path.contains("sampling")),
        "{:?}",
        result.diagnostics
    );
}
