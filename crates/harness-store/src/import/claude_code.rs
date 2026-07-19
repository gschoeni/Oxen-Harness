//! Reading Claude Code's local transcripts.
//!
//! Claude Code persists one JSONL file per session under
//! `~/.claude/projects/<project-slug>/<session-uuid>.jsonl`. Each line is a
//! typed record; `user` / `assistant` lines carry a full Anthropic-format
//! message plus metadata (`cwd`, `timestamp`, the answering model). Everything
//! else (`file-history-snapshot`, `ai-title`, mode changes, …) is bookkeeping
//! this importer skips.
//!
//! Anthropic content blocks map to the store's native shape: `text` →
//! `content`, `thinking` → `reasoning`, `tool_use` → `tool_calls`, and a user
//! line's `tool_result` blocks become `tool` messages. One API assistant
//! message streams as several consecutive JSONL lines sharing a `message.id`,
//! so those merge back into a single assistant turn.

use std::fs;
use std::path::Path;

use serde_json::{json, Map, Value};

use super::{has_exchange, parse_iso_secs, text_of, ImportedConversation};

/// How many sessions a full import would consider: one per transcript file.
/// Cheap — counts files without parsing them.
pub fn scan(root: &Path) -> usize {
    session_files(root).len()
}

/// Read every session transcript under `root` (`~/.claude/projects`) into
/// normalized conversations, oldest first. Unreadable files and unparseable
/// lines are skipped — importing a foreign format is best-effort by nature.
pub fn load(root: &Path) -> Vec<ImportedConversation> {
    let mut out: Vec<ImportedConversation> = session_files(root)
        .iter()
        .filter_map(|path| parse_session_file(path))
        .collect();
    out.sort_by_key(|c| c.created_at);
    out
}

/// Every `<project-slug>/<session-uuid>.jsonl` under the projects root.
fn session_files(root: &Path) -> Vec<std::path::PathBuf> {
    let Ok(projects) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    for project in projects.flatten() {
        let Ok(entries) = fs::read_dir(project.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                files.push(path);
            }
        }
    }
    files
}

fn parse_session_file(path: &Path) -> Option<ImportedConversation> {
    let source_ref = path.file_stem()?.to_str()?.to_string();
    let raw = fs::read_to_string(path).ok()?;

    let mut workspace = String::new();
    let mut model = String::new();
    let mut created_at: Option<i64> = None;
    let mut messages: Vec<Value> = Vec::new();
    // The `message.id` of the last assistant line, to merge streamed
    // continuations of the same API message. Any user line breaks the run.
    let mut last_assistant_id: Option<String> = None;

    for line in raw.lines() {
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let kind = record.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if kind != "user" && kind != "assistant" {
            continue;
        }
        // Sidechains are subagent transcripts; meta lines are injected
        // caveats/command output, not something the user typed.
        if record.get("isSidechain").and_then(|v| v.as_bool()) == Some(true)
            || record.get("isMeta").and_then(|v| v.as_bool()) == Some(true)
        {
            continue;
        }
        if workspace.is_empty() {
            if let Some(cwd) = record.get("cwd").and_then(|c| c.as_str()) {
                workspace = cwd.to_string();
            }
        }
        if created_at.is_none() {
            created_at = record
                .get("timestamp")
                .and_then(|t| t.as_str())
                .and_then(parse_iso_secs);
        }
        let Some(message) = record.get("message") else {
            continue;
        };

        if kind == "user" {
            last_assistant_id = None;
            append_user(&mut messages, message.get("content"));
        } else {
            if model.is_empty() {
                if let Some(m) = message.get("model").and_then(|m| m.as_str()) {
                    model = m.to_string();
                }
            }
            let api_id = message.get("id").and_then(|i| i.as_str()).map(String::from);
            let continues = api_id.is_some() && api_id == last_assistant_id;
            append_assistant(&mut messages, message.get("content"), continues);
            last_assistant_id = api_id;
        }
    }

    if !has_exchange(&messages) {
        return None;
    }
    let created_at = created_at
        .or_else(|| file_mtime_secs(path))
        .unwrap_or_default();
    Some(ImportedConversation {
        source_ref,
        workspace,
        model,
        created_at,
        messages,
    })
}

/// A user line: plain text becomes a `user` message; an Anthropic content
/// array splits into `tool` messages (one per `tool_result` block) plus a
/// `user` message for any text. Image blocks carry no text and are dropped.
fn append_user(messages: &mut Vec<Value>, content: Option<&Value>) {
    match content {
        Some(Value::String(s)) => {
            if !s.trim().is_empty() {
                messages.push(json!({"role": "user", "content": s}));
            }
        }
        Some(Value::Array(blocks)) => {
            let mut text = Vec::new();
            for block in blocks {
                match block.get("type").and_then(|t| t.as_str()) {
                    Some("tool_result") => messages.push(json!({
                        "role": "tool",
                        "tool_call_id": block.get("tool_use_id").cloned().unwrap_or_default(),
                        "content": text_of(block.get("content")),
                    })),
                    Some("text") => {
                        if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                            text.push(t);
                        }
                    }
                    _ => {}
                }
            }
            let text = text.join("\n");
            if !text.trim().is_empty() {
                messages.push(json!({"role": "user", "content": text}));
            }
        }
        _ => {}
    }
}

/// An assistant line's content blocks, merged into the previous assistant
/// message when it continues the same API message (`continues`), else pushed
/// as a new turn.
fn append_assistant(messages: &mut Vec<Value>, content: Option<&Value>, continues: bool) {
    let Some(Value::Array(blocks)) = content else {
        return;
    };
    let mut text = Vec::new();
    let mut reasoning = Vec::new();
    let mut tool_calls = Vec::new();
    for block in blocks {
        match block.get("type").and_then(|t| t.as_str()) {
            Some("text") => {
                if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                    text.push(t.to_string());
                }
            }
            Some("thinking") => {
                if let Some(t) = block.get("thinking").and_then(|t| t.as_str()) {
                    reasoning.push(t.to_string());
                }
            }
            Some("tool_use") => tool_calls.push(json!({
                "id": block.get("id").cloned().unwrap_or_default(),
                "type": "function",
                "function": {
                    "name": block.get("name").cloned().unwrap_or_default(),
                    "arguments": block
                        .get("input")
                        .map(|i| i.to_string())
                        .unwrap_or_else(|| "{}".into()),
                },
            })),
            _ => {}
        }
    }
    if text.is_empty() && reasoning.is_empty() && tool_calls.is_empty() {
        return;
    }

    if continues {
        if let Some(Value::Object(prev)) = messages.last_mut() {
            merge_joined(prev, "content", text);
            merge_joined(prev, "reasoning", reasoning);
            if !tool_calls.is_empty() {
                match prev.get_mut("tool_calls") {
                    Some(Value::Array(existing)) => existing.extend(tool_calls),
                    _ => {
                        prev.insert("tool_calls".into(), Value::Array(tool_calls));
                    }
                }
            }
            return;
        }
    }

    let mut msg = Map::new();
    msg.insert("role".into(), "assistant".into());
    msg.insert("content".into(), text.join("\n").into());
    if !reasoning.is_empty() {
        msg.insert("reasoning".into(), reasoning.join("\n").into());
    }
    if !tool_calls.is_empty() {
        msg.insert("tool_calls".into(), Value::Array(tool_calls));
    }
    messages.push(Value::Object(msg));
}

/// Join `extra` text onto an existing string field, newline-separated.
fn merge_joined(obj: &mut Map<String, Value>, key: &str, extra: Vec<String>) {
    if extra.is_empty() {
        return;
    }
    let prev = obj.get(key).and_then(|v| v.as_str()).unwrap_or("");
    let joined: Vec<&str> = std::iter::once(prev)
        .filter(|s| !s.is_empty())
        .chain(extra.iter().map(|s| s.as_str()))
        .collect();
    obj.insert(key.into(), joined.join("\n").into());
}

fn file_mtime_secs(path: &Path) -> Option<i64> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A miniature session transcript in Claude Code's on-disk shape: metadata
    /// lines, a user turn, a streamed two-line assistant message (thinking +
    /// text with a tool call), the tool result, and a closing assistant line.
    fn write_fixture(dir: &Path) -> std::path::PathBuf {
        let project = dir.join("-Users-me-Code-proj");
        fs::create_dir_all(&project).unwrap();
        let path = project.join("11111111-2222-3333-4444-555555555555.jsonl");
        let lines = [
            r#"{"type":"mode","mode":"normal","sessionId":"x"}"#,
            r#"{"type":"user","cwd":"/Users/me/Code/proj","timestamp":"2026-07-18T18:05:51.041Z","message":{"role":"user","content":"find the bug"}}"#,
            r#"{"type":"assistant","message":{"id":"msg_1","model":"claude-fable-5","role":"assistant","content":[{"type":"thinking","thinking":"hmm","signature":"sig"}]}}"#,
            r#"{"type":"assistant","message":{"id":"msg_1","model":"claude-fable-5","role":"assistant","content":[{"type":"text","text":"Looking now."},{"type":"tool_use","id":"toolu_1","name":"Read","input":{"file_path":"/a.rs"}}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":[{"type":"text","text":"fn main() {}"}]}]}}"#,
            r#"{"type":"assistant","message":{"id":"msg_2","model":"claude-fable-5","role":"assistant","content":[{"type":"text","text":"No bug."}]}}"#,
            r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"injected caveat"}}"#,
            r#"{"type":"user","isSidechain":true,"message":{"role":"user","content":"subagent prompt"}}"#,
        ];
        fs::write(&path, lines.join("\n")).unwrap();
        path
    }

    #[test]
    fn parses_a_session_into_native_messages() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture(dir.path());

        assert_eq!(scan(dir.path()), 1);
        let convs = load(dir.path());
        assert_eq!(convs.len(), 1);
        let conv = &convs[0];
        assert_eq!(conv.source_ref, "11111111-2222-3333-4444-555555555555");
        assert_eq!(conv.workspace, "/Users/me/Code/proj");
        assert_eq!(conv.model, "claude-fable-5");
        assert_eq!(conv.created_at, 1784397951);

        // user → merged assistant (thinking + text + tool call) → tool → assistant.
        let roles: Vec<&str> = conv
            .messages
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect();
        assert_eq!(roles, ["user", "assistant", "tool", "assistant"]);

        let assistant = &conv.messages[1];
        assert_eq!(assistant["content"], "Looking now.");
        assert_eq!(assistant["reasoning"], "hmm");
        assert_eq!(assistant["tool_calls"][0]["id"], "toolu_1");
        assert_eq!(assistant["tool_calls"][0]["function"]["name"], "Read");

        let tool = &conv.messages[2];
        assert_eq!(tool["tool_call_id"], "toolu_1");
        assert_eq!(tool["content"], "fn main() {}");

        // Meta and sidechain lines were skipped.
        assert!(!conv
            .messages
            .iter()
            .any(|m| m.to_string().contains("injected caveat")
                || m.to_string().contains("subagent prompt")));
    }

    #[test]
    fn a_session_without_an_exchange_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("-p");
        fs::create_dir_all(&project).unwrap();
        fs::write(
            project.join("empty.jsonl"),
            r#"{"type":"mode","mode":"normal"}"#,
        )
        .unwrap();
        assert!(load(dir.path()).is_empty());
    }
}
