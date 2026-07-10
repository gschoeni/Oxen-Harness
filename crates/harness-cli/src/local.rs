//! CLI glue for local models: the `models` subcommands and starting a
//! `llama-server` for a `--local` session.
//!
//! Model ids resolve offline-first via [`harness_local::resolve_runnable`]:
//! anything already in the store — a catalog model at any quant, a Hugging
//! Face model installed from the desktop app, or a GGUF dropped into the
//! models directory by hand — starts without touching the network. Only a
//! catalog model with nothing on disk triggers a download.

use std::io::Write;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use harness_local::{
    catalog, detect_hardware, fit, format_bytes, install_hint, llama_server_path, resolve_runnable,
    LocalServer, ModelRef, ModelStore, Runnable,
};

use crate::theme::{self, Ui};

/// `models` subcommand actions.
#[derive(Debug, clap::Subcommand)]
pub enum ModelsAction {
    /// List local models — the catalog plus everything downloaded.
    List,
    /// Download a model's weights (shows progress + disk usage).
    Pull {
        /// A catalog id (e.g. `qwen3-8b`) — see `models list`.
        id: String,
    },
    /// Delete a downloaded model (or a stale partial download) by id.
    Remove {
        /// A catalog id or an installed model id from `models list`.
        id: String,
    },
    /// Print the directory models are stored in.
    Path,
}

/// Handle a `models` subcommand.
pub async fn run_models(action: ModelsAction, ui: &Ui) -> Result<()> {
    let store = ModelStore::open().context("opening the local model store")?;
    match action {
        ModelsAction::List => list(&store, ui),
        ModelsAction::Pull { id } => pull(&store, &id, ui).await.map(|_| ()),
        ModelsAction::Remove { id } => remove(&store, &id, ui),
        ModelsAction::Path => {
            println!("{}", store.dir().display());
            Ok(())
        }
    }
}

/// Resolve an id, turning "unknown model" into a message that lists what *can*
/// be named: catalog ids and everything installed.
fn resolve(store: &ModelStore, id: &str) -> Result<Runnable> {
    resolve_runnable(store, id).map_err(|e| {
        let mut known: Vec<String> = catalog().into_iter().map(|m| m.id).collect();
        let extras: Vec<String> = store
            .installed()
            .into_iter()
            .map(|m| m.id)
            .filter(|i| !known.contains(i))
            .collect();
        known.extend(extras);
        anyhow::anyhow!("{e}. Known models: {}", known.join(", "))
    })
}

fn list(store: &ModelStore, ui: &Ui) -> Result<()> {
    let installed = store.installed();
    let mut covered: Vec<String> = Vec::new(); // installed ids shown via a catalog row

    let mut rows: Vec<theme::ModelRow> = catalog()
        .iter()
        .map(|spec| {
            // A catalog model is "on disk" when any of its quants is installed.
            let on_disk = harness_local::quant_refs(spec)
                .into_iter()
                .find(|r| store.is_installed(&r.id));
            let (size, installed) = match &on_disk {
                Some(r) => {
                    covered.push(r.id.clone());
                    (store.installed_size(&r.id).unwrap_or(r.size_bytes), true)
                }
                None => (spec.approx_bytes, false),
            };
            theme::ModelRow {
                id: spec.id.clone(),
                params: spec.params.clone(),
                size: format_bytes(size),
                installed,
                note: spec.note.clone(),
            }
        })
        .collect();

    // Everything else on disk (Hugging Face installs, hand-placed GGUFs) is
    // just as runnable — list it under its store id.
    for m in installed {
        if covered.contains(&m.id) || catalog::find(&m.id).is_some() {
            continue;
        }
        rows.push(theme::ModelRow {
            note: if m.display == m.id {
                "Downloaded model; runs offline.".to_string()
            } else {
                format!("{} — downloaded; runs offline.", m.display)
            },
            id: m.id,
            params: m.params,
            size: format_bytes(m.size_bytes),
            installed: true,
        });
    }

    print!(
        "{}",
        theme::models_table(
            ui,
            &rows,
            &format_bytes(store.total_disk_used()),
            &store.dir().display().to_string(),
        )
    );

    // Interrupted downloads hold real disk space but never show as installed;
    // surface them so the bytes aren't silently lost.
    let partials = store.partial_downloads();
    if !partials.is_empty() {
        println!();
        for (id, bytes) in partials {
            println!(
                "  {} {}",
                ui.brown("◐ partial download:"),
                ui.dim(&format!(
                    "{id} ({}) — reclaim the space with `models remove {id}`",
                    format_bytes(bytes)
                )),
            );
        }
    }
    Ok(())
}

fn remove(store: &ModelStore, id: &str, ui: &Ui) -> Result<()> {
    // Candidate store ids: the id itself, plus — for a catalog id — every
    // quant it may have been downloaded at.
    let mut ids = vec![id.to_string()];
    if let Some(spec) = catalog::find(id) {
        ids.extend(harness_local::quant_refs(&spec).into_iter().map(|r| r.id));
    }

    let mut freed: u64 = 0;
    let mut any = false;
    for candidate in ids {
        let reclaimed = store.installed_size(&candidate).unwrap_or(0);
        if store.remove(&candidate)? {
            any = true;
            freed += reclaimed;
        }
    }
    if any {
        println!(
            "  {} {}",
            ui.green("Lightened the wagon:"),
            ui.cream(&format!("removed {id} ({} freed)", format_bytes(freed))),
        );
    } else {
        println!(
            "  {} {}",
            ui.brown("Nothing to unpack:"),
            ui.dim(&format!("{id} isn't downloaded")),
        );
    }
    Ok(())
}

/// Make sure `id`'s weights are on disk, downloading if needed (with a live
/// progress bar). Returns the ready-to-serve model + path.
async fn pull(store: &ModelStore, id: &str, ui: &Ui) -> Result<(ModelRef, std::path::PathBuf)> {
    let (spec_display, model) = match resolve(store, id)? {
        Runnable::Installed { model, path } => {
            println!(
                "  {} {}",
                ui.green("Already in the wagon:"),
                ui.cream(&format!(
                    "{} ({})",
                    model.id,
                    format_bytes(store.installed_size(&model.id).unwrap_or(0))
                )),
            );
            return Ok((model, path));
        }
        Runnable::Downloadable { spec, model } => (spec.display, model),
    };

    let (repo, size) = match &model.origin {
        harness_local::Origin::HuggingFace { repo, .. }
        | harness_local::Origin::Oxen { repo, .. } => (repo.clone(), model.size_bytes),
    };
    println!(
        "  {} {}",
        ui.brown("Loading the wagon:"),
        ui.cream(&format!(
            "{spec_display} · ~{} · from {repo}",
            format_bytes(size)
        )),
    );

    let animate = ui.animates();
    let mut last_draw = Instant::now() - Duration::from_secs(1);
    let mut stdout = std::io::stdout();

    let result = store
        .download(&model, None, |p| {
            // Live in-place bar on a TTY; skip per-chunk noise when piped.
            if !animate {
                return;
            }
            let done = p.total == Some(p.downloaded);
            if done || last_draw.elapsed() >= Duration::from_millis(120) {
                let detail = match p.total {
                    Some(t) => format!("{} / {}", format_bytes(p.downloaded), format_bytes(t)),
                    None => format_bytes(p.downloaded),
                };
                let _ = write!(
                    stdout,
                    "\r{}\x1b[K",
                    theme::progress_bar(ui, p.fraction(), &detail)
                );
                let _ = stdout.flush();
                last_draw = Instant::now();
            }
        })
        .await;

    if animate {
        println!();
    }
    let path = result.with_context(|| format!("downloading {id}"))?;
    println!(
        "  {} {}",
        ui.green("🏞  Arrived:"),
        ui.cream(&format!(
            "{id} ready at {} ({})",
            path.display(),
            format_bytes(store.installed_size(&model.id).unwrap_or(0)),
        )),
    );
    Ok((model, path))
}

/// Ensure a model's weights are available and launch `llama-server` for it,
/// returning the running server (kept alive for the session). Anything already
/// downloaded starts without touching the network; a catalog model with
/// nothing on disk is downloaded first.
pub async fn start_for(id: &str, ui: &Ui) -> Result<(LocalServer, String)> {
    // Fail fast if llama-server isn't installed — before any big download.
    if llama_server_path().is_none() {
        bail!("llama-server isn't installed. {}", install_hint());
    }

    let store = ModelStore::open().context("opening the local model store")?;
    let (model, path) = match resolve(&store, id)? {
        Runnable::Installed { model, path } => (model, path),
        Runnable::Downloadable { .. } => pull(&store, id, ui).await?,
    };

    println!(
        "  {} {}",
        ui.brown("Hitching the oxen:"),
        ui.dim(&format!("starting llama-server for {id} (loading model…)")),
    );
    // Size the served context to this machine: weights + KV cache must fit the
    // usable memory budget, capped by the model's native window.
    let profile = detect_hardware();
    let weight_bytes = store.installed_size(&model.id).unwrap_or(0);
    let native = store.native_context(&model.id);
    let context = fit::plan_context(profile.usable_budget, weight_bytes, native);
    let server = LocalServer::start_with_context(&path, id, context, |_| {})
        .await
        .with_context(|| format!("starting llama-server for {id}"))?;
    println!(
        "  {} {}",
        ui.green("Trail is clear:"),
        ui.cream(&format!("{id} serving at {}", server.base_url())),
    );
    Ok((server, id.to_string()))
}
