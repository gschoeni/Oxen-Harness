//! SQLite-backed history store.

use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use rusqlite_migration::{Migrations, M};
use serde::de::DeserializeOwned;
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
        // M3 — an app-wide key/value counter table for cheap running aggregates
        // (e.g. all-time total tokens used), so we never rescan every transcript.
        M::up(
            "CREATE TABLE IF NOT EXISTS app_meta (
                 key    TEXT PRIMARY KEY,
                 value  INTEGER NOT NULL
             );",
        ),
        // M4 — a per-session training-data review status: '' = unreviewed,
        // 'kept' = include in the fine-tuning dataset, 'rejected' = exclude.
        // Defaults to '' so every existing chat starts unreviewed.
        M::up("ALTER TABLE sessions ADD COLUMN review_status TEXT NOT NULL DEFAULT '';"),
        // M5 — timestamped per-model usage accounting, one row per model call.
        // Keeping events (rather than only lifetime counters) supports daily
        // and yearly reporting without reconstructing usage from transcripts.
        // Dollar estimates
        // are derived from the current Oxen catalog when presented; keeping the
        // observed provider usage separate from mutable pricing avoids silently
        // recording an unavailable catalog lookup as a real $0 charge.
        M::up(
            "CREATE TABLE IF NOT EXISTS usage_events (
                 id                INTEGER PRIMARY KEY AUTOINCREMENT,
                 model             TEXT NOT NULL,
                 source            TEXT NOT NULL,
                 prompt_tokens     INTEGER NOT NULL,
                 completion_tokens INTEGER NOT NULL,
                 created_at        INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_usage_events_created_at
                 ON usage_events(created_at);",
        ),
        // M6 — repair the first usage release, which stored aggregate rows in
        // `model_usage` while already claiming schema version 5. Preserve those
        // totals as events so the later daily usage queries have a real table.
        M::up_with_hook(
            "CREATE TABLE IF NOT EXISTS usage_events (
                 id                INTEGER PRIMARY KEY AUTOINCREMENT,
                 model             TEXT NOT NULL,
                 source            TEXT NOT NULL,
                 prompt_tokens     INTEGER NOT NULL,
                 completion_tokens INTEGER NOT NULL,
                 created_at        INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_usage_events_created_at
                 ON usage_events(created_at);",
            |tx| {
                let has_legacy_summary: bool = tx.query_row(
                    "SELECT EXISTS(
                         SELECT 1 FROM sqlite_master
                         WHERE type = 'table' AND name = 'model_usage'
                     )",
                    [],
                    |row| row.get(0),
                )?;
                if has_legacy_summary {
                    tx.execute(
                        "INSERT INTO usage_events
                             (model, source, prompt_tokens, completion_tokens, created_at)
                         SELECT model, 'unpriced', prompt_tokens, completion_tokens, updated_at
                         FROM model_usage",
                        [],
                    )?;
                    tx.execute("DROP TABLE model_usage", [])?;
                }
                Ok(())
            },
        ),
        // M7 — a compact active-context checkpoint. The messages table remains
        // verbatim; this projection lets a resumed agent avoid loading history
        // that was already summarized or pruned in memory.
        M::up(
            "CREATE TABLE IF NOT EXISTS context_snapshots (
                 session_id  TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
                 through_seq INTEGER NOT NULL,
                 raw_json    TEXT NOT NULL
             );",
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
    /// Training-data review status: `""` (unreviewed), `"kept"`, or `"rejected"`.
    pub review_status: String,
}

/// Accumulated usage for a single model, summed across every session and
/// project — the source data for per-model token and dollar summaries.
#[derive(Debug, Clone, Serialize)]
pub struct ModelUsage {
    pub model: String,
    /// `oxen_cloud` for hub.oxen.ai (catalog-priced), otherwise `unpriced`
    /// for local or custom endpoints whose billing cannot be inferred.
    pub source: String,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
}

/// Token throughput for one local-calendar day, used by the yearly activity
/// grid. `date` is `YYYY-MM-DD` in the machine's local timezone.
#[derive(Debug, Clone, Serialize)]
pub struct DailyUsage {
    pub date: String,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
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

    /// Read an app-wide counter (`app_meta`), or `None` if it was never set.
    pub fn meta_get_i64(&self, key: &str) -> Result<Option<i64>, HistoryError> {
        use rusqlite::OptionalExtension;
        let conn = self.lock();
        let value = conn
            .query_row("SELECT value FROM app_meta WHERE key = ?1", [key], |row| {
                row.get::<_, i64>(0)
            })
            .optional()?;
        Ok(value)
    }

    /// Set an app-wide counter (`app_meta`) to an absolute value.
    pub fn meta_set_i64(&self, key: &str, value: i64) -> Result<(), HistoryError> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO app_meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![key, value],
        )?;
        Ok(())
    }

    /// Atomically add `delta` to an app-wide counter (`app_meta`), creating it at
    /// `delta` if absent, and return the new total. Keeps running aggregates cheap
    /// to update without read-modify-write races across sessions.
    pub fn meta_add_i64(&self, key: &str, delta: i64) -> Result<i64, HistoryError> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO app_meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = value + excluded.value",
            rusqlite::params![key, delta],
        )?;
        let value = conn.query_row("SELECT value FROM app_meta WHERE key = ?1", [key], |row| {
            row.get::<_, i64>(0)
        })?;
        Ok(value)
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
                    (SELECT COUNT(*) FROM messages m WHERE m.session_id = s.id) AS msg_count,
                    s.review_status
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
                review_status: row.get(6)?,
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
        let content = crate::content::derive_content_text(value.get("content"));
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

    /// Append an already-serialized message without building an intermediate
    /// `serde_json::Value`. `content` is only the small queryable preview/title;
    /// `raw_json` remains the complete source of truth.
    pub fn append_raw_message(
        &self,
        session_id: &str,
        role: &str,
        content: Option<&str>,
        raw_json: &str,
    ) -> Result<i64, HistoryError> {
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

    /// Deserialize messages directly into their destination type, avoiding an
    /// intermediate JSON tree. Only rows after `after_seq` are returned.
    pub fn messages_typed_after<T: DeserializeOwned>(
        &self,
        session_id: &str,
        after_seq: i64,
    ) -> Result<Vec<T>, HistoryError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT raw_json FROM messages
             WHERE session_id = ?1 AND seq > ?2 ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map(rusqlite::params![session_id, after_seq], |row| {
            row.get::<_, String>(0)
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }

    /// Save the agent's compact active context through a persisted message seq.
    pub fn save_context_snapshot<T: Serialize>(
        &self,
        session_id: &str,
        through_seq: i64,
        messages: &T,
    ) -> Result<(), HistoryError> {
        let raw = serde_json::to_string(messages)?;
        let conn = self.lock();
        conn.execute(
            "INSERT INTO context_snapshots (session_id, through_seq, raw_json)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(session_id) DO UPDATE SET
                 through_seq = excluded.through_seq,
                 raw_json = excluded.raw_json",
            rusqlite::params![session_id, through_seq, raw],
        )?;
        Ok(())
    }

    /// Load the latest compact active-context checkpoint, when one exists.
    pub fn context_snapshot<T: DeserializeOwned>(
        &self,
        session_id: &str,
    ) -> Result<Option<(i64, T)>, HistoryError> {
        use rusqlite::OptionalExtension;
        let conn = self.lock();
        let row = conn
            .query_row(
                "SELECT through_seq, raw_json FROM context_snapshots WHERE session_id = ?1",
                [session_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        row.map(|(seq, raw)| Ok((seq, serde_json::from_str(&raw)?)))
            .transpose()
    }

    /// Permanently delete a session and its messages. Idempotent: deleting a
    /// session that doesn't exist is a no-op. Messages are removed first to
    /// respect the foreign key, both in one transaction so it's all-or-nothing.
    pub fn delete_session(&self, session_id: &str) -> Result<(), HistoryError> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM messages WHERE session_id = ?1", [session_id])?;
        tx.execute("DELETE FROM sessions WHERE id = ?1", [session_id])?;
        tx.commit()?;
        Ok(())
    }

    /// Set a session's training-data review status (`""`, `"kept"`, or
    /// `"rejected"`). Errors if the session doesn't exist.
    pub fn set_review_status(&self, session_id: &str, status: &str) -> Result<(), HistoryError> {
        let conn = self.lock();
        let changed = conn.execute(
            "UPDATE sessions SET review_status = ?2 WHERE id = ?1",
            rusqlite::params![session_id, status],
        )?;
        if changed == 0 {
            return Err(HistoryError::SessionNotFound(session_id.to_string()));
        }
        Ok(())
    }

    /// A session's training-data review status (`""` when unreviewed). Errors
    /// if the session doesn't exist.
    pub fn review_status(&self, session_id: &str) -> Result<String, HistoryError> {
        let conn = self.lock();
        conn.query_row(
            "SELECT review_status FROM sessions WHERE id = ?1",
            [session_id],
            |row| row.get(0),
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                HistoryError::SessionNotFound(session_id.to_string())
            }
            other => other.into(),
        })
    }

    /// Set the review status for many sessions at once (bulk keep/reject from the
    /// dataset builder), in a single transaction. Returns how many rows changed.
    pub fn set_review_status_many(
        &self,
        session_ids: &[String],
        status: &str,
    ) -> Result<usize, HistoryError> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let mut changed = 0usize;
        {
            let mut stmt = tx.prepare("UPDATE sessions SET review_status = ?2 WHERE id = ?1")?;
            for id in session_ids {
                changed += stmt.execute(rusqlite::params![id, status])?;
            }
        }
        tx.commit()?;
        Ok(changed)
    }

    /// Record one model call's provider-reported (or fallback-estimated) prompt
    /// and completion tokens. A zero call is a no-op.
    pub fn record_model_usage(
        &self,
        model: &str,
        source: &str,
        prompt_delta: usize,
        completion_delta: usize,
    ) -> Result<(), HistoryError> {
        let prompt = prompt_delta as i64;
        let completion = completion_delta as i64;
        if prompt == 0 && completion == 0 {
            return Ok(());
        }
        self.record_model_usage_at(model, source, prompt, completion, now())?;
        Ok(())
    }

    fn record_model_usage_at(
        &self,
        model: &str,
        source: &str,
        prompt_tokens: i64,
        completion_tokens: i64,
        created_at: i64,
    ) -> Result<(), HistoryError> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO usage_events
                 (model, source, prompt_tokens, completion_tokens, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![model, source, prompt_tokens, completion_tokens, created_at],
        )?;
        Ok(())
    }

    /// The per-model usage breakdown, busiest first — every model that has
    /// accumulated usage, with separate prompt and completion counts.
    pub fn model_usage_breakdown(&self) -> Result<Vec<ModelUsage>, HistoryError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT model, source, SUM(prompt_tokens), SUM(completion_tokens)
             FROM usage_events
             GROUP BY model, source
             ORDER BY SUM(prompt_tokens + completion_tokens) DESC, model ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ModelUsage {
                model: row.get(0)?,
                source: row.get(1)?,
                prompt_tokens: row.get(2)?,
                completion_tokens: row.get(3)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Per-model usage for one local-calendar day (`YYYY-MM-DD`).
    pub fn model_usage_for_day(&self, date: &str) -> Result<Vec<ModelUsage>, HistoryError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT model, source, SUM(prompt_tokens), SUM(completion_tokens)
             FROM usage_events
             WHERE date(created_at, 'unixepoch', 'localtime') = ?1
             GROUP BY model, source
             ORDER BY SUM(prompt_tokens + completion_tokens) DESC, model ASC",
        )?;
        let rows = stmt.query_map([date], |row| {
            Ok(ModelUsage {
                model: row.get(0)?,
                source: row.get(1)?,
                prompt_tokens: row.get(2)?,
                completion_tokens: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(HistoryError::from)
    }

    /// Daily token totals for `year`, in the machine's local timezone. Days
    /// with no activity are omitted; the UI fills those cells with zero.
    pub fn daily_usage(&self, year: i32) -> Result<Vec<DailyUsage>, HistoryError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT date(created_at, 'unixepoch', 'localtime') AS day,
                    SUM(prompt_tokens), SUM(completion_tokens)
             FROM usage_events
             WHERE strftime('%Y', created_at, 'unixepoch', 'localtime') = ?1
             GROUP BY day
             ORDER BY day ASC",
        )?;
        let year = year.to_string();
        let rows = stmt.query_map([year], |row| {
            Ok(DailyUsage {
                date: row.get(0)?,
                prompt_tokens: row.get(1)?,
                completion_tokens: row.get(2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(HistoryError::from)
    }

    /// Total model throughput represented by the per-model ledger.
    pub fn total_model_tokens(&self) -> Result<i64, HistoryError> {
        let conn = self.lock();
        conn.query_row(
            "SELECT COALESCE(SUM(prompt_tokens + completion_tokens), 0)
             FROM usage_events",
            [],
            |row| row.get(0),
        )
        .map_err(HistoryError::from)
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

    /// Export one or more sessions as chat-completions fine-tuning data, in the
    /// shape Oxen.ai expects: one JSON object per line, each a single
    /// conversation under a `messages` key —
    /// `{"messages":[{"role":"system","content":"…"}, …]}`.
    ///
    /// Each session becomes one conversation. Messages are normalized for
    /// training: multimodal content arrays are flattened to their text (image
    /// parts dropped). When `include_tools` is false, tool-result messages are
    /// omitted and assistant `tool_calls` are stripped, leaving a clean
    /// text-only dialogue; when true, `tool_calls` and tool results are
    /// preserved so the data can teach tool use. Sessions with no usable
    /// messages are skipped, so the output never has blank conversations.
    pub fn export_chat_completions(
        &self,
        session_ids: &[String],
        include_tools: bool,
    ) -> Result<String, HistoryError> {
        let mut out = String::new();
        for sid in session_ids {
            let messages = self.messages(sid)?;
            if let Some(conversation) =
                crate::export::conversation_from_messages(&messages, include_tools)
            {
                let line = serde_json::json!({ "messages": conversation });
                out.push_str(&serde_json::to_string(&line)?);
                out.push('\n');
            }
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
    fn compact_context_checkpoint_round_trips_with_later_messages() {
        let store = store();
        let session = store.create_session(&meta()).unwrap();
        store
            .append_message(&session, &Message::user("old"))
            .unwrap();
        store
            .save_context_snapshot(&session, 0, &vec![Message::user("summary")])
            .unwrap();
        store
            .append_message(&session, &Message::assistant("new"))
            .unwrap();

        let (seq, compact): (i64, Vec<Message>) =
            store.context_snapshot(&session).unwrap().unwrap();
        let later: Vec<Message> = store.messages_typed_after(&session, seq).unwrap();
        assert_eq!(seq, 0);
        assert_eq!(compact[0].content, "summary");
        assert_eq!(later[0].content, "new");
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
    fn export_chat_completions_groups_one_conversation_per_line() {
        let store = store();
        let session = store.create_session(&meta()).unwrap();
        store
            .append_message(&session, &Message::system("be helpful"))
            .unwrap();
        store
            .append_message(&session, &Message::user("hi"))
            .unwrap();
        store
            .append_message(&session, &Message::assistant("hello"))
            .unwrap();

        let out = store
            .export_chat_completions(std::slice::from_ref(&session), false)
            .unwrap();
        // One conversation → one JSONL line.
        assert_eq!(out.lines().count(), 1);
        let row: serde_json::Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
        let msgs = row["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[2]["content"], "hello");
    }

    #[test]
    fn export_flattens_multimodal_user_message_and_drops_image_data() {
        let store = store();
        let session = store.create_session(&meta()).unwrap();
        // A user message with an attached image serializes content as a Parts
        // array; the training export must keep the typed text and drop the image.
        store
            .append_message(
                &session,
                &serde_json::json!({
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "what is in this image?"},
                        {"type": "image_url", "image_url": {"url": "data:image/png;base64,SECRETPIXELS"}}
                    ]
                }),
            )
            .unwrap();
        store
            .append_message(&session, &Message::assistant("a cat"))
            .unwrap();

        let out = store
            .export_chat_completions(std::slice::from_ref(&session), true)
            .unwrap();
        let row: serde_json::Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
        let msgs = row["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[0]["content"], "what is in this image?");
        // The base64 image bytes must never reach the fine-tuning dataset.
        assert!(
            !out.contains("SECRETPIXELS") && !out.contains("image_url"),
            "image data leaked into the training export: {out}"
        );
    }

    #[test]
    fn export_chat_completions_handles_tools_per_flag() {
        let store = store();
        let session = store.create_session(&meta()).unwrap();
        store
            .append_message(&session, &Message::user("read it"))
            .unwrap();
        store
            .append_message(
                &session,
                &serde_json::json!({
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "read_file", "arguments": "{}" }
                    }]
                }),
            )
            .unwrap();
        store
            .append_message(
                &session,
                &serde_json::json!({ "role": "tool", "tool_call_id": "call_1", "content": "file body" }),
            )
            .unwrap();
        store
            .append_message(&session, &Message::assistant("done"))
            .unwrap();

        // Without tools: the tool result is dropped and the empty tool-call-only
        // assistant turn carries no content, so only user + final assistant remain.
        let stripped = store
            .export_chat_completions(std::slice::from_ref(&session), false)
            .unwrap();
        let row: serde_json::Value =
            serde_json::from_str(stripped.lines().next().unwrap()).unwrap();
        let msgs = row["messages"].as_array().unwrap();
        assert!(msgs.iter().all(|m| m["role"] != "tool"));
        assert!(msgs.iter().all(|m| m.get("tool_calls").is_none()));

        // With tools: tool_calls and the tool result are preserved.
        let full = store
            .export_chat_completions(std::slice::from_ref(&session), true)
            .unwrap();
        let row: serde_json::Value = serde_json::from_str(full.lines().next().unwrap()).unwrap();
        let msgs = row["messages"].as_array().unwrap();
        assert!(msgs
            .iter()
            .any(|m| m["role"] == "tool" && m["tool_call_id"] == "call_1"));
        assert!(msgs.iter().any(|m| m.get("tool_calls").is_some()));
    }

    #[test]
    fn export_chat_completions_skips_conversations_without_a_turn_pair() {
        let store = store();
        // System prompt only — no user/assistant exchange, so it's skipped.
        let bare = store.create_session(&meta()).unwrap();
        store
            .append_message(&bare, &Message::system("be helpful"))
            .unwrap();

        let out = store.export_chat_completions(&[bare], false).unwrap();
        assert_eq!(out.lines().count(), 0);
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
    fn delete_session_removes_it_and_its_messages() {
        let store = store();
        let session = store.create_session(&meta()).unwrap();
        store
            .append_message(&session, &Message::user("hello"))
            .unwrap();
        assert_eq!(store.list_sessions().unwrap().len(), 1);

        store.delete_session(&session).unwrap();
        assert!(store.list_sessions().unwrap().is_empty());
        assert!(store.messages(&session).unwrap().is_empty());
        // Idempotent: deleting again is fine.
        store.delete_session(&session).unwrap();
    }

    #[test]
    fn meta_counter_starts_absent_then_adds_and_sets() {
        let store = store();
        // Absent until first written.
        assert_eq!(store.meta_get_i64("total_tokens_used").unwrap(), None);
        // Add creates it at the delta and returns the new total.
        assert_eq!(store.meta_add_i64("total_tokens_used", 100).unwrap(), 100);
        assert_eq!(store.meta_add_i64("total_tokens_used", 50).unwrap(), 150);
        assert_eq!(store.meta_get_i64("total_tokens_used").unwrap(), Some(150));
        // Set overwrites to an absolute value.
        store.meta_set_i64("total_tokens_used", 7).unwrap();
        assert_eq!(store.meta_get_i64("total_tokens_used").unwrap(), Some(7));
    }

    #[test]
    fn model_usage_accumulates_per_model_and_aggregates() {
        let store = store();
        // No usage recorded yet: empty breakdown, zero grand total.
        assert!(store.model_usage_breakdown().unwrap().is_empty());
        assert_eq!(store.total_model_tokens().unwrap(), 0);

        // Two turns on model A and one on model B accumulate per model.
        store
            .record_model_usage("claude-opus-4-8", "oxen_cloud", 1000, 200)
            .unwrap();
        store
            .record_model_usage("claude-opus-4-8", "oxen_cloud", 500, 100)
            .unwrap();
        store
            .record_model_usage("gemini-2-5-flash", "oxen_cloud", 2000, 400)
            .unwrap();

        let breakdown = store.model_usage_breakdown().unwrap();
        assert_eq!(breakdown.len(), 2);
        // Ordered by total throughput, busiest first.
        assert_eq!(breakdown[0].model, "gemini-2-5-flash");
        assert_eq!(breakdown[1].model, "claude-opus-4-8");
        assert_eq!(breakdown[1].prompt_tokens, 1500);
        assert_eq!(breakdown[1].completion_tokens, 300);

        assert_eq!(store.total_model_tokens().unwrap(), 4200);
    }

    #[test]
    fn record_model_usage_ignores_empty_calls() {
        let store = store();
        store.record_model_usage("m", "unpriced", 0, 0).unwrap();
        assert!(store.model_usage_breakdown().unwrap().is_empty());
    }

    #[test]
    fn model_usage_persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.sqlite");
        {
            let store = HistoryStore::open(&path).unwrap();
            store
                .record_model_usage("m", "oxen_cloud", 100, 50)
                .unwrap();
        }
        let reopened = HistoryStore::open(&path).unwrap();
        assert_eq!(reopened.total_model_tokens().unwrap(), 150);
    }

    #[test]
    fn daily_usage_and_day_breakdown_follow_local_calendar_dates() {
        let store = store();
        // 2025-06-15 12:00:00 UTC stays near June 15 in every practical local
        // timezone; derive SQLite's exact local date so the test pins the same
        // conversion used by production queries.
        let timestamp = 1_750_003_200_i64;
        store
            .record_model_usage_at("model-a", "oxen_cloud", 100, 25, timestamp)
            .unwrap();
        store
            .record_model_usage_at("model-b", "unpriced", 50, 10, timestamp)
            .unwrap();
        let (date, year): (String, i32) = {
            let conn = store.lock();
            conn.query_row(
                "SELECT date(?1, 'unixepoch', 'localtime'),
                        CAST(strftime('%Y', ?1, 'unixepoch', 'localtime') AS INTEGER)",
                [timestamp],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
        };

        let days = store.daily_usage(year).unwrap();
        assert_eq!(days.len(), 1);
        assert_eq!(days[0].date, date);
        assert_eq!(days[0].prompt_tokens, 150);
        assert_eq!(days[0].completion_tokens, 35);

        let rows = store.model_usage_for_day(&date).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].model, "model-a");
        assert_eq!(rows[0].prompt_tokens, 100);
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
        assert_eq!(user_version, 7);
    }

    #[test]
    fn upgrades_the_released_v5_usage_summary_to_usage_events() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v5-usage.sqlite");

        // The first usage release shipped this aggregate table at user_version
        // 5. Its replacement needs a new migration so existing databases do
        // not report their obsolete version as current and skip the ledger.
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE model_usage (
                     model             TEXT PRIMARY KEY,
                     prompt_tokens     INTEGER NOT NULL DEFAULT 0,
                     completion_tokens INTEGER NOT NULL DEFAULT 0,
                     cost_micros       INTEGER NOT NULL DEFAULT 0,
                     updated_at        INTEGER NOT NULL
                 );
                 INSERT INTO model_usage
                     (model, prompt_tokens, completion_tokens, cost_micros, updated_at)
                 VALUES ('claude-opus-4-8', 1200, 300, 42, 1_700_000_000);
                 PRAGMA user_version = 5;",
            )
            .unwrap();
        }

        let store = HistoryStore::open(&path).unwrap();
        let usage = store.model_usage_breakdown().unwrap();
        assert_eq!(usage.len(), 1);
        assert_eq!(usage[0].model, "claude-opus-4-8");
        assert_eq!(usage[0].source, "unpriced");
        assert_eq!(usage[0].prompt_tokens, 1200);
        assert_eq!(usage[0].completion_tokens, 300);
    }

    #[test]
    fn review_status_defaults_empty_and_updates() {
        let store = store();
        let session = store.create_session(&meta()).unwrap();
        store
            .append_message(&session, &Message::user("hi"))
            .unwrap();

        // Unreviewed by default.
        assert_eq!(store.list_sessions().unwrap()[0].review_status, "");

        store.set_review_status(&session, "kept").unwrap();
        assert_eq!(store.list_sessions().unwrap()[0].review_status, "kept");
        assert_eq!(store.review_status(&session).unwrap(), "kept");
        assert!(matches!(
            store.review_status("nope").unwrap_err(),
            HistoryError::SessionNotFound(_)
        ));

        store.set_review_status(&session, "rejected").unwrap();
        assert_eq!(store.list_sessions().unwrap()[0].review_status, "rejected");

        // Unknown session errors.
        assert!(matches!(
            store.set_review_status("nope", "kept").unwrap_err(),
            HistoryError::SessionNotFound(_)
        ));
    }

    #[test]
    fn set_review_status_many_updates_all_and_reports_count() {
        let store = store();
        let a = store.create_session(&meta()).unwrap();
        let b = store.create_session(&meta()).unwrap();
        store.append_message(&a, &Message::user("a")).unwrap();
        store.append_message(&b, &Message::user("b")).unwrap();

        let changed = store
            .set_review_status_many(&[a.clone(), b.clone(), "missing".into()], "kept")
            .unwrap();
        // Two real sessions updated; the missing id changes nothing.
        assert_eq!(changed, 2);
        let sessions = store.list_sessions().unwrap();
        assert!(sessions.iter().all(|s| s.review_status == "kept"));
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
