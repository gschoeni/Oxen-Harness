//! Where local models come from, unified across the curated catalog, Hugging
//! Face (paste or search), and Oxen.ai-hosted weights.
//!
//! Everything resolves to a [`ModelRef`] — one GGUF, at one quant, with enough
//! metadata to download it, run it under an alias, and show it nicely. The store
//! persists a `ModelRef` sidecar next to each download so arbitrary models keep
//! their identity across restarts.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::LocalError;

/// Known GGUF quantization tokens, longest-first so `Q4_K_M` matches before `Q4`.
const KNOWN_QUANTS: &[&str] = &[
    "IQ2_XXS", "IQ3_XXS", "Q2_G64", "PQ2_0", "IQ4_XS", "IQ4_NL", "IQ2_XS", "IQ3_XS", "IQ3_M",
    "IQ3_S", "IQ2_M", "Q3_K_L", "Q3_K_M", "Q3_K_S", "Q4_K_M", "Q4_K_S", "Q5_K_M", "Q5_K_S", "Q1_0",
    "Q2_0", "Q2_K", "Q6_K", "Q4_0", "Q4_1", "Q5_0", "Q5_1", "Q8_0", "BF16", "F16", "F32",
];

/// Best-effort: extract the quant token from a GGUF filename (e.g.
/// `Qwen3-8B-Q4_K_M.gguf` → `Q4_K_M`). Returns the longest known token present.
pub fn parse_quant(file: &str) -> Option<String> {
    let upper = file.to_ascii_uppercase();
    KNOWN_QUANTS
        .iter()
        .find(|q| upper.contains(*q))
        .map(|q| q.to_string())
}

/// Best-effort parameter-size label from a repo/file name (e.g. `…-8B-…` →
/// `8B`, `…-30B-A3B-…` → `30B-A3B`).
pub fn parse_params(name: &str) -> String {
    let upper = name.to_ascii_uppercase();
    // Scan for a `<num>B` token, optionally followed by an MoE `-A<num>B` suffix.
    let bytes = upper.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'B' {
                let mut end = i + 1;
                // Capture a MoE active-params suffix like `-A3B`.
                if upper[end..].starts_with("-A") {
                    let mut j = end + 2;
                    while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b'.') {
                        j += 1;
                    }
                    if j < bytes.len() && bytes[j] == b'B' {
                        end = j + 1;
                    }
                }
                return upper[start..end].to_string();
            }
        }
        i += 1;
    }
    String::new()
}

/// Approximate billions of parameters from a label like `8B` or `30B-A3B`, for
/// footprint estimates. Uses the leading (total) figure.
pub fn params_billions(label: &str) -> Option<f64> {
    let upper = label.to_ascii_uppercase();
    let digits: String = upper
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    digits.parse().ok()
}

/// Where a model's weights are hosted.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Origin {
    /// A Hugging Face repo + GGUF file at a revision.
    HuggingFace {
        repo: String,
        file: String,
        revision: String,
    },
    /// An Oxen.ai repo + file at a revision (hub HTTP download).
    Oxen {
        repo: String,
        file: String,
        revision: String,
    },
}

/// One downloadable model: a single GGUF at a single quant, plus the metadata the
/// UI and runtime need. `id` doubles as the served alias and on-disk basename.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelRef {
    pub id: String,
    pub display: String,
    /// Parameter-size label (e.g. `8B`); empty if unknown.
    pub params: String,
    /// Quant token (e.g. `Q4_K_M`); empty if unknown.
    pub quant: String,
    /// Native context window in tokens; 0 if unknown (the server caps it anyway).
    pub context: u32,
    /// Download size in bytes (0 if unknown).
    pub size_bytes: u64,
    pub origin: Origin,
}

impl ModelRef {
    /// The direct HTTP URL to download the weights.
    pub fn download_url(&self) -> String {
        match &self.origin {
            Origin::HuggingFace {
                repo,
                file,
                revision,
            } => format!("https://huggingface.co/{repo}/resolve/{revision}/{file}?download=true"),
            // Oxen Hub serves files over HTTP at this path. Stub: adjust if the
            // hosted-weights repos land under a different route.
            Origin::Oxen {
                repo,
                file,
                revision,
            } => format!("https://hub.oxen.ai/api/repos/{repo}/file/{revision}/{file}"),
        }
    }

    /// The auth header value (bearer token), if the origin needs one. HF uses the
    /// caller-supplied token; Oxen uses the caller-supplied Oxen key.
    pub fn needs_auth(&self) -> bool {
        matches!(self.origin, Origin::Oxen { .. })
    }
}

/// Build a stable slug for a model from its source coordinates, safe as a
/// filename and as a served alias.
pub fn slug(repo: &str, quant: &str) -> String {
    let base = repo.rsplit('/').next().unwrap_or(repo);
    let raw = if quant.is_empty() {
        base.to_string()
    } else {
        format!("{base}-{quant}")
    };
    raw.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

// ===========================================================================
// Hugging Face — paste a repo, list its GGUF quants, or search the hub.
// ===========================================================================

/// A search hit from the Hugging Face hub.
#[derive(Debug, Clone, Serialize)]
pub struct HfHit {
    /// The repo id, e.g. `bartowski/Qwen_Qwen3-8B-GGUF`.
    pub repo: String,
    pub downloads: u64,
    pub likes: u64,
    /// Parsed parameter-size label, best-effort.
    pub params: String,
}

#[derive(Deserialize)]
struct HfTreeEntry {
    #[serde(rename = "type")]
    kind: String,
    path: String,
    #[serde(default)]
    size: u64,
}

#[derive(Deserialize)]
struct HfSearchEntry {
    id: String,
    #[serde(default)]
    downloads: u64,
    #[serde(default)]
    likes: u64,
}

fn hf_request(client: &reqwest::Client, url: &str, token: Option<&str>) -> reqwest::RequestBuilder {
    let mut req = client.get(url);
    if let Some(t) = token.filter(|t| !t.trim().is_empty()) {
        req = req.bearer_auth(t.trim());
    }
    req
}

/// Parse a pasted Hugging Face reference into `(repo, optional file, revision)`.
/// Accepts `owner/name`, a full `huggingface.co/...` URL, or a direct
/// `.../resolve/<rev>/<file>.gguf` / `.../blob/<rev>/<file>.gguf` link.
pub fn parse_hf_input(input: &str) -> Option<(String, Option<String>, String)> {
    let s = input.trim().trim_end_matches('/');
    let s = s
        .strip_prefix("https://huggingface.co/")
        .or_else(|| s.strip_prefix("http://huggingface.co/"))
        .or_else(|| s.strip_prefix("huggingface.co/"))
        .unwrap_or(s);
    let parts: Vec<&str> = s.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() < 2 {
        return None;
    }
    let repo = format!("{}/{}", parts[0], parts[1]);
    // A resolve/blob link: owner/name/(resolve|blob)/<rev>/<path...>
    if parts.len() >= 5 && (parts[2] == "resolve" || parts[2] == "blob") {
        let revision = parts[3].to_string();
        let file = parts[4..].join("/");
        return Some((repo, Some(file), revision));
    }
    // owner/name/<file>.gguf
    if parts.len() >= 3 && parts[parts.len() - 1].ends_with(".gguf") {
        let file = parts[2..].join("/");
        return Some((repo, Some(file), "main".to_string()));
    }
    Some((repo, None, "main".to_string()))
}

/// List the GGUF files in a Hugging Face repo as installable [`ModelRef`]s, one
/// per quant, sorted largest-first (best quality first). `token` enables gated
/// or private repos.
pub async fn hf_list_quants(
    repo: &str,
    revision: &str,
    token: Option<&str>,
) -> Result<Vec<ModelRef>, LocalError> {
    let client = reqwest::Client::new();
    let url = format!("https://huggingface.co/api/models/{repo}/tree/{revision}?recursive=true");
    let resp = hf_request(&client, &url, token)
        .header("User-Agent", "oxen-harness")
        .send()
        .await
        .map_err(|e| LocalError::Download(format!("Hugging Face request failed: {e}")))?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED
        || resp.status() == reqwest::StatusCode::FORBIDDEN
    {
        return Err(LocalError::Download(format!(
            "no access to {repo} — it may be gated or private; add a Hugging Face token"
        )));
    }
    if !resp.status().is_success() {
        return Err(LocalError::Download(format!(
            "Hugging Face returned HTTP {} for {repo}",
            resp.status().as_u16()
        )));
    }
    let entries: Vec<HfTreeEntry> = resp
        .json()
        .await
        .map_err(|e| LocalError::Download(format!("could not read Hugging Face response: {e}")))?;

    let params = parse_params(repo);
    let base = repo.rsplit('/').next().unwrap_or(repo);
    let mapped = entries
        .into_iter()
        .filter(|e| e.kind == "file" && e.path.to_ascii_lowercase().ends_with(".gguf"))
        // Skip multi-part shards for v1 (e.g. `-00001-of-00002`); single-file only.
        .filter(|e| !e.path.contains("-of-"))
        // Skip auxiliary GGUFs that aren't standalone models (e.g. `mmproj-*`
        // multimodal projectors, which only work paired with a base model).
        .filter(|e| !is_auxiliary_gguf(&e.path))
        .map(|e| {
            let quant = parse_quant(&e.path).unwrap_or_default();
            ModelRef {
                // A filename-based id so two files in one repo (e.g. a standard
                // and an `MTP` build of the same quant) never collide on disk.
                id: id_from_file(&e.path),
                display: format!(
                    "{base}{}",
                    if quant.is_empty() {
                        String::new()
                    } else {
                        format!(" · {quant}")
                    }
                ),
                params: params.clone(),
                quant,
                context: 0,
                size_bytes: e.size,
                origin: Origin::HuggingFace {
                    repo: repo.to_string(),
                    file: e.path,
                    revision: revision.to_string(),
                },
            }
        });

    // Collapse to one entry per quant: repos often ship several builds of the
    // same quant (e.g. a standard and an `-MTP-` variant) which would otherwise
    // show as confusing duplicates. Keep the canonical one — the shortest
    // filename, i.e. the plain build without an extra variant tag.
    let mut by_quant: std::collections::HashMap<String, ModelRef> =
        std::collections::HashMap::new();
    for r in mapped {
        match by_quant.get(&r.quant) {
            Some(kept) if hf_file_len(kept) <= hf_file_len(&r) => {}
            _ => {
                by_quant.insert(r.quant.clone(), r);
            }
        }
    }
    let mut refs: Vec<ModelRef> = by_quant.into_values().collect();
    refs.sort_by_key(|r| std::cmp::Reverse(r.size_bytes));
    if refs.is_empty() {
        return Err(LocalError::Download(format!(
            "no GGUF files found in {repo}"
        )));
    }
    Ok(refs)
}

/// Whether a GGUF is an auxiliary file rather than a standalone model — e.g. a
/// multimodal projector or speculative-decoding drafter, both of which only
/// work paired with a base model and should not be offered on their own.
fn is_auxiliary_gguf(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    name.starts_with("mmproj") || name.contains("-mmproj") || name.contains("-dspark-")
}

/// The GGUF filename length for an HF ref (for picking the canonical/shortest
/// build per quant). Non-HF origins sort last.
fn hf_file_len(m: &ModelRef) -> usize {
    match &m.origin {
        Origin::HuggingFace { file, .. } => file.len(),
        _ => usize::MAX,
    }
}

/// A filename-safe, unique model id from a GGUF filename (its stem). This keeps
/// distinct files in one repo (a standard vs. an `MTP` build of the same quant)
/// from colliding on disk or on the served alias.
pub fn id_from_file(file: &str) -> String {
    let base = file.rsplit('/').next().unwrap_or(file);
    let stem = if base.to_ascii_lowercase().ends_with(".gguf") {
        &base[..base.len() - 5]
    } else {
        base
    };
    stem.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

/// Search the Hugging Face hub for GGUF repos matching `query`.
pub async fn hf_search(query: &str, token: Option<&str>) -> Result<Vec<HfHit>, LocalError> {
    let client = reqwest::Client::new();
    let q = urlencode(query);
    let url = format!(
        "https://huggingface.co/api/models?search={q}&filter=gguf&sort=downloads&direction=-1&limit=25"
    );
    let resp = hf_request(&client, &url, token)
        .header("User-Agent", "oxen-harness")
        .send()
        .await
        .map_err(|e| LocalError::Download(format!("Hugging Face search failed: {e}")))?;
    if !resp.status().is_success() {
        return Err(LocalError::Download(format!(
            "Hugging Face search returned HTTP {}",
            resp.status().as_u16()
        )));
    }
    let entries: Vec<HfSearchEntry> = resp
        .json()
        .await
        .map_err(|e| LocalError::Download(format!("could not read search response: {e}")))?;
    Ok(entries
        .into_iter()
        .map(|e| HfHit {
            params: parse_params(&e.id),
            repo: e.id,
            downloads: e.downloads,
            likes: e.likes,
        })
        .collect())
}

// ===========================================================================
// Oxen.ai cloud models — the hosted inference catalog served at
// `hub.oxen.ai/api/ai/models`, for autocompleting the Cloud Models settings.
// (These are hosted API models, not downloadable GGUF weights.)
// ===========================================================================

/// A cloud model offered by the Oxen.ai inference API.
#[derive(Debug, Clone, Serialize)]
pub struct OxenModelHit {
    /// The model id sent to the inference API, e.g. `claude-fable-5`.
    pub id: String,
    /// A friendly display name, e.g. `Claude Fable 5`.
    pub name: String,
    /// The organization/developer that made the model, best-effort.
    pub developer: String,
    /// A one-line summary, best-effort (may be empty).
    pub summary: String,
    /// The longer markdown description from the catalog (may be empty).
    pub description: String,
    /// The API route the model serves, e.g. `/chat/completions` (chat models)
    /// or `/images/generate` — lets callers filter to what they can run.
    pub endpoint: String,
    /// Per-token pricing when the model is token-billed (absent for image/
    /// time-billed models or when the catalog omits rates).
    pub pricing: Option<ModelPricing>,
    /// The model's input/output modalities, e.g. `["text"]` / `["text","image"]`.
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
}

#[derive(Deserialize)]
struct OxenModelsResponse {
    #[serde(default)]
    data: Vec<OxenModelEntry>,
}

#[derive(Deserialize)]
struct OxenModelEntry {
    id: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    endpoint: Option<String>,
    #[serde(default)]
    developer: Option<OxenDeveloper>,
    #[serde(default)]
    capabilities: Option<OxenCapabilities>,
    #[serde(default)]
    pricing: Option<OxenPricing>,
}

/// Per-token pricing for a token-billed model, as reported by the Oxen models
/// API. Costs are in US dollars *per token* (not per million).
#[derive(Debug, Clone, Copy, Deserialize)]
struct OxenPricing {
    #[serde(default)]
    input_cost_per_token: Option<f64>,
    #[serde(default)]
    output_cost_per_token: Option<f64>,
}

impl OxenPricing {
    /// Usable per-token rates, when at least one is present; missing rates count
    /// as 0 so a model priced only on output still yields a real number.
    fn rates(self) -> Option<ModelPricing> {
        match (self.input_cost_per_token, self.output_cost_per_token) {
            (None, None) => None,
            (input, output) => Some(ModelPricing {
                input_cost_per_token: input.unwrap_or(0.0),
                output_cost_per_token: output.unwrap_or(0.0),
            }),
        }
    }
}

#[derive(Deserialize)]
struct OxenDeveloper {
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize)]
struct OxenCapabilities {
    #[serde(default)]
    input: Vec<String>,
    #[serde(default)]
    output: Vec<String>,
}

/// GET `{base_url}/models` from an Oxen-compatible inference endpoint and parse
/// the catalog. `base_url` is the same base that serves model requests (e.g.
/// `https://hub.oxen.ai/api/ai`), so self-hosted endpoints list their own models.
async fn fetch_oxen_models(
    base_url: &str,
    token: Option<&str>,
) -> Result<Vec<OxenModelEntry>, LocalError> {
    let client = reqwest::Client::new();
    let mut req = client
        .get(format!("{}/models", base_url.trim_end_matches('/')))
        .header("User-Agent", "oxen-harness");
    if let Some(t) = token.filter(|t| !t.trim().is_empty()) {
        req = req.bearer_auth(t.trim());
    }
    let resp = req
        .send()
        .await
        .map_err(|e| LocalError::Download(format!("Oxen models request failed: {e}")))?;
    if !resp.status().is_success() {
        return Err(LocalError::Download(format!(
            "Oxen models API returned HTTP {}",
            resp.status().as_u16()
        )));
    }
    let body: OxenModelsResponse = resp
        .json()
        .await
        .map_err(|e| LocalError::Download(format!("could not read Oxen models response: {e}")))?;
    Ok(body.data)
}

/// Fetch `base_url`'s model catalog and filter it to those matching `query`
/// (case-insensitive substring over id / name / developer). An empty query
/// returns the whole catalog. Results are sorted so text models come first
/// (the common chat case), then alphabetically by name.
pub async fn oxen_search_models(
    base_url: &str,
    query: &str,
    token: Option<&str>,
) -> Result<Vec<OxenModelHit>, LocalError> {
    let needle = query.trim().to_ascii_lowercase();
    let mut hits: Vec<OxenModelHit> = fetch_oxen_models(base_url, token)
        .await?
        .into_iter()
        .map(|e| {
            let name = e
                .display_name
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| e.id.clone());
            let developer = e.developer.and_then(|d| d.name).unwrap_or_default();
            let (inputs, outputs) = e
                .capabilities
                .map(|c| (c.input, c.output))
                .unwrap_or_default();
            OxenModelHit {
                id: e.id,
                name,
                developer,
                summary: e.summary.unwrap_or_default(),
                description: e.description.unwrap_or_default(),
                endpoint: e.endpoint.unwrap_or_default(),
                pricing: e.pricing.and_then(OxenPricing::rates),
                inputs,
                outputs,
            }
        })
        .filter(|h| {
            needle.is_empty()
                || h.id.to_ascii_lowercase().contains(&needle)
                || h.name.to_ascii_lowercase().contains(&needle)
                || h.developer.to_ascii_lowercase().contains(&needle)
        })
        .collect();

    hits.sort_by(|a, b| {
        let a_text = a.outputs.iter().any(|o| o == "text");
        let b_text = b.outputs.iter().any(|o| o == "text");
        b_text.cmp(&a_text).then_with(|| {
            a.name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase())
        })
    });
    Ok(hits)
}

/// Per-token US-dollar pricing for a cloud model (input and output priced
/// separately, matching the Oxen models API's `input_cost_per_token` /
/// `output_cost_per_token`). Costs are per single token.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ModelPricing {
    pub input_cost_per_token: f64,
    pub output_cost_per_token: f64,
}

impl ModelPricing {
    /// The dollar cost of `input_tokens` prompt tokens plus `output_tokens`
    /// completion tokens at these rates.
    pub fn cost_of(&self, input_tokens: usize, output_tokens: usize) -> f64 {
        input_tokens as f64 * self.input_cost_per_token
            + output_tokens as f64 * self.output_cost_per_token
    }
}

/// Fetch the Oxen.ai cloud model catalog and return the per-token pricing for
/// `model_id`, if the API lists it with token-based pricing. Returns `None` for
/// unknown/local models or when the catalog omits pricing — callers should treat
/// that as "cost unavailable" rather than $0.
pub async fn oxen_model_pricing(
    model_id: &str,
    token: Option<&str>,
) -> Result<Option<ModelPricing>, LocalError> {
    Ok(oxen_model_pricing_catalog(token).await?.remove(model_id))
}

/// Fetch all token-priced Oxen cloud models in one catalog request. Usage
/// reports call this once and price every recorded model from the returned map,
/// avoiding one network round-trip per row.
pub async fn oxen_model_pricing_catalog(
    token: Option<&str>,
) -> Result<HashMap<String, ModelPricing>, LocalError> {
    oxen_model_pricing_catalog_at("https://hub.oxen.ai/api/ai", token).await
}

/// Fetch token pricing from an Oxen-compatible inference endpoint's model
/// catalog. Self-hosted endpoints can advertise their own rates, so callers
/// should use the same base URL that serves their model requests.
pub async fn oxen_model_pricing_catalog_at(
    base_url: &str,
    token: Option<&str>,
) -> Result<HashMap<String, ModelPricing>, LocalError> {
    Ok(fetch_oxen_models(base_url, token)
        .await?
        .into_iter()
        .filter_map(|entry| {
            entry
                .pricing
                .and_then(OxenPricing::rates)
                .map(|rates| (entry.id, rates))
        })
        .collect())
}

// ===========================================================================
// Oxen.ai-hosted weights — plumbing is real (Origin::Oxen + download_url); the
// featured catalog is a stub until the hosted-weights repos are published.
// ===========================================================================

/// Featured Oxen.ai-hosted models. Empty for now — the download path
/// ([`ModelRef::download_url`] for [`Origin::Oxen`]) is wired up, so populate
/// this once the repos exist (namespace TBD).
pub fn oxen_featured() -> Vec<ModelRef> {
    Vec::new()
}

/// Minimal percent-encoding for a query string (alnum + a few safe chars pass).
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pricing_catalog_uses_the_configured_endpoint() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/api/ai/models")
            .match_header("authorization", "Bearer endpoint-key")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"data":[{"id":"muse-spark-1-1","pricing":{
                    "input_cost_per_token":0.000001,"output_cost_per_token":0.000002
                }}]}"#,
            )
            .create_async()
            .await;

        let pricing = oxen_model_pricing_catalog_at(
            &format!("{}/api/ai", server.url()),
            Some("endpoint-key"),
        )
        .await
        .unwrap();

        assert_eq!(pricing["muse-spark-1-1"].cost_of(1_000, 500), 0.002);
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn model_search_uses_the_configured_endpoint_and_carries_details() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/api/ai/models")
            .match_header("authorization", "Bearer endpoint-key")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"data":[
                    {"id":"muse-spark-1-1","display_name":"Muse Spark 1.1",
                     "summary":"Fast and cheap","description":"A **speedy** model.",
                     "endpoint":"/chat/completions",
                     "developer":{"name":"Muse"},
                     "capabilities":{"input":["text"],"output":["text"]},
                     "pricing":{"input_cost_per_token":0.000001,"output_cost_per_token":0.000002}},
                    {"id":"pix-gen","display_name":"Pix Gen",
                     "endpoint":"/images/generate",
                     "capabilities":{"input":["text"],"output":["image"]},
                     "pricing":{"cost_per_image":0.01,"method":"per_image"}}
                ]}"#,
            )
            .expect(2)
            .create_async()
            .await;

        let hits = oxen_search_models(
            &format!("{}/api/ai", server.url()),
            "",
            Some("endpoint-key"),
        )
        .await
        .unwrap();

        // Text-output models sort first; image models still list (no pricing).
        assert_eq!(hits.len(), 2);
        let muse = &hits[0];
        assert_eq!(muse.id, "muse-spark-1-1");
        assert_eq!(muse.developer, "Muse");
        assert_eq!(muse.summary, "Fast and cheap");
        assert_eq!(muse.description, "A **speedy** model.");
        assert_eq!(muse.endpoint, "/chat/completions");
        let rates = muse.pricing.unwrap();
        assert_eq!(rates.input_cost_per_token, 0.000001);
        assert_eq!(rates.output_cost_per_token, 0.000002);
        assert!(hits[1].pricing.is_none());

        // Query filters over id / name / developer.
        let hits = oxen_search_models(
            &format!("{}/api/ai", server.url()),
            "muse",
            Some("endpoint-key"),
        )
        .await
        .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "muse-spark-1-1");
        mock.assert_async().await;
    }

    #[test]
    fn parses_quant_longest_match() {
        assert_eq!(
            parse_quant("Qwen3-8B-Q4_K_M.gguf").as_deref(),
            Some("Q4_K_M")
        );
        assert_eq!(parse_quant("model-Q8_0.gguf").as_deref(), Some("Q8_0"));
        assert_eq!(parse_quant("model-IQ4_XS.gguf").as_deref(), Some("IQ4_XS"));
        assert_eq!(parse_quant("Bonsai-27B-Q1_0.gguf").as_deref(), Some("Q1_0"));
        assert_eq!(
            parse_quant("Ternary-Bonsai-27B-Q2_g64.gguf").as_deref(),
            Some("Q2_G64")
        );
        assert_eq!(parse_quant("model.gguf"), None);
    }

    #[test]
    fn every_fit_quant_is_parseable_from_a_filename() {
        // fit::QUANTS drives sizing; KNOWN_QUANTS drives parse_quant. If fit
        // knows a quant that KNOWN_QUANTS doesn't, a downloaded file at that
        // quant would parse as "unknown". Keep the two lists in lockstep.
        for q in crate::fit::QUANTS {
            assert!(
                parse_quant(&format!("model-{}.gguf", q.name)).as_deref() == Some(q.name),
                "fit::QUANTS has `{}`, not recoverable by parse_quant/KNOWN_QUANTS",
                q.name
            );
        }
    }

    #[test]
    fn parses_params_including_moe() {
        assert_eq!(parse_params("bartowski/Qwen_Qwen3-8B-GGUF"), "8B");
        assert_eq!(parse_params("Qwen3-30B-A3B-GGUF"), "30B-A3B");
        assert_eq!(parse_params("some-random-repo"), "");
        assert_eq!(params_billions("30B-A3B"), Some(30.0));
        assert_eq!(params_billions("0.6B"), Some(0.6));
    }

    #[test]
    fn slug_is_filename_safe() {
        assert_eq!(
            slug("bartowski/Qwen_Qwen3-8B-GGUF", "Q4_K_M"),
            "qwen_qwen3-8b-gguf-q4_k_m".replace('_', "-")
        );
    }

    #[test]
    fn parses_hf_inputs() {
        assert_eq!(
            parse_hf_input("bartowski/Qwen_Qwen3-8B-GGUF"),
            Some(("bartowski/Qwen_Qwen3-8B-GGUF".into(), None, "main".into()))
        );
        assert_eq!(
            parse_hf_input("https://huggingface.co/owner/name/resolve/main/file-Q4_K_M.gguf"),
            Some((
                "owner/name".into(),
                Some("file-Q4_K_M.gguf".into()),
                "main".into()
            ))
        );
        assert_eq!(
            parse_hf_input("https://huggingface.co/owner/name"),
            Some(("owner/name".into(), None, "main".into()))
        );
        assert_eq!(parse_hf_input("notarepo"), None);
    }

    #[test]
    fn auxiliary_gguf_is_detected() {
        assert!(is_auxiliary_gguf("mmproj-Qwythos-9B-F16.gguf"));
        assert!(is_auxiliary_gguf("mmproj-Qwythos-9B-f16.gguf"));
        assert!(is_auxiliary_gguf("some/path/mmproj-model.gguf"));
        assert!(is_auxiliary_gguf("Bonsai-27B-dspark-Q4_1.gguf"));
        assert!(!is_auxiliary_gguf("Qwythos-9B-Q4_K_M.gguf"));
    }

    #[test]
    fn id_from_file_is_unique_and_safe() {
        // The standard and MTP builds of the same quant get distinct ids.
        let a = id_from_file("Qwythos-9B-Claude-Mythos-5-1M-BF16.gguf");
        let b = id_from_file("Qwythos-9B-Claude-Mythos-5-1M-MTP-BF16.gguf");
        assert_ne!(a, b);
        assert_eq!(a, "qwythos-9b-claude-mythos-5-1m-bf16");
        assert!(b.contains("mtp"));
        // Strips a subdir + is lowercase/filename-safe.
        assert_eq!(id_from_file("sub/Model_Q8_0.gguf"), "model-q8-0");
    }

    #[test]
    fn download_urls() {
        let hf = ModelRef {
            id: "x".into(),
            display: "x".into(),
            params: "".into(),
            quant: "Q4_K_M".into(),
            context: 0,
            size_bytes: 0,
            origin: Origin::HuggingFace {
                repo: "o/n".into(),
                file: "f.gguf".into(),
                revision: "main".into(),
            },
        };
        assert_eq!(
            hf.download_url(),
            "https://huggingface.co/o/n/resolve/main/f.gguf?download=true"
        );
        assert!(!hf.needs_auth());
    }
}
