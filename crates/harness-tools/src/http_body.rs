//! Bounded HTTP response reads for tools that consume untrusted endpoints.

use reqwest::Response;

pub(crate) async fn read(mut response: Response, max_bytes: usize) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|len| len > max_bytes as u64)
    {
        return Err(format!("response exceeds the {max_bytes}-byte limit"));
    }
    let mut body =
        Vec::with_capacity(response.content_length().unwrap_or(0).min(max_bytes as u64) as usize);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("could not read response: {e}"))?
    {
        if body.len().saturating_add(chunk.len()) > max_bytes {
            return Err(format!("response exceeds the {max_bytes}-byte limit"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

pub(crate) async fn text(response: Response, max_bytes: usize) -> Result<String, String> {
    String::from_utf8(read(response, max_bytes).await?)
        .map_err(|_| "response was not valid UTF-8".to_string())
}
