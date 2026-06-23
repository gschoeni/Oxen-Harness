//! Web search tool backed by the Brave Search API.
//!
//! Gives the agent a way to look things up online — current events, library
//! docs, unfamiliar APIs, error messages — without leaving the loop. It calls
//! Brave's web search endpoint (<https://brave.com/search/api/>) and returns
//! ranked results as `title / url / snippet` text.
//!
//! The API key is read from `BRAVE_API_KEY` (or `BRAVE_SEARCH_API_KEY`). When no
//! key is configured the tool is left out of the default registry, so the model
//! is never offered a capability it cannot use.

use async_trait::async_trait;
use serde::Deserialize;

use crate::args::{opt_usize, require_str};
use crate::{Tool, ToolError};

/// Primary environment variable holding the Brave Search API key.
pub const BRAVE_API_KEY_ENV: &str = "BRAVE_API_KEY";
/// Alternate environment variable name accepted for the key.
pub const BRAVE_API_KEY_ENV_ALT: &str = "BRAVE_SEARCH_API_KEY";

const BRAVE_ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";
const DEFAULT_RESULTS: usize = 5;
const MAX_RESULTS: usize = 20;

/// Resolve the Brave API key from the environment, if configured.
pub fn brave_api_key() -> Option<String> {
    [BRAVE_API_KEY_ENV, BRAVE_API_KEY_ENV_ALT]
        .into_iter()
        .find_map(|var| std::env::var(var).ok())
        .filter(|key| !key.trim().is_empty())
}

/// Search the web via the Brave Search API.
pub struct WebSearchTool {
    http: reqwest::Client,
    endpoint: String,
    api_key: Option<String>,
}

impl WebSearchTool {
    /// Build the tool, resolving the API key from the environment.
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::new(),
            endpoint: BRAVE_ENDPOINT.to_string(),
            api_key: brave_api_key(),
        }
    }

    /// Whether an API key is configured (and thus the tool can run).
    pub fn is_configured(&self) -> bool {
        self.api_key.is_some()
    }

    #[cfg(test)]
    fn with_config(endpoint: impl Into<String>, api_key: Option<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            endpoint: endpoint.into(),
            api_key,
        }
    }
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }
    fn description(&self) -> &str {
        "Search the web with Brave Search and return ranked results (title, URL, snippet). \
         Use it for current events, documentation, unfamiliar libraries/APIs, or to research \
         an error message — anything that may have changed since training or isn't in the \
         workspace."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "The search query." },
                "count": {
                    "type": "integer",
                    "description": "Number of results to return (1-20, default 5).",
                    "default": DEFAULT_RESULTS
                }
            },
            "required": ["query"]
        })
    }
    async fn invoke(&self, args: serde_json::Value) -> Result<String, ToolError> {
        let query = require_str(&args, "query")?;
        let count = opt_usize(&args, "count", DEFAULT_RESULTS).clamp(1, MAX_RESULTS);
        let api_key = self.api_key.as_deref().ok_or_else(|| {
            ToolError::Execution(format!(
                "web search is unavailable: set {BRAVE_API_KEY_ENV} \
                 (get a key at https://brave.com/search/api/)"
            ))
        })?;

        let response = self
            .http
            .get(&self.endpoint)
            .header("X-Subscription-Token", api_key)
            .header("Accept", "application/json")
            .query(&[("q", query), ("count", &count.to_string())])
            .send()
            .await
            .map_err(|e| ToolError::Execution(format!("web search request failed: {e}")))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| ToolError::Execution(format!("web search read failed: {e}")))?;

        if !status.is_success() {
            return Err(ToolError::Execution(format!(
                "web search error ({}): {}",
                status.as_u16(),
                body.trim()
            )));
        }

        let parsed: BraveResponse = serde_json::from_str(&body)
            .map_err(|e| ToolError::Execution(format!("web search decode failed: {e}")))?;
        Ok(format_results(query, &parsed))
    }
}

#[derive(Debug, Default, Deserialize)]
struct BraveResponse {
    #[serde(default)]
    web: Option<BraveWeb>,
}

#[derive(Debug, Default, Deserialize)]
struct BraveWeb {
    #[serde(default)]
    results: Vec<BraveResult>,
}

#[derive(Debug, Default, Deserialize)]
struct BraveResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    description: String,
}

fn format_results(query: &str, parsed: &BraveResponse) -> String {
    let results = parsed.web.as_ref().map(|w| &w.results[..]).unwrap_or(&[]);
    if results.is_empty() {
        return format!("no web results for `{query}`");
    }

    let mut out = format!("Top {} web results for `{query}`:\n", results.len());
    for (i, r) in results.iter().enumerate() {
        out.push_str(&format!(
            "\n{}. {}\n   {}\n   {}\n",
            i + 1,
            strip_tags(&r.title),
            r.url,
            strip_tags(&r.description)
        ));
    }
    out
}

/// Brave wraps matched terms in HTML tags (e.g. `<strong>`); drop them so the
/// snippet reads as plain text.
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body() -> &'static str {
        r#"{
            "web": {
                "results": [
                    {"title": "Oxen.ai <strong>docs</strong>", "url": "https://docs.oxen.ai", "description": "The <strong>Oxen</strong> data engine."},
                    {"title": "Rust", "url": "https://rust-lang.org", "description": "A language."}
                ]
            }
        }"#
    }

    #[tokio::test]
    async fn formats_brave_results() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/res/v1/web/search")
            .match_query(mockito::Matcher::UrlEncoded("q".into(), "oxen".into()))
            .match_header("x-subscription-token", "sk-brave")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body())
            .create_async()
            .await;

        let tool = WebSearchTool::with_config(
            format!("{}/res/v1/web/search", server.url()),
            Some("sk-brave".into()),
        );
        let out = tool
            .invoke(serde_json::json!({"query": "oxen", "count": 2}))
            .await
            .unwrap();

        assert!(out.contains("docs.oxen.ai"));
        assert!(out.contains("rust-lang.org"));
        // HTML highlight tags are stripped.
        assert!(out.contains("Oxen.ai docs"));
        assert!(!out.contains("<strong>"));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn errors_without_an_api_key() {
        let tool = WebSearchTool::with_config("http://localhost/unused", None);
        let err = tool
            .invoke(serde_json::json!({"query": "anything"}))
            .await
            .unwrap_err();
        match err {
            ToolError::Execution(msg) => assert!(msg.contains(BRAVE_API_KEY_ENV)),
            other => panic!("expected Execution error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn surfaces_api_errors() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/res/v1/web/search")
            .match_query(mockito::Matcher::Any)
            .with_status(422)
            .with_body(r#"{"error":"bad query"}"#)
            .create_async()
            .await;

        let tool = WebSearchTool::with_config(
            format!("{}/res/v1/web/search", server.url()),
            Some("sk-brave".into()),
        );
        let err = tool
            .invoke(serde_json::json!({"query": "x"}))
            .await
            .unwrap_err();
        match err {
            ToolError::Execution(msg) => assert!(msg.contains("422"), "got: {msg}"),
            other => panic!("expected Execution error, got {other:?}"),
        }
    }

    #[test]
    fn strip_tags_removes_markup() {
        assert_eq!(strip_tags("a <b>bold</b> word"), "a bold word");
        assert_eq!(strip_tags("plain"), "plain");
    }

    #[test]
    fn empty_results_report_no_matches() {
        let parsed = BraveResponse::default();
        assert_eq!(format_results("q", &parsed), "no web results for `q`");
    }
}
