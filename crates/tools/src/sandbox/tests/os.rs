#![allow(clippy::unwrap_used, clippy::expect_used)]
use {
    super::*,
    crate::sandbox::{
        platform::ulimit_wrapped_command,
        router::{os_sandbox_backend_name, select_backend_by_name},
    },
};

#[test]
fn backend_ids_parse_os_backends() {
    assert_eq!(
        SandboxBackendId::from_name("seatbelt"),
        SandboxBackendId::Seatbelt
    );
    assert_eq!(
        SandboxBackendId::from_name("bubblewrap"),
        SandboxBackendId::Bubblewrap
    );
}

#[test]
fn schema_config_carries_os_paths() {
    let schema = moltis_config::schema::SandboxConfig {
        os_read_paths: vec!["/opt/docs".into()],
        os_write_paths: vec!["projects".into()],
        ..Default::default()
    };
    let config = SandboxConfig::from(&schema);
    assert_eq!(config.os_read_paths, vec![PathBuf::from("/opt/docs")]);
    assert_eq!(config.os_write_paths, vec![PathBuf::from("projects")]);
}

#[test]
fn os_backend_selected_by_name_when_available() {
    let config = SandboxConfig::default();
    let backend = select_backend_by_name("os", &config);
    match os_sandbox_backend_name() {
        Some(name) => {
            assert_eq!(backend.backend_name(), name);
            assert!(backend.provides_fs_isolation());
            assert!(backend.runs_on_host());
        },
        None => assert_eq!(backend.backend_name(), "restricted-host"),
    }
}

#[test]
fn os_backend_for_other_platform_falls_back_to_restricted_host() {
    let other = if cfg!(target_os = "macos") {
        "bubblewrap"
    } else {
        "seatbelt"
    };
    let backend = select_backend_by_name(other, &SandboxConfig::default());
    assert_eq!(backend.backend_name(), "restricted-host");
    assert!(!backend.provides_fs_isolation());
    assert!(backend.runs_on_host());
}

#[cfg(target_os = "macos")]
#[test]
fn seatbelt_is_detected_on_macos() {
    assert_eq!(os_sandbox_backend_name(), Some("seatbelt"));
}

#[test]
fn ulimit_nproc_only_when_requested() {
    let limits = ResourceLimits::default();
    assert!(!ulimit_wrapped_command(&limits, "true").contains("ulimit -u"));
    let explicit = ResourceLimits {
        pids_max: Some(64),
        ..ResourceLimits::default()
    };
    assert!(ulimit_wrapped_command(&explicit, "true").contains("ulimit -u 64"));
}
