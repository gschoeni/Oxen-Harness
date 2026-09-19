//! The hub's async generation queue: enqueue, poll, cancel, download.
//!
//! `POST /api/ai/queue` returns generation ids; `GET /api/ai/queue/:id`
//! reports `queued → processing → succeeded | failed | cancelled` with a
//! presigned `result_url` on success. The URL expires, so a result is
//! downloaded as soon as it appears. A job the server never settles (seen in
//! the wild: jobs stuck `processing` for weeks) hits a client deadline and
//! reads as timed out — never trust the server to reach a terminal state.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{MediaKind, USER_AGENT};

/// What can go wrong talking to the queue.
#[derive(Debug, thiserror::Error)]
pub enum QueueError {
    #[error("the hub rejected the API key (401): check Settings → Connection")]
    Unauthorized,
    #[error("hub error {status}: {message}")]
    Api { status: u16, message: String },
    #[error("network error: {0}")]
    Http(String),
    #[error("unexpected response: {0}")]
    Json(String),
    #[error("generation {id} still {status} after {elapsed}s — gave up waiting")]
    Timeout {
        id: String,
        status: String,
        elapsed: u64,
    },
    #[error("the result is {bytes} bytes, over the {max} byte download cap")]
    TooLarge { bytes: u64, max: u64 },
}

/// One generation as the queue reports it. Unknown fields (echoed params,
/// provider error detail) are kept in `rest` for the manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GenerationRecord {
    pub generation_id: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub media_type: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub result_url: Option<String>,
    #[serde(default)]
    pub error_message: Option<String>,
    #[serde(default)]
    pub error_retryable: Option<bool>,
    #[serde(default)]
    pub enqueued_at: Option<i64>,
    #[serde(default)]
    pub started_at: Option<i64>,
    #[serde(default)]
    pub completed_at: Option<i64>,
    #[serde(default)]
    pub seed: Option<Value>,
    #[serde(flatten)]
    pub rest: serde_json::Map<String, Value>,
}

impl GenerationRecord {
    /// `succeeded`, `failed`, or `cancelled`.
    pub fn is_terminal(&self) -> bool {
        matches!(self.status.as_str(), "succeeded" | "failed" | "cancelled")
    }
}

#[derive(Debug, Deserialize)]
struct EnqueueResponse {
    #[serde(default)]
    generations: Vec<Enqueued>,
}

#[derive(Debug, Deserialize)]
struct Enqueued {
    generation_id: String,
}

#[derive(Debug, Deserialize)]
struct ListResponse {
    #[serde(default)]
    generations: Vec<GenerationRecord>,
}

/// Cap on a downloaded result: a 4K video is tens of MB; nothing legitimate
/// is a gigabyte.
pub const MAX_DOWNLOAD_BYTES: u64 = 1024 * 1024 * 1024;

/// How long to wait before declaring a job stuck.
pub fn deadline_for(kind: MediaKind) -> Duration {
    match kind {
        MediaKind::Image => Duration::from_secs(5 * 60),
        MediaKind::Video => Duration::from_secs(30 * 60),
    }
}

/// Poll pacing: images finish in 5–30 s, videos in minutes. Starts fast and
/// backs off so a slow job isn't hammered.
fn poll_interval(kind: MediaKind, polls: u32) -> Duration {
    let (start, max) = match kind {
        MediaKind::Image => (2, 6),
        MediaKind::Video => (5, 20),
    };
    Duration::from_secs((start + polls as u64).min(max))
}

/// The queue client. `api_root` is the hub's `/api` root (the LLM client's
/// `/api/ai` base with `/ai` trimmed), so `/api/ai/queue` and `/api/events`
/// both resolve.
#[derive(Debug, Clone)]
pub struct QueueClient {
    http: reqwest::Client,
    api_root: String,
    api_key: String,
}

impl QueueClient {
    /// Build from the `/api/ai` base URL the rest of the harness uses.
    pub fn new(base_url: &str, api_key: impl Into<String>) -> Self {
        Self {
            // The hub reads a User-Agent containing "oxen" as the Oxen CLI
            // and answers 426 when its version isn't current — so this
            // client must not say "oxen" (a result download returned the
            // CLI-out-of-date error under the obvious name).
            http: reqwest::Client::builder()
                .user_agent(USER_AGENT)
                .build()
                .unwrap_or_default(),
            api_root: api_root_from_base(base_url),
            api_key: api_key.into(),
        }
    }

    /// `https://hub.oxen.ai/api`
    pub fn api_root(&self) -> &str {
        &self.api_root
    }

    fn queue_url(&self) -> String {
        format!("{}/ai/queue", self.api_root)
    }

    /// Enqueue one request (`model`, `num_generations`, model params).
    /// Returns the generation ids, in order.
    pub async fn enqueue(&self, body: &Value) -> Result<Vec<String>, QueueError> {
        // Enqueueing bills; a 5xx may have queued the job, so only a 429 or
        // a connect failure is re-sent.
        let res = Self::send(
            self.http
                .post(self.queue_url())
                .bearer_auth(&self.api_key)
                .json(body)
                .timeout(Duration::from_secs(120)),
            false,
        )
        .await?;
        let parsed: EnqueueResponse = Self::body(res).await?;
        if parsed.generations.is_empty() {
            return Err(QueueError::Json("enqueue returned no generations".into()));
        }
        Ok(parsed
            .generations
            .into_iter()
            .map(|g| g.generation_id)
            .collect())
    }

    /// One generation's current record.
    pub async fn get(&self, id: &str) -> Result<GenerationRecord, QueueError> {
        let res = Self::send(
            self.http
                .get(format!("{}/{id}", self.queue_url()))
                .bearer_auth(&self.api_key)
                .timeout(Duration::from_secs(30)),
            true,
        )
        .await?;
        Self::body(res).await
    }

    /// The account's generations (newest first as the hub returns them),
    /// optionally filtered by status.
    pub async fn list(&self, status: Option<&str>) -> Result<Vec<GenerationRecord>, QueueError> {
        let mut req = self
            .http
            .get(self.queue_url())
            .bearer_auth(&self.api_key)
            .timeout(Duration::from_secs(30));
        if let Some(status) = status {
            req = req.query(&[("status", status)]);
        }
        let res = Self::send(req, true).await?;
        let parsed: ListResponse = Self::body(res).await?;
        Ok(parsed.generations)
    }

    /// Cancel a queued or processing generation.
    pub async fn cancel(&self, id: &str) -> Result<(), QueueError> {
        let res = Self::send(
            self.http
                .delete(format!("{}/{id}", self.queue_url()))
                .bearer_auth(&self.api_key)
                .timeout(Duration::from_secs(30)),
            true,
        )
        .await?;
        let _: Value = Self::body(res).await?;
        Ok(())
    }

    /// Fetch a result by its presigned URL (no auth header — the signature
    /// is in the URL, and a bearer on a foreign host would leak the key).
    pub async fn download(&self, url: &str) -> Result<Vec<u8>, QueueError> {
        let res = Self::send(
            self.http.get(url).timeout(Duration::from_secs(10 * 60)),
            true,
        )
        .await?;
        let status = res.status();
        if !status.is_success() {
            return Err(QueueError::Api {
                status: status.as_u16(),
                message: format!("download failed: {status}"),
            });
        }
        if let Some(len) = res.content_length() {
            if len > MAX_DOWNLOAD_BYTES {
                return Err(QueueError::TooLarge {
                    bytes: len,
                    max: MAX_DOWNLOAD_BYTES,
                });
            }
        }
        let bytes = res
            .bytes()
            .await
            .map_err(|e| QueueError::Http(e.to_string()))?;
        if bytes.len() as u64 > MAX_DOWNLOAD_BYTES {
            return Err(QueueError::TooLarge {
                bytes: bytes.len() as u64,
                max: MAX_DOWNLOAD_BYTES,
            });
        }
        Ok(bytes.to_vec())
    }

    /// Poll `id` until it settles or `deadline` passes, reporting each
    /// status change to `on_status`. On timeout the job is cancelled
    /// (best-effort) so the account isn't billed for a result nobody reads.
    pub async fn wait(
        &self,
        id: &str,
        kind: MediaKind,
        deadline: Duration,
        on_status: impl FnMut(&str),
    ) -> Result<GenerationRecord, QueueError> {
        self.wait_with_feed(id, kind, deadline, None, on_status)
            .await
    }

    /// [`Self::wait`], listening on the completion feed as well: the hub's
    /// event ends the wait the moment the job settles, and polling continues
    /// underneath on a slower cadence as the safety net.
    pub async fn wait_with_feed(
        &self,
        id: &str,
        kind: MediaKind,
        deadline: Duration,
        feed: Option<std::sync::Arc<crate::events::CompletionFeed>>,
        mut on_status: impl FnMut(&str),
    ) -> Result<GenerationRecord, QueueError> {
        let started = Instant::now();
        let mut polls = 0u32;
        let mut last_status = String::new();
        let mut transient_failures = 0u32;
        let mut rx = feed.as_ref().map(|f| f.subscribe());
        loop {
            match self.get(id).await {
                Ok(record) => {
                    transient_failures = 0;
                    if record.status != last_status {
                        last_status = record.status.clone();
                        on_status(&last_status);
                    }
                    if record.is_terminal() {
                        return Ok(record);
                    }
                }
                Err(QueueError::Unauthorized) => return Err(QueueError::Unauthorized),
                Err(e) => {
                    // A blip mid-poll shouldn't abandon a paid job; three in
                    // a row is a real outage.
                    transient_failures += 1;
                    if transient_failures >= 3 {
                        return Err(e);
                    }
                }
            }
            if started.elapsed() >= deadline {
                let _ = self.cancel(id).await;
                return Err(QueueError::Timeout {
                    id: id.to_string(),
                    status: if last_status.is_empty() {
                        "unknown".into()
                    } else {
                        last_status
                    },
                    elapsed: started.elapsed().as_secs(),
                });
            }
            // With the feed open, poll rarely; the event is what ends the wait.
            let connected = feed.as_ref().is_some_and(|f| f.is_connected());
            let nap = if connected {
                poll_interval(kind, polls) * 4
            } else {
                poll_interval(kind, polls)
            };
            polls += 1;
            let Some(rx) = rx.as_mut() else {
                tokio::time::sleep(nap).await;
                continue;
            };
            let sleep = tokio::time::sleep(nap);
            tokio::pin!(sleep);
            loop {
                tokio::select! {
                    _ = &mut sleep => break,
                    event = rx.recv() => match event {
                        Ok(c) if c.generation_id == id => {
                            // Confirm through the record (seed, echoed params);
                            // fall back to the event's own fields.
                            if let Ok(record) = self.get(id).await {
                                if record.is_terminal() {
                                    if record.status != last_status {
                                        on_status(&record.status);
                                    }
                                    return Ok(record);
                                }
                            }
                            on_status(&c.status);
                            return Ok(GenerationRecord {
                                generation_id: id.to_string(),
                                status: c.status,
                                media_type: c.media_type,
                                model: c.model,
                                result_url: c.url,
                                error_message: c.error,
                                error_retryable: None,
                                enqueued_at: None,
                                started_at: None,
                                completed_at: None,
                                seed: None,
                                rest: Default::default(),
                            });
                        }
                        Ok(_) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    },
                }
            }
        }
    }

    /// Decode a JSON body, turning the hub's error envelope into a
    /// [`QueueError`].
    /// Send with the brief retry schedule (see [`harness_http::send_with_retry`]):
    /// a 429 from the queue or a blip is retried after a jittered wait that
    /// honors `Retry-After`, instead of failing the generation outright.
    async fn send(
        request: reqwest::RequestBuilder,
        safe_to_repeat: bool,
    ) -> Result<reqwest::Response, QueueError> {
        harness_http::send_with_retry(request, &harness_http::Backoff::brief(), safe_to_repeat)
            .await
            .map_err(|e| QueueError::Http(e.to_string()))
    }

    async fn body<T: serde::de::DeserializeOwned>(res: reqwest::Response) -> Result<T, QueueError> {
        let status = res.status();
        let text = res
            .text()
            .await
            .map_err(|e| QueueError::Http(e.to_string()))?;
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(QueueError::Unauthorized);
        }
        if !status.is_success() {
            return Err(QueueError::Api {
                status: status.as_u16(),
                message: error_message(&text),
            });
        }
        serde_json::from_str(&text).map_err(|e| {
            QueueError::Json(format!(
                "{e} (body: {})",
                text.chars().take(200).collect::<String>()
            ))
        })
    }
}

/// The hub's `/api` root from a `/api/ai` base URL (or a bare host URL).
pub fn api_root_from_base(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    match trimmed.strip_suffix("/ai") {
        Some(root) => root.to_string(),
        None if trimmed.ends_with("/api") => trimmed.to_string(),
        None => format!("{trimmed}/api"),
    }
}

/// Pull a human-readable message out of the hub's error envelope
/// (`{"error":{"type":..,"title":..}}`, `{"error":"..."}`, or plain text).
fn error_message(text: &str) -> String {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return text.chars().take(300).collect();
    };
    if let Some(err) = v.get("error") {
        if let Some(s) = err.as_str() {
            return s.to_string();
        }
        for key in ["title", "message", "detail", "type"] {
            if let Some(s) = err.get(key).and_then(Value::as_str) {
                return s.to_string();
            }
        }
    }
    for key in ["status_message", "message"] {
        if let Some(s) = v.get(key).and_then(Value::as_str) {
            return s.to_string();
        }
    }
    text.chars().take(300).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_root_derivation() {
        assert_eq!(
            api_root_from_base("https://hub.oxen.ai/api/ai"),
            "https://hub.oxen.ai/api"
        );
        assert_eq!(
            api_root_from_base("https://hub.oxen.ai/api/ai/"),
            "https://hub.oxen.ai/api"
        );
        assert_eq!(
            api_root_from_base("http://localhost:3001/api"),
            "http://localhost:3001/api"
        );
        assert_eq!(
            api_root_from_base("http://localhost:3001"),
            "http://localhost:3001/api"
        );
    }

    #[test]
    fn error_envelopes_read_as_messages() {
        assert_eq!(
            error_message(r#"{"error":{"type":"unauthenticated","title":"Please login"}}"#),
            "Please login"
        );
        assert_eq!(
            error_message(r#"{"error":"Model not found: x"}"#),
            "Model not found: x"
        );
        assert_eq!(error_message("plain failure"), "plain failure");
    }

    #[tokio::test]
    async fn enqueue_poll_and_cancel_over_http() {
        let mut server = mockito::Server::new_async().await;
        let enqueue = server
            .mock("POST", "/api/ai/queue")
            .match_header("authorization", "Bearer k")
            .with_status(200)
            .with_body(r#"{"generations":[{"generation_id":"g1","status":"queued"},{"generation_id":"g2","status":"queued"}]}"#)
            .create_async()
            .await;
        let first = server
            .mock("GET", "/api/ai/queue/g1")
            .with_body(r#"{"generation_id":"g1","status":"processing"}"#)
            .expect(1)
            .create_async()
            .await;
        let second = server
            .mock("GET", "/api/ai/queue/g1")
            .with_body(r#"{"generation_id":"g1","status":"succeeded","result_url":"http://x/y.png","media_type":"image","seed":42}"#)
            .create_async()
            .await;
        let client = QueueClient::new(&format!("{}/api/ai", server.url()), "k");
        let ids = client
            .enqueue(&serde_json::json!({"model": "m", "prompt": "p"}))
            .await
            .unwrap();
        assert_eq!(ids, vec!["g1", "g2"]);
        enqueue.assert_async().await;

        let mut seen = Vec::new();
        let record = client
            .wait("g1", MediaKind::Image, Duration::from_secs(30), |s| {
                seen.push(s.to_string())
            })
            .await
            .unwrap();
        assert_eq!(record.status, "succeeded");
        assert_eq!(record.result_url.as_deref(), Some("http://x/y.png"));
        assert_eq!(seen, vec!["processing", "succeeded"]);
        first.assert_async().await;
        second.assert_async().await;

        let cancel = server
            .mock("DELETE", "/api/ai/queue/g2")
            .with_body(r#"{"status":"success","generation_id":"g2"}"#)
            .create_async()
            .await;
        client.cancel("g2").await.unwrap();
        cancel.assert_async().await;
    }

    /// Probe a real result URL: `OXEN_MEDIA_SMOKE_URL=<url> cargo test -p
    /// harness-media probe_download -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn probe_download() {
        let Ok(url) = std::env::var("OXEN_MEDIA_SMOKE_URL") else {
            return;
        };
        let mut builder = reqwest::Client::builder();
        if let Ok(ua) = std::env::var("OXEN_MEDIA_SMOKE_UA") {
            builder = builder.user_agent(ua);
        }
        let http = builder.build().unwrap();
        let res = http.get(&url).send().await.unwrap();
        eprintln!("status {} version {:?}", res.status(), res.version());
        for (k, v) in res.headers() {
            eprintln!("  {k}: {}", v.to_str().unwrap_or("?"));
        }
        let body = res.bytes().await.unwrap();
        eprintln!(
            "body {} bytes; head {:?}",
            body.len(),
            &body[..body.len().min(64)]
        );
    }

    #[tokio::test]
    async fn a_feed_event_ends_the_wait_before_the_next_poll() {
        let mut server = mockito::Server::new_async().await;
        // The queue keeps saying "processing"; only the feed knows it's done.
        server
            .mock("GET", "/api/ai/queue/g9")
            .with_body(r#"{"generation_id":"g9","status":"processing"}"#)
            .create_async()
            .await;
        let client = QueueClient::new(&format!("{}/api/ai", server.url()), "k");
        let feed = crate::events::CompletionFeed::start(
            reqwest::Client::new(),
            "http://127.0.0.1:9/api".into(),
            "k".into(),
        );
        let publisher = feed.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            publisher.publish(crate::events::Completion {
                generation_id: "g9".into(),
                status: "succeeded".into(),
                media_type: Some("image".into()),
                model: Some("m".into()),
                url: Some("http://x/g9.png".into()),
                error: None,
            });
        });
        let started = Instant::now();
        let record = client
            .wait_with_feed(
                "g9",
                MediaKind::Video,
                Duration::from_secs(30),
                Some(feed),
                |_| {},
            )
            .await
            .unwrap();
        assert_eq!(record.status, "succeeded");
        assert_eq!(record.result_url.as_deref(), Some("http://x/g9.png"));
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "took {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn unauthorized_is_named() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/api/ai/queue")
            .with_status(401)
            .with_body(r#"{"error":{"type":"unauthenticated"}}"#)
            .create_async()
            .await;
        let client = QueueClient::new(&format!("{}/api/ai", server.url()), "bad");
        let err = client.enqueue(&serde_json::json!({})).await.unwrap_err();
        assert!(matches!(err, QueueError::Unauthorized));
    }

    #[tokio::test]
    async fn timeout_cancels_and_reports() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/api/ai/queue/slow")
            .with_body(r#"{"generation_id":"slow","status":"processing"}"#)
            .create_async()
            .await;
        let cancel = server
            .mock("DELETE", "/api/ai/queue/slow")
            .with_body(r#"{"status":"success"}"#)
            .create_async()
            .await;
        let client = QueueClient::new(&format!("{}/api/ai", server.url()), "k");
        let err = client
            .wait("slow", MediaKind::Image, Duration::from_millis(10), |_| {})
            .await
            .unwrap_err();
        assert!(matches!(err, QueueError::Timeout { .. }), "{err}");
        cancel.assert_async().await;
    }
}
