#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;

fn make_detection(home: &Path) -> OpenClawDetection {
    OpenClawDetection {
        home_dir: home.to_path_buf(),
        has_config: false,
        has_credentials: false,
        has_mcp_servers: false,
        workspace_dir: home.join("workspace"),
        has_memory: false,
        has_skills: false,
        agent_ids: vec!["main".to_string()],
        session_count: 1,
        unsupported_channels: Vec::new(),
        has_workspace_files: false,
        workspace_files_found: Vec::new(),
    }
}

fn default_mapping() -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("main".to_string(), "main".to_string());
    m
}

fn setup_session(home: &Path, agent: &str, key: &str, lines: &[&str]) {
    let dir = home
        .join("agents")
        .join(agent)
        .join("agent")
        .join("sessions");
    std::fs::create_dir_all(&dir).unwrap();
    let content = lines.join("\n");
    std::fs::write(dir.join(format!("{key}.jsonl")), content).unwrap();
}

fn setup_session_legacy_layout(home: &Path, agent: &str, key: &str, lines: &[&str]) {
    let dir = home.join("agents").join(agent).join("sessions");
    std::fs::create_dir_all(&dir).unwrap();
    let content = lines.join("\n");
    std::fs::write(dir.join(format!("{key}.jsonl")), content).unwrap();
}

fn imported_history_path(destination: &Path, key: &str) -> std::path::PathBuf {
    let path =
        antre_sessions::store::SessionStore::new(destination.to_path_buf()).history_path_for(key);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    path
}

#[test]
fn convert_basic_session() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let dest = tmp.path().join("sessions");
    let mem = tmp.path().join("memory").join("sessions");

    setup_session(home, "main", "test-session", &[
        r#"{"type":"session-meta","agentId":"main"}"#,
        r#"{"type":"message","message":{"role":"user","content":"Hello"}}"#,
        r#"{"type":"message","message":{"role":"assistant","content":"Hi there!"}}"#,
        r#"{"type":"custom","customType":"model-snapshot","data":{}}"#,
    ]);

    let detection = make_detection(home);
    let report = import_sessions(&detection, &dest, &mem, &default_mapping());

    assert_eq!(report.status, ImportStatus::Success);
    assert_eq!(report.items_imported, 1);

    // Verify converted JSONL
    let converted_path = imported_history_path(&dest, "oc:main:test-session");
    assert!(converted_path.is_file());

    let content = std::fs::read_to_string(&converted_path).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 2); // Only message records, not meta/custom

    let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(first["role"], "user");
    assert_eq!(first["content"], "Hello");

    let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(second["role"], "assistant");
    assert_eq!(second["content"], "Hi there!");
}

#[test]
fn convert_basic_session_legacy_layout() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let dest = tmp.path().join("sessions");
    let mem = tmp.path().join("memory").join("sessions");

    setup_session_legacy_layout(home, "main", "legacy-session", &[
        r#"{"type":"session-meta","agentId":"main"}"#,
        r#"{"type":"message","message":{"role":"user","content":"Hello"}}"#,
        r#"{"type":"message","message":{"role":"assistant","content":"Hi there!"}}"#,
    ]);

    let detection = make_detection(home);
    let report = import_sessions(&detection, &dest, &mem, &default_mapping());

    assert_eq!(report.status, ImportStatus::Success);
    assert_eq!(report.items_imported, 1);
    assert!(imported_history_path(&dest, "oc:main:legacy-session").is_file());
}

#[test]
fn convert_tool_messages() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let dest = tmp.path().join("sessions");
    let mem = tmp.path().join("memory").join("sessions");

    setup_session(home, "main", "tools", &[
        r#"{"type":"message","message":{"role":"user","content":"Run ls"}}"#,
        r#"{"type":"message","message":{"role":"tool","content":"file.txt","toolUseId":"call_1"}}"#,
    ]);

    let detection = make_detection(home);
    let report = import_sessions(&detection, &dest, &mem, &default_mapping());

    assert_eq!(report.items_imported, 1);

    let content = std::fs::read_to_string(imported_history_path(&dest, "oc:main:tools")).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 2);

    let tool: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(tool["role"], "tool");
    assert_eq!(tool["tool_call_id"], "call_1");
}

#[test]
fn import_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let dest = tmp.path().join("sessions");
    let mem = tmp.path().join("memory").join("sessions");

    setup_session(home, "main", "existing", &[
        r#"{"type":"message","message":{"role":"user","content":"test"}}"#,
    ]);

    // First import
    let detection = make_detection(home);
    let report1 = import_sessions(&detection, &dest, &mem, &default_mapping());
    assert_eq!(report1.items_imported, 1);

    // Second import — should skip
    let report2 = import_sessions(&detection, &dest, &mem, &default_mapping());
    assert_eq!(report2.items_imported, 0);
    assert_eq!(report2.items_skipped, 1);
}

#[test]
fn no_agents_returns_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let mem = tmp.path().join("memory").join("sessions");
    let detection = OpenClawDetection {
        home_dir: tmp.path().to_path_buf(),
        has_config: false,
        has_credentials: false,
        has_mcp_servers: false,
        workspace_dir: tmp.path().join("workspace"),
        has_memory: false,
        has_skills: false,
        agent_ids: Vec::new(),
        session_count: 0,
        unsupported_channels: Vec::new(),
        has_workspace_files: false,
        workspace_files_found: Vec::new(),
    };

    let report = import_sessions(&detection, &tmp.path().join("dest"), &mem, &HashMap::new());
    assert_eq!(report.status, ImportStatus::Skipped);
}

#[test]
fn multi_agent_imports_all() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let dest = tmp.path().join("sessions");
    let mem = tmp.path().join("memory").join("sessions");

    setup_session(home, "main", "s1", &[
        r#"{"type":"message","message":{"role":"user","content":"hi from main"}}"#,
    ]);
    setup_session(home, "secondary", "s2", &[
        r#"{"type":"message","message":{"role":"user","content":"hi from secondary"}}"#,
    ]);

    let mut detection = make_detection(home);
    detection.agent_ids = vec!["main".to_string(), "secondary".to_string()];

    let mut mapping = HashMap::new();
    mapping.insert("main".to_string(), "main".to_string());
    mapping.insert("secondary".to_string(), "research".to_string());

    let report = import_sessions(&detection, &dest, &mem, &mapping);
    assert_eq!(report.status, ImportStatus::Success);
    assert_eq!(report.items_imported, 2);

    // Verify both sessions were imported with correct keys
    assert!(imported_history_path(&dest, "oc:main:s1").is_file());
    assert!(imported_history_path(&dest, "oc:research:s2").is_file());

    // Verify metadata has correct agent_id
    let metadata: HashMap<String, ImportedSessionEntry> =
        serde_json::from_str(&std::fs::read_to_string(dest.join("metadata.json")).unwrap())
            .unwrap();
    let main_entry = metadata.get("oc:main:s1").unwrap();
    assert!(main_entry.agent_id.is_none()); // default agent has no agent_id
    let research_entry = metadata.get("oc:research:s2").unwrap();
    assert_eq!(research_entry.agent_id.as_deref(), Some("research"));
}

#[test]
fn session_metadata_written() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let dest = tmp.path().join("sessions");
    let mem = tmp.path().join("memory").join("sessions");

    setup_session(home, "main", "meta-test", &[
        r#"{"type":"message","message":{"role":"user","content":"hello"}}"#,
    ]);

    let detection = make_detection(home);
    import_sessions(&detection, &dest, &mem, &default_mapping());

    let metadata_path = dest.join("metadata.json");
    assert!(metadata_path.is_file());

    let content = std::fs::read_to_string(&metadata_path).unwrap();
    let metadata: HashMap<String, serde_json::Value> = serde_json::from_str(&content).unwrap();
    assert!(metadata.contains_key("oc:main:meta-test"));
}

#[test]
fn malformed_lines_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let dest = tmp.path().join("sessions");
    let mem = tmp.path().join("memory").join("sessions");

    setup_session(home, "main", "messy", &[
        r#"not valid json"#,
        r#"{"type":"message","message":{"role":"user","content":"valid"}}"#,
        r#"{"broken":true}"#,
    ]);

    let detection = make_detection(home);
    let report = import_sessions(&detection, &dest, &mem, &default_mapping());

    assert_eq!(report.items_imported, 1);
    let content = std::fs::read_to_string(imported_history_path(&dest, "oc:main:messy")).unwrap();
    assert_eq!(content.lines().count(), 1);
}

#[test]
fn session_transcript_written() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let dest = tmp.path().join("sessions");
    let mem = tmp.path().join("memory").join("sessions");

    setup_session(home, "main", "chat", &[
        r#"{"type":"session-meta","agentId":"main"}"#,
        r#"{"type":"message","message":{"role":"user","content":"What is Rust?"}}"#,
        r#"{"type":"message","message":{"role":"assistant","content":"Rust is a systems programming language."}}"#,
        r#"{"type":"custom","customType":"model-snapshot","data":{"model":"claude-opus-4-6"}}"#,
    ]);

    let detection = make_detection(home);
    let report = import_sessions(&detection, &dest, &mem, &default_mapping());

    assert_eq!(report.items_imported, 1);

    // Verify transcript markdown was written
    let transcript_path = mem.join("oc-main-chat.md");
    assert!(transcript_path.is_file());

    let content = std::fs::read_to_string(&transcript_path).unwrap();
    assert!(content.contains("# Session: OpenClaw: chat"));
    assert!(content.contains("Imported from OpenClaw"));
    assert!(content.contains("Model: claude-opus-4-6"));
    assert!(content.contains("**User:** What is Rust?"));
    assert!(content.contains("**Assistant:** Rust is a systems programming language."));
}

#[test]
fn model_extracted_from_custom_record() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let dest = tmp.path().join("sessions");
    let mem = tmp.path().join("memory").join("sessions");

    setup_session(home, "main", "model-test", &[
        r#"{"type":"custom","customType":"model-snapshot","data":{"model":"gpt-4o"}}"#,
        r#"{"type":"message","message":{"role":"user","content":"test"}}"#,
    ]);

    let detection = make_detection(home);
    import_sessions(&detection, &dest, &mem, &default_mapping());

    let metadata_path = dest.join("metadata.json");
    let content = std::fs::read_to_string(&metadata_path).unwrap();
    let metadata: HashMap<String, serde_json::Value> = serde_json::from_str(&content).unwrap();
    let entry = metadata.get("oc:main:model-test").unwrap();
    assert_eq!(entry["model"], "gpt-4o");
}

#[test]
fn incremental_import_detects_growth() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let dest = tmp.path().join("sessions");
    let mem = tmp.path().join("memory").join("sessions");

    // Initial import with 1 message
    setup_session(home, "main", "growing", &[
        r#"{"type":"message","message":{"role":"user","content":"Hello"}}"#,
    ]);

    let detection = make_detection(home);
    let report1 = import_sessions(&detection, &dest, &mem, &default_mapping());
    assert_eq!(report1.items_imported, 1);
    assert_eq!(report1.items_updated, 0);

    let content1 =
        std::fs::read_to_string(imported_history_path(&dest, "oc:main:growing")).unwrap();
    assert_eq!(content1.lines().count(), 1);

    // Append a new message to the source
    setup_session(home, "main", "growing", &[
        r#"{"type":"message","message":{"role":"user","content":"Hello"}}"#,
        r#"{"type":"message","message":{"role":"assistant","content":"Hi there!"}}"#,
    ]);

    // Re-import should detect growth and update
    let report2 = import_sessions(&detection, &dest, &mem, &default_mapping());
    assert_eq!(report2.items_imported, 0);
    assert_eq!(report2.items_updated, 1);
    assert_eq!(report2.items_skipped, 0);

    // Destination should now have 2 messages
    let content2 =
        std::fs::read_to_string(imported_history_path(&dest, "oc:main:growing")).unwrap();
    assert_eq!(content2.lines().count(), 2);
}

#[test]
fn incremental_import_noop_when_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let dest = tmp.path().join("sessions");
    let mem = tmp.path().join("memory").join("sessions");

    setup_session(home, "main", "stable", &[
        r#"{"type":"message","message":{"role":"user","content":"test"}}"#,
    ]);

    let detection = make_detection(home);
    let report1 = import_sessions(&detection, &dest, &mem, &default_mapping());
    assert_eq!(report1.items_imported, 1);

    // Re-import without changes — should skip
    let report2 = import_sessions(&detection, &dest, &mem, &default_mapping());
    assert_eq!(report2.items_imported, 0);
    assert_eq!(report2.items_updated, 0);
    assert_eq!(report2.items_skipped, 1);
}

#[test]
fn incremental_import_preserves_id_and_created_at() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let dest = tmp.path().join("sessions");
    let mem = tmp.path().join("memory").join("sessions");

    setup_session(home, "main", "preserve", &[
        r#"{"type":"message","message":{"role":"user","content":"first"}}"#,
    ]);

    let detection = make_detection(home);
    import_sessions(&detection, &dest, &mem, &default_mapping());

    // Read original metadata
    let metadata_path = dest.join("metadata.json");
    let content = std::fs::read_to_string(&metadata_path).unwrap();
    let metadata: HashMap<String, ImportedSessionEntry> = serde_json::from_str(&content).unwrap();
    let original = metadata.get("oc:main:preserve").unwrap();
    let original_id = original.id.clone();
    let original_created_at = original.created_at;
    assert_eq!(original.version, 0);

    // Append and re-import
    setup_session(home, "main", "preserve", &[
        r#"{"type":"message","message":{"role":"user","content":"first"}}"#,
        r#"{"type":"message","message":{"role":"assistant","content":"second"}}"#,
    ]);

    import_sessions(&detection, &dest, &mem, &default_mapping());

    let content2 = std::fs::read_to_string(&metadata_path).unwrap();
    let metadata2: HashMap<String, ImportedSessionEntry> = serde_json::from_str(&content2).unwrap();
    let updated = metadata2.get("oc:main:preserve").unwrap();

    assert_eq!(updated.id, original_id);
    assert_eq!(updated.created_at, original_created_at);
    assert_eq!(updated.version, 1);
    assert_eq!(updated.message_count, 2);
}

#[test]
fn incremental_import_regenerates_transcript() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let dest = tmp.path().join("sessions");
    let mem = tmp.path().join("memory").join("sessions");

    setup_session(home, "main", "transcript", &[
        r#"{"type":"message","message":{"role":"user","content":"What is Rust?"}}"#,
    ]);

    let detection = make_detection(home);
    import_sessions(&detection, &dest, &mem, &default_mapping());

    let transcript_path = mem.join("oc-main-transcript.md");
    let content1 = std::fs::read_to_string(&transcript_path).unwrap();
    assert!(content1.contains("**User:** What is Rust?"));
    assert!(!content1.contains("systems programming language"));

    // Append response and re-import
    setup_session(home, "main", "transcript", &[
        r#"{"type":"message","message":{"role":"user","content":"What is Rust?"}}"#,
        r#"{"type":"message","message":{"role":"assistant","content":"A systems programming language."}}"#,
    ]);

    import_sessions(&detection, &dest, &mem, &default_mapping());

    let content2 = std::fs::read_to_string(&transcript_path).unwrap();
    assert!(content2.contains("**User:** What is Rust?"));
    assert!(content2.contains("**Assistant:** A systems programming language."));
}

#[test]
fn incremental_import_upgrades_legacy_metadata() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let dest = tmp.path().join("sessions");
    let mem = tmp.path().join("memory").join("sessions");
    std::fs::create_dir_all(&dest).unwrap();

    setup_session(home, "main", "legacy", &[
        r#"{"type":"message","message":{"role":"user","content":"old message"}}"#,
    ]);

    // Write legacy metadata without source_line_count (will deserialize as 0)
    let legacy_metadata = serde_json::json!({
        "oc:main:legacy": {
            "id": "legacy-id-123",
            "key": "oc:main:legacy",
            "label": "OpenClaw: legacy",
            "model": null,
            "created_at": 1000,
            "updated_at": 1000,
            "message_count": 1,
            "last_seen_message_count": 0,
            "archived": false,
            "version": 0
        }
    });
    std::fs::write(
        dest.join("metadata.json"),
        serde_json::to_string_pretty(&legacy_metadata).unwrap(),
    )
    .unwrap();

    // Also write a destination JSONL so it looks like a previous import happened
    std::fs::write(
        imported_history_path(&dest, "oc:main:legacy"),
        r#"{"role":"user","content":"old message"}"#,
    )
    .unwrap();

    // Re-import should detect legacy (source_line_count == 0) and re-import
    let detection = make_detection(home);
    let report = import_sessions(&detection, &dest, &mem, &default_mapping());
    assert_eq!(report.items_updated, 1);
    assert_eq!(report.items_imported, 0);

    // Verify metadata now has source_line_count set
    let metadata_path = dest.join("metadata.json");
    let content = std::fs::read_to_string(&metadata_path).unwrap();
    let metadata: HashMap<String, ImportedSessionEntry> = serde_json::from_str(&content).unwrap();
    let entry = metadata.get("oc:main:legacy").unwrap();
    assert!(entry.source_line_count > 0);
    assert_eq!(entry.id, "legacy-id-123"); // Preserved
    assert_eq!(entry.version, 1); // Bumped
}

#[test]
fn preserves_source_timestamps_and_preview() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let dest = tmp.path().join("sessions");
    let mem = tmp.path().join("memory").join("sessions");

    setup_session(home, "main", "timed", &[
        r#"{"type":"message","timestamp":"2026-01-28T06:46:35.768Z","message":{"role":"user","content":"hello from old openclaw","timestamp":1769582795764}}"#,
        r#"{"type":"message","timestamp":"2026-01-28T06:46:41.626Z","message":{"role":"assistant","content":"this is the assistant reply","timestamp":1769582801626}}"#,
    ]);

    let detection = make_detection(home);
    let report = import_sessions(&detection, &dest, &mem, &default_mapping());
    assert_eq!(report.status, ImportStatus::Success);
    assert_eq!(report.items_imported, 1);

    let converted = std::fs::read_to_string(imported_history_path(&dest, "oc:main:timed")).unwrap();
    let mut lines = converted.lines();
    let first: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
    let second: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
    assert_eq!(first["created_at"], 1769582795764_u64);
    assert_eq!(second["created_at"], 1769582801626_u64);

    let metadata: HashMap<String, ImportedSessionEntry> =
        serde_json::from_str(&std::fs::read_to_string(dest.join("metadata.json")).unwrap())
            .unwrap();
    let entry = metadata.get("oc:main:timed").unwrap();
    assert_eq!(entry.created_at, 1769582795764_u64);
    assert_eq!(entry.updated_at, 1769582801626_u64);
    assert!(
        entry
            .preview
            .as_deref()
            .is_some_and(|p| p.contains("hello from old openclaw"))
    );
}

fn write_sessions_json(home: &Path, agent: &str, content: &str) {
    let dir = home
        .join("agents")
        .join(agent)
        .join("agent")
        .join("sessions");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("sessions.json"), content).unwrap();
}

#[test]
fn session_labels_from_index() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let dest = tmp.path().join("sessions");
    let mem = tmp.path().join("memory").join("sessions");

    let session_uuid = "d8da3601-1234-4abc-9def-aabbccddeeff";

    // Create sessions.json index with rich metadata
    write_sessions_json(
        home,
        "main",
        &serde_json::json!({
            "agent:main:main": {
                "sessionId": session_uuid,
                "updatedAt": 1770000000000_u64,
                "origin": {
                    "label": "Fabien (@fabienpenso) id:377114917",
                    "chatType": "direct"
                }
            }
        })
        .to_string(),
    );

    // Create matching JSONL file named by UUID
    setup_session(home, "main", session_uuid, &[
        r#"{"type":"message","message":{"role":"user","content":"Hello"}}"#,
        r#"{"type":"message","message":{"role":"assistant","content":"Hi!"}}"#,
    ]);

    let detection = make_detection(home);
    let report = import_sessions(&detection, &dest, &mem, &default_mapping());
    assert_eq!(report.status, ImportStatus::Success);
    assert_eq!(report.items_imported, 1);

    // Verify label is "Main" (from agent:main:main key pattern)
    let metadata: HashMap<String, ImportedSessionEntry> =
        serde_json::from_str(&std::fs::read_to_string(dest.join("metadata.json")).unwrap())
            .unwrap();
    let key = format!("oc:main:{session_uuid}");
    let entry = metadata.get(&key).unwrap();
    assert_eq!(entry.label.as_deref(), Some("Main"));
}

#[test]
fn session_label_telegram_from_index() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let dest = tmp.path().join("sessions");
    let mem = tmp.path().join("memory").join("sessions");

    let session_uuid = "aabb1122-3344-4556-8899-001122334455";

    write_sessions_json(
        home,
        "main",
        &serde_json::json!({
            "agent:main:telegram:dm:377114917": {
                "sessionId": session_uuid,
                "updatedAt": 1770000000000_u64,
                "origin": {
                    "label": "Fabien (@fabienpenso) id:377114917",
                    "provider": "telegram",
                    "chatType": "direct"
                }
            }
        })
        .to_string(),
    );

    setup_session(home, "main", session_uuid, &[
        r#"{"type":"message","message":{"role":"user","content":"Hi via Telegram"}}"#,
    ]);

    let detection = make_detection(home);
    let report = import_sessions(&detection, &dest, &mem, &default_mapping());
    assert_eq!(report.status, ImportStatus::Success);

    let metadata: HashMap<String, ImportedSessionEntry> =
        serde_json::from_str(&std::fs::read_to_string(dest.join("metadata.json")).unwrap())
            .unwrap();
    let key = format!("oc:main:{session_uuid}");
    let entry = metadata.get(&key).unwrap();
    assert_eq!(entry.label.as_deref(), Some("Telegram: Fabien"));
}

#[test]
fn session_label_cron_from_index() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let dest = tmp.path().join("sessions");
    let mem = tmp.path().join("memory").join("sessions");

    let session_uuid = "cron1234-5678-4abc-9def-000000000001";

    write_sessions_json(
        home,
        "main",
        &serde_json::json!({
            "agent:main:cron:daily-summary": {
                "sessionId": session_uuid,
                "updatedAt": 1770000000000_u64
            }
        })
        .to_string(),
    );

    setup_session(home, "main", session_uuid, &[
        r#"{"type":"message","message":{"role":"assistant","content":"Daily summary done"}}"#,
    ]);

    let detection = make_detection(home);
    let report = import_sessions(&detection, &dest, &mem, &default_mapping());
    assert_eq!(report.status, ImportStatus::Success);

    let metadata: HashMap<String, ImportedSessionEntry> =
        serde_json::from_str(&std::fs::read_to_string(dest.join("metadata.json")).unwrap())
            .unwrap();
    let key = format!("oc:main:{session_uuid}");
    let entry = metadata.get(&key).unwrap();
    assert_eq!(entry.label.as_deref(), Some("Cron"));
}

#[test]
fn session_label_fallback_without_index() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let dest = tmp.path().join("sessions");
    let mem = tmp.path().join("memory").join("sessions");

    // No sessions.json — just a JSONL file with a UUID-like name
    setup_session(home, "main", "abcd1234-dead-beef-cafe-112233445566", &[
        r#"{"type":"message","message":{"role":"user","content":"test"}}"#,
    ]);

    let detection = make_detection(home);
    let report = import_sessions(&detection, &dest, &mem, &default_mapping());
    assert_eq!(report.status, ImportStatus::Success);

    let metadata: HashMap<String, ImportedSessionEntry> =
        serde_json::from_str(&std::fs::read_to_string(dest.join("metadata.json")).unwrap())
            .unwrap();
    let entry = metadata
        .get("oc:main:abcd1234-dead-beef-cafe-112233445566")
        .unwrap();
    // Fallback: first 8 chars of the stem
    assert_eq!(entry.label.as_deref(), Some("OpenClaw: abcd1234"));
}

#[test]
fn build_session_label_unit_tests() {
    // Main session
    assert_eq!(build_session_label("agent:main:main", None), "Main");

    // Telegram with label
    let origin = OpenClawSessionOrigin {
        label: Some("Alice (@alice) id:123".to_string()),
        provider: Some("telegram".to_string()),
        chat_type: Some("direct".to_string()),
    };
    assert_eq!(
        build_session_label("agent:main:telegram:dm:123", Some(&origin)),
        "Telegram: Alice"
    );

    // Telegram without label
    assert_eq!(
        build_session_label("agent:main:telegram:dm:123", None),
        "Telegram"
    );

    // Signal with label
    let signal_origin = OpenClawSessionOrigin {
        label: Some("Bob".to_string()),
        provider: Some("signal".to_string()),
        chat_type: Some("direct".to_string()),
    };
    assert_eq!(
        build_session_label("agent:main:signal:dm:+1234567890", Some(&signal_origin)),
        "Signal: Bob"
    );

    // Cron
    assert_eq!(build_session_label("agent:main:cron:daily", None), "Cron");

    // Unknown key with origin label
    let other_origin = OpenClawSessionOrigin {
        label: Some("Charlie".to_string()),
        provider: None,
        chat_type: None,
    };
    assert_eq!(
        build_session_label("agent:main:unknown:thing", Some(&other_origin)),
        "OpenClaw: Charlie"
    );

    // Unknown key without origin
    assert_eq!(
        build_session_label("agent:main:unknown:thing", None),
        "OpenClaw: agent:main:unknown:thing"
    );
}

#[test]
fn load_session_label_map_parses_index() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    std::fs::write(
        dir.join("sessions.json"),
        serde_json::json!({
            "agent:main:main": {
                "sessionId": "uuid-1",
                "updatedAt": 1000,
                "origin": { "label": "Test", "chatType": "direct" }
            },
            "agent:main:telegram:dm:99": {
                "sessionId": "uuid-2",
                "updatedAt": 2000,
                "origin": { "label": "User99", "provider": "telegram" }
            },
            "agent:main:no-session-id": {
                "updatedAt": 3000
            }
        })
        .to_string(),
    )
    .unwrap();

    let map = load_session_label_map(dir);
    assert_eq!(map.len(), 2); // no-session-id entry is excluded
    assert!(map.contains_key("uuid-1"));
    assert!(map.contains_key("uuid-2"));

    let entry = &map["uuid-1"];
    assert_eq!(entry.logical_key, "agent:main:main");
    assert_eq!(
        entry.origin.as_ref().and_then(|o| o.label.as_deref()),
        Some("Test")
    );
}

#[test]
fn jsonl_child_session_points_at_parent() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let dest = tmp.path().join("sessions");
    let mem = tmp.path().join("memory").join("sessions");

    let message = r#"{"type":"message","message":{"role":"user","content":"Hi"}}"#;
    setup_session(home, "main", "uuid-parent", &[message]);
    setup_session(home, "main", "uuid-child", &[message]);
    write_sessions_json(
        home,
        "main",
        r#"{
            "agent:main:main": {"sessionId": "uuid-parent"},
            "agent:main:subagent:x": {"sessionId": "uuid-child", "spawnedBy": "agent:main:main"}
        }"#,
    );

    let detection = make_detection(home);
    let report = import_sessions(&detection, &dest, &mem, &default_mapping());
    assert_eq!(report.items_imported, 2);

    let content = std::fs::read_to_string(dest.join("metadata.json")).unwrap();
    let metadata: HashMap<String, ImportedSessionEntry> = serde_json::from_str(&content).unwrap();
    assert_eq!(
        metadata["oc:main:uuid-child"].parent_session_key.as_deref(),
        Some("oc:main:uuid-parent")
    );
    assert_eq!(metadata["oc:main:uuid-parent"].parent_session_key, None);
}
