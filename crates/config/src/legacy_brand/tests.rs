use std::ffi::OsString;

use super::*;

fn vars(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
    pairs
        .iter()
        .map(|(key, value)| (OsString::from(key), OsString::from(value)))
        .collect()
}

#[test]
fn legacy_env_is_promoted_when_new_name_is_unset() {
    let promotions = legacy_env_promotions(vars(&[
        ("MOLTIS_DATA_DIR", "/var/lib/moltis/data"),
        ("PATH", "/usr/bin"),
    ]));
    assert_eq!(promotions, vec![EnvPromotion {
        legacy: "MOLTIS_DATA_DIR".into(),
        current: "ANTRE_DATA_DIR".into(),
        value: "/var/lib/moltis/data".into(),
    }]);
}

#[test]
fn new_env_name_wins_over_legacy() {
    let promotions = legacy_env_promotions(vars(&[
        ("MOLTIS_DATA_DIR", "/old"),
        ("ANTRE_DATA_DIR", "/new"),
        ("MOLTIS_BEHIND_PROXY", "true"),
    ]));
    assert_eq!(promotions.len(), 1);
    assert_eq!(promotions[0].current, "ANTRE_BEHIND_PROXY");
}

#[test]
fn bare_prefix_and_lookalikes_are_ignored() {
    let promotions = legacy_env_promotions(vars(&[
        ("MOLTIS_", "x"),
        ("NOT_MOLTIS_X", "x"),
        ("moltis_lowercase", "x"),
    ]));
    assert!(promotions.is_empty());
}

#[test]
fn promotions_are_sorted_by_legacy_name() {
    let promotions = legacy_env_promotions(vars(&[("MOLTIS_B", "2"), ("MOLTIS_A", "1")]));
    let names: Vec<&str> = promotions.iter().map(|p| p.legacy.as_str()).collect();
    assert_eq!(names, ["MOLTIS_A", "MOLTIS_B"]);
}

#[test]
fn resolve_default_dir_prefers_current() {
    let home = tempfile::tempdir().unwrap();
    let current = home.path().join(".antre");
    let legacy = legacy_data_dir(home.path());

    assert_eq!(
        resolve_default_dir(current.clone(), legacy.clone()),
        current
    );

    std::fs::create_dir(&legacy).unwrap();
    assert_eq!(resolve_default_dir(current.clone(), legacy.clone()), legacy);

    std::fs::create_dir(&current).unwrap();
    assert_eq!(resolve_default_dir(current.clone(), legacy), current);
}

#[test]
fn legacy_config_file_names_are_recognized() {
    assert!(is_legacy_config_file(Path::new("/etc/x/moltis.toml")));
    assert!(is_legacy_config_file(Path::new("moltis.json")));
    assert!(!is_legacy_config_file(Path::new("/etc/x/antre.toml")));
    assert!(!is_legacy_config_file(Path::new("/etc/moltis.toml/other")));
}

#[test]
fn move_dir_moves_content_and_leaves_marker_and_symlink() {
    let home = tempfile::tempdir().unwrap();
    let legacy = legacy_data_dir(home.path());
    let current = home.path().join(".antre");
    std::fs::create_dir_all(legacy.join("sessions")).unwrap();
    std::fs::write(legacy.join("sessions/a.jsonl"), "data").unwrap();

    let steps = move_dir(&legacy, &current);

    assert!(
        matches!(steps.as_slice(), [MigrationStep::MovedDir { .. }]),
        "{steps:?}"
    );
    assert_eq!(
        std::fs::read_to_string(current.join("sessions/a.jsonl")).unwrap(),
        "data"
    );
    let marker = std::fs::read_to_string(current.join(MIGRATION_MARKER)).unwrap();
    assert!(marker.contains(&legacy.display().to_string()));
    #[cfg(unix)]
    {
        assert!(
            std::fs::symlink_metadata(&legacy)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read_to_string(legacy.join("sessions/a.jsonl")).unwrap(),
            "data"
        );
    }
}

#[test]
fn move_dir_never_touches_an_existing_target() {
    let home = tempfile::tempdir().unwrap();
    let legacy = legacy_data_dir(home.path());
    let current = home.path().join(".antre");
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(legacy.join("old"), "old").unwrap();
    std::fs::create_dir_all(&current).unwrap();
    std::fs::write(current.join("new"), "new").unwrap();

    assert!(move_dir(&legacy, &current).is_empty());
    assert!(legacy.join("old").exists());
    assert!(current.join("new").exists());
    assert!(!current.join("old").exists());
}

#[test]
fn move_dir_is_idempotent() {
    let home = tempfile::tempdir().unwrap();
    let legacy = legacy_data_dir(home.path());
    let current = home.path().join(".antre");
    std::fs::create_dir_all(&legacy).unwrap();

    assert_eq!(move_dir(&legacy, &current).len(), 1);
    // The old path is now a symlink (unix) or gone, the new one exists.
    assert!(move_dir(&legacy, &current).is_empty());
}

#[test]
fn move_dir_without_legacy_dir_does_nothing() {
    let home = tempfile::tempdir().unwrap();
    let current = home.path().join(".antre");
    assert!(move_dir(&legacy_data_dir(home.path()), &current).is_empty());
    assert!(!current.exists());
}

#[test]
fn move_dir_failure_keeps_legacy_dir() {
    let home = tempfile::tempdir().unwrap();
    let legacy = legacy_data_dir(home.path());
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(legacy.join("keep"), "x").unwrap();
    // A file where the parent of the target should be makes the move fail.
    let blocker = home.path().join("blocker");
    std::fs::write(&blocker, "").unwrap();

    let steps = move_dir(&legacy, &blocker.join("antre"));

    assert!(
        matches!(steps.as_slice(), [MigrationStep::DirMoveFailed { .. }]),
        "{steps:?}"
    );
    assert!(steps[0].is_failure());
    assert!(legacy.join("keep").exists());
}

#[test]
fn rename_legacy_files_renames_db_and_wal() {
    let data = tempfile::tempdir().unwrap();
    std::fs::write(data.path().join("moltis.db"), "db").unwrap();
    std::fs::write(data.path().join("moltis.db-wal"), "wal").unwrap();
    std::fs::create_dir_all(data.path().join("docs/moltis")).unwrap();
    std::fs::write(data.path().join("docs/moltis/GUIDELINES.md"), "g").unwrap();

    let steps = rename_legacy_files(data.path());

    assert_eq!(steps.len(), 3, "{steps:?}");
    assert!(steps.iter().all(|s| !s.is_failure()));
    assert_eq!(
        std::fs::read_to_string(data.path().join("antre.db")).unwrap(),
        "db"
    );
    assert_eq!(
        std::fs::read_to_string(data.path().join("antre.db-wal")).unwrap(),
        "wal"
    );
    assert!(data.path().join("docs/antre/GUIDELINES.md").exists());
    assert!(!data.path().join("moltis.db").exists());
    let marker = std::fs::read_to_string(data.path().join(MIGRATION_MARKER)).unwrap();
    assert_eq!(marker.lines().count(), 3);
}

#[test]
fn rename_legacy_files_keeps_both_when_new_exists() {
    let data = tempfile::tempdir().unwrap();
    std::fs::write(data.path().join("moltis.db"), "old").unwrap();
    std::fs::write(data.path().join("antre.db"), "new").unwrap();

    assert!(rename_legacy_files(data.path()).is_empty());
    assert_eq!(
        std::fs::read_to_string(data.path().join("antre.db")).unwrap(),
        "new"
    );
    assert!(data.path().join("moltis.db").exists());
}

#[test]
fn steps_display_names_both_paths() {
    let step = MigrationStep::MovedDir {
        from: PathBuf::from("/h/.moltis"),
        to: PathBuf::from("/h/.antre"),
    };
    let text = step.to_string();
    assert!(text.contains("/h/.moltis") && text.contains("/h/.antre"));

    let step = MigrationStep::LegacyConfigFile(PathBuf::from("/c/moltis.yaml"));
    assert!(step.to_string().contains("antre.yaml"));
}

#[test]
fn legacy_service_files_finds_installed_unit() {
    let home = tempfile::tempdir().unwrap();
    assert!(legacy_service_files(home.path(), &LEGACY_GATEWAY_SERVICE).is_empty());

    let path = if cfg!(target_os = "macos") {
        home.path()
            .join("Library/LaunchAgents/org.moltis.gateway.plist")
    } else {
        home.path().join(".config/systemd/user/moltis.service")
    };
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "").unwrap();

    assert_eq!(
        legacy_service_files(home.path(), &LEGACY_GATEWAY_SERVICE),
        vec![path]
    );
    assert!(legacy_service_files(home.path(), &LEGACY_NODE_SERVICE).is_empty());
}
