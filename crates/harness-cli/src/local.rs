//! CLI glue for local models: the `models` subcommands and starting a
//! `llama-server` for a `--local` session.

use std::io::Write;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use harness_local::{
    catalog, detect_hardware, find, fit, format_bytes, install_hint, llama_server_path,
    LocalServer, ModelSpec, ModelStore,
};

use crate::theme::{self, Ui};

/// `models` subcommand actions.
#[derive(Debug, clap::Subcommand)]
pub enum ModelsAction {
    /// List local models, their sizes, and what's downloaded.
    List,
    /// Download a model's weights (shows progress + disk usage).
    Pull {
        /// Catalog id, e.g. `qwen3-8b`.
        id: String,
    },
    /// Delete a downloaded model and reclaim its disk space.
    Remove {
        /// Catalog id, e.g. `qwen3-8b`.
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
        ModelsAction::Pull { id } => pull(&store, resolve(&id)?, ui).await,
        ModelsAction::Remove { id } => remove(&store, resolve(&id)?, ui),
        ModelsAction::Path => {
            println!("{}", store.dir().display());
            Ok(())
        }
    }
}

fn resolve(id: &str) -> Result<&'static ModelSpec> {
    find(id).ok_or_else(|| {
        let ids: Vec<&str> = catalog().iter().map(|m| m.id).collect();
        anyhow!("unknown model `{id}`. Known models: {}", ids.join(", "))
    })
}

/// The default download reference (the catalog's published quant) for a curated
/// model. The store is keyed by a model's ref id, so curated CLI commands map
/// their spec to this ref to download/check/remove it.
fn default_ref(spec: &ModelSpec) -> harness_local::ModelRef {
    harness_local::quant_refs(spec)
        .into_iter()
        .find(|r| r.quant == spec.quant)
        .unwrap_or_else(|| {
            harness_local::quant_refs(spec)
                .into_iter()
                .next()
                .expect("curated model has at least one quant")
        })
}

fn list(store: &ModelStore, ui: &Ui) -> Result<()> {
    let rows: Vec<theme::ModelRow> = catalog()
        .iter()
        .map(|spec| {
            let r = default_ref(spec);
            let size = store.installed_size(&r.id).unwrap_or(spec.approx_bytes);
            theme::ModelRow {
                id: spec.id,
                params: spec.params,
                size: format_bytes(size),
                installed: store.is_installed(&r.id),
                note: spec.note,
            }
        })
        .collect();
    print!(
        "{}",
        theme::models_table(
            ui,
            &rows,
            &format_bytes(store.total_disk_used()),
            &store.dir().display().to_string(),
        )
    );
    Ok(())
}

fn remove(store: &ModelStore, spec: &ModelSpec, ui: &Ui) -> Result<()> {
    let r = default_ref(spec);
    let reclaimed = store.installed_size(&r.id);
    if store.remove(&r.id)? {
        let freed = reclaimed.map(format_bytes).unwrap_or_default();
        println!(
            "  {} {}",
            ui.green("Lightened the wagon:"),
            ui.cream(&format!("removed {} ({freed} freed)", spec.id)),
        );
    } else {
        println!(
            "  {} {}",
            ui.brown("Nothing to unpack:"),
            ui.dim(&format!("{} isn't downloaded", spec.id)),
        );
    }
    Ok(())
}

/// Download a model, rendering a live progress bar.
async fn pull(store: &ModelStore, spec: &ModelSpec, ui: &Ui) -> Result<()> {
    let model = default_ref(spec);
    if store.is_installed(&model.id) {
        println!(
            "  {} {}",
            ui.green("Already in the wagon:"),
            ui.cream(&format!(
                "{} ({})",
                spec.id,
                format_bytes(store.installed_size(&model.id).unwrap_or(spec.approx_bytes))
            )),
        );
        return Ok(());
    }

    println!(
        "  {} {}",
        ui.brown("Loading the wagon:"),
        ui.cream(&format!(
            "{} · ~{} · from {}",
            spec.display,
            format_bytes(spec.approx_bytes),
            spec.repo
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
    let path = result.with_context(|| format!("downloading {}", spec.id))?;
    println!(
        "  {} {}",
        ui.green("🏞  Arrived:"),
        ui.cream(&format!(
            "{} ready at {} ({})",
            spec.id,
            path.display(),
            format_bytes(store.installed_size(&model.id).unwrap_or(0)),
        )),
    );
    Ok(())
}

/// Ensure a model is downloaded and launch `llama-server` for it, returning the
/// running server (kept alive for the session). Auto-downloads if missing.
pub async fn start_for(id: &str, ui: &Ui) -> Result<(LocalServer, String)> {
    let spec = resolve(id)?;

    // Fail fast if llama-server isn't installed — before any big download.
    if llama_server_path().is_none() {
        bail!("llama-server isn't installed. {}", install_hint());
    }

    let store = ModelStore::open().context("opening the local model store")?;
    let model = default_ref(spec);
    if !store.is_installed(&model.id) {
        pull(&store, spec, ui).await?;
    }

    println!(
        "  {} {}",
        ui.brown("Hitching the oxen:"),
        ui.dim(&format!(
            "starting llama-server for {} (loading model…)",
            spec.id
        )),
    );
    // Size the served context to this machine: weights + KV cache must fit the
    // usable memory budget, capped by the model's native window.
    let profile = detect_hardware();
    let weight_bytes = store.installed_size(&model.id).unwrap_or(0);
    let native = store.native_context(&model.id);
    let context = fit::plan_context(profile.usable_budget, weight_bytes, native);
    let server = LocalServer::start_with_context(&store.path_for(&model.id), id, context, |_| {})
        .await
        .with_context(|| format!("starting llama-server for {id}"))?;
    println!(
        "  {} {}",
        ui.green("Trail is clear:"),
        ui.cream(&format!("{} serving at {}", spec.id, server.base_url())),
    );
    Ok((server, spec.id.to_string()))
}
