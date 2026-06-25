//! `oxen-harness oxen` — version the harness config directory
//! (`~/.oxen-harness`) with Oxen, so settings, themes, and loops can be tracked
//! and shared. Opt-in: nothing is versioned until `oxen init` is run; after that
//! config writes snapshot automatically.

use anyhow::Result;
use harness_runtime::config_repo;

use crate::theme::Ui;

#[derive(Debug, clap::Subcommand)]
pub(crate) enum OxenAction {
    /// Make `~/.oxen-harness` an Oxen repo and commit the current config.
    Init,
    /// Commit the current config state (no-op if nothing changed).
    Snapshot {
        /// Commit message.
        #[arg(short, long, default_value = "Update oxen-harness config")]
        message: String,
    },
    /// Show whether config versioning is enabled.
    Status,
}

pub(crate) fn run_oxen(action: OxenAction, ui: &Ui) -> Result<()> {
    match action {
        OxenAction::Init => {
            config_repo::init()?;
            println!(
                "  {} {}",
                ui.green("✓ versioning enabled"),
                ui.dim("config changes will be committed to ~/.oxen-harness")
            );
        }
        OxenAction::Snapshot { message } => {
            if !config_repo::is_versioned() {
                println!(
                    "  {}",
                    ui.dim("config versioning isn't enabled — run: oxen-harness oxen init")
                );
                return Ok(());
            }
            config_repo::snapshot(&message);
            println!("  {}", ui.green("✓ snapshot committed"));
        }
        OxenAction::Status => {
            if config_repo::is_versioned() {
                println!("  {}", ui.green("config versioning is enabled"));
            } else {
                println!(
                    "  {}",
                    ui.dim("config versioning is off — run: oxen-harness oxen init")
                );
            }
        }
    }
    Ok(())
}
