//! Reading Cursor's local chat history.
//!
//! Cursor keeps every conversation ("composer") in a SQLite key/value store at
//! `<Cursor user dir>/globalStorage/state.vscdb`: a `composerData:<uuid>` row
//! holds the conversation metadata plus an ordered list of message ids, and
//! each message ("bubble") lives at `bubbleId:<composerId>:<bubbleId>`. Bubble
//! `type` 1 is the user, 2 the assistant; an assistant bubble carries either
//! text, thinking, or one tool call in `toolFormerData` (name, params JSON,
//! result). Older builds inlined bubbles in a `conversation` array instead —
//! both shapes are read.
//!
//! Which project a conversation belongs to is only recorded indirectly: some
//! per-workspace databases under `workspaceStorage/<hash>/` list their
//! composer ids, and `workspace.json` beside them names the folder. That
//! mapping is applied best-effort; conversations it can't place keep an empty
//! workspace. Newer Cursor builds also move some content into encrypted
//! `agentKv:` blobs — whatever text the bubbles still carry is imported.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Map, Value};

use super::{has_exchange, ImportedConversation};

/// How many conversations a full import would consider (including empty
/// drafts, which the import itself then drops). Cheap — one COUNT query.
pub fn scan(user_dir: &Path) -> usize {
    let Ok(conn) = open_read_only(&global_db(user_dir)) else {
        return 0;
    };
    conn.query_row(
        "SELECT COUNT(*) FROM cursorDiskKV WHERE key LIKE 'composerData:%'",
        [],
        |row| row.get::<_, i64>(0),
    )
    .map(|n| n.max(0) as usize)
    .unwrap_or(0)
}

/// Read every conversation from a Cursor user directory
/// (`~/Library/Application Support/Cursor/User` on macOS), oldest first.
/// Unparseable rows are skipped — best-effort over a foreign format.
pub fn load(user_dir: &Path) -> Vec<ImportedConversation> {
    let Ok(conn) = open_read_only(&global_db(user_dir)) else {
        return Vec::new();
    };
    let workspaces = workspace_by_composer(user_dir);

    let Ok(mut stmt) =
        conn.prepare("SELECT key, value FROM cursorDiskKV WHERE key LIKE 'composerData:%'")
    else {
        return Vec::new();
    };
    let rows: Vec<(String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default();

    let mut out: Vec<ImportedConversation> = rows
        .iter()
        .filter_map(|(key, value)| {
            let composer_id = key.strip_prefix("composerData:")?;
            let data: Value = serde_json::from_str(value).ok()?;
            parse_composer(&conn, composer_id, &data, &workspaces)
        })
        .collect();
    out.sort_by_key(|c| c.created_at);
    out
}

fn global_db(user_dir: &Path) -> std::path::PathBuf {
    user_dir.join("globalStorage").join("state.vscdb")
}

fn open_read_only(path: &Path) -> Result<Connection, rusqlite::Error> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
}

/// Best-effort composer-id → project-folder map, from the per-workspace
/// databases (`workspaceStorage/<hash>/state.vscdb`, ItemTable key
/// `composer.composerData`) and the `workspace.json` naming each folder.
fn workspace_by_composer(user_dir: &Path) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Ok(entries) = fs::read_dir(user_dir.join("workspaceStorage")) else {
        return map;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        let Some(folder) = workspace_folder(&dir) else {
            continue;
        };
        let Ok(conn) = open_read_only(&dir.join("state.vscdb")) else {
            continue;
        };
        let raw: Option<String> = conn
            .query_row(
                "SELECT value FROM ItemTable WHERE key = 'composer.composerData'",
                [],
                |row| row.get(0),
            )
            .ok();
        let Some(data) = raw.and_then(|r| serde_json::from_str::<Value>(&r).ok()) else {
            continue;
        };
        for composer in data
            .get("allComposers")
            .and_then(|c| c.as_array())
            .into_iter()
            .flatten()
        {
            if let Some(id) = composer.get("composerId").and_then(|i| i.as_str()) {
                map.insert(id.to_string(), folder.clone());
            }
        }
    }
    map
}

/// The folder a workspace-storage dir belongs to, from its `workspace.json`
/// (`{"folder": "file:///Users/me/proj"}`).
fn workspace_folder(dir: &Path) -> Option<String> {
    let raw = fs::read_to_string(dir.join("workspace.json")).ok()?;
    let json: Value = serde_json::from_str(&raw).ok()?;
    let folder = json.get("folder")?.as_str()?;
    Some(folder.strip_prefix("file://").unwrap_or(folder).to_string())
}

fn parse_composer(
    conn: &Connection,
    composer_id: &str,
    data: &Value,
    workspaces: &HashMap<String, String>,
) -> Option<ImportedConversation> {
    let bubbles = load_bubbles(conn, composer_id, data);

    let mut messages: Vec<Value> = Vec::new();
    // Thinking arrives in its own bubble; hold it for the next assistant turn.
    let mut pending_reasoning: Vec<String> = Vec::new();
    for bubble in &bubbles {
        append_bubble(&mut messages, &mut pending_reasoning, bubble);
    }
    if !has_exchange(&messages) {
        return None;
    }

    Some(ImportedConversation {
        source_ref: composer_id.to_string(),
        workspace: workspaces.get(composer_id).cloned().unwrap_or_default(),
        model: model_of(data),
        created_at: data
            .get("createdAt")
            .and_then(|c| c.as_i64())
            .map(|ms| ms / 1000)
            .unwrap_or_default(),
        messages,
    })
}

/// A composer's bubbles in conversation order: fetched by id from the
/// key/value table (current builds), or taken inline from a `conversation`
/// array (older builds).
fn load_bubbles(conn: &Connection, composer_id: &str, data: &Value) -> Vec<Value> {
    let headers = data
        .get("fullConversationHeadersOnly")
        .and_then(|h| h.as_array());
    if let Some(headers) = headers.filter(|h| !h.is_empty()) {
        let Ok(mut stmt) = conn.prepare_cached("SELECT value FROM cursorDiskKV WHERE key = ?1")
        else {
            return Vec::new();
        };
        return headers
            .iter()
            .filter_map(|h| h.get("bubbleId")?.as_str())
            .filter_map(|bubble_id| {
                let raw: String = stmt
                    .query_row([format!("bubbleId:{composer_id}:{bubble_id}")], |row| {
                        row.get(0)
                    })
                    .ok()?;
                serde_json::from_str(&raw).ok()
            })
            .collect();
    }
    data.get("conversation")
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_default()
}

/// The composer's model name, wherever this Cursor version recorded it.
fn model_of(data: &Value) -> String {
    let config = data.get("modelConfig");
    config
        .and_then(|c| c.get("modelName").or_else(|| c.get("model")))
        .and_then(|m| m.as_str())
        .filter(|m| !m.is_empty())
        .unwrap_or("cursor")
        .to_string()
}

/// Convert one bubble into native messages. User text becomes a `user` turn; an
/// assistant bubble becomes text, a tool call + `tool` result pair, or (when
/// it only thinks) reasoning held for the next assistant turn.
fn append_bubble(messages: &mut Vec<Value>, pending_reasoning: &mut Vec<String>, bubble: &Value) {
    let text = bubble
        .get("text")
        .and_then(|t| t.as_str())
        .unwrap_or_default();
    match bubble.get("type").and_then(|t| t.as_i64()) {
        Some(1) => {
            if !text.trim().is_empty() {
                messages.push(json!({"role": "user", "content": text}));
            }
        }
        Some(2) => {
            let thinking = thinking_text(bubble);
            if !thinking.is_empty() {
                pending_reasoning.push(thinking);
            }
            if let Some(tool) = bubble.get("toolFormerData") {
                append_tool_call(messages, pending_reasoning, tool);
            } else if !text.trim().is_empty() {
                let mut msg = Map::new();
                msg.insert("role".into(), "assistant".into());
                msg.insert("content".into(), text.into());
                take_reasoning(pending_reasoning, &mut msg);
                messages.push(Value::Object(msg));
            }
        }
        _ => {}
    }
}

/// An assistant tool bubble: one `tool_calls` turn plus its `tool` result.
fn append_tool_call(messages: &mut Vec<Value>, pending_reasoning: &mut Vec<String>, tool: &Value) {
    let name = tool
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or_default();
    if name.is_empty() {
        return;
    }
    let call_id = tool
        .get("toolCallId")
        .map(json_as_string)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("cursor-{name}"));
    let arguments = tool
        .get("rawArgs")
        .or_else(|| tool.get("params"))
        .map(json_as_string)
        .unwrap_or_else(|| "{}".into());

    let mut msg = Map::new();
    msg.insert("role".into(), "assistant".into());
    msg.insert("content".into(), "".into());
    msg.insert(
        "tool_calls".into(),
        json!([{
            "id": call_id,
            "type": "function",
            "function": {"name": name, "arguments": arguments},
        }]),
    );
    take_reasoning(pending_reasoning, &mut msg);
    messages.push(Value::Object(msg));
    messages.push(json!({
        "role": "tool",
        "tool_call_id": call_id,
        "content": tool.get("result").map(json_as_string).unwrap_or_default(),
    }));
}

/// Attach accumulated thinking to an assistant message being built.
fn take_reasoning(pending: &mut Vec<String>, msg: &mut Map<String, Value>) {
    if !pending.is_empty() {
        msg.insert("reasoning".into(), pending.join("\n").into());
        pending.clear();
    }
}

/// A bubble's thinking text, across the shapes Cursor has used for it.
fn thinking_text(bubble: &Value) -> String {
    let blocks = super::text_of(bubble.get("allThinkingBlocks"));
    if !blocks.is_empty() {
        return blocks;
    }
    bubble
        .get("thinking")
        .and_then(|t| t.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or_default()
        .to_string()
}

/// JSON strings verbatim; anything else (objects, numbers) serialized —
/// Cursor stores tool params/results as JSON strings in current builds but
/// has inlined objects before.
fn json_as_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a miniature Cursor user dir: the global key/value DB with one
    /// composer + bubbles, and a workspace-storage entry mapping it to a folder.
    fn write_fixture(dir: &Path) {
        let global = dir.join("globalStorage");
        fs::create_dir_all(&global).unwrap();
        let conn = Connection::open(global.join("state.vscdb")).unwrap();
        conn.execute_batch("CREATE TABLE cursorDiskKV (key TEXT PRIMARY KEY, value TEXT);")
            .unwrap();
        let composer = json!({
            "composerId": "comp-1",
            "createdAt": 1784570751000i64,
            "modelConfig": {"modelName": "gpt-5.2"},
            "fullConversationHeadersOnly": [
                {"bubbleId": "b1", "type": 1},
                {"bubbleId": "b2", "type": 2},
                {"bubbleId": "b3", "type": 2},
                {"bubbleId": "b4", "type": 2},
            ],
        });
        let bubbles = [
            ("b1", json!({"type": 1, "text": "what does main do?"})),
            (
                "b2",
                json!({"type": 2, "text": "", "allThinkingBlocks": [{"text": "let me look"}]}),
            ),
            (
                "b3",
                json!({"type": 2, "text": "", "toolFormerData": {
                    "name": "read_file",
                    "toolCallId": "call-1",
                    "rawArgs": "{\"path\":\"/m.rs\"}",
                    "result": "{\"contents\":\"fn main(){}\"}",
                }}),
            ),
            ("b4", json!({"type": 2, "text": "It prints nothing."})),
        ];
        let insert = |key: String, value: String| {
            conn.execute(
                "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
                rusqlite::params![key, value],
            )
            .unwrap();
        };
        insert("composerData:comp-1".into(), composer.to_string());
        // An empty draft that must not import.
        insert(
            "composerData:draft".into(),
            json!({"composerId": "draft", "fullConversationHeadersOnly": []}).to_string(),
        );
        for (id, bubble) in bubbles {
            insert(format!("bubbleId:comp-1:{id}"), bubble.to_string());
        }

        let ws = dir.join("workspaceStorage").join("abc123");
        fs::create_dir_all(&ws).unwrap();
        fs::write(
            ws.join("workspace.json"),
            json!({"folder": "file:///Users/me/proj"}).to_string(),
        )
        .unwrap();
        let ws_conn = Connection::open(ws.join("state.vscdb")).unwrap();
        ws_conn
            .execute_batch("CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value TEXT);")
            .unwrap();
        ws_conn
            .execute(
                "INSERT INTO ItemTable (key, value) VALUES ('composer.composerData', ?1)",
                [json!({"allComposers": [{"composerId": "comp-1"}]}).to_string()],
            )
            .unwrap();
    }

    #[test]
    fn parses_composers_bubbles_and_workspace_mapping() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture(dir.path());

        assert_eq!(scan(dir.path()), 2); // includes the draft; load drops it
        let convs = load(dir.path());
        assert_eq!(convs.len(), 1);
        let conv = &convs[0];
        assert_eq!(conv.source_ref, "comp-1");
        assert_eq!(conv.workspace, "/Users/me/proj");
        assert_eq!(conv.model, "gpt-5.2");
        assert_eq!(conv.created_at, 1784570751);

        let roles: Vec<&str> = conv
            .messages
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect();
        assert_eq!(roles, ["user", "assistant", "tool", "assistant"]);

        // The thinking-only bubble attached to the next assistant turn (the tool call).
        let call = &conv.messages[1];
        assert_eq!(call["reasoning"], "let me look");
        assert_eq!(call["tool_calls"][0]["function"]["name"], "read_file");
        assert_eq!(
            call["tool_calls"][0]["function"]["arguments"],
            "{\"path\":\"/m.rs\"}"
        );
        assert_eq!(conv.messages[2]["tool_call_id"], "call-1");
        assert_eq!(conv.messages[3]["content"], "It prints nothing.");
    }

    #[test]
    fn missing_database_scans_and_loads_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(scan(dir.path()), 0);
        assert!(load(dir.path()).is_empty());
    }
}
