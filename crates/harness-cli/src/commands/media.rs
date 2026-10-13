//! `oxen-harness media …` — on-device image models: list them, pull their
//! weights (and the engine runtime), remove them.
//!
//! Generation itself happens through the agent's `generate_image` tool with
//! a `local/` model id; this command only manages what is on disk.

use std::io::Write;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use harness_local::format_bytes;
use harness_media_local::{EngineStatus, LocalMediaService, ModelSpec};
use tokio_util::sync::CancellationToken;

use crate::theme::{self, Ui};

#[derive(Debug, clap::Subcommand)]
pub enum MediaAction {
    /// List local image models: installed, size, license, typical speed.
    List,
    /// Download a model's weights and the engine runtime it needs.
    Pull {
        /// A model id from `media list` (e.g. `local/iris-3b-turbo`).
        id: String,
    },
    /// Delete a model's weights (files another installed model shares stay).
    Remove {
        /// A model id from `media list`.
        id: String,
    },
    /// Print the directory weights are stored in.
    Path,
}

pub async fn handle_cli(action: MediaAction, ui: &Ui) -> Result<()> {
    let service = harness_media_local::service().context("loading the local media catalog")?;
    match action {
        MediaAction::List => {
            list(&service, ui);
            Ok(())
        }
        MediaAction::Pull { id } => pull(&service, &id, ui).await,
        MediaAction::Remove { id } => remove(&service, &id, ui),
        MediaAction::Path => {
            println!("{}", service.store().root().display());
            Ok(())
        }
    }
}

fn spec<'a>(service: &'a LocalMediaService, id: &str) -> Result<&'a ModelSpec> {
    service.spec(id).ok_or_else(|| {
        let known: Vec<&str> = service.specs().iter().map(|s| s.id.as_str()).collect();
        anyhow::anyhow!("no local media model `{id}`. Known: {}", known.join(", "))
    })
}

fn list(service: &LocalMediaService, ui: &Ui) {
    for s in service.specs() {
        let missing = service.store().missing_bytes(s);
        let state = if missing == 0 {
            ui.green("● installed")
        } else {
            ui.dim(&format!("○ {} to pull", format_bytes(missing)))
        };
        let speed = s
            .typical_secs
            .map(|t| format!(" · ~{t:.0}s/image"))
            .unwrap_or_default();
        println!(
            "  {}  {}  {}",
            ui.cream(&format!("{:<24}", s.id)),
            state,
            ui.dim(&format!(
                "{} · {}{speed} · {}",
                format_bytes(s.total_bytes()),
                s.license,
                s.display_name
            )),
        );
        if let Some(summary) = &s.summary {
            println!("  {}  {}", " ".repeat(24), ui.dim(summary));
        }
    }
    let runtime = match service
        .engine(harness_media_local::sdcpp::ENGINE_ID)
        .map(|e| e.status())
    {
        Some(EngineStatus::Ready(v)) => format!("stable-diffusion.cpp {v}"),
        Some(EngineStatus::NotInstalled(how)) => format!("not installed — {how}"),
        None => "not built in".to_string(),
    };
    println!();
    println!("  {} {}", ui.brown("Engine:"), ui.dim(&runtime));
    println!(
        "  {} {}",
        ui.brown("Weights:"),
        ui.dim(&service.store().root().display().to_string())
    );
    println!(
        "  {}",
        ui.dim("Use one by asking the agent for an image with that model id.")
    );
}

/// A redraw-throttled progress line on a TTY; silent when piped.
struct Bar<'a> {
    ui: &'a Ui,
    last: Instant,
}

impl<'a> Bar<'a> {
    fn new(ui: &'a Ui) -> Self {
        Self {
            ui,
            last: Instant::now() - Duration::from_secs(1),
        }
    }

    fn draw(&mut self, label: &str, done: u64, total: u64) {
        if !self.ui.animates() {
            return;
        }
        if done < total && self.last.elapsed() < Duration::from_millis(120) {
            return;
        }
        let fraction = if total == 0 {
            None
        } else {
            Some(done as f64 / total as f64)
        };
        let detail = format!("{label} {} / {}", format_bytes(done), format_bytes(total));
        let mut out = std::io::stdout();
        let _ = write!(
            out,
            "\r{}\x1b[K",
            theme::progress_bar(self.ui, fraction, &detail)
        );
        let _ = out.flush();
        self.last = Instant::now();
    }

    fn finish(&self) {
        if self.ui.animates() {
            println!();
        }
    }
}

async fn pull(service: &LocalMediaService, id: &str, ui: &Ui) -> Result<()> {
    let s = spec(service, id)?;
    let cancel = CancellationToken::new();
    let on_ctrl_c = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            on_ctrl_c.cancel();
        }
    });

    if s.engine == harness_media_local::sdcpp::ENGINE_ID
        && harness_media_local::sdcpp::binary().is_none()
    {
        println!(
            "  {} {}",
            ui.brown("Fetching the engine:"),
            ui.cream(&format!(
                "stable-diffusion.cpp {}",
                harness_media_local::sdcpp::PINNED_VERSION
            )),
        );
        let mut bar = Bar::new(ui);
        let installed = harness_media_local::sdcpp::install_runtime(&cancel, |done, total| {
            bar.draw("sd-server", done, total.unwrap_or(0));
        })
        .await;
        bar.finish();
        installed.context("installing the stable-diffusion.cpp runtime")?;
    }

    let missing = service.store().missing_bytes(s);
    if missing == 0 {
        println!(
            "  {} {}",
            ui.green("Already in the wagon:"),
            ui.cream(&format!("{} ({})", s.id, format_bytes(s.total_bytes()))),
        );
        return Ok(());
    }
    // The store may not exist yet; measure the disk it will live on.
    let root = service.store().root();
    let probe = root.ancestors().find(|p| p.exists()).unwrap_or(root);
    if let Some((_, available)) = harness_local::disk_space(probe) {
        if available < missing {
            anyhow::bail!(
                "{} needs {} more on disk and only {} is free",
                s.id,
                format_bytes(missing),
                format_bytes(available)
            );
        }
    }
    println!(
        "  {} {}",
        ui.brown("Loading the wagon:"),
        ui.cream(&format!(
            "{} · {} to download · {}",
            s.display_name,
            format_bytes(missing),
            s.license
        )),
    );
    let mut bar = Bar::new(ui);
    let pulled = service
        .store()
        .pull(s, &cancel, |p| {
            let name = p.file.rsplit('/').next().unwrap_or(&p.file).to_string();
            bar.draw(&name, p.done_bytes, p.total_bytes);
        })
        .await;
    bar.finish();
    pulled.with_context(|| format!("pulling {}", s.id))?;
    println!(
        "  {} {}",
        ui.green("🏞  Arrived:"),
        ui.cream(&format!(
            "{} is ready — ask for an image with model `{}`",
            s.display_name, s.id
        )),
    );
    Ok(())
}

fn remove(service: &LocalMediaService, id: &str, ui: &Ui) -> Result<()> {
    let s = spec(service, id)?;
    let others: Vec<ModelSpec> = service
        .specs()
        .iter()
        .filter(|o| o.id != s.id && service.store().is_installed(o))
        .cloned()
        .collect();
    let freed = service
        .store()
        .remove(s, &others)
        .with_context(|| format!("removing {}", s.id))?;
    if freed == 0 {
        println!(
            "  {} {}",
            ui.brown("Nothing to unpack:"),
            ui.dim(&format!("{} has no files of its own on disk", s.id)),
        );
    } else {
        println!(
            "  {} {}",
            ui.green("Lightened the wagon:"),
            ui.cream(&format!("removed {} ({} freed)", s.id, format_bytes(freed))),
        );
    }
    Ok(())
}
