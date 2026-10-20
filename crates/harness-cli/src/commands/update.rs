//! `oxen-harness update` and `/update`: say which version this is, whether a
//! newer CLI release is on GitHub, and install it in place.
//!
//! Every interactive launch kicks off [`spawn_startup_check`] (cached for an
//! hour, so it rarely touches the network) and the banner's Version row
//! shows the answer. `/update` installs and then restarts into the new
//! binary on the same session; the subcommand installs and exits.

use std::ffi::OsString;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context, Result};
use harness_update::{Component, Release, UpdateError, UpdateStatus, Updater, Version};

use crate::repl_loop::ReplContext;
use crate::theme::Ui;

/// The version this binary is.
pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The startup check's answer, once it lands: the banner and later banners
/// (`/location` redraws) read it from here.
static KNOWN: OnceLock<UpdateStatus> = OnceLock::new();

/// Arguments to `oxen-harness update`.
#[derive(Debug, clap::Args)]
pub(crate) struct UpdateArgs {
    /// Only report the installed and latest versions; install nothing.
    #[arg(long)]
    check: bool,
}

fn cache_file() -> Option<std::path::PathBuf> {
    harness_config::paths::base_dir()
        .ok()
        .map(|dir| harness_update::cache_path(&dir, Component::Cli))
}

/// Start the launch-time check in the background. Its answer is recorded
/// for [`known`]; a failure (offline, rate-limited) only means the banner
/// shows the version without an update hint, so it is logged, not shown.
pub(crate) fn spawn_startup_check() -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let cache = cache_file();
        match Updater::default()
            .check(
                Component::Cli,
                VERSION,
                cache.as_deref(),
                harness_update::CHECK_MAX_AGE,
            )
            .await
        {
            Ok(status) => {
                let _ = KNOWN.set(status);
            }
            Err(e) => tracing::debug!("update check failed: {e}"),
        }
    })
}

/// What the startup check found, if it has finished.
pub(crate) fn known() -> Option<&'static UpdateStatus> {
    KNOWN.get()
}

/// The banner's Version row value: `v0.0.7 (latest)`, or the update and how
/// to get it.
pub(crate) fn banner_value(ui: &Ui, status: Option<&UpdateStatus>) -> String {
    let current = ui.cream(&format!("v{VERSION}"));
    match status {
        Some(s) if s.available => {
            let latest = s.latest.as_deref().unwrap_or("?");
            format!(
                "{current} {} {}",
                ui.accent(&format!("→ v{latest} available")),
                ui.dim("· run /update")
            )
        }
        Some(s) if s.latest.is_some() => format!("{current} {}", ui.dim("(latest)")),
        _ => current,
    }
}

/// `oxen-harness update [--check]`.
pub(crate) async fn run_cli(args: UpdateArgs, ui: &Ui) -> Result<()> {
    let Some(release) = newest_or_report(ui).await? else {
        return Ok(());
    };
    if args.check {
        println!("  {}", ui.dim("Run `oxen-harness update` to install it."));
        return Ok(());
    }
    install(ui, &release).await?;
    println!(
        "  {}",
        ui.dim("The next oxen-harness you start runs the new version.")
    );
    Ok(())
}

/// `/update`: install, then restart into the new binary on this session.
/// Returns `Ok(true)` when the REPL should exit for the restart.
pub(crate) async fn handle_repl(ui: &Ui, ctx: &ReplContext<'_>) -> Result<bool> {
    let Some(release) = newest_or_report(ui).await? else {
        return Ok(false);
    };
    install(ui, &release).await?;
    println!("  {}", ui.dim("Restarting on this trail…"));
    let _ = RELAUNCH.set(vec!["--resume".into(), ctx.session().into()]);
    Ok(true)
}

/// Print the installed and latest versions; the newest release when it is an
/// update, `None` when there is nothing to install.
async fn newest_or_report(ui: &Ui) -> Result<Option<Release>> {
    let current = Version::parse(VERSION).context("this build has no release version")?;
    println!(
        "  {} v{current}",
        ui.brown(&format!("{:>9} :", "Installed"))
    );
    let latest = Updater::default()
        .latest(Component::Cli)
        .await
        .context("checking GitHub for a newer release")?;
    let Some(release) = latest else {
        println!("  {}", ui.dim("No CLI releases are published yet."));
        return Ok(None);
    };
    println!(
        "  {} v{}  {}",
        ui.brown(&format!("{:>9} :", "Latest")),
        release.version,
        ui.dim(&release.page_url)
    );
    if release.version <= current {
        println!("  {}", ui.green("You're on the latest version."));
        return Ok(None);
    }
    if Component::Cli.asset_name(&release.version).is_none() {
        println!(
            "  {}",
            ui.dim(&format!(
                "There's no in-place update for this platform: download it from {}",
                release.page_url
            ))
        );
        return Ok(None);
    }
    Ok(Some(release))
}

async fn install(ui: &Ui, release: &Release) -> Result<()> {
    let exe = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .context("finding the running oxen-harness binary")?;
    println!(
        "  {}",
        ui.dim(&format!("Downloading v{}…", release.version))
    );
    let archive = Updater::default()
        .download(Component::Cli, release)
        .await
        .with_context(|| format!("downloading {}", release.tag))?;
    let target = exe.clone();
    tokio::task::spawn_blocking(move || harness_update::install_cli(&archive, &target))
        .await
        .context("the install did not finish")?
        .map_err(|e| permission_hint(e, &exe))?;
    println!(
        "  {}",
        ui.green(&format!(
            "✓ Updated to v{} ({})",
            release.version,
            exe.display()
        ))
    );
    Ok(())
}

/// A binary in a root-owned folder (`/usr/local/bin`) can't be replaced by
/// the user: say how to do it rather than just "permission denied".
fn permission_hint(e: UpdateError, exe: &std::path::Path) -> anyhow::Error {
    match &e {
        UpdateError::Io { source, .. } if source.kind() == std::io::ErrorKind::PermissionDenied => {
            anyhow::anyhow!(
                "{e}\n  {} is not writable by you: run `sudo oxen-harness update`",
                exe.display()
            )
        }
        _ => e.into(),
    }
}

/// Arguments to restart with after `/update`, set just before the REPL exits.
static RELAUNCH: OnceLock<Vec<OsString>> = OnceLock::new();

/// After the REPL has torn the terminal down: if `/update` asked for it,
/// replace this process with the freshly installed binary.
pub(crate) fn relaunch_if_requested() -> Result<()> {
    let Some(args) = RELAUNCH.get() else {
        return Ok(());
    };
    let exe = std::env::current_exe().context("finding the updated oxen-harness binary")?;
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // `exec` only returns on failure.
        let err = std::process::Command::new(&exe).args(args).exec();
        Err(err).with_context(|| format!("restarting {}", exe.display()))
    }
    #[cfg(not(unix))]
    {
        std::process::Command::new(&exe)
            .args(args)
            .spawn()
            .with_context(|| format!("restarting {}", exe.display()))?;
        Ok(())
    }
}

/// How long the banner waits for a check that hasn't landed yet. A cached
/// answer is back in microseconds; this only bounds a cold network check.
pub(crate) const BANNER_WAIT: Duration = Duration::from_millis(300);

#[cfg(test)]
mod tests {
    use super::*;

    fn status(latest: Option<&str>, available: bool) -> UpdateStatus {
        UpdateStatus {
            current: VERSION.into(),
            latest: latest.map(Into::into),
            available,
            release_url: None,
            notes: None,
            published_at: None,
            installable: true,
            checked_at: 0,
        }
    }

    #[test]
    fn the_banner_says_the_version_and_how_to_update() {
        let ui = Ui::plain();
        assert_eq!(banner_value(&ui, None), format!("v{VERSION}"));
        assert_eq!(
            banner_value(&ui, Some(&status(Some(VERSION), false))),
            format!("v{VERSION} (latest)")
        );
        assert_eq!(
            banner_value(&ui, Some(&status(Some("9.9.9"), true))),
            format!("v{VERSION} → v9.9.9 available · run /update")
        );
    }
}
