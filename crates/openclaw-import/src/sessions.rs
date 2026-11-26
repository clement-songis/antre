//! Import session JSONL files from OpenClaw to Moltis format.
//!
//! OpenClaw sessions live at either:
//! - `~/.openclaw/agents/<id>/sessions/<key>.jsonl` (legacy layout), or
//! - `~/.openclaw/agents/<id>/agent/sessions/<key>.jsonl` (newer layout).
//!
//! Moltis sessions live at `<data_dir>/sessions/<safe-key>.jsonl` with metadata
//! in `metadata.json`.

use std::{
    collections::HashMap,
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

/// Minimal session metadata for the Moltis `metadata.json` index.
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
    #[serde(default)]
    pub version: u64,
}

/// A converted Moltis message (matches `PersistedMessage` serde format).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "role", rename_all = "lowercase")]
enum MoltisMessage {
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
/// In addition to converting JSONL files, this also generates markdown
/// transcripts in `memory_sessions_dir` (typically `<data>/memory/sessions/`)
/// so that imported conversations are searchable by the Moltis memory system.
///
/// `agent_id_mapping` maps OpenClaw agent IDs to Moltis agent IDs. If empty,
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

    let mut imported = 0;
    let mut updated = 0;
    let mut skipped = 0;
    let mut errors = Vec::new();
    let warnings = Vec::new();
    let mut entries = Vec::new();

    if let Err(e) = std::fs::create_dir_all(dest_sessions_dir) {
        return CategoryReport::failed(
            ImportCategory::Sessions,
            format!("failed to create destination directory: {e}"),
        );
    }

    // Load existing metadata once to detect incremental changes
    let metadata_path = dest_sessions_dir.join("metadata.json");
    let existing_metadata = load_session_metadata(&metadata_path);

    for openclaw_agent_id in &detection.agent_ids {
        let moltis_agent_id = agent_id_mapping
            .get(openclaw_agent_id.as_str())
            .cloned()
            .unwrap_or_else(|| openclaw_agent_id.clone());

        let agent_dir = detection.home_dir.join("agents").join(openclaw_agent_id);
        let Some(sessions_dir) = resolve_agent_sessions_dir(&agent_dir) else {
            debug!(agent = %openclaw_agent_id, "no sessions directory found, skipping");
            continue;
        };

        let Ok(dir_entries) = std::fs::read_dir(&sessions_dir) else {
            debug!(agent = %openclaw_agent_id, "failed to read sessions directory, skipping");
            continue;
        };

        let session_label_map = load_session_label_map(&sessions_dir);

        for entry in dir_entries.flatten() {
            let path = entry.path();
            if !path.is_file() || path.extension().is_some_and(|e| e != "jsonl") {
                continue;
            }

            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown");
            let dest_key = format!("oc:{moltis_agent_id}:{stem}");
            let dest_file =
                moltis_sessions::store::SessionStore::new(dest_sessions_dir.to_path_buf())
                    .history_path_for(&dest_key);
            if let Some(parent) = dest_file.parent()
                && let Err(error) = std::fs::create_dir_all(parent)
            {
                warn!(%error, key = %dest_key, "failed to create session storage directory");
                errors.push(format!("{dest_key}: {error}"));
                continue;
            }

            let source_lines = count_lines(&path);

            // Check if we already have this session and whether it has grown
            let existing_entry = existing_metadata.get(&dest_key);
            let is_update = if let Some(prev) = existing_entry {
                if prev.source_line_count > 0 && source_lines <= prev.source_line_count {
                    debug!(key = %dest_key, "session unchanged, skipping");
                    skipped += 1;
                    continue;
                }
                true
            } else {
                false
            };

            match convert_session(&path, &dest_file) {
                Ok(stats) => {
                    let label = session_label_map
                        .get(stem)
                        .map(|(logical_key, origin)| {
                            build_session_label(logical_key, origin.as_ref())
                        })
                        .unwrap_or_else(|| {
                            let prefix_len = 8.min(stem.len());
                            format!("OpenClaw: {}", &stem[..prefix_len])
                        });

                    if is_update {
                        debug!(key = %dest_key, messages = stats.message_count, "updated session (incremental)");
                    } else {
                        debug!(key = %dest_key, messages = stats.message_count, "imported session");
                    }

                    if !stats.transcript.is_empty()
                        && let Err(e) = write_transcript(
                            memory_sessions_dir,
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
                    let agent_id = if moltis_agent_id == "main" {
                        None
                    } else {
                        Some(moltis_agent_id.clone())
                    };

                    entries.push(ImportedSessionEntry {
                        id,
                        key: dest_key,
                        label: Some(label),
                        model: stats.last_model,
                        preview: stats.preview,
                        created_at,
                        updated_at: stats.last_timestamp.unwrap_or_else(now_ms),
                        message_count: stats.message_count,
                        last_seen_message_count: stats.message_count,
                        source_line_count: source_lines,
                        agent_id,
                        archived: false,
                        version,
                    });
                    if is_update {
                        updated += 1;
                    } else {
                        imported += 1;
                    }
                },
                Err(e) => {
                    warn!(source = %path.display(), error = %e, "failed to convert session");
                    errors.push(format!("failed to convert {}: {e}", path.display()));
                },
            }
        }
    }

    // Write/merge session metadata
    if !entries.is_empty()
        && let Err(e) = merge_session_metadata(&metadata_path, &entries)
    {
        errors.push(format!("failed to update metadata.json: {e}"));
    }

    let total_changed = imported + updated;
    let status = if !errors.is_empty() && total_changed > 0 {
        ImportStatus::Partial
    } else if !errors.is_empty() {
        ImportStatus::Failed
    } else if total_changed == 0 {
        ImportStatus::Skipped
    } else {
        ImportStatus::Success
    };

    CategoryReport {
        category: ImportCategory::Sessions,
        status,
        items_imported: imported,
        items_updated: updated,
        items_skipped: skipped,
        warnings,
        errors,
    }
}

/// Load the `sessions.json` index and build a reverse map from session-ID
/// (UUID filename stem) to the logical key and origin metadata.
fn load_session_label_map(
    sessions_dir: &Path,
) -> HashMap<String, (String, Option<OpenClawSessionOrigin>)> {
    let index_path = sessions_dir.join("sessions.json");
    let Ok(content) = std::fs::read_to_string(&index_path) else {
        return HashMap::new();
    };
    let Ok(entries): Result<HashMap<String, OpenClawSessionIndexEntry>, _> =
        serde_json::from_str(&content)
    else {
        return HashMap::new();
    };

    let mut map = HashMap::new();
    for (logical_key, entry) in entries {
        if let Some(ref sid) = entry.session_id {
            map.insert(sid.clone(), (logical_key, entry.origin));
        }
    }
    map
}

/// Build a human-readable session label from the logical key and origin.
///
/// Key patterns:
/// - `agent:*:main`             → "Main"
/// - `agent:*:telegram:dm:*`    → "Telegram: {cleaned label}"
/// - `agent:*:signal:dm:*`      → "Signal: {cleaned label}"
/// - `agent:*:cron:*`           → "Cron"
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

    // agent:<name>:cron:* → "Cron"
    if parts.len() >= 3 && parts[0] == "agent" && parts[2] == "cron" {
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

fn convert_session(src: &Path, dest: &Path) -> crate::error::Result<ConvertStats> {
    let file = std::fs::File::open(src)?;
    let reader = BufReader::new(file);

    let mut dest_file = std::fs::File::create(dest)?;
    let mut stats = ConvertStats {
        message_count: 0,
        first_timestamp: None,
        last_timestamp: None,
        last_model: None,
        preview: None,
        transcript: Vec::new(),
    };

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let record: OpenClawSessionRecord = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(_) => continue, // Skip malformed lines
        };

        match record {
            OpenClawSessionRecord::Message { timestamp, message } => {
                let message_timestamp = message
                    .timestamp
                    .as_ref()
                    .and_then(OpenClawTimestamp::to_millis)
                    .or_else(|| timestamp.as_ref().and_then(OpenClawTimestamp::to_millis));
                if let Some(msg) = convert_message(&message, message_timestamp, &mut stats) {
                    let json = serde_json::to_string(&msg)?;
                    writeln!(dest_file, "{json}")?;
                    stats.message_count += 1;
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
                    stats.last_model = Some(model.to_string());
                }
            },
            OpenClawSessionRecord::SessionMeta { .. } => {
                // Session metadata is used for detection/scan only
            },
        }
    }

    stats.preview = build_preview(&stats.transcript);

    Ok(stats)
}

fn convert_message(
    msg: &crate::types::OpenClawMessage,
    timestamp_ms: Option<u64>,
    stats: &mut ConvertStats,
) -> Option<MoltisMessage> {
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
        OpenClawRole::System => Some(MoltisMessage::System {
            content,
            created_at: Some(created_at),
        }),
        OpenClawRole::User => Some(MoltisMessage::User {
            content,
            created_at: Some(created_at),
        }),
        OpenClawRole::Assistant => Some(MoltisMessage::Assistant {
            content,
            created_at: Some(created_at),
            model: None,
            provider: None,
        }),
        OpenClawRole::Tool | OpenClawRole::ToolResult => {
            let tool_call_id = msg.tool_use_id.clone().unwrap_or_default();
            Some(MoltisMessage::Tool {
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
/// messages so they become searchable by the Moltis memory system.
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
