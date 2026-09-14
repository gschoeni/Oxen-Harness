//! The hub's completion feed: `GET /api/events` is a Server-Sent Events
//! stream that emits `media_generation_completed` the moment a generation
//! settles, with the presigned result URL. One feed per session context,
//! opened before the first enqueue (the hub says: subscribe first or miss
//! the event), reconnecting with backoff, and broadcast to every waiter.
//!
//! Polling stays as the safety net: a waiter that hears nothing still asks
//! the queue on a slow cadence, so a dropped connection costs latency, not
//! a lost result.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use futures_util::StreamExt;
use serde::Deserialize;
use tokio::sync::broadcast;

/// One `media_generation_completed` event.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Completion {
    pub generation_id: String,
    /// `succeeded` or `failed`.
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub media_type: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// The presigned result URL (succeeded only).
    #[serde(default)]
    pub url: Option<String>,
    /// The failure reason (failed only).
    #[serde(default)]
    pub error: Option<String>,
}

/// The event name the hub uses.
pub const COMPLETED_EVENT: &str = "media_generation_completed";

/// A live subscription to the hub's event stream.
pub struct CompletionFeed {
    tx: broadcast::Sender<Completion>,
    connected: AtomicBool,
}

impl CompletionFeed {
    /// Open the feed in the background. The task holds only a weak handle,
    /// so dropping the last `Arc` ends it.
    pub fn start(http: reqwest::Client, api_root: String, api_key: String) -> Arc<Self> {
        let (tx, _) = broadcast::channel(64);
        let feed = Arc::new(Self {
            tx,
            connected: AtomicBool::new(false),
        });
        let weak = Arc::downgrade(&feed);
        tokio::spawn(run(weak, http, api_root, api_key));
        feed
    }

    /// Whether the stream is currently open (waiters poll slower when so).
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Completion> {
        self.tx.subscribe()
    }

    /// Deliver an event to every waiter (also used by tests).
    pub fn publish(&self, completion: Completion) {
        let _ = self.tx.send(completion);
    }
}

async fn run(feed: Weak<CompletionFeed>, http: reqwest::Client, api_root: String, api_key: String) {
    let url = format!("{api_root}/events");
    let mut backoff = Duration::from_secs(1);
    loop {
        let Some(strong) = feed.upgrade() else { return };
        let connect = http
            .get(&url)
            .bearer_auth(&api_key)
            .header("accept", "text/event-stream")
            .send()
            .await;
        match connect {
            Ok(res) if res.status().is_success() => {
                strong.connected.store(true, Ordering::Relaxed);
                backoff = Duration::from_secs(1);
                let mut stream = res.bytes_stream();
                let mut decoder = EventDecoder::default();
                drop(strong);
                while let Some(chunk) = stream.next().await {
                    let Some(strong) = feed.upgrade() else { return };
                    let Ok(bytes) = chunk else { break };
                    for (event, data) in decoder.push(&String::from_utf8_lossy(&bytes)) {
                        if event.as_deref() == Some(COMPLETED_EVENT)
                            || data.contains(COMPLETED_EVENT)
                        {
                            if let Ok(completion) = serde_json::from_str::<Completion>(&data) {
                                strong.publish(completion);
                            }
                        }
                    }
                }
                if let Some(strong) = feed.upgrade() {
                    strong.connected.store(false, Ordering::Relaxed);
                }
            }
            Ok(res) => {
                tracing::debug!("media event feed: HTTP {}", res.status());
                strong.connected.store(false, Ordering::Relaxed);
                // A 401 won't fix itself quickly; back off harder.
                backoff = backoff.max(Duration::from_secs(30));
            }
            Err(e) => {
                tracing::debug!("media event feed: {e}");
                strong.connected.store(false, Ordering::Relaxed);
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(60));
    }
}

/// An SSE decoder that keeps the `event:` name with each `data:` payload
/// (the LLM client's decoder drops names, which this feed needs).
#[derive(Debug, Default)]
pub struct EventDecoder {
    buf: String,
    event: Option<String>,
    data: Vec<String>,
}

impl EventDecoder {
    /// Feed bytes; return every complete `(event, data)` message.
    pub fn push(&mut self, text: &str) -> Vec<(Option<String>, String)> {
        self.buf.push_str(text);
        let mut out = Vec::new();
        while let Some(newline) = self.buf.find('\n') {
            let line: String = self.buf.drain(..=newline).collect();
            let line = line.trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                if !self.data.is_empty() {
                    out.push((self.event.take(), self.data.join("\n")));
                    self.data.clear();
                } else {
                    self.event = None;
                }
            } else if let Some(rest) = line.strip_prefix("event:") {
                self.event = Some(rest.trim().to_string());
            } else if let Some(rest) = line.strip_prefix("data:") {
                self.data.push(rest.trim().to_string());
            }
            // Comments (`:keep-alive`) and other fields are ignored.
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_keeps_event_names_and_ignores_keepalives() {
        let mut d = EventDecoder::default();
        let msgs = d.push(": keep-alive\n\nevent: media_generation_completed\ndata: {\"generation_id\":\"g1\",\"status\":\"succeeded\"}\n\n");
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].0.as_deref(), Some("media_generation_completed"));
        assert!(msgs[0].1.contains("\"g1\""));
        // Split across chunks.
        let mut d = EventDecoder::default();
        assert!(d.push("event: x\ndata: {\"generation_id\":").is_empty());
        let msgs = d.push("\"g2\"}\n\n");
        assert_eq!(msgs[0].1, "{\"generation_id\":\"g2\"}");
    }

    #[tokio::test]
    async fn feed_broadcasts_completions_from_the_stream() {
        let mut server = mockito::Server::new_async().await;
        let body = concat!(
            ": keep-alive\n\n",
            "event: media_generation_completed\n",
            "data: {\"generation_id\":\"g1\",\"status\":\"succeeded\",\"media_type\":\"image\",\"model\":\"m\",\"url\":\"http://x/y.png\"}\n\n",
            "event: media_generation_completed\n",
            "data: {\"generation_id\":\"g2\",\"status\":\"failed\",\"media_type\":\"video\",\"error\":\"nope\"}\n\n"
        );
        let _m = server
            .mock("GET", "/api/events")
            .match_header("authorization", "Bearer k")
            .with_header("content-type", "text/event-stream")
            .with_body(body)
            .create_async()
            .await;
        let feed = CompletionFeed::start(
            reqwest::Client::new(),
            format!("{}/api", server.url()),
            "k".into(),
        );
        let mut rx = feed.subscribe();
        let first = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("event in time")
            .unwrap();
        assert_eq!(first.generation_id, "g1");
        assert_eq!(first.url.as_deref(), Some("http://x/y.png"));
        let second = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("event in time")
            .unwrap();
        assert_eq!(second.status, "failed");
        assert_eq!(second.error.as_deref(), Some("nope"));
    }
}
