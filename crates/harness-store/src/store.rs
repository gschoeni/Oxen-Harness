//! SQLite-backed history store.

use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use rusqlite_migration::{Migrations, M};
use serde::Serialize;

/// The ordered schema migrations. The on-disk `user_version` records how many
/// have run, so each opens cleanly whether the database is brand-new, was
/// created before versioning existed, or is already current.
fn migrations() -> Migrations<'static> {
    Migrations::new(vec![
        // M1 — the original schema, verbatim. `IF NOT EXISTS` makes it a no-op
        // against databases created before migrations were introduced (their
        // `user_version` is still 0 but the tables already exist), so they adopt
        // the chain without error.
        M::up(
            "CREATE TABLE IF NOT EXISTS sessions (
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
        ),
        // M2 — enforce one row per (session_id, seq) and record the runtime a
        // session was created under so resuming it stays unambiguous. The unique
        // index supersedes the plain lookup index from M1. New columns default
        // to empty/NULL, so existing rows migrate without backfill.
        M::up(
            "DROP INDEX IF EXISTS idx_messages_session;
             CREATE UNIQUE INDEX IF NOT EXISTS idx_messages_session_seq
                 ON messages(session_id, seq);
             ALTER TABLE sessions ADD COLUMN provider TEXT NOT NULL DEFAULT '';
             ALTER TABLE sessions ADD COLUMN base_url TEXT NOT NULL DEFAULT '';
             ALTER TABLE sessions ADD COLUMN mode TEXT NOT NULL DEFAULT '';
             ALTER TABLE sessions ADD COLUMN context_window INTEGER;
             ALTER TABLE sessions ADD COLUMN system_prompt_version TEXT NOT NULL DEFAULT '';
             ALTER TABLE sessions ADD COLUMN theme TEXT NOT NULL DEFAULT '';
             ALTER TABLE sessions ADD COLUMN transcript_version INTEGER NOT NULL DEFAULT 1;",
        ),
    ])
}

/// Errors from the history store.
#[derive(Debug, thiserror::Error)]
pub enum HistoryError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error("schema migration failed: {0}")]
    Migration(#[from] rusqlite_migration::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("session not found: {0}")]
    SessionNotFound(String),
}

/// Metadata describing a session (one working-directory-scoped run).
///
/// Beyond `workspace` + `model`, sessions record enough of the runtime they were
/// created under that resuming an old one isn't ambiguous as providers, local
/// models, tools, and themes evolve. Unknown fields default to empty/`None`, so
/// callers can fill only what they have (`..Default::default()`).
#[derive(Debug, Clone, Default)]
pub struct SessionMeta {
    pub workspace: String,
    pub model: String,
    /// The inference provider (currently always `"oxen"`).
    pub provider: String,
    /// The resolved base URL the session ran against (captures local vs cloud).
    pub base_url: String,
    /// `"local"` or `"cloud"` when known, else empty.
    pub mode: String,
    /// The model's context window in tokens, if a non-default one was set.
    pub context_window: Option<i64>,
    /// An identifier for the system-prompt revision the session was seeded with.
    pub system_prompt_version: String,
    /// The active theme slug at creation time.
    pub theme: String,
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

    fn from_connection(mut conn: Connection) -> Result<Self, HistoryError> {
        // `foreign_keys` is a per-connection pragma and a no-op inside a
        // transaction, so set it before running migrations (which open their
        // own transaction).
        conn.pragma_update(None, "foreign_keys", true)?;
        migrations().to_latest(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Create a new session, returning its generated id.
    pub fn create_session(&self, meta: &SessionMeta) -> Result<String, HistoryError> {
        let id = uuid::Uuid::new_v4().to_string();
        let conn = self.lock();
        conn.execute(
            "INSERT INTO sessions
                 (id, workspace, model, provider, base_url, mode,
                  context_window, system_prompt_version, theme, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                id,
                meta.workspace,
                meta.model,
                meta.provider,
                meta.base_url,
                meta.mode,
                meta.context_window,
                meta.system_prompt_version,
                meta.theme,
                now()
            ],
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
            "SELECT workspace, model, provider, base_url, mode,
                    context_window, system_prompt_version, theme
             FROM sessions WHERE id = ?1",
            [session_id],
            |row| {
                Ok(SessionMeta {
                    workspace: row.get(0)?,
                    model: row.get(1)?,
                    provider: row.get(2)?,
                    base_url: row.get(3)?,
                    mode: row.get(4)?,
                    context_window: row.get(5)?,
                    system_prompt_version: row.get(6)?,
                    theme: row.get(7)?,
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
        let content = derive_content_text(value.get("content"));
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

/// The plain-text rendering of a message's `content` for the queryable `content`
/// column (and the session title derived from it).
///
/// A plain string is used as-is. A multimodal `Parts` array — what a user
/// message with attachments serializes to — is flattened to its `text` parts
/// joined by newlines, so a session that opened with an image still titles by
/// the words the user typed instead of recording `NULL`. Image/file parts carry
/// no displayable text and are skipped. Returns `None` when there's no text
/// (e.g. an assistant turn that's only tool calls).
fn derive_content_text(content: Option<&serde_json::Value>) -> Option<String> {
    match content {
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(serde_json::Value::Array(parts)) => {
            let text: Vec<&str> = parts
                .iter()
                .filter(|p| p.get("type").and_then(|t| t.as_str()) == Some("text"))
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect();
            (!text.is_empty()).then(|| text.join("\n"))
        }
        _ => None,
    }
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
            ..Default::default()
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

    #[test]
    fn migrations_are_valid_and_reach_latest_version() {
        // rusqlite_migration checks the chain round-trips and the final
        // user_version matches the migration count.
        assert!(migrations().validate().is_ok());
        let store = store();
        let conn = store.lock();
        let user_version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(user_version, 2);
    }

    #[test]
    fn opens_and_upgrades_a_pre_versioning_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.sqlite");

        // Reproduce a database written by the original (pre-migration) code:
        // the M1 tables, user_version left at 0, and a real row of data.
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE sessions (
                     id TEXT PRIMARY KEY, workspace TEXT NOT NULL,
                     model TEXT NOT NULL, created_at INTEGER NOT NULL);
                 CREATE TABLE messages (
                     id INTEGER PRIMARY KEY AUTOINCREMENT,
                     session_id TEXT NOT NULL REFERENCES sessions(id),
                     seq INTEGER NOT NULL, role TEXT NOT NULL,
                     content TEXT, raw_json TEXT NOT NULL,
                     created_at INTEGER NOT NULL);
                 INSERT INTO sessions (id, workspace, model, created_at)
                     VALUES ('s1', '/tmp/p', 'claude-opus-4-8', 1);
                 INSERT INTO messages (session_id, seq, role, content, raw_json, created_at)
                     VALUES ('s1', 0, 'user', 'hi', '{\"role\":\"user\",\"content\":\"hi\"}', 1);",
            )
            .unwrap();
        }

        // Opening runs the migrations: data survives and the new metadata columns
        // exist (defaulted), so an old session resumes without ambiguity.
        let store = HistoryStore::open(&path).unwrap();
        let msgs = store.messages("s1").unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["content"], "hi");

        let loaded = store.session_meta("s1").unwrap();
        assert_eq!(loaded.model, "claude-opus-4-8");
        assert_eq!(loaded.provider, ""); // defaulted by the migration
        assert_eq!(loaded.context_window, None);
    }

    #[test]
    fn records_and_reads_back_rich_session_metadata() {
        let store = store();
        let id = store
            .create_session(&SessionMeta {
                workspace: "/tmp/p".into(),
                model: "qwen3".into(),
                provider: "oxen".into(),
                base_url: "http://localhost:8080/api/ai".into(),
                mode: "local".into(),
                context_window: Some(32_000),
                system_prompt_version: "v3".into(),
                theme: "oregon".into(),
            })
            .unwrap();

        let got = store.session_meta(&id).unwrap();
        assert_eq!(got.mode, "local");
        assert_eq!(got.base_url, "http://localhost:8080/api/ai");
        assert_eq!(got.context_window, Some(32_000));
        assert_eq!(got.theme, "oregon");
    }

    #[test]
    fn multimodal_user_message_titles_by_its_text_part() {
        let store = store();
        let session = store.create_session(&meta()).unwrap();
        // A user message with an attachment serializes content as an array of
        // parts. The title must come from the text part, not be NULL.
        let multimodal = serde_json::json!({
            "role": "user",
            "content": [
                { "type": "text", "text": "what's in this screenshot?" },
                { "type": "image_url", "image_url": { "url": "data:image/png;base64,AAAA" } }
            ]
        });
        store.append_message(&session, &multimodal).unwrap();

        let sessions = store.list_sessions().unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(
            sessions[0].title.as_deref(),
            Some("what's in this screenshot?")
        );
    }
}
