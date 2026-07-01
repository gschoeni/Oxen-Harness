//! One streamed-HTTP-download primitive shared by the model downloader
//! ([`crate::store`]) and the llama.cpp runtime installer ([`crate::runtime`]).
//!
//! Both need the same thing: send a GET, sanity-check the status, and stream the
//! body to a file while reporting byte progress. They differ only at the edges —
//! auth, a User-Agent, whether a gated 401/403 gets a friendlier message, and
//! how they surface progress — so those are the [`FetchOpts`] and the progress
//! callback; the request-and-stream core lives here once.

use std::path::Path;

use futures_util::StreamExt;
use reqwest::StatusCode;

use crate::LocalError;

/// Edge options for [`fetch_to_file`]. All default to "off".
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct FetchOpts<'a> {
    /// Bearer token to authorize the request (blank/whitespace is ignored).
    pub token: Option<&'a str>,
    /// `User-Agent` header to send, if any.
    pub user_agent: Option<&'a str>,
    /// When set, a `401`/`403` response returns this message instead of the
    /// generic HTTP error — used to hint that a model is gated/private.
    pub gated_message: Option<&'a str>,
}

/// GET `url` and stream the response body to `dest`, invoking
/// `on_progress(downloaded, total)` once before the first byte and after every
/// chunk (`total` is `None` when the server sends no `Content-Length`).
///
/// `dest` is written directly; callers wanting atomic replacement pass a
/// temporary path and rename it on success.
pub(crate) async fn fetch_to_file<F>(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    opts: FetchOpts<'_>,
    mut on_progress: F,
) -> Result<(), LocalError>
where
    F: FnMut(u64, Option<u64>),
{
    let mut req = client.get(url);
    if let Some(ua) = opts.user_agent {
        req = req.header("User-Agent", ua);
    }
    if let Some(token) = opts.token.filter(|t| !t.trim().is_empty()) {
        req = req.bearer_auth(token.trim());
    }

    let resp = req
        .send()
        .await
        .map_err(|e| LocalError::Download(format!("request failed: {e}")))?;
    if let Some(message) = opts.gated_message {
        if matches!(
            resp.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ) {
            return Err(LocalError::Download(message.to_string()));
        }
    }
    if !resp.status().is_success() {
        return Err(LocalError::Download(format!(
            "HTTP {} fetching {url}",
            resp.status().as_u16()
        )));
    }

    let total = resp.content_length();
    let mut file = tokio::fs::File::create(dest).await?;
    let mut downloaded: u64 = 0;
    on_progress(downloaded, total);

    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| LocalError::Download(format!("stream error: {e}")))?;
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
        downloaded += chunk.len() as u64;
        on_progress(downloaded, total);
    }
    tokio::io::AsyncWriteExt::flush(&mut file).await?;
    Ok(())
}
