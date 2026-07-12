//! Web page fetch tool — pull a specific page into the agent's context.
//!
//! Where [`crate::web`]'s `web_search` *finds* URLs (returning titles + links),
//! `web_fetch` *reads* one: it does an HTTP GET, and when the server returns
//! HTML it converts the page to clean Markdown (via [`htmd`], which drops
//! `<script>`/`<style>`/`<nav>`/`<head>` and renders headings, lists, code
//! blocks and links) so the model gets the prose, not the markup. Markdown and
//! plain-text responses pass through untouched. The result is capped at
//! `DEFAULT_MAX_CHARS` characters so a single fetch can't blow the context
//! window — the agent's compression layer shrinks it further if needed.
//!
//! This is the "read" half of autonomous web exploration: the model searches,
//! sees a promising URL, and fetches it for the full text. No API key required.

use async_trait::async_trait;
use harness_core::text::truncate_with_marker;

use crate::{ToolError, TypedTool};

/// Tool name for [`WebFetchTool`].
pub const WEB_FETCH_TOOL: &str = "web_fetch";

/// Characters of converted content returned before truncation. ~100K chars is
/// roughly 25K tokens — enough for a long docs page, bounded so one fetch can't
/// dominate the window. The model can pass a smaller `max_chars` to spend less.
const DEFAULT_MAX_CHARS: usize = 100_000;

/// Hard ceiling on `max_chars` — a caller can't ask for more than the default.
const MAX_CHARS_CEILING: usize = DEFAULT_MAX_CHARS;

/// Reject a response whose advertised `Content-Length` exceeds this (10 MiB), so
/// a stray link to a huge asset fails fast instead of buffering into memory.
const MAX_FETCH_BYTES: u64 = 10 * 1024 * 1024;

/// How long a single fetch may take before giving up.
const TIMEOUT_SECS: u64 = 30;

/// User-Agent sent with every fetch, so servers can identify the caller.
const USER_AGENT: &str = concat!("OxenHarness-WebFetch/", env!("CARGO_PKG_VERSION"));

/// Fetch a web page and return its readable content as Markdown.
pub struct WebFetchTool {
    http: reqwest::Client,
}

impl WebFetchTool {
    /// Build the tool with a client configured for polite, bounded fetches
    /// (identifying User-Agent, request timeout, transparent redirects).
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
            .build()
            // The builder only fails if the TLS backend can't initialize; fall
            // back to a default client so the tool is still constructible.
            .unwrap_or_default();
        Self { http }
    }
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Arguments to `web_fetch`.
#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct WebFetchArgs {
    /// The page URL to fetch. `https://` is assumed when no scheme is given.
    pub url: String,
    /// Max characters of content to return (default 100000, the cap). Lower it
    /// to save context when you only need the top of a long page.
    pub max_chars: Option<usize>,
}

#[async_trait]
impl TypedTool for WebFetchTool {
    const NAME: &'static str = WEB_FETCH_TOOL;
    type Args = WebFetchArgs;

    fn description(&self) -> &str {
        "Fetch a web page by URL and return its content as clean Markdown for reading. \
         Use it to pull a specific page into context — library/API docs, a README, a \
         changelog, a blog post, a spec — either after `web_search` surfaces a URL or when \
         one is given to you. HTML is converted to Markdown; large pages are truncated. \
         Don't use it for questions answerable from what you already know."
    }

    async fn run(&self, args: WebFetchArgs) -> Result<String, ToolError> {
        let url = normalize_url(&args.url)?;
        let max_chars = args
            .max_chars
            .unwrap_or(DEFAULT_MAX_CHARS)
            .clamp(1, MAX_CHARS_CEILING);

        let response = self
            .http
            .get(&url)
            // Prefer Markdown when the server can negotiate it, else HTML, else
            // anything — mirrors how docs sites can hand back Markdown directly.
            .header(
                reqwest::header::ACCEPT,
                "text/markdown, text/html;q=0.9, text/plain;q=0.8, */*;q=0.5",
            )
            .send()
            .await
            .map_err(|e| ToolError::Execution(format!("fetch failed: {e}")))?;

        // Bail on a huge advertised body before reading it into memory.
        if let Some(len) = response.content_length() {
            if len > MAX_FETCH_BYTES {
                return Err(ToolError::Execution(format!(
                    "page is {len} bytes, over the {MAX_FETCH_BYTES}-byte fetch limit"
                )));
            }
        }

        let status = response.status();
        let final_url = response.url().to_string();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();

        if !status.is_success() {
            let body = crate::http_body::text(response, MAX_FETCH_BYTES as usize)
                .await
                .unwrap_or_default();
            let snippet: String = body.trim().chars().take(200).collect();
            return Err(ToolError::Execution(format!(
                "HTTP {} fetching {final_url}{}",
                status.as_u16(),
                if snippet.is_empty() {
                    String::new()
                } else {
                    format!(": {snippet}")
                }
            )));
        }

        let is_html = content_type.contains("html");
        let body = crate::http_body::text(response, MAX_FETCH_BYTES as usize)
            .await
            .map_err(ToolError::Execution)?;

        let content = if is_html {
            html_to_markdown(&body)
        } else {
            // Markdown, plain text, JSON, etc. — already readable as-is.
            body
        };
        let content = content.trim();

        if content.is_empty() {
            return Err(ToolError::Execution(format!(
                "fetched {final_url} but it had no readable text content"
            )));
        }

        let body = truncate_with_marker(
            content,
            max_chars,
            &format!("\n\n… [content truncated at {max_chars} chars]"),
        );
        Ok(format!("Fetched {final_url}\n\n{body}"))
    }
}

/// Convert an HTML page to readable Markdown, dropping non-content elements
/// (scripts, styles, the head, embedded frames) whose text would otherwise leak
/// in — htmd's default renders some of those as blocks, so we skip them
/// explicitly. Falls back to the raw HTML if conversion fails outright.
fn html_to_markdown(html: &str) -> String {
    htmd::HtmlToMarkdown::builder()
        .skip_tags(vec![
            "script", "style", "head", "title", "noscript", "iframe", "svg",
        ])
        .build()
        .convert(html)
        .unwrap_or_else(|_| html.to_string())
}

/// Trim the URL and default a missing scheme to `https://`, rejecting anything
/// that isn't plausibly an `http(s)` web address (e.g. `file:`, `ftp:`).
fn normalize_url(raw: &str) -> Result<String, ToolError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ToolError::InvalidArguments("url is empty".into()));
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        Ok(trimmed.to_string())
    } else if trimmed.contains("://") {
        // A scheme we don't fetch (file:, ftp:, data:, …).
        Err(ToolError::InvalidArguments(format!(
            "only http and https URLs are supported, got `{trimmed}`"
        )))
    } else {
        Ok(format!("https://{trimmed}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_defaults_scheme_and_rejects_others() {
        assert_eq!(
            normalize_url("docs.oxen.ai").unwrap(),
            "https://docs.oxen.ai"
        );
        assert_eq!(
            normalize_url("  http://localhost:3000/x  ").unwrap(),
            "http://localhost:3000/x"
        );
        assert_eq!(normalize_url("https://a.com").unwrap(), "https://a.com");
        assert!(normalize_url("file:///etc/passwd").is_err());
        assert!(normalize_url("   ").is_err());
    }

    #[tokio::test]
    async fn converts_html_to_markdown() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/doc")
            .with_status(200)
            .with_header("content-type", "text/html; charset=utf-8")
            .with_body(
                "<html><head><title>Ignored</title><style>.x{}</style></head>\
                 <body><h1>Install</h1><p>Run <code>oxen init</code>.</p>\
                 <script>evil()</script></body></html>",
            )
            .create_async()
            .await;

        let tool = WebFetchTool::new();
        let out = tool
            .invoke(serde_json::json!({ "url": format!("{}/doc", server.url()) }))
            .await
            .unwrap();

        assert!(out.contains("Fetched"));
        // Heading and inline code survive as Markdown.
        assert!(out.contains("# Install"), "got: {out}");
        assert!(out.contains("`oxen init`"), "got: {out}");
        // Script/style content is dropped.
        assert!(!out.contains("evil"), "got: {out}");
        assert!(!out.contains(".x{"), "got: {out}");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn passes_through_markdown_content() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/readme.md")
            .with_status(200)
            .with_header("content-type", "text/markdown")
            .with_body("# Title\n\nSome **docs**.")
            .create_async()
            .await;

        let tool = WebFetchTool::new();
        let out = tool
            .invoke(serde_json::json!({ "url": format!("{}/readme.md", server.url()) }))
            .await
            .unwrap();
        assert!(out.contains("# Title"));
        assert!(out.contains("Some **docs**."));
    }

    #[tokio::test]
    async fn truncates_long_content_to_max_chars() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/big")
            .with_status(200)
            .with_header("content-type", "text/plain")
            .with_body("A".repeat(500))
            .create_async()
            .await;

        let tool = WebFetchTool::new();
        let out = tool
            .invoke(serde_json::json!({ "url": format!("{}/big", server.url()), "max_chars": 50 }))
            .await
            .unwrap();
        assert!(out.contains("content truncated at 50 chars"));
    }

    #[tokio::test]
    async fn surfaces_http_errors() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/missing")
            .with_status(404)
            .with_body("not found")
            .create_async()
            .await;

        let tool = WebFetchTool::new();
        let err = tool
            .invoke(serde_json::json!({ "url": format!("{}/missing", server.url()) }))
            .await
            .unwrap_err();
        match err {
            ToolError::Execution(msg) => assert!(msg.contains("404"), "got: {msg}"),
            other => panic!("expected Execution error, got {other:?}"),
        }
    }
}
