//! Import OpenClaw sessions to Antre format.
//!
//! OpenClaw sessions live at either:
//! - `~/.openclaw/agents/<id>/agent/openclaw-agent.sqlite` (current releases,
//!   see [`sqlite`]),
//! - `~/.openclaw/agents/<id>/sessions/<key>.jsonl` (legacy layout), or
//! - `~/.openclaw/agents/<id>/agent/sessions/<key>.jsonl` (newer JSONL layout).
//!
//! Antre sessions live at `<data_dir>/sessions/<safe-key>.jsonl` with metadata
//! in `metadata.json`.

pub(crate) mod sqlite;

use std::{
    collections::{HashMap, HashSet},
    io::{BufRead, BufReader, Write},
    path::Path,
};

use {
    serde::{Deserialize, Serialize},
    tracing::{debug, warn},
};

use crate::{
    detect::{OpenClawDetection, resolve_agent_sessions_dir},
    identity::normalize_display_name,
    report::{CategoryReport, ImportCategory, ImportStatus},
    types::{
        OpenClawContent, OpenClawRole, OpenClawSessionIndexEntry, OpenClawSessionOrigin,
        OpenClawSessionRecord, OpenClawTimestamp,
    },
};

/// Minimal session metadata for the Antre `metadata.json` index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportedSessionEntry {
    pub id: String,
    pub key: String,
    pub label: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    pub message_count: u32,
    #[serde(default)]
    pub last_seen_message_count: u32,
    #[serde(default)]
    pub source_line_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub archived: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_key: Option<String>,
    #[serde(default)]
    pub version: u64,
}

/// A converted Antre message (matches `PersistedMessage` serde format).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "role", rename_all = "lowercase")]
enum AntreMessage {
    System {
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        created_at: Option<u64>,
    },
    User {
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        created_at: Option<u64>,
    },
    Assistant {
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        created_at: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
    },
    Tool {
        tool_call_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        created_at: Option<u64>,
    },
}

/// Import sessions from all agents in an OpenClaw installation.
///
/// In addition to converting transcripts, this also generates markdown
/// transcripts in `memory_sessions_dir` (typically `<data>/memory/sessions/`)
/// so that imported conversations are searchable by the Antre memory system.
///
/// Each agent's `agent/openclaw-agent.sqlite` database is read first; JSONL
/// files whose session ID was already found there are skipped.
///
/// `agent_id_mapping` maps OpenClaw agent IDs to Antre agent IDs. If empty,
/// agent IDs are used as-is (backward compatible for single-agent installs).
pub fn import_sessions(
    detection: &OpenClawDetection,
    dest_sessions_dir: &Path,
    memory_sessions_dir: &Path,
    agent_id_mapping: &HashMap<String, String>,
) -> CategoryReport {
    if detection.agent_ids.is_empty() {
        return CategoryReport::skipped(ImportCategory::Sessions);
    }

    if let Err(e) = std::fs::create_dir_all(dest_sessions_dir) {
        return CategoryReport::failed(
            ImportCategory::Sessions,
            format!("failed to create destination directory: {e}"),
        );
    }

    let mut importer = SessionImporter::new(dest_sessions_dir, memory_sessions_dir);

    // First pass: read every agent's index, so a child session can point at a
    // parent stored by another agent.
    let mut agents = Vec::new();
    let mut parents = ParentIndex::default();
    for openclaw_agent_id in &detection.agent_ids {
        let antre_agent_id = agent_id_mapping
            .get(openclaw_agent_id.as_str())
            .cloned()
            .unwrap_or_else(|| openclaw_agent_id.clone());
        let agent_dir = detection.home_dir.join("agents").join(openclaw_agent_id);

        let sqlite = sqlite::AgentSessions::load(&mut importer, &agent_dir, openclaw_agent_id);
        if let Some(sqlite) = &sqlite {
            sqlite.index_parents(&antre_agent_id, &mut parents);
        }
        let jsonl_index = resolve_agent_sessions_dir(&agent_dir)
            .map(|dir| load_session_label_map(&dir))
            .unwrap_or_default();
        for (session_id, entry) in &jsonl_index {
            // Already registered under its SQLite conversation.
            if sqlite.as_ref().is_some_and(|db| db.has_window(session_id)) {
                continue;
            }
            parents.insert(&entry.logical_key, dest_key(&antre_agent_id, session_id));
        }
        agents.push((
            openclaw_agent_id,
            antre_agent_id,
            agent_dir,
            sqlite,
            jsonl_index,
        ));
    }

    for (openclaw_agent_id, antre_agent_id, agent_dir, sqlite, jsonl_index) in agents {
        let sqlite_session_ids = sqlite
            .map(|sqlite| sqlite.import(&mut importer, &antre_agent_id, &parents))
            .unwrap_or_default();
        import_agent_jsonl_sessions(
            &mut importer,
            &agent_dir,
            openclaw_agent_id,
            &antre_agent_id,
            &jsonl_index,
            &parents,
            &sqlite_session_ids,
        );
    }

    importer.finish()
}

fn import_agent_jsonl_sessions(
    importer: &mut SessionImporter<'_>,
    agent_dir: &Path,
    openclaw_agent_id: &str,
    antre_agent_id: &str,
    session_index: &HashMap<String, JsonlIndexEntry>,
    parents: &ParentIndex,
    already_imported: &HashSet<String>,
) {
    let Some(sessions_dir) = resolve_agent_sessions_dir(agent_dir) else {
        debug!(agent = %openclaw_agent_id, "no sessions directory found, skipping");
        return;
    };

    let Ok(dir_entries) = std::fs::read_dir(&sessions_dir) else {
        debug!(agent = %openclaw_agent_id, "failed to read sessions directory, skipping");
        return;
    };

    for entry in dir_entries.flatten() {
        let path = entry.path();
        if !path.is_file() || path.extension().is_some_and(|e| e != "jsonl") {
            continue;
        }

        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");
        if already_imported.contains(stem) {
            debug!(agent = %openclaw_agent_id, session = stem, "session already read from SQLite, skipping JSONL copy");
            continue;
        }

        let index_entry = session_index.get(stem);
        let label = index_entry
            .map(|entry| build_session_label(&entry.logical_key, entry.origin.as_ref()))
            .unwrap_or_else(|| {
                let prefix_len = 8.min(stem.len());
                format!("OpenClaw: {}", &stem[..prefix_len])
            });
        let dest_key = dest_key(antre_agent_id, stem);
        let parent_key = index_entry
            .and_then(|entry| entry.parent_key.as_deref())
            .and_then(|parent| parents.resolve(parent))
            .filter(|parent| *parent != dest_key);

        importer.import_one(
            SessionSource {
                dest_key,
                antre_agent_id,
                label,
                source_line_count: count_lines(&path),
                archived: false,
                parent_key,
                description: path.display().to_string(),
            },
            |sink| {
                let reader = BufReader::new(std::fs::File::open(&path)?);
                for line in reader.lines() {
                    sink.push_line(&line?)?;
                }
                Ok(())
            },
        );
    }
}

/// Where one OpenClaw session comes from and how it is labelled in Antre.
pub(crate) struct SessionSource<'a> {
    pub(crate) dest_key: String,
    pub(crate) antre_agent_id: &'a str,
    pub(crate) label: String,
    /// Size of the source (JSONL lines or SQLite events), used to skip
    /// sessions that did not grow since the previous import.
    pub(crate) source_line_count: u32,
    pub(crate) archived: bool,
    /// Antre key of the parent session, for subagent and child sessions.
    pub(crate) parent_key: Option<String>,
    /// Human-readable source location for error messages.
    pub(crate) description: String,
}

/// Shared bookkeeping for one import run: metadata, counters and errors.
pub(crate) struct SessionImporter<'a> {
    dest_sessions_dir: &'a Path,
    memory_sessions_dir: &'a Path,
    metadata_path: std::path::PathBuf,
    existing_metadata: HashMap<String, ImportedSessionEntry>,
    entries: Vec<ImportedSessionEntry>,
    imported: usize,
    updated: usize,
    skipped: usize,
    errors: Vec<String>,
    warnings: Vec<String>,
}

impl<'a> SessionImporter<'a> {
    fn new(dest_sessions_dir: &'a Path, memory_sessions_dir: &'a Path) -> Self {
        // Load existing metadata once to detect incremental changes
        let metadata_path = dest_sessions_dir.join("metadata.json");
        let existing_metadata = load_session_metadata(&metadata_path);
        Self {
            dest_sessions_dir,
            memory_sessions_dir,
            metadata_path,
            existing_metadata,
            entries: Vec::new(),
            imported: 0,
            updated: 0,
            skipped: 0,
            errors: Vec::new(),
            warnings: Vec::new(),
        }
    }

    pub(crate) fn warn(&mut self, message: String) {
        self.warnings.push(message);
    }

    /// Convert one session unless it is unchanged since the last import.
    ///
    /// `feed` pushes the source records (JSONL lines) into the sink.
    pub(crate) fn import_one(
        &mut self,
        source: SessionSource<'_>,
        feed: impl FnOnce(&mut SessionSink) -> crate::error::Result<()>,
    ) {
        let dest_key = source.dest_key;
        let dest_file =
            antre_sessions::store::SessionStore::new(self.dest_sessions_dir.to_path_buf())
                .history_path_for(&dest_key);
        if let Some(parent) = dest_file.parent()
            && let Err(error) = std::fs::create_dir_all(parent)
        {
            warn!(%error, key = %dest_key, "failed to create session storage directory");
            self.errors.push(format!("{dest_key}: {error}"));
            return;
        }

        // Check if we already have this session and whether it has grown
        let existing_entry = self.existing_metadata.get(&dest_key);
        let is_update = if let Some(prev) = existing_entry {
            if prev.source_line_count > 0 && source.source_line_count <= prev.source_line_count {
                debug!(key = %dest_key, "session unchanged, skipping");
                self.skipped += 1;
                return;
            }
            true
        } else {
            false
        };

        let stats = match SessionSink::create(&dest_file).and_then(|mut sink| {
            feed(&mut sink)?;
            Ok(sink.finish())
        }) {
            Ok(stats) => stats,
            Err(e) => {
                warn!(source = %source.description, error = %e, "failed to convert session");
                self.errors
                    .push(format!("failed to convert {}: {e}", source.description));
                return;
            },
        };

        let label = source.label;
        if is_update {
            debug!(key = %dest_key, messages = stats.message_count, "updated session (incremental)");
        } else {
            debug!(key = %dest_key, messages = stats.message_count, "imported session");
        }

        if !stats.transcript.is_empty()
            && let Err(e) = write_transcript(
                self.memory_sessions_dir,
                &dest_key,
                &label,
                stats.last_model.as_deref(),
                &stats,
            )
        {
            warn!(key = %dest_key, error = %e, "failed to write session transcript");
        }

        let (id, created_at, version) = if let Some(prev) = existing_entry {
            (
                prev.id.clone(),
                prev.created_at,
                prev.version.saturating_add(1),
            )
        } else {
            (uuid_v4(), stats.first_timestamp.unwrap_or_else(now_ms), 0)
        };

        // Set agent_id for non-default agents
        let agent_id = (source.antre_agent_id != "main").then(|| source.antre_agent_id.to_string());

        self.entries.push(ImportedSessionEntry {
            id,
            key: dest_key,
            label: Some(label),
            model: stats.last_model,
            preview: stats.preview,
            created_at,
            updated_at: stats.last_timestamp.unwrap_or_else(now_ms),
            message_count: stats.message_count,
            last_seen_message_count: stats.message_count,
            source_line_count: source.source_line_count,
            agent_id,
            archived: source.archived,
            parent_session_key: source.parent_key,
            version,
        });
        if is_update {
            self.updated += 1;
        } else {
            self.imported += 1;
        }
    }

    fn finish(mut self) -> CategoryReport {
        // Write/merge session metadata
        if !self.entries.is_empty()
            && let Err(e) = merge_session_metadata(&self.metadata_path, &self.entries)
        {
            self.errors
                .push(format!("failed to update metadata.json: {e}"));
        }

        let total_changed = self.imported + self.updated;
        let status = if !self.errors.is_empty() && total_changed > 0 {
            ImportStatus::Partial
        } else if !self.errors.is_empty() {
            ImportStatus::Failed
        } else if total_changed == 0 {
            ImportStatus::Skipped
        } else {
            ImportStatus::Success
        };

        CategoryReport {
            category: ImportCategory::Sessions,
            status,
            items_imported: self.imported,
            items_updated: self.updated,
            items_skipped: self.skipped,
            warnings: self.warnings,
            errors: self.errors,
        }
    }
}

/// What `sessions.json` says about one session ID.
struct JsonlIndexEntry {
    logical_key: String,
    origin: Option<OpenClawSessionOrigin>,
    parent_key: Option<String>,
}

/// Load the `sessions.json` index and build a reverse map from session-ID
/// (UUID filename stem) to the logical key and origin metadata.
fn load_session_label_map(sessions_dir: &Path) -> HashMap<String, JsonlIndexEntry> {
    let index_path = sessions_dir.join("sessions.json");
    let Ok(content) = std::fs::read_to_string(&index_path) else {
        return HashMap::new();
    };
    let Ok(entries): Result<HashMap<String, OpenClawSessionIndexEntry>, _> =
        serde_json::from_str(&content)
    else {
        return HashMap::new();
    };

    entries
        .into_iter()
        .filter_map(|(logical_key, entry)| {
            let session_id = entry.session_id.clone()?;
            Some((session_id, JsonlIndexEntry {
                parent_key: entry.parent_key().map(str::to_string),
                logical_key,
                origin: entry.origin,
            }))
        })
        .collect()
}

fn dest_key(antre_agent_id: &str, session_id: &str) -> String {
    format!("oc:{antre_agent_id}:{session_id}")
}

/// Antre key of each OpenClaw logical session key, so a child session can
/// point at its parent. The first registration wins: SQLite conversations
/// are registered before JSONL files.
#[derive(Default)]
struct ParentIndex(HashMap<String, String>);

impl ParentIndex {
    fn insert(&mut self, logical_key: &str, dest_key: String) {
        self.0.entry(logical_key.to_string()).or_insert(dest_key);
    }

    fn resolve(&self, parent_key: &str) -> Option<String> {
        self.0.get(parent_key).cloned()
    }
}

/// Build a human-readable session label from the logical key and origin.
///
/// Key patterns:
/// - `agent:*:main`             → "Main"
/// - `agent:*:telegram:dm:*`    → "Telegram: {cleaned label}"
/// - `agent:*:signal:dm:*`      → "Signal: {cleaned label}"
/// - `agent:*:cron:*`           → "Cron: {cleaned label}"
/// - Other with origin label    → "OpenClaw: {cleaned label}"
/// - Fallback (no index entry)  → "OpenClaw: {uuid prefix}"
fn build_session_label(logical_key: &str, origin: Option<&OpenClawSessionOrigin>) -> String {
    let parts: Vec<&str> = logical_key.split(':').collect();

    // agent:<name>:main → "Main"
    if parts.len() == 3 && parts[0] == "agent" && parts[2] == "main" {
        return "Main".to_string();
    }

    // agent:<name>:telegram:dm:<id> → "Telegram: {label}"
    if parts.len() >= 4 && parts[0] == "agent" && parts[2] == "telegram" {
        if let Some(cleaned) = origin
            .and_then(|o| o.label.as_deref())
            .and_then(normalize_display_name)
        {
            return format!("Telegram: {cleaned}");
        }
        return "Telegram".to_string();
    }

    // agent:<name>:signal:dm:<id> → "Signal: {label}"
    if parts.len() >= 4 && parts[0] == "agent" && parts[2] == "signal" {
        if let Some(cleaned) = origin
            .and_then(|o| o.label.as_deref())
            .and_then(normalize_display_name)
        {
            return format!("Signal: {cleaned}");
        }
        return "Signal".to_string();
    }

    // agent:<name>:cron:* → "Cron: {label}"
    if parts.len() >= 3 && parts[0] == "agent" && parts[2] == "cron" {
        if let Some(cleaned) = origin
            .and_then(|o| o.label.as_deref())
            .and_then(normalize_display_name)
        {
            return format!("Cron: {cleaned}");
        }
        return "Cron".to_string();
    }

    // Fallback: use cleaned origin label if available
    if let Some(cleaned) = origin
        .and_then(|o| o.label.as_deref())
        .and_then(normalize_display_name)
    {
        return format!("OpenClaw: {cleaned}");
    }

    format!("OpenClaw: {logical_key}")
}

struct ConvertStats {
    message_count: u32,
    first_timestamp: Option<u64>,
    last_timestamp: Option<u64>,
    last_model: Option<String>,
    preview: Option<String>,
    /// Collected transcript entries for markdown export.
    transcript: Vec<TranscriptEntry>,
}

/// A single entry in a session transcript (for markdown export).
struct TranscriptEntry {
    role: &'static str,
    content: String,
}

/// Writes converted Antre messages while collecting stats.
pub(crate) struct SessionSink {
    dest_file: std::io::BufWriter<std::fs::File>,
    stats: ConvertStats,
}

impl SessionSink {
    fn create(dest: &Path) -> crate::error::Result<Self> {
        Ok(Self {
            dest_file: std::io::BufWriter::new(std::fs::File::create(dest)?),
            stats: ConvertStats {
                message_count: 0,
                first_timestamp: None,
                last_timestamp: None,
                last_model: None,
                preview: None,
                transcript: Vec::new(),
            },
        })
    }

    /// Convert one OpenClaw transcript record (a JSONL line). Blank and
    /// malformed records are skipped.
    pub(crate) fn push_line(&mut self, line: &str) -> crate::error::Result<()> {
        if line.trim().is_empty() {
            return Ok(());
        }

        let Ok(record) = serde_json::from_str::<OpenClawSessionRecord>(line) else {
            return Ok(());
        };

        match record {
            OpenClawSessionRecord::Message { timestamp, message } => {
                let message_timestamp = message
                    .timestamp
                    .as_ref()
                    .and_then(OpenClawTimestamp::to_millis)
                    .or_else(|| timestamp.as_ref().and_then(OpenClawTimestamp::to_millis));
                if let Some(msg) = convert_message(&message, message_timestamp, &mut self.stats) {
                    let json = serde_json::to_string(&msg)?;
                    writeln!(self.dest_file, "{json}")?;
                    self.stats.message_count += 1;
                }
            },
            OpenClawSessionRecord::Custom { custom_type, data } => {
                // Extract model name from model-snapshot records
                if custom_type.as_deref() == Some("model-snapshot")
                    && let Some(model) = data
                        .as_ref()
                        .and_then(|d| d.get("model"))
                        .and_then(|m| m.as_str())
                {
                    self.stats.last_model = Some(model.to_string());
                }
            },
            OpenClawSessionRecord::SessionMeta { .. } => {
                // Session metadata is used for detection/scan only
            },
        }
        Ok(())
    }

    fn finish(mut self) -> ConvertStats {
        if let Err(error) = self.dest_file.flush() {
            warn!(%error, "failed to flush converted session");
        }
        self.stats.preview = build_preview(&self.stats.transcript);
        self.stats
    }
}

fn convert_message(
    msg: &crate::types::OpenClawMessage,
    timestamp_ms: Option<u64>,
    stats: &mut ConvertStats,
) -> Option<AntreMessage> {
    let content = msg.content.as_ref().map(OpenClawContent::as_text)?;
    if content.is_empty() {
        return None;
    }

    let created_at = timestamp_ms.unwrap_or_else(now_ms);
    if stats.first_timestamp.is_none() {
        stats.first_timestamp = Some(created_at);
    }
    stats.last_timestamp = Some(created_at);

    // Collect user/assistant messages for the markdown transcript
    let role_label = match msg.role {
        OpenClawRole::User => Some("User"),
        OpenClawRole::Assistant => Some("Assistant"),
        _ => None,
    };
    if let Some(label) = role_label {
        stats.transcript.push(TranscriptEntry {
            role: label,
            content: content.clone(),
        });
    }

    match msg.role {
        OpenClawRole::System => Some(AntreMessage::System {
            content,
            created_at: Some(created_at),
        }),
        OpenClawRole::User => Some(AntreMessage::User {
            content,
            created_at: Some(created_at),
        }),
        OpenClawRole::Assistant => Some(AntreMessage::Assistant {
            content,
            created_at: Some(created_at),
            model: None,
            provider: None,
        }),
        OpenClawRole::Tool | OpenClawRole::ToolResult => {
            let tool_call_id = msg.tool_use_id.clone().unwrap_or_default();
            Some(AntreMessage::Tool {
                tool_call_id,
                content,
                created_at: Some(created_at),
            })
        },
    }
}

/// Count lines in a file without parsing content.
fn count_lines(path: &Path) -> u32 {
    let Ok(file) = std::fs::File::open(path) else {
        return 0;
    };
    BufReader::new(file).lines().count() as u32
}

/// Load existing session metadata from disk.
fn load_session_metadata(path: &Path) -> HashMap<String, ImportedSessionEntry> {
    if !path.is_file() {
        return HashMap::new();
    }
    let Ok(content) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    serde_json::from_str(&content).unwrap_or_default()
}

fn merge_session_metadata(
    path: &Path,
    new_entries: &[ImportedSessionEntry],
) -> crate::error::Result<()> {
    let mut existing = load_session_metadata(path);

    for entry in new_entries {
        existing.insert(entry.key.clone(), entry.clone());
    }

    let json = serde_json::to_string_pretty(&existing)?;
    std::fs::write(path, json)?;
    Ok(())
}

fn build_preview(transcript: &[TranscriptEntry]) -> Option<String> {
    const TARGET_CHARS: usize = 140;
    const MAX_CHARS: usize = 200;

    let mut combined = String::new();
    for entry in transcript {
        let normalized = normalize_whitespace(&entry.content);
        if normalized.is_empty() {
            continue;
        }
        if !combined.is_empty() {
            combined.push(' ');
        }
        combined.push_str(&normalized);
        if combined.chars().count() >= TARGET_CHARS {
            break;
        }
    }

    if combined.is_empty() {
        return None;
    }

    Some(truncate_preview(&combined, MAX_CHARS))
}

fn normalize_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_preview(text: &str, max_chars: usize) -> String {
    let len = text.chars().count();
    if len <= max_chars {
        return text.to_string();
    }

    let cutoff = max_chars.saturating_sub(3);
    let mut out = String::new();
    for ch in text.chars().take(cutoff) {
        out.push(ch);
    }
    out.push_str("...");
    out
}

/// Write a markdown transcript of a session for memory search indexing.
///
/// The file is placed in `memory/sessions/` and includes all user/assistant
/// messages so they become searchable by the Antre memory system.
fn write_transcript(
    dir: &Path,
    dest_key: &str,
    label: &str,
    model: Option<&str>,
    stats: &ConvertStats,
) -> crate::error::Result<()> {
    std::fs::create_dir_all(dir)?;

    // Use hyphens instead of colons for filesystem safety
    let safe_name = dest_key.replace(':', "-");
    let path = dir.join(format!("{safe_name}.md"));

    let mut content = format!("# Session: {label}\n\n");
    content.push_str("*Imported from OpenClaw*");
    if let Some(m) = model {
        content.push_str(&format!(" | Model: {m}"));
    }
    content.push_str(&format!(" | Messages: {}\n\n---\n\n", stats.message_count));

    for entry in &stats.transcript {
        content.push_str(&format!("**{}:** {}\n\n", entry.role, entry.content));
    }

    std::fs::write(path, content)?;
    Ok(())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn uuid_v4() -> String {
    // Simple UUID v4 without pulling in the uuid crate.
    // Format: 8-4-4-4-12 hex characters.
    use std::time::{SystemTime, UNIX_EPOCH};
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        (seed >> 96) as u32,
        (seed >> 80) as u16,
        (seed >> 64) as u16 & 0x0FFF,
        ((seed >> 48) as u16 & 0x3FFF) | 0x8000,
        seed as u64 & 0xFFFF_FFFF_FFFF,
    )
}

#[cfg(test)]
mod tests;
