use std::{collections::HashMap, path::Path};

use {rusqlite::Connection, serde_json::Value};

use {
    super::AGENT_DB_FILE,
    crate::{
        detect::{OpenClawDetection, detect_at},
        report::ImportStatus,
        sessions::{ImportedSessionEntry, import_sessions},
    },
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The subset of OpenClaw's agent schema (v44) that the importer reads.
const SCHEMA: &str = "
CREATE TABLE session_nodes (
  session_key TEXT NOT NULL PRIMARY KEY,
  current_session_id TEXT NOT NULL,
  entry_json TEXT NOT NULL,
  updated_at INTEGER NOT NULL,
  label TEXT,
  display_name TEXT,
  archived_at INTEGER
) STRICT;
CREATE TABLE session_windows (
  session_id TEXT NOT NULL PRIMARY KEY,
  session_key TEXT NOT NULL,
  previous_session_id TEXT,
  reason TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  FOREIGN KEY (session_key) REFERENCES session_nodes(session_key) ON DELETE CASCADE
) STRICT;
CREATE TABLE transcript_events (
  session_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  event_json TEXT,
  created_at INTEGER NOT NULL,
  event_zstd BLOB,
  event_utf8_bytes INTEGER,
  navigation_json TEXT,
  PRIMARY KEY (session_id, seq)
) STRICT;
CREATE TABLE session_transcript_cold_archives (
  session_id TEXT NOT NULL PRIMARY KEY,
  generation TEXT NOT NULL,
  archive_name TEXT NOT NULL UNIQUE,
  archive_sha256 TEXT NOT NULL,
  event_count INTEGER NOT NULL,
  raw_bytes INTEGER NOT NULL,
  archive_bytes INTEGER NOT NULL,
  last_seq INTEGER NOT NULL,
  archived_at INTEGER NOT NULL,
  storage TEXT NOT NULL,
  archive_blob BLOB
) STRICT;
CREATE TABLE session_transcript_cold_archive_chunks (
  session_id TEXT NOT NULL,
  chunk_index INTEGER NOT NULL,
  archive_name TEXT NOT NULL UNIQUE,
  archive_sha256 TEXT NOT NULL,
  first_seq INTEGER NOT NULL,
  last_seq INTEGER NOT NULL,
  event_count INTEGER NOT NULL,
  raw_bytes INTEGER NOT NULL,
  archive_bytes INTEGER NOT NULL,
  archive_blob BLOB,
  PRIMARY KEY (session_id, chunk_index)
) STRICT, WITHOUT ROWID;
";

struct Fixture {
    conn: Connection,
}

impl Fixture {
    fn create(home: &Path, agent: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let dir = home.join("agents").join(agent).join("agent");
        std::fs::create_dir_all(&dir)?;
        let conn = Connection::open(dir.join(AGENT_DB_FILE))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    fn node(&self, key: &str, session_id: &str, entry: &Value) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO session_nodes (session_key, current_session_id, entry_json, updated_at) \
             VALUES (?1, ?2, ?3, 0)",
            (key, session_id, entry.to_string()),
        )?;
        Ok(())
    }

    fn window(&self, key: &str, session_id: &str, created_at: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO session_windows (session_id, session_key, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?3)",
            (session_id, key, created_at),
        )?;
        Ok(())
    }

    fn event(&self, session_id: &str, seq: i64, event: &Value) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO transcript_events (session_id, seq, event_json, created_at) \
             VALUES (?1, ?2, ?3, 0)",
            (session_id, seq, event.to_string()),
        )?;
        Ok(())
    }

    fn compressed_event(
        &self,
        session_id: &str,
        seq: i64,
        event: &Value,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let json = event.to_string();
        let compressed = zstd::bulk::compress(json.as_bytes(), 3)?;
        self.conn.execute(
            "INSERT INTO transcript_events \
             (session_id, seq, event_zstd, event_utf8_bytes, navigation_json, created_at) \
             VALUES (?1, ?2, ?3, ?4, '{}', 0)",
            (session_id, seq, compressed, json.len() as i64),
        )?;
        Ok(())
    }

    /// A cold event keeps its row but loses its payload.
    fn cold_event_row(&self, session_id: &str, seq: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO transcript_events \
             (session_id, seq, event_utf8_bytes, navigation_json, created_at) \
             VALUES (?1, ?2, 1, '{}', 0)",
            (session_id, seq),
        )?;
        Ok(())
    }

    fn cold_archive(
        &self,
        session_id: &str,
        storage: &str,
        name: &str,
        blob: Option<Vec<u8>>,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO session_transcript_cold_archives \
             (session_id, generation, archive_name, archive_sha256, event_count, raw_bytes, \
              archive_bytes, last_seq, archived_at, storage, archive_blob) \
             VALUES (?1, 'g1', ?2, ?3, 1, 0, 0, 0, 0, ?4, ?5)",
            (session_id, name, "0".repeat(64), storage, blob),
        )?;
        Ok(())
    }

    fn cold_chunk(
        &self,
        session_id: &str,
        index: i64,
        name: &str,
        blob: Option<Vec<u8>>,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO session_transcript_cold_archive_chunks \
             (session_id, chunk_index, archive_name, archive_sha256, first_seq, last_seq, \
              event_count, raw_bytes, archive_bytes, archive_blob) \
             VALUES (?1, ?2, ?3, ?4, 0, 0, 1, 0, 0, ?5)",
            (session_id, index, name, "0".repeat(64), blob),
        )?;
        Ok(())
    }
}

fn message(role: &str, content: &str) -> Value {
    serde_json::json!({
        "type": "message",
        "timestamp": "2026-03-01T10:00:00Z",
        "message": { "role": role, "content": content },
    })
}

/// A zstd cold archive in OpenClaw's envelope format.
fn cold_archive_bytes(session_id: &str, events: &[(i64, Value)]) -> std::io::Result<Vec<u8>> {
    let mut lines = vec![
        serde_json::json!({"kind": "header", "version": 1, "sessionId": session_id, "generation": "g1"})
            .to_string(),
    ];
    for (seq, event) in events {
        lines.push(
            serde_json::json!({
                "kind": "event",
                "row": { "seq": seq, "event_json": event.to_string(), "created_at": 0 },
            })
            .to_string(),
        );
        lines.push(
            serde_json::json!({"kind": "active", "row": {"active_position": seq, "event_seq": seq, "message_position": null, "context_eligible": null}})
                .to_string(),
        );
    }
    zstd::bulk::compress(lines.join("\n").as_bytes(), 3)
}

fn detection(home: &Path, agents: &[&str]) -> OpenClawDetection {
    OpenClawDetection {
        home_dir: home.to_path_buf(),
        has_config: false,
        has_credentials: false,
        has_mcp_servers: false,
        workspace_dir: home.join("workspace"),
        has_memory: false,
        has_skills: false,
        agent_ids: agents.iter().map(ToString::to_string).collect(),
        session_count: 0,
        unsupported_channels: Vec::new(),
        has_workspace_files: false,
        workspace_files_found: Vec::new(),
    }
}

fn identity_mapping(agents: &[&str]) -> HashMap<String, String> {
    agents
        .iter()
        .map(|a| (a.to_string(), a.to_string()))
        .collect()
}

fn history(dest: &Path, key: &str) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let path = moltis_sessions::store::SessionStore::new(dest.to_path_buf()).history_path_for(key);
    let content = std::fs::read_to_string(path)?;
    content
        .lines()
        .map(|line| serde_json::from_str(line).map_err(Into::into))
        .collect()
}

fn metadata(
    dest: &Path,
) -> Result<HashMap<String, ImportedSessionEntry>, Box<dyn std::error::Error>> {
    Ok(serde_json::from_str(&std::fs::read_to_string(
        dest.join("metadata.json"),
    )?)?)
}

fn contents(messages: &[Value]) -> Vec<&str> {
    messages
        .iter()
        .filter_map(|m| m["content"].as_str())
        .collect()
}

#[test]
fn imports_plain_and_compressed_events() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let home = tmp.path().join("openclaw");
    let dest = tmp.path().join("sessions");
    let mem = tmp.path().join("memory").join("sessions");

    let db = Fixture::create(&home, "main")?;
    db.node(
        "agent:main:telegram:dm:42",
        "s-tg",
        &serde_json::json!({"sessionId": "s-tg", "origin": {"label": "Alice (@alice)", "provider": "telegram"}}),
    )?;
    db.window("agent:main:telegram:dm:42", "s-tg", 1)?;
    db.event(
        "s-tg",
        0,
        &serde_json::json!({"type": "session", "version": 3, "id": "s-tg"}),
    )?;
    db.event("s-tg", 1, &message("user", "Hello"))?;
    db.compressed_event("s-tg", 2, &message("assistant", "Hi from zstd"))?;
    db.event(
        "s-tg",
        3,
        &serde_json::json!({"type": "custom", "customType": "model-snapshot", "data": {"model": "gpt-5"}}),
    )?;

    let report = import_sessions(
        &detection(&home, &["main"]),
        &dest,
        &mem,
        &identity_mapping(&["main"]),
    );
    assert_eq!(report.status, ImportStatus::Success, "{:?}", report.errors);
    assert_eq!(report.items_imported, 1);

    let messages = history(&dest, "oc:main:s-tg")?;
    assert_eq!(contents(&messages), ["Hello", "Hi from zstd"]);
    assert_eq!(messages[1]["role"], "assistant");

    let meta = metadata(&dest)?;
    let entry = meta.get("oc:main:s-tg").ok_or("missing metadata")?;
    assert_eq!(entry.label.as_deref(), Some("Telegram: Alice"));
    assert_eq!(entry.model.as_deref(), Some("gpt-5"));
    assert_eq!(entry.agent_id, None);
    assert_eq!(entry.source_line_count, 4);
    assert!(!entry.archived);
    assert!(mem.join("oc-main-s-tg.md").is_file());
    Ok(())
}

#[test]
fn imports_every_window_and_keeps_agent_ids() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let home = tmp.path().join("openclaw");
    let dest = tmp.path().join("sessions");
    let mem = tmp.path().join("memory").join("sessions");

    let main = Fixture::create(&home, "main")?;
    main.node(
        "agent:main:main",
        "s-new",
        &serde_json::json!({"sessionId": "s-new"}),
    )?;
    main.window("agent:main:main", "s-old", 1)?;
    main.window("agent:main:main", "s-new", 2)?;
    main.window("agent:main:main", "s-empty", 3)?;
    main.event("s-old", 1, &message("user", "before reset"))?;
    main.event("s-new", 1, &message("user", "after reset"))?;
    main.conn.execute(
        "UPDATE session_nodes SET archived_at = 5 WHERE session_key = 'agent:main:main'",
        [],
    )?;

    let cyber = Fixture::create(&home, "cyberclaw")?;
    cyber.node(
        "agent:cyberclaw:discord:channel:9",
        "s-cy",
        &serde_json::json!({"sessionId": "s-cy"}),
    )?;
    cyber
        .conn
        .execute("UPDATE session_nodes SET display_name = 'ops room'", [])?;
    cyber.window("agent:cyberclaw:discord:channel:9", "s-cy", 1)?;
    cyber.event("s-cy", 1, &message("user", "scan"))?;

    let agents = ["cyberclaw", "main"];
    let report = import_sessions(
        &detection(&home, &agents),
        &dest,
        &mem,
        &identity_mapping(&agents),
    );
    assert_eq!(report.status, ImportStatus::Success, "{:?}", report.errors);
    assert_eq!(report.items_imported, 3);

    let meta = metadata(&dest)?;
    assert!(!meta.contains_key("oc:main:s-empty"));
    let old = meta.get("oc:main:s-old").ok_or("missing s-old")?;
    assert_eq!(old.label.as_deref(), Some("Main"));
    assert!(old.archived);
    assert_eq!(contents(&history(&dest, "oc:main:s-new")?), ["after reset"]);

    let cy = meta.get("oc:cyberclaw:s-cy").ok_or("missing s-cy")?;
    assert_eq!(cy.agent_id.as_deref(), Some("cyberclaw"));
    assert_eq!(cy.label.as_deref(), Some("OpenClaw: ops room"));
    Ok(())
}

#[test]
fn reads_cold_archives_from_database_chunks_and_files() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let home = tmp.path().join("openclaw");
    let dest = tmp.path().join("sessions");
    let mem = tmp.path().join("memory").join("sessions");

    let db = Fixture::create(&home, "main")?;
    for id in ["s-blob", "s-chunks", "s-file"] {
        db.node(&format!("agent:main:{id}"), id, &serde_json::json!({}))?;
        db.window(&format!("agent:main:{id}"), id, 1)?;
    }

    // Embedded archive, with one event still hot.
    db.cold_event_row("s-blob", 1)?;
    db.event("s-blob", 2, &message("assistant", "hot reply"))?;
    let blob = cold_archive_bytes("s-blob", &[(1, message("user", "cold question"))])?;
    db.cold_archive("s-blob", "sqlite", "blob.jsonl.zst", Some(blob))?;

    // Embedded archive split in chunks.
    db.cold_event_row("s-chunks", 1)?;
    db.cold_event_row("s-chunks", 2)?;
    db.cold_archive("s-chunks", "sqlite", "parent.jsonl.zst", Some(Vec::new()))?;
    let first = cold_archive_bytes("s-chunks", &[(1, message("user", "chunk one"))])?;
    let second = cold_archive_bytes("s-chunks", &[(2, message("assistant", "chunk two"))])?;
    db.cold_chunk("s-chunks", 0, "c0.jsonl.zst", Some(first))?;
    db.cold_chunk("s-chunks", 1, "c1.jsonl.zst", Some(second))?;

    // File-backed archive published next to the sessions.
    db.cold_event_row("s-file", 1)?;
    db.cold_archive("s-file", "file", "file.jsonl.zst", None)?;
    let cold_dir = home
        .join("agents")
        .join("main")
        .join("sessions")
        .join("cold");
    std::fs::create_dir_all(&cold_dir)?;
    std::fs::write(
        cold_dir.join("file.jsonl.zst"),
        cold_archive_bytes("s-file", &[(1, message("user", "from file"))])?,
    )?;

    let report = import_sessions(
        &detection(&home, &["main"]),
        &dest,
        &mem,
        &identity_mapping(&["main"]),
    );
    assert_eq!(report.status, ImportStatus::Success, "{:?}", report.errors);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(report.items_imported, 3);

    assert_eq!(contents(&history(&dest, "oc:main:s-blob")?), [
        "cold question",
        "hot reply"
    ]);
    assert_eq!(contents(&history(&dest, "oc:main:s-chunks")?), [
        "chunk one",
        "chunk two"
    ]);
    assert_eq!(contents(&history(&dest, "oc:main:s-file")?), ["from file"]);
    Ok(())
}

#[test]
fn missing_cold_archive_is_a_warning() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let home = tmp.path().join("openclaw");
    let dest = tmp.path().join("sessions");
    let mem = tmp.path().join("memory").join("sessions");

    let db = Fixture::create(&home, "main")?;
    db.node("agent:main:main", "s1", &serde_json::json!({}))?;
    db.window("agent:main:main", "s1", 1)?;
    db.cold_event_row("s1", 1)?;
    db.event("s1", 2, &message("user", "still here"))?;
    db.cold_archive("s1", "file", "../escape.jsonl.zst", None)?;

    let report = import_sessions(
        &detection(&home, &["main"]),
        &dest,
        &mem,
        &identity_mapping(&["main"]),
    );
    assert_eq!(report.status, ImportStatus::Success, "{:?}", report.errors);
    assert_eq!(report.warnings.len(), 1);
    assert_eq!(contents(&history(&dest, "oc:main:s1")?), ["still here"]);
    Ok(())
}

#[test]
fn sqlite_wins_over_jsonl_without_duplicates() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let home = tmp.path().join("openclaw");
    let dest = tmp.path().join("sessions");
    let mem = tmp.path().join("memory").join("sessions");

    let db = Fixture::create(&home, "main")?;
    db.node("agent:main:main", "shared", &serde_json::json!({}))?;
    db.window("agent:main:main", "shared", 1)?;
    db.event("shared", 1, &message("user", "from sqlite"))?;

    let jsonl_dir = home
        .join("agents")
        .join("main")
        .join("agent")
        .join("sessions");
    std::fs::create_dir_all(&jsonl_dir)?;
    std::fs::write(
        jsonl_dir.join("shared.jsonl"),
        message("user", "from jsonl").to_string(),
    )?;
    std::fs::write(
        jsonl_dir.join("legacy.jsonl"),
        message("user", "jsonl only").to_string(),
    )?;
    std::fs::write(jsonl_dir.join("gone.jsonl.deleted.1700000000.zst"), b"x")?;

    let found = detect_at(home.clone()).ok_or("not detected")?;
    assert_eq!(found.session_count, 2);

    let report = import_sessions(&found, &dest, &mem, &identity_mapping(&["main"]));
    assert_eq!(report.status, ImportStatus::Success, "{:?}", report.errors);
    assert_eq!(report.items_imported, 2);
    assert_eq!(contents(&history(&dest, "oc:main:shared")?), [
        "from sqlite"
    ]);
    assert_eq!(contents(&history(&dest, "oc:main:legacy")?), ["jsonl only"]);
    Ok(())
}

#[test]
fn reimport_skips_unchanged_and_updates_grown_sessions() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let home = tmp.path().join("openclaw");
    let dest = tmp.path().join("sessions");
    let mem = tmp.path().join("memory").join("sessions");
    let found = detection(&home, &["main"]);
    let mapping = identity_mapping(&["main"]);

    let db = Fixture::create(&home, "main")?;
    db.node("agent:main:main", "s1", &serde_json::json!({}))?;
    db.window("agent:main:main", "s1", 1)?;
    db.event("s1", 1, &message("user", "one"))?;

    assert_eq!(
        import_sessions(&found, &dest, &mem, &mapping).items_imported,
        1
    );
    let first_id = metadata(&dest)?
        .get("oc:main:s1")
        .ok_or("missing s1")?
        .id
        .clone();

    let again = import_sessions(&found, &dest, &mem, &mapping);
    assert_eq!(again.status, ImportStatus::Skipped);
    assert_eq!(again.items_skipped, 1);

    db.event("s1", 2, &message("assistant", "two"))?;
    let grown = import_sessions(&found, &dest, &mem, &mapping);
    assert_eq!(grown.items_updated, 1);
    assert_eq!(contents(&history(&dest, "oc:main:s1")?), ["one", "two"]);
    let entry = metadata(&dest)?;
    let entry = entry.get("oc:main:s1").ok_or("missing s1")?;
    assert_eq!(entry.id, first_id);
    assert_eq!(entry.version, 1);
    Ok(())
}

#[test]
fn unreadable_database_is_reported() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let home = tmp.path().join("openclaw");
    let dest = tmp.path().join("sessions");
    let mem = tmp.path().join("memory").join("sessions");

    let dir = home.join("agents").join("main").join("agent");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(AGENT_DB_FILE), b"not a database")?;

    let found = detect_at(home.clone()).ok_or("not detected")?;
    assert_eq!(found.session_count, 0);

    let report = import_sessions(&found, &dest, &mem, &identity_mapping(&["main"]));
    assert_eq!(report.status, ImportStatus::Failed);
    assert_eq!(report.errors.len(), 1);
    Ok(())
}

#[test]
fn child_sessions_point_at_the_parent_transcript_of_their_time() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let home = tmp.path().join("openclaw");
    let dest = tmp.path().join("sessions");
    let mem = tmp.path().join("memory").join("sessions");

    let parent_key = "agent:main:telegram:dm:42";
    let main = Fixture::create(&home, "main")?;
    main.node(parent_key, "p-new", &serde_json::json!({}))?;
    main.window(parent_key, "p-old", 10)?;
    main.window(parent_key, "p-new", 30)?;
    main.event("p-old", 1, &message("user", "spawn a scan"))?;
    main.event("p-new", 1, &message("user", "after reset"))?;
    main.node(
        "agent:main:dashboard:child",
        "c-dash",
        &serde_json::json!({"parentSessionKey": parent_key, "spawnedBy": "agent:main:other"}),
    )?;
    main.window("agent:main:dashboard:child", "c-dash", 35)?;
    main.event("c-dash", 1, &message("user", "child"))?;

    let cyber = Fixture::create(&home, "cyberclaw")?;
    for (key, id, created_at, spawned_by) in [
        ("agent:cyberclaw:subagent:a", "c-early", 20, parent_key),
        ("agent:cyberclaw:subagent:b", "c-late", 40, parent_key),
        (
            "agent:cyberclaw:subagent:c",
            "c-orphan",
            50,
            "agent:gone:main",
        ),
    ] {
        cyber.node(key, id, &serde_json::json!({"spawnedBy": spawned_by}))?;
        cyber.window(key, id, created_at)?;
        cyber.event(id, 1, &message("user", "task"))?;
    }

    let agents = ["cyberclaw", "main"];
    let report = import_sessions(
        &detection(&home, &agents),
        &dest,
        &mem,
        &identity_mapping(&agents),
    );
    assert_eq!(report.status, ImportStatus::Success, "{:?}", report.errors);

    let meta = metadata(&dest)?;
    let parent_of = |key: &str| {
        meta.get(key)
            .and_then(|entry| entry.parent_session_key.clone())
    };
    assert_eq!(
        parent_of("oc:cyberclaw:c-early").as_deref(),
        Some("oc:main:p-old")
    );
    assert_eq!(
        parent_of("oc:cyberclaw:c-late").as_deref(),
        Some("oc:main:p-new")
    );
    assert_eq!(
        parent_of("oc:main:c-dash").as_deref(),
        Some("oc:main:p-new")
    );
    assert_eq!(parent_of("oc:cyberclaw:c-orphan"), None);
    assert_eq!(parent_of("oc:main:p-new"), None);
    Ok(())
}
