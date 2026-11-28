//! Read OpenClaw transcripts from `agents/<id>/agent/openclaw-agent.sqlite`.
//!
//! Table mapping: `docs/src/openclaw-import.md` ("Session storage").

use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

use {
    rusqlite::{Connection, OpenFlags, OptionalExtension, params},
    serde::Deserialize,
    tracing::{debug, warn},
};

use {
    super::{
        ParentIndex, SessionImporter, SessionSink, SessionSource, build_session_label, dest_key,
    },
    crate::{
        error::{Error, Result},
        types::{OpenClawSessionIndexEntry, OpenClawSessionOrigin},
    },
};

/// File name of the per-agent database, under `agents/<id>/agent/`.
pub(crate) const AGENT_DB_FILE: &str = "openclaw-agent.sqlite";

/// OpenClaw's own bound for one decompressed cold archive.
const MAX_COLD_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;

/// OpenClaw's own bound for one decompressed transcript event.
const MAX_EVENT_BYTES: usize = 4 * 1024 * 1024;

/// Windows that hold at least one transcript event.
const WINDOWS_WITH_EVENTS: &str = "SELECT w.session_id FROM session_windows w \
     WHERE EXISTS (SELECT 1 FROM transcript_events e WHERE e.session_id = w.session_id)";

/// Path of the agent database, if this agent has one.
pub(crate) fn agent_db_path(agent_dir: &Path) -> Option<PathBuf> {
    let path = agent_dir.join("agent").join(AGENT_DB_FILE);
    path.is_file().then_some(path)
}

/// IDs of the sessions (transcript windows) stored in the agent database.
///
/// Returns an empty set when the agent has no database or it cannot be read.
pub(crate) fn agent_session_ids(agent_dir: &Path) -> HashSet<String> {
    let Some(db_path) = agent_db_path(agent_dir) else {
        return HashSet::new();
    };
    let ids = open_read_only(&db_path).and_then(|conn| {
        let mut stmt = conn.prepare(WINDOWS_WITH_EVENTS)?;
        let ids = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<HashSet<_>>>()?;
        Ok(ids)
    });
    ids.unwrap_or_else(|error| {
        debug!(path = %db_path.display(), %error, "cannot list OpenClaw SQLite sessions");
        HashSet::new()
    })
}

/// One agent's database and the conversations it holds.
pub(super) struct AgentSessions {
    db_path: PathBuf,
    db: AgentDb,
    conversations: Vec<Conversation>,
}

impl AgentSessions {
    /// Open the agent database and list its windows. Failures are reported
    /// on `importer` and yield `None`.
    pub(super) fn load(
        importer: &mut SessionImporter<'_>,
        agent_dir: &Path,
        openclaw_agent_id: &str,
    ) -> Option<Self> {
        let db_path = agent_db_path(agent_dir)?;
        let loaded = AgentDb::open(&db_path, agent_dir).and_then(|db| {
            let windows = db.list_windows()?;
            Ok((db, windows))
        });
        match loaded {
            Ok((db, windows)) => {
                let conversations = Conversation::group(windows);
                debug!(agent = %openclaw_agent_id, sessions = conversations.len(), "reading OpenClaw SQLite sessions");
                Some(Self {
                    db_path,
                    db,
                    conversations,
                })
            },
            Err(error) => {
                warn!(path = %db_path.display(), %error, "failed to read OpenClaw session database");
                importer
                    .errors
                    .push(format!("failed to read {}: {error}", db_path.display()));
                None
            },
        }
    }

    /// Whether `session_id` is one of this database's transcript windows.
    pub(super) fn has_window(&self, session_id: &str) -> bool {
        self.conversations
            .iter()
            .any(|c| c.windows.iter().any(|w| w.session_id == session_id))
    }

    /// Register every conversation so child sessions can find their parent.
    pub(super) fn index_parents(&self, antre_agent_id: &str, parents: &mut ParentIndex) {
        for conversation in &self.conversations {
            parents.insert(&conversation.key, conversation.dest_key(antre_agent_id));
        }
    }

    /// Import every conversation as one Antre session. Returns the IDs of
    /// all transcript windows found in the database (imported, unchanged or
    /// failed) so the JSONL pass does not import them a second time.
    pub(super) fn import(
        self,
        importer: &mut SessionImporter<'_>,
        antre_agent_id: &str,
        parents: &ParentIndex,
    ) -> HashSet<String> {
        let mut seen = HashSet::new();
        for conversation in self.conversations {
            let mut missing_events = 0_u32;
            let dest_key = conversation.dest_key(antre_agent_id);
            let parent_key = conversation
                .parent_key()
                .and_then(|parent| parents.resolve(conversation_key(parent)))
                .filter(|parent| *parent != dest_key);
            let window_ids: Vec<&str> = conversation
                .windows
                .iter()
                .map(|window| window.session_id.as_str())
                .collect();
            importer.import_one(
                SessionSource {
                    dest_key,
                    antre_agent_id,
                    label: conversation.label(),
                    source_line_count: conversation.source_size(),
                    archived: conversation.archived(),
                    parent_key,
                    description: format!(
                        "{} (session {})",
                        self.db_path.display(),
                        window_ids.join(", ")
                    ),
                },
                |sink| {
                    for id in &window_ids {
                        missing_events += self.db.feed_events(id, sink)?;
                    }
                    Ok(())
                },
            );
            if missing_events > 0 {
                importer.warn(format!(
                    "session {}: {missing_events} archived event(s) could not be read",
                    conversation.key
                ));
            }
            seen.extend(conversation.windows.into_iter().map(|w| w.session_id));
        }
        seen
    }
}

/// The conversation a logical key belongs to. OpenClaw gives each isolated
/// cron run its own key (`agent:<id>:cron:<job>:run:<run>`); runs belong to
/// their job.
fn conversation_key(session_key: &str) -> &str {
    match session_key.split_once(":run:") {
        Some((job, _)) if is_cron_key(job) => job,
        _ => session_key,
    }
}

fn is_cron_key(key: &str) -> bool {
    key.split(':').nth(2) == Some("cron")
}

/// Every transcript window of one logical OpenClaw session, oldest first.
///
/// OpenClaw starts a new window on `/new`, resets and compaction; Antre has
/// no such notion, so the windows become one session in order.
struct Conversation {
    key: String,
    windows: Vec<Window>,
}

impl Conversation {
    /// Group windows (already sorted by creation time) by conversation.
    fn group(windows: Vec<Window>) -> Vec<Self> {
        let mut conversations: Vec<Self> = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();
        for window in windows {
            let key = conversation_key(&window.session_key).to_string();
            match index.get(&key) {
                Some(&i) => conversations[i].windows.push(window),
                None => {
                    index.insert(key.clone(), conversations.len());
                    conversations.push(Self {
                        key,
                        windows: vec![window],
                    });
                },
            }
        }
        conversations
    }

    /// Cron jobs land in Antre' cron list; other conversations keep the key
    /// of their first window, which never changes as windows are added.
    fn dest_key(&self, antre_agent_id: &str) -> String {
        if is_cron_key(&self.key)
            && let Some(job) = self.key.split(':').nth(3)
        {
            return format!("cron:oc-{antre_agent_id}-{job}");
        }
        dest_key(antre_agent_id, &self.first().session_id)
    }

    fn first(&self) -> &Window {
        &self.windows[0]
    }

    fn latest(&self) -> &Window {
        &self.windows[self.windows.len() - 1]
    }

    /// From the conversation's own node, not a cron run's.
    fn label(&self) -> String {
        self.windows
            .iter()
            .rev()
            .find(|window| window.session_key == self.key)
            .unwrap_or_else(|| self.latest())
            .label(&self.key)
    }

    fn archived(&self) -> bool {
        self.windows.iter().all(|window| window.archived)
    }

    fn parent_key(&self) -> Option<&str> {
        self.windows
            .iter()
            .rev()
            .find_map(|window| window.parent_key.as_deref())
    }

    fn source_size(&self) -> u32 {
        self.windows.iter().fold(0_u32, |total, window| {
            total.saturating_add(window.source_size)
        })
    }
}

fn open_read_only(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    // OpenClaw may still be running and holding a write lock.
    conn.busy_timeout(Duration::from_secs(5))?;
    Ok(conn)
}

/// One transcript window with the metadata of its logical session.
struct Window {
    session_id: String,
    session_key: String,
    origin: Option<OpenClawSessionOrigin>,
    display_name: Option<String>,
    node_label: Option<String>,
    archived: bool,
    /// Logical key of the session that spawned this one.
    parent_key: Option<String>,
    /// Highest event `seq` + 1: transcripts are append-only, so it grows with
    /// every new event and is cheap to read from the primary key.
    source_size: u32,
}

impl Window {
    fn label(&self, conversation_key: &str) -> String {
        // A title set on the session (renamed by the user, or generated) is
        // free text: it must not go through the sender-name cleanup, which
        // rejects any ':' and cuts at " (".
        let title = self
            .node_label
            .as_deref()
            .or(self.display_name.as_deref())
            .map(str::trim)
            .filter(|title| !title.is_empty());
        if let Some(label) = title.and_then(|title| titled_session_label(conversation_key, title)) {
            return label;
        }
        let origin = OpenClawSessionOrigin {
            label: self
                .origin
                .as_ref()
                .and_then(|origin| origin.label.clone())
                .or_else(|| self.display_name.clone())
                .or_else(|| self.node_label.clone()),
            provider: self.origin.as_ref().and_then(|o| o.provider.clone()),
            chat_type: self.origin.as_ref().and_then(|o| o.chat_type.clone()),
        };
        build_session_label(conversation_key, Some(&origin))
    }
}

/// Label of a session that has a title, or `None` for the kinds whose label
/// does not come from it (the main session, Telegram and Signal chats).
fn titled_session_label(logical_key: &str, title: &str) -> Option<String> {
    let parts: Vec<&str> = logical_key.split(':').collect();
    match parts.as_slice() {
        ["agent", _, "main"] => None,
        ["agent", _, "telegram" | "signal", ..] => None,
        ["agent", _, "cron", ..] => Some(format!("Cron: {title}")),
        _ => Some(format!("OpenClaw: {title}")),
    }
}

/// Envelope of one line of a cold archive; only events matter here.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum ColdRecord {
    Event {
        row: ColdEvent,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct ColdEvent {
    seq: i64,
    event_json: String,
}

struct AgentDb {
    conn: Connection,
    /// Directories where OpenClaw publishes file-backed cold archives.
    cold_dirs: [PathBuf; 2],
}

impl AgentDb {
    fn open(db_path: &Path, agent_dir: &Path) -> Result<Self> {
        Ok(Self {
            conn: open_read_only(db_path)?,
            cold_dirs: [
                agent_dir.join("sessions").join("cold"),
                agent_dir.join("agent").join("sessions").join("cold"),
            ],
        })
    }

    /// `expr` when `table.column` exists, `NULL` otherwise, so older schema
    /// versions without the promoted column still import.
    fn optional_column(&self, table: &str, column: &str, expr: &str) -> String {
        let exists = self
            .conn
            .query_row(
                "SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2",
                params![table, column],
                |_| Ok(()),
            )
            .optional()
            .ok()
            .flatten()
            .is_some();
        if exists {
            expr.to_string()
        } else {
            "NULL".to_string()
        }
    }

    fn list_windows(&self) -> Result<Vec<Window>> {
        let sql = format!(
            "SELECT w.session_id, w.session_key, n.entry_json, {display_name}, {label}, \
                    {archived_at}, w.created_at, \
                    (SELECT MAX(seq) FROM transcript_events e WHERE e.session_id = w.session_id) \
             FROM session_windows w JOIN session_nodes n ON n.session_key = w.session_key \
             ORDER BY w.created_at, w.session_id",
            display_name = self.optional_column("session_nodes", "display_name", "n.display_name"),
            label = self.optional_column("session_nodes", "label", "n.label"),
            archived_at = self.optional_column("session_nodes", "archived_at", "n.archived_at"),
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, Option<i64>>(7)?,
            ))
        })?;

        let mut windows = Vec::new();
        for row in rows {
            let (
                session_id,
                session_key,
                entry_json,
                display_name,
                node_label,
                archived_at,
                _created_at,
                max_seq,
            ) = row?;
            let Some(max_seq) = max_seq else {
                continue;
            };
            let entry = entry_json
                .as_deref()
                .and_then(|json| serde_json::from_str::<OpenClawSessionIndexEntry>(json).ok());
            let parent_key = entry
                .as_ref()
                .and_then(|entry| entry.parent_key())
                .map(str::to_string);
            let origin = entry.and_then(|entry| entry.origin);
            windows.push(Window {
                session_id,
                session_key,
                origin,
                display_name: display_name.filter(|s| !s.trim().is_empty()),
                node_label: node_label.filter(|s| !s.trim().is_empty()),
                archived: archived_at.is_some(),
                parent_key,
                source_size: u32::try_from(max_seq.saturating_add(1)).unwrap_or(0),
            });
        }
        Ok(windows)
    }

    /// Push every event of one window into `sink`, in order.
    ///
    /// Returns how many events had no readable payload (cold archive missing).
    fn feed_events(&self, session_id: &str, sink: &mut SessionSink) -> Result<u32> {
        let mut stmt = self.conn.prepare(
            "SELECT seq, event_json, event_zstd, event_utf8_bytes FROM transcript_events \
             WHERE session_id = ?1 ORDER BY seq",
        )?;
        let mut rows = stmt.query(params![session_id])?;
        let mut cold: Option<HashMap<i64, String>> = None;
        let mut missing = 0_u32;

        while let Some(row) = rows.next()? {
            let seq: i64 = row.get(0)?;
            if let Some(json) = row.get::<_, Option<String>>(1)? {
                sink.push_line(&json)?;
                continue;
            }
            if let Some(compressed) = row.get::<_, Option<Vec<u8>>>(2)? {
                let raw_bytes: Option<i64> = row.get(3)?;
                sink.push_line(&decompress_event(&compressed, raw_bytes)?)?;
                continue;
            }
            let cold_events = cold.get_or_insert_with(|| {
                self.load_cold_events(session_id).unwrap_or_else(|error| {
                    warn!(session = session_id, %error, "failed to read cold transcript archive");
                    HashMap::new()
                })
            });
            match cold_events.get(&seq) {
                Some(json) => sink.push_line(json)?,
                None => missing += 1,
            }
        }
        Ok(missing)
    }

    /// Events of a window moved to cold storage, keyed by `seq`.
    fn load_cold_events(&self, session_id: &str) -> Result<HashMap<i64, String>> {
        let archive = self
            .conn
            .query_row(
                "SELECT storage, archive_name, archive_blob \
                 FROM session_transcript_cold_archives WHERE session_id = ?1",
                params![session_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<Vec<u8>>>(2)?,
                    ))
                },
            )
            .optional()?;
        let Some((storage, archive_name, archive_blob)) = archive else {
            return Ok(HashMap::new());
        };
        let embedded = storage == "sqlite";

        let mut stmt = self.conn.prepare(
            "SELECT archive_name, archive_blob FROM session_transcript_cold_archive_chunks \
             WHERE session_id = ?1 ORDER BY chunk_index",
        )?;
        let mut parts = stmt
            .query_map(params![session_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<Vec<u8>>>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if parts.is_empty() {
            parts.push((archive_name, archive_blob));
        }

        let mut events = HashMap::new();
        for (name, blob) in parts {
            let compressed = match blob {
                Some(blob) if embedded => blob,
                _ => self.read_cold_file(&name)?,
            };
            for line in decompress_bounded(&compressed, MAX_COLD_ARCHIVE_BYTES)?.lines() {
                if let Ok(ColdRecord::Event { row }) = serde_json::from_str::<ColdRecord>(line) {
                    events.insert(row.seq, row.event_json);
                }
            }
        }
        Ok(events)
    }

    fn read_cold_file(&self, archive_name: &str) -> Result<Vec<u8>> {
        // The name comes from the database: never let it leave the cold dir.
        if Path::new(archive_name).file_name() != Some(std::ffi::OsStr::new(archive_name)) {
            return Err(Error::message(format!(
                "invalid cold archive name {archive_name:?}"
            )));
        }
        let path = self
            .cold_dirs
            .iter()
            .map(|dir| dir.join(archive_name))
            .find(|path| path.is_file())
            .ok_or_else(|| Error::message(format!("cold archive {archive_name} not found")))?;
        Ok(std::fs::read(path)?)
    }
}

fn decompress_event(compressed: &[u8], raw_bytes: Option<i64>) -> Result<String> {
    let limit = raw_bytes
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| (1..=MAX_EVENT_BYTES).contains(n))
        .unwrap_or(MAX_EVENT_BYTES);
    let decoded = zstd::bulk::decompress(compressed, limit)?;
    String::from_utf8(decoded)
        .map_err(|error| Error::message(format!("transcript event is not UTF-8: {error}")))
}

fn decompress_bounded(compressed: &[u8], limit: u64) -> Result<String> {
    let mut decoded = Vec::new();
    zstd::stream::read::Decoder::new(compressed)?
        .take(limit + 1)
        .read_to_end(&mut decoded)?;
    if decoded.len() as u64 > limit {
        return Err(Error::message("cold transcript archive is too large"));
    }
    String::from_utf8(decoded)
        .map_err(|error| Error::message(format!("cold transcript archive is not UTF-8: {error}")))
}

#[cfg(test)]
mod tests;
