//! The console bridge: how browser-side errors reach the harness.
//!
//! The preview webview shows external (localhost) content, so it has no IPC
//! back to the host. Instead the host injects a small script (see
//! [`init_script`]) that wraps `console.error`/`console.warn` and the global
//! error events, and `sendBeacon`s each line — a CORS-safelisted POST that
//! needs no preflight — to this bridge: a tiny localhost HTTP listener with
//! per-session ring buffers. The buffers feed the `preview_console` tool (the
//! agent reads what the page complained about) and error-level lines are
//! surfaced to the host for the UI's "Fix it" banner.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Retained console lines per session, and the per-line length cap.
const MAX_LINES: usize = 200;
const MAX_LINE_CHARS: usize = 600;
/// Requests are tiny beacons; anything bigger is not ours.
const MAX_REQUEST_BYTES: usize = 64 * 1024;
/// A beacon that takes longer than this is not our injected script (or its
/// sender is gone) — never park a task on a half-open socket.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// Concurrent connections and distinct session buffers are bounded: the port
/// is reachable by any local process (and by no-cors POSTs from pages in the
/// user's browser), so nothing here may grow without limit.
const MAX_CONNECTIONS: usize = 32;
const MAX_SESSIONS: usize = 64;

/// One captured browser console line.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConsoleLine {
    /// "error" or "warn".
    pub level: String,
    pub text: String,
}

/// Called for every error-level line with `Some(text)`, and with `None` when
/// the page (re)loaded — which retires everything it said before. The desktop
/// turns the former into the "Fix it" banner and the latter into clearing it:
/// an error the reload already fixed must not keep accusing the app.
pub type OnConsoleError = Arc<dyn Fn(&str, Option<&str>) + Send + Sync>;

/// The listener plus its per-session buffers. One per app.
pub struct ConsoleBridge {
    port: u16,
    buffers: Arc<Mutex<HashMap<String, VecDeque<ConsoleLine>>>>,
}

impl ConsoleBridge {
    /// Bind on an ephemeral localhost port and start serving beacons.
    pub async fn start(on_error: OnConsoleError) -> std::io::Result<Arc<Self>> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let buffers: Arc<Mutex<HashMap<String, VecDeque<ConsoleLine>>>> = Arc::default();

        let accept_buffers = buffers.clone();
        tokio::spawn(async move {
            let permits = Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTIONS));
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                // At the cap, shed new connections instead of queueing them —
                // beacons are lossy by nature.
                let Ok(permit) = permits.clone().try_acquire_owned() else {
                    continue;
                };
                let buffers = accept_buffers.clone();
                let on_error = on_error.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    let _ = tokio::time::timeout(
                        REQUEST_TIMEOUT,
                        serve_one(stream, &buffers, &on_error),
                    )
                    .await;
                });
            }
        });

        Ok(Arc::new(Self { port, buffers }))
    }

    /// The port the injected script should beacon to.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The last `n` lines captured for `session`.
    pub fn tail(&self, session: &str, n: usize) -> Vec<ConsoleLine> {
        let buffers = self.buffers.lock().unwrap();
        let Some(lines) = buffers.get(session) else {
            return Vec::new();
        };
        let skip = lines.len().saturating_sub(n);
        lines.iter().skip(skip).cloned().collect()
    }

    /// Drop `session`'s buffer (page reloaded / server restarted).
    pub fn clear(&self, session: &str) {
        self.buffers.lock().unwrap().remove(session);
    }
}

/// Handle one beacon connection: `POST /log/<session>` with a JSON body.
async fn serve_one(
    mut stream: tokio::net::TcpStream,
    buffers: &Mutex<HashMap<String, VecDeque<ConsoleLine>>>,
    on_error: &OnConsoleError,
) -> std::io::Result<()> {
    let mut raw = Vec::new();
    let mut chunk = [0u8; 4096];
    // Read until the headers and declared body are complete (or limits hit).
    loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&chunk[..n]);
        if raw.len() > MAX_REQUEST_BYTES {
            break;
        }
        if let Some((headers, body_start)) = split_headers(&raw) {
            let content_length = header_value(headers, "content-length")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(0)
                .min(MAX_REQUEST_BYTES);
            if raw.len() - body_start >= content_length {
                break;
            }
        }
    }

    let response = match parse_beacon(&raw) {
        Some((session, line)) if line.level == LOAD_LEVEL => {
            // The page (re)loaded: everything the old document said is history.
            buffers.lock().unwrap().remove(&session);
            on_error(&session, None);
            "HTTP/1.1 204 No Content\r\ncontent-length: 0\r\n\r\n"
        }
        Some((session, line)) => {
            let stored = {
                let mut buffers = buffers.lock().unwrap();
                // Unknown session names come from anyone on localhost; don't
                // let them mint unbounded buffers.
                if buffers.len() >= MAX_SESSIONS && !buffers.contains_key(&session) {
                    false
                } else {
                    let lines = buffers.entry(session.clone()).or_default();
                    if lines.len() == MAX_LINES {
                        lines.pop_front();
                    }
                    lines.push_back(line.clone());
                    true
                }
            };
            if stored && line.level == "error" {
                on_error(&session, Some(&line.text));
            }
            "HTTP/1.1 204 No Content\r\ncontent-length: 0\r\n\r\n"
        }
        None => "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n",
    };
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

/// Split a raw request at the end of its headers, returning (headers, body
/// offset). Only the header bytes need to be UTF-8 — a binary body must not
/// prevent us from finding the header terminator (which would hang the read).
fn split_headers(raw: &[u8]) -> Option<(&str, usize)> {
    let end = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    let headers = std::str::from_utf8(&raw[..end]).ok()?;
    Some((headers, end + 4))
}

fn header_value<'a>(headers: &'a str, name: &str) -> Option<&'a str> {
    headers.lines().skip(1).find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
    })
}

/// The pseudo-level a fresh document beacons on load (not a console line —
/// it retires the previous document's lines).
const LOAD_LEVEL: &str = "load";

/// Parse a `POST /log/<session>` beacon into its session and console line.
fn parse_beacon(raw: &[u8]) -> Option<(String, ConsoleLine)> {
    let (headers, body_start) = split_headers(raw)?;
    let request_line = headers.lines().next()?;
    let mut parts = request_line.split_whitespace();
    if parts.next()? != "POST" {
        return None;
    }
    let session = parts.next()?.strip_prefix("/log/")?.to_string();
    if session.is_empty()
        || !session
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return None;
    }
    let body = raw.get(body_start..)?;
    let parsed: ConsoleLine = serde_json::from_slice(body).ok()?;
    let level = match parsed.level.as_str() {
        "error" | "warn" | LOAD_LEVEL => parsed.level,
        _ => return None,
    };
    let text: String = parsed.text.chars().take(MAX_LINE_CHARS).collect();
    Some((session, ConsoleLine { level, text }))
}

/// The script injected into the preview webview: report `console.error`/`warn`
/// and uncaught errors to the bridge, without changing page behavior.
pub fn init_script(port: u16, session: &str) -> String {
    format!(
        r#"(function () {{
  if (window.__oxenConsoleBridge) return;
  window.__oxenConsoleBridge = true;
  const post = (level, text) => {{
    try {{
      navigator.sendBeacon(
        'http://127.0.0.1:{port}/log/{session}',
        JSON.stringify({{ level, text: String(text).slice(0, 600) }})
      );
    }} catch (e) {{ /* never break the page */ }}
  }};
  const fmt = (args) => args.map((a) => {{
    if (typeof a === 'string') return a;
    if (a instanceof Error) return a.message + (a.stack ? '\n' + a.stack.split('\n').slice(0, 3).join('\n') : '');
    try {{ return JSON.stringify(a); }} catch (e) {{ return String(a); }}
  }}).join(' ');
  for (const level of ['error', 'warn']) {{
    const orig = console[level].bind(console);
    console[level] = (...args) => {{ post(level, fmt(args)); orig(...args); }};
  }}
  window.addEventListener('error', (e) =>
    post('error', e.message + ' (' + (e.filename || '?') + ':' + (e.lineno || 0) + ')'));
  window.addEventListener('unhandledrejection', (e) =>
    post('error', 'Unhandled promise rejection: ' + fmt([e.reason])));
  // A fresh document retires whatever the previous one complained about — the
  // reload may well have been the fix.
  post('load', 'page loaded');
}})();"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    async fn beacon(port: u16, path: &str, body: &str) {
        use tokio::io::AsyncWriteExt;
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let req = format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        s.write_all(req.as_bytes()).await.unwrap();
        let mut out = Vec::new();
        let _ = s.read_to_end(&mut out).await;
    }

    #[tokio::test]
    async fn a_page_load_retires_the_previous_document_s_errors() {
        // The reload may well have been the fix — a stale error must not keep
        // accusing a working app (banner) or mislead the agent (preview_console).
        let cleared = Arc::new(AtomicUsize::new(0));
        let counter = cleared.clone();
        let bridge = ConsoleBridge::start(Arc::new(move |_s, text| {
            if text.is_none() {
                counter.fetch_add(1, Ordering::SeqCst);
            }
        }))
        .await
        .unwrap();

        beacon(
            bridge.port(),
            "/log/s1",
            r#"{"level":"error","text":"boom at app.js:1"}"#,
        )
        .await;
        assert_eq!(bridge.tail("s1", 10).len(), 1);

        beacon(
            bridge.port(),
            "/log/s1",
            r#"{"level":"load","text":"page loaded"}"#,
        )
        .await;
        assert!(
            bridge.tail("s1", 10).is_empty(),
            "load must clear the buffer"
        );
        assert_eq!(
            cleared.load(Ordering::SeqCst),
            1,
            "the host is told to clear the banner"
        );
    }

    #[tokio::test]
    async fn captures_lines_and_reports_errors() {
        let errors = Arc::new(AtomicUsize::new(0));
        let counter = errors.clone();
        let bridge = ConsoleBridge::start(Arc::new(move |_s, text| {
            if text.is_some() {
                counter.fetch_add(1, Ordering::SeqCst);
            }
        }))
        .await
        .unwrap();

        beacon(
            bridge.port(),
            "/log/s1",
            r#"{"level":"error","text":"boom at app.js:1"}"#,
        )
        .await;
        beacon(
            bridge.port(),
            "/log/s1",
            r#"{"level":"warn","text":"deprecated"}"#,
        )
        .await;
        beacon(
            bridge.port(),
            "/log/s2",
            r#"{"level":"error","text":"other"}"#,
        )
        .await;

        let lines = bridge.tail("s1", 10);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].level, "error");
        assert!(lines[0].text.contains("boom"));
        assert_eq!(bridge.tail("s2", 10).len(), 1);
        assert_eq!(errors.load(Ordering::SeqCst), 2);

        bridge.clear("s1");
        assert!(bridge.tail("s1", 10).is_empty());
    }

    #[tokio::test]
    async fn rejects_junk_requests() {
        let bridge = ConsoleBridge::start(Arc::new(|_, _| {})).await.unwrap();

        beacon(
            bridge.port(),
            "/log/../etc",
            r#"{"level":"error","text":"x"}"#,
        )
        .await;
        beacon(bridge.port(), "/other", r#"{"level":"error","text":"x"}"#).await;
        beacon(bridge.port(), "/log/s1", "not json").await;
        beacon(bridge.port(), "/log/s1", r#"{"level":"info","text":"x"}"#).await;
        assert!(bridge.tail("s1", 10).is_empty());
    }

    #[test]
    fn init_script_targets_the_bridge() {
        let script = init_script(4242, "abc-123");
        assert!(script.contains("http://127.0.0.1:4242/log/abc-123"));
        assert!(script.contains("sendBeacon"));
    }
}
