//! `oxen-harness config` — save an Oxen API key from the shell, for a first
//! run or a new server. The key goes into the `oxen` CLI's auth config
//! (`~/.config/oxen/auth_config.toml`, per host — the file `oxen login`
//! writes and every front end of the harness reads), and the host becomes the
//! endpoint the harness talks to. A host that already has a key keeps it
//! unless `--force` says otherwise.

use anyhow::{bail, Context, Result};
use harness_llm::auth::{self, DEFAULT_OXEN_HOST};
use std::io::IsTerminal;

use crate::commands::auth::read_masked_key;
use crate::theme::Ui;

#[derive(Debug, clap::Args)]
pub(crate) struct ConfigArgs {
    /// The Oxen host the key is for, as `host[:port]` or a URL. It also
    /// becomes the endpoint the harness talks to.
    #[arg(long, default_value = DEFAULT_OXEN_HOST)]
    host: String,
    /// The API key. Leave it out to be prompted with masked input — a key
    /// passed here lands in your shell history.
    #[arg(long, value_name = "KEY")]
    api_key: Option<String>,
    /// Replace the key already saved for the host.
    #[arg(long)]
    force: bool,
}

/// What `config` does about the key, given what the auth config holds.
#[derive(Debug, PartialEq, Eq)]
enum KeyStep {
    /// The host has a key and nobody asked to replace it.
    Keep,
    /// Save this key.
    Save(String),
    /// Ask for the key with masked input.
    Prompt,
}

fn key_step(already_saved: bool, args: &ConfigArgs) -> KeyStep {
    match (&args.api_key, already_saved && !args.force) {
        (_, true) => KeyStep::Keep,
        (Some(key), false) => KeyStep::Save(key.trim().to_string()),
        (None, false) => KeyStep::Prompt,
    }
}

/// `localhost:3001`, `http://localhost:3001/api/ai`, and `localhost:3001/`
/// all name the one auth-config entry `localhost:3001`.
fn normalize_host(host: &str) -> String {
    harness_llm::host_from_base_url(&harness_llm::base_url_from_host(host))
}

pub(crate) fn run_config(args: ConfigArgs, ui: &Ui) -> Result<()> {
    let host = normalize_host(&args.host);
    if host.is_empty() {
        bail!("--host can't be empty");
    }
    let path = auth::auth_config_path()
        .context("could not determine the home directory for the Oxen config")?;
    let already_saved = auth::configured_api_key(&host)?.is_some();

    let key = match key_step(already_saved, &args) {
        KeyStep::Keep => None,
        KeyStep::Save(key) => Some(key),
        KeyStep::Prompt => {
            if !std::io::stdin().is_terminal() {
                bail!(
                    "no API key given — pass --api-key <KEY>, or run in a terminal to be prompted"
                );
            }
            let entered = read_masked_key(ui, &host, &path.display().to_string())
                .context("reading the API key")?
                .entered();
            match entered {
                Some(key) => Some(key),
                None => bail!("cancelled — nothing was saved"),
            }
        }
    };
    match key {
        Some(key) if key.is_empty() => bail!("the API key can't be empty"),
        Some(key) => {
            auth::save_api_key(&host, &key)?;
            println!(
                "  {} {}",
                ui.green("🔑 ✓"),
                ui.cream(&format!(
                    "Saved the API key for {host} to {}",
                    path.display()
                ))
            );
        }
        None => println!(
            "  {} {} {}",
            ui.green("🔑 ✓"),
            ui.cream(&format!(
                "{host} already has an API key in {}",
                path.display()
            )),
            ui.dim("— pass --force to replace it.")
        ),
    }

    // The default host is saved as "no override", so OXEN_HOST/OXEN_BASE_URL
    // keep working for runs that set them.
    let saved_host = if host == DEFAULT_OXEN_HOST {
        ""
    } else {
        host.as_str()
    };
    harness_runtime::connection::set_oxen_host(saved_host)
        .with_context(|| format!("saving {host} as the harness endpoint"))?;
    let base_url =
        harness_runtime::connection::effective_base_url(&harness_runtime::connection::load());
    println!(
        "  {} {}",
        ui.green("🌐 ✓"),
        ui.cream(&format!("The harness now talks to {base_url}"))
    );

    // `OXEN_API_KEY` outranks the auth config for every host, so a key saved
    // here would be silently ignored while it is set.
    if std::env::var(auth::API_KEY_ENV).is_ok_and(|v| !v.trim().is_empty()) {
        println!(
            "  {} {}",
            ui.accent("⚠"),
            ui.dim(&format!(
                "{} is set (in your environment or ~/.oxen-harness/.env) and takes \
                 precedence over {} — unset it to use the key saved here.",
                auth::API_KEY_ENV,
                path.display()
            ))
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        args: ConfigArgs,
    }

    fn args(argv: &[&str]) -> ConfigArgs {
        let mut full = vec!["config"];
        full.extend_from_slice(argv);
        Cli::try_parse_from(full).expect("valid arguments").args
    }

    #[test]
    fn the_host_defaults_to_the_hub() {
        assert_eq!(args(&[]).host, "hub.oxen.ai");
    }

    #[test]
    fn an_existing_key_is_kept_unless_forced() {
        assert_eq!(
            key_step(true, &args(&["--api-key", "sk-new"])),
            KeyStep::Keep
        );
        assert_eq!(key_step(true, &args(&[])), KeyStep::Keep);
        assert_eq!(
            key_step(true, &args(&["--api-key", " sk-new ", "--force"])),
            KeyStep::Save("sk-new".into())
        );
        assert_eq!(key_step(true, &args(&["--force"])), KeyStep::Prompt);
    }

    #[test]
    fn a_missing_key_is_saved_or_asked_for() {
        assert_eq!(
            key_step(false, &args(&["--api-key", "sk-new"])),
            KeyStep::Save("sk-new".into())
        );
        assert_eq!(key_step(false, &args(&[])), KeyStep::Prompt);
    }

    #[test]
    fn hosts_and_urls_name_the_same_entry() {
        assert_eq!(normalize_host("localhost:3001"), "localhost:3001");
        assert_eq!(
            normalize_host("http://localhost:3001/api/ai"),
            "localhost:3001"
        );
        assert_eq!(normalize_host("https://hub.oxen.ai/"), "hub.oxen.ai");
    }
}
