//! SQLite-backed history store.

use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use serde::Serialize;

/// Errors from the history store.
#[derive(Debug, thiserror::Error)]
pub enum HistoryError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("session not found: {0}")]
    SessionNotFound(String),
}

/// Metadata describing a session (one working-directory-scoped run).
#[derive(Debug, Clone)]
pub struct SessionMeta {
    pub workspace: String,
    pub model: String,
}

/// A session as shown in the chat-history list: its metadata plus a derived
/// title (the first user message) and how many messages it holds.
#[derive(Debug, Clone, Serialize)]
pub struct SessionSummary {
    pub id: String,
    pub workspace: String,
    pub model: String,
    pub created_at: i64,
    /// The first user message's text, used as the conversation title.
    pub title: Option<String>,
    pub message_count: i64,
}

/// A SQLite store of sessions and their verbatim message transcripts.
///
/// The connection is wrapped in a `Mutex` so the store is `Send + Sync` and can
/// be shared across threads (e.g. via `Arc`) by the agent loop and, later, the
/// Tauri app.
pub struct HistoryStore {
    conn: Mutex<Connection>,
}

impl HistoryStore {
    /// Open (creating if needed) a store at `path`, running migrations.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, HistoryError> {
        let conn = Connection::open(path)?;
        Self::from_connection(conn)
    }

    /// Open an in-memory store (used for tests).
    pub fn open_in_memory() -> Result<Self, HistoryError> {
        let conn = Connection::open_in_memory()?;
        Self::from_connection(conn)
    }

    fn from_connection(conn: Connection) -> Result<Self, HistoryError> {
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS sessions (
                 id          TEXT PRIMARY KEY,
                 workspace   TEXT NOT NULL,
                 model       TEXT NOT NULL,
                 created_at  INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS messages (
                 id          INTEGER PRIMARY KEY AUTOINCREMENT,
                 session_id  TEXT NOT NULL REFERENCES sessions(id),
                 seq         INTEGER NOT NULL,
                 role        TEXT NOT NULL,
                 content     TEXT,
                 raw_json    TEXT NOT NULL,
                 created_at  INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_messages_session
                 ON messages(session_id, seq);",
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Create a new session, returning its generated id.
    pub fn create_session(&self, meta: &SessionMeta) -> Result<String, HistoryError> {
        let id = uuid::Uuid::new_v4().to_string();
        let conn = self.lock();
        conn.execute(
            "INSERT INTO sessions (id, workspace, model, created_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![id, meta.workspace, meta.model, now()],
        )?;
        Ok(id)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().expect("history store mutex poisoned")
    }

    /// Look up a session's metadata, erroring if it does not exist.
    ///
    /// Used to resume a previous session (restoring its workspace + model).
    pub fn session_meta(&self, session_id: &str) -> Result<SessionMeta, HistoryError> {
        let conn = self.lock();
        conn.query_row(
            "SELECT workspace, model FROM sessions WHERE id = ?1",
            [session_id],
            |row| {
                Ok(SessionMeta {
                    workspace: row.get(0)?,
                    model: row.get(1)?,
                })
            },
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                HistoryError::SessionNotFound(session_id.to_string())
            }
            other => HistoryError::Sqlite(other),
        })
    }

    /// List sessions that hold at least one user message, newest first.
    ///
    /// Each summary carries the first user message as a title so the UI can show
    /// a readable label. Brand-new sessions that only contain the seeded system
    /// prompt are omitted — they have no user turn to title them with.
    pub fn list_sessions(&self) -> Result<Vec<SessionSummary>, HistoryError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT s.id, s.workspace, s.model, s.created_at,
                    (SELECT m.content FROM messages m
                       WHERE m.session_id = s.id AND m.role = 'user'
                         AND m.content IS NOT NULL
                       ORDER BY m.seq ASC LIMIT 1) AS title,
                    (SELECT COUNT(*) FROM messages m WHERE m.session_id = s.id) AS msg_count
             FROM sessions s
             ORDER BY s.created_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(SessionSummary {
                id: row.get(0)?,
                workspace: row.get(1)?,
                model: row.get(2)?,
                created_at: row.get(3)?,
                title: row.get(4)?,
                message_count: row.get(5)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            let summary = row?;
            // Skip sessions with no user turn (e.g. opened but never used).
            if summary.title.is_some() {
                out.push(summary);
            }
        }
        Ok(out)
    }

    /// Append a message to a session, stored verbatim.
    ///
    /// `message` is serialized in full to the `raw_json` column. The `role` and
    /// any top-level `content` string are also extracted for convenient queries.
    /// The per-session `seq` is assigned automatically.
    pub fn append_message<T: Serialize>(
        &self,
        session_id: &str,
        message: &T,
    ) -> Result<i64, HistoryError> {
        let value = serde_json::to_value(message)?;
        let role = value
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let content = value
            .get("content")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let raw_json = serde_json::to_string(&value)?;

        let conn = self.lock();
        let exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sessions WHERE id = ?1",
            [session_id],
            |row| row.get(0),
        )?;
        if exists == 0 {
            return Err(HistoryError::SessionNotFound(session_id.to_string()));
        }

        let next_seq: i64 = conn.query_row(
            "SELECT COALESCE(MAX(seq), -1) + 1 FROM messages WHERE session_id = ?1",
            [session_id],
            |row| row.get(0),
        )?;

        conn.execute(
            "INSERT INTO messages (session_id, seq, role, content, raw_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![session_id, next_seq, role, content, raw_json, now()],
        )?;
        Ok(next_seq)
    }

    /// Return the verbatim message JSON values for a session, ordered by `seq`.
    pub fn messages(&self, session_id: &str) -> Result<Vec<serde_json::Value>, HistoryError> {
        let conn = self.lock();
        let mut stmt =
            conn.prepare("SELECT raw_json FROM messages WHERE session_id = ?1 ORDER BY seq ASC")?;
        let rows = stmt.query_map([session_id], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            let raw: String = row?;
            out.push(serde_json::from_str(&raw)?);
        }
        Ok(out)
    }

    /// Export a session's transcript as JSONL (one verbatim message per line).
    pub fn export_jsonl(&self, session_id: &str) -> Result<String, HistoryError> {
        let messages = self.messages(session_id)?;
        let mut out = String::new();
        for msg in &messages {
            out.push_str(&serde_json::to_string(msg)?);
            out.push('\n');
        }
        Ok(out)
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_core::Message;

    fn store() -> HistoryStore {
        HistoryStore::open_in_memory().unwrap()
    }

    fn meta() -> SessionMeta {
        SessionMeta {
            workspace: "/tmp/proj".into(),
            model: "claude-opus-4-8".into(),
        }
    }

    #[test]
    fn append_and_read_back_in_order() {
        let store = store();
        let session = store.create_session(&meta()).unwrap();

        store
            .append_message(&session, &Message::system("be helpful"))
            .unwrap();
        store
            .append_message(&session, &Message::user("hi"))
            .unwrap();

        let msgs = store.messages(&session).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[1]["content"], "hi");
    }

    #[test]
    fn seq_increments_per_session() {
        let store = store();
        let session = store.create_session(&meta()).unwrap();
        let s0 = store.append_message(&session, &Message::user("a")).unwrap();
        let s1 = store.append_message(&session, &Message::user("b")).unwrap();
        assert_eq!((s0, s1), (0, 1));
    }

    #[test]
    fn stores_tool_calls_verbatim() {
        let store = store();
        let session = store.create_session(&meta()).unwrap();
        // A rich assistant message with tool_calls is preserved exactly.
        let assistant = serde_json::json!({
            "role": "assistant",
            "content": null,
            "tool_calls": [{
                "id": "call_1",
                "type": "function",
                "function": { "name": "read_file", "arguments": "{\"path\":\"a.rs\"}" }
            }]
        });
        store.append_message(&session, &assistant).unwrap();

        let msgs = store.messages(&session).unwrap();
        assert_eq!(msgs[0]["tool_calls"][0]["function"]["name"], "read_file");
    }

    #[test]
    fn export_jsonl_has_one_line_per_message() {
        let store = store();
        let session = store.create_session(&meta()).unwrap();
        store
            .append_message(&session, &Message::user("one"))
            .unwrap();
        store
            .append_message(&session, &Message::assistant("two"))
            .unwrap();

        let jsonl = store.export_jsonl(&session).unwrap();
        assert_eq!(jsonl.lines().count(), 2);
    }

    #[test]
    fn session_meta_round_trips_and_errors_when_missing() {
        let store = store();
        let session = store.create_session(&meta()).unwrap();
        let loaded = store.session_meta(&session).unwrap();
        assert_eq!(loaded.workspace, "/tmp/proj");
        assert_eq!(loaded.model, "claude-opus-4-8");

        let err = store.session_meta("does-not-exist").unwrap_err();
        assert!(matches!(err, HistoryError::SessionNotFound(_)));
    }

    #[test]
    fn list_sessions_titles_by_first_user_message_and_skips_empty() {
        let store = store();

        // An untouched session (system prompt only) is omitted from the list.
        let empty = store.create_session(&meta()).unwrap();
        store
            .append_message(&empty, &Message::system("be helpful"))
            .unwrap();

        // A used session is listed and titled by its first user message.
        let used = store.create_session(&meta()).unwrap();
        store
            .append_message(&used, &Message::system("be helpful"))
            .unwrap();
        store
            .append_message(&used, &Message::user("build a parser"))
            .unwrap();
        store
            .append_message(&used, &Message::user("now add tests"))
            .unwrap();

        let sessions = store.list_sessions().unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, used);
        assert_eq!(sessions[0].title.as_deref(), Some("build a parser"));
        assert_eq!(sessions[0].message_count, 3);
    }

    #[test]
    fn append_to_unknown_session_errors() {
        let store = store();
        let err = store
            .append_message("nope", &Message::user("x"))
            .unwrap_err();
        assert!(matches!(err, HistoryError::SessionNotFound(_)));
    }

    #[test]
    fn persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.sqlite");
        let session = {
            let store = HistoryStore::open(&path).unwrap();
            let session = store.create_session(&meta()).unwrap();
            store
                .append_message(&session, &Message::user("durable"))
                .unwrap();
            session
        };

        let reopened = HistoryStore::open(&path).unwrap();
        let msgs = reopened.messages(&session).unwrap();
        assert_eq!(msgs[0]["content"], "durable");
    }
}
