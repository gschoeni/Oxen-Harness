//! Models, local and cloud: the setup wizard's catalog (curated, Hugging Face,
//! and Oxen-hosted, each quant annotated with how it fits this machine), the
//! llama.cpp runtime install, weight downloads, and switching what the chat
//! runs on. A local switch starts a fresh server + session; a cloud switch
//! swaps the live conversation in place (continuing the chat).

use harness_local::{fit, install_llama_server, ModelRef, ModelStore};
use harness_protocol::{ProtocolEvent, SessionInfo};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::state::AppState;

/// The installed local models plus disk + runtime context (for the manage view).
#[derive(Clone, Serialize)]
pub(crate) struct InstalledView {
    models: Vec<ModelRef>,
    /// Bytes used by downloaded models in the store directory.
    total_disk_bytes: u64,
    dir: String,
    runtime: harness_local::RuntimeStatus,
    /// Total bytes on the volume holding the model store (null if unknown).
    disk_total: Option<u64>,
    /// Free bytes on that volume — used to warn before a download won't fit.
    disk_free: Option<u64>,
}

/// One quant of a catalog model, annotated with how well it fits this machine
/// and the exact [`ModelRef`] to download it.
#[derive(Clone, Serialize)]
pub(crate) struct QuantOption {
    quant: String,
    size_bytes: u64,
    fit: harness_local::Fit,
    installed: bool,
    /// The concrete download reference the UI passes back to `download_model`.
    model: ModelRef,
}

/// A model offered in the setup wizard: a family with one or more quants, the
/// quant we recommend for this machine, and its source.
#[derive(Clone, Serialize)]
pub(crate) struct CatalogModel {
    id: String,
    display: String,
    params: String,
    context: u32,
    note: String,
    /// `"curated"`, `"huggingface"`, or `"oxen"`.
    source: String,
    quants: Vec<QuantOption>,
    /// The quant auto-picked for this machine (best quality that fits), if any.
    recommended_quant: Option<String>,
    /// The best fit achievable across this model's quants (for badges/sorting).
    best_fit: harness_local::Fit,
}

/// The installed local models, total disk used, and the runtime status.
#[tauri::command]
pub(crate) async fn installed_local_models() -> Result<InstalledView, String> {
    let store = ModelStore::open().map_err(|e| e.to_string())?;
    let (disk_total, disk_free) = match harness_local::disk_space(store.dir()) {
        Some((total, free)) => (Some(total), Some(free)),
        None => (None, None),
    };
    Ok(InstalledView {
        models: store.installed(),
        total_disk_bytes: store.total_disk_used(),
        dir: store.dir().display().to_string(),
        runtime: harness_local::runtime::status(),
        disk_total,
        disk_free,
    })
}

/// The descriptive half of a [`CatalogModel`] — who the model is, before its
/// quants are annotated for this machine.
struct CatalogIdentity {
    id: String,
    display: String,
    params: String,
    context: u32,
    note: String,
    source: &'static str,
}

/// Annotate a list of installable refs (largest-first) into a [`CatalogModel`]:
/// fit + installed state per quant, plus the auto-picked recommended quant.
fn annotate_catalog_model(
    identity: CatalogIdentity,
    refs: Vec<ModelRef>,
    profile: &harness_local::HardwareProfile,
    store: &ModelStore,
) -> CatalogModel {
    let candidates: Vec<fit::QuantCandidate> = refs
        .iter()
        .map(|r| fit::QuantCandidate {
            quant: r.quant.clone(),
            weight_bytes: r.size_bytes,
        })
        .collect();
    let recommended_quant =
        fit::pick_quant(&candidates, fit::PLANNED_CONTEXT, profile.usable_budget)
            .map(|c| c.quant.clone());

    let quants: Vec<QuantOption> = refs
        .into_iter()
        .map(|r| QuantOption {
            quant: r.quant.clone(),
            size_bytes: r.size_bytes,
            fit: fit::fit_on(profile, r.size_bytes),
            installed: store.is_installed(&r.id),
            model: r,
        })
        .collect();
    // Best fit across quants (smallest quant usually fits best).
    let best_fit = quants
        .iter()
        .map(|q| q.fit)
        .min_by_key(|f| match f {
            harness_local::Fit::Good => 0,
            harness_local::Fit::Tight => 1,
            harness_local::Fit::TooBig => 2,
        })
        .unwrap_or(harness_local::Fit::TooBig);

    CatalogModel {
        id: identity.id,
        display: identity.display,
        params: identity.params,
        context: identity.context,
        note: identity.note,
        source: identity.source.to_string(),
        quants,
        recommended_quant,
        best_fit,
    }
}

/// The model catalog for the setup wizard: the curated family (hardware-fit and
/// quant annotated) plus any featured Oxen.ai-hosted models. Hugging Face models
/// come in via `resolve_hf_model` / `search_hf_models` instead.
#[tauri::command]
pub(crate) async fn list_model_catalog() -> Result<Vec<CatalogModel>, String> {
    let profile = harness_local::detect_hardware();
    let store = ModelStore::open().map_err(|e| e.to_string())?;

    let mut out: Vec<CatalogModel> = harness_local::catalog()
        .iter()
        .map(|spec| {
            annotate_catalog_model(
                CatalogIdentity {
                    id: spec.id.to_string(),
                    display: spec.display.to_string(),
                    params: spec.params.to_string(),
                    context: spec.context,
                    note: spec.note.to_string(),
                    source: "curated",
                },
                harness_local::quant_refs(spec),
                &profile,
                &store,
            )
        })
        .collect();

    // Featured Oxen.ai-hosted models (a stub today), grouped by repo.
    for model in harness_local::source::oxen_featured() {
        out.push(annotate_catalog_model(
            CatalogIdentity {
                id: model.id.clone(),
                display: model.display.clone(),
                params: model.params.clone(),
                context: model.context,
                note: String::new(),
                source: "oxen",
            },
            vec![model],
            &profile,
            &store,
        ));
    }
    Ok(out)
}

/// Resolve a pasted Hugging Face reference (repo or direct GGUF link) into a
/// [`CatalogModel`] with its quants annotated for this machine.
#[tauri::command]
pub(crate) async fn resolve_hf_model(input: String) -> Result<CatalogModel, String> {
    let (repo, file, revision) = harness_local::source::parse_hf_input(&input)
        .ok_or_else(|| "enter a Hugging Face repo like `owner/name` or a GGUF link".to_string())?;
    let token = hf_token();

    let refs = match file {
        // A direct link to one GGUF: resolve just that file.
        Some(f) => {
            let quant = harness_local::source::parse_quant(&f).unwrap_or_default();
            vec![ModelRef {
                id: harness_local::source::id_from_file(&f),
                display: format!(
                    "{}{}",
                    repo.rsplit('/').next().unwrap_or(&repo),
                    if quant.is_empty() {
                        String::new()
                    } else {
                        format!(" · {quant}")
                    }
                ),
                params: harness_local::source::parse_params(&repo),
                quant,
                context: 0,
                size_bytes: 0,
                origin: harness_local::Origin::HuggingFace {
                    repo: repo.clone(),
                    file: f,
                    revision,
                },
            }]
        }
        None => harness_local::source::hf_list_quants(&repo, &revision, token.as_deref())
            .await
            .map_err(|e| e.to_string())?,
    };

    let profile = harness_local::detect_hardware();
    let store = ModelStore::open().map_err(|e| e.to_string())?;
    let display = repo.clone();
    let params = harness_local::source::parse_params(&repo);
    Ok(annotate_catalog_model(
        CatalogIdentity {
            id: repo,
            display,
            params,
            context: 0,
            note: String::new(),
            source: "huggingface",
        },
        refs,
        &profile,
        &store,
    ))
}

/// Search the Hugging Face hub for GGUF repos.
#[tauri::command]
pub(crate) async fn search_hf_models(query: String) -> Result<Vec<harness_local::HfHit>, String> {
    harness_local::source::hf_search(&query, hf_token().as_deref())
        .await
        .map_err(|e| e.to_string())
}

/// Search the hosted model catalog served by the *configured* Oxen endpoint
/// (the Connection page's host, or the default hub), filtered by `query`, for
/// browsing/autocompleting the Cloud Models settings. An empty query returns
/// the full catalog, including per-token pricing and descriptions.
#[tauri::command]
pub(crate) async fn search_oxen_models(
    query: String,
) -> Result<Vec<harness_local::source::OxenModelHit>, String> {
    let cfg = harness_runtime::connection::load();
    let base_url = harness_runtime::connection::effective_base_url(&cfg);
    let key = harness_runtime::connection::effective_api_key(&base_url);
    let token = (!key.trim().is_empty()).then_some(key);
    harness_local::source::oxen_search_models(&base_url, &query, token.as_deref())
        .await
        .map_err(|e| e.to_string())
}

/// The Hugging Face token secret name (stored in `~/.oxen-harness/.env`).
const HF_TOKEN_ENV: &str = "HF_TOKEN";

/// The saved Hugging Face token, if any.
fn hf_token() -> Option<String> {
    harness_config::secrets::get(HF_TOKEN_ENV).filter(|t| !t.trim().is_empty())
}

/// Whether a Hugging Face token is currently saved.
#[tauri::command]
pub(crate) async fn hf_token_present() -> bool {
    hf_token().is_some()
}

/// Save (or clear, with an empty string) the Hugging Face token for gated repos.
#[tauri::command]
pub(crate) async fn set_hf_token(token: String) -> Result<(), String> {
    harness_config::secrets::set(HF_TOKEN_ENV, token.trim()).map_err(|e| e.to_string())
}

/// The bearer token to use for a model's origin (HF token / Oxen API key).
fn token_for(model: &ModelRef) -> Option<String> {
    match &model.origin {
        harness_local::Origin::HuggingFace { .. } => hf_token(),
        harness_local::Origin::Oxen { .. } => {
            harness_config::secrets::get("OXEN_API_KEY").filter(|t| !t.trim().is_empty())
        }
    }
}

/// Install `llama-server` for the user, streaming progress via `llama://install`.
#[tauri::command]
pub(crate) async fn install_llama(app: AppHandle) -> Result<(), String> {
    install_llama_server(|line| {
        let _ = app.emit("llama://install", line.to_string());
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// The machine's compute profile (RAM, accelerator), so the setup flow can
/// recommend models that fit and auto-pick a quantization.
#[tauri::command]
pub(crate) async fn detect_hardware() -> harness_local::HardwareProfile {
    harness_local::detect_hardware()
}

/// Status of the self-managed llama.cpp runtime (downloaded by us vs found on the
/// system vs absent), for the local-model setup screen.
#[tauri::command]
pub(crate) async fn runtime_status() -> harness_local::RuntimeStatus {
    harness_local::runtime::status()
}

/// Download + set up the self-managed `llama-server` for this platform, streaming
/// progress (log lines + bytes) via `runtime://install`. No Homebrew required.
#[tauri::command]
pub(crate) async fn install_runtime(app: AppHandle) -> Result<(), String> {
    harness_local::runtime::install(|event| {
        let _ = app.emit("runtime://install", event);
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Download a model's weights (from any source), emitting `models://progress` as
/// it streams. The `model` is a concrete [`ModelRef`] the UI chose (a specific
/// quant); the token for its origin is resolved server-side.
#[tauri::command]
pub(crate) async fn download_model(
    state: State<'_, AppState>,
    model: ModelRef,
) -> Result<(), String> {
    let store = ModelStore::open().map_err(|e| e.to_string())?;
    let token = token_for(&model);
    let id = model.id.clone();
    let sink = state.sink.clone();
    store
        .download(&model, token.as_deref(), |p| {
            sink.emit(ProtocolEvent::DownloadProgress {
                id: id.clone(),
                downloaded: p.downloaded,
                total: p.total,
                fraction: p.fraction(),
            });
        })
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Delete a downloaded model by its id.
#[tauri::command]
pub(crate) async fn remove_model(id: String) -> Result<(), String> {
    let store = ModelStore::open().map_err(|e| e.to_string())?;
    store.remove(&id).map_err(|e| e.to_string())?;
    Ok(())
}

/// Switch the session to a downloaded local model: start `llama-server` (with a
/// context window sized to this machine) and rebuild the agent against it. The
/// model must already be downloaded.
#[tauri::command]
pub(crate) async fn use_local_model(
    state: State<'_, AppState>,
    id: String,
) -> Result<SessionInfo, String> {
    state.use_local_model(&id).await
}

// ===========================================================================
// Cloud models — a small catalog of built-in models plus any the user adds,
// and the selected default. Switching swaps the live conversation in place
// (continuing the chat), unlike a local model, which needs a fresh server.
// ===========================================================================

/// The cloud model catalog (built-ins + custom), with the selected one flagged.
#[tauri::command]
pub(crate) async fn list_cloud_models() -> Result<Vec<harness_runtime::models::CloudModel>, String>
{
    Ok(harness_runtime::models::catalog())
}

/// Add (or rename) a custom cloud model; returns the updated catalog.
#[tauri::command]
pub(crate) async fn add_cloud_model(
    id: String,
    name: String,
) -> Result<Vec<harness_runtime::models::CloudModel>, String> {
    harness_runtime::models::add(&id, &name).map_err(|e| e.to_string())
}

/// Remove a custom cloud model (built-ins can't be removed); returns the catalog.
#[tauri::command]
pub(crate) async fn remove_cloud_model(
    id: String,
) -> Result<Vec<harness_runtime::models::CloudModel>, String> {
    harness_runtime::models::remove(&id).map_err(|e| e.to_string())
}

/// Switch the current chat to a cloud `model`, continuing the same conversation:
/// the transcript stays, only the model (and, if coming from a local model, the
/// client) is swapped. Also makes it the default for new chats and persists the
/// choice so it survives a restart.
#[tauri::command]
pub(crate) async fn set_model(
    state: State<'_, AppState>,
    model: String,
) -> Result<SessionInfo, String> {
    state.set_model(&model).await
}

/// Select the cloud model used by future chats without changing any live chat.
#[tauri::command]
pub(crate) async fn select_cloud_model_for_new_chats(
    state: State<'_, AppState>,
    model: String,
) -> Result<(), String> {
    let model = model.trim().to_string();
    if model.is_empty() {
        return Err("model id cannot be empty".into());
    }
    harness_runtime::models::set_selected(&model).map_err(|error| error.to_string())?;
    *state.cloud_model.lock().await = model;
    *state.local_server.lock().await = None;
    *state.local_model.lock().await = None;
    Ok(())
}
