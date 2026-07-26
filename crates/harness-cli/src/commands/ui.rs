//! `oxen-harness ui [dir]` — open the desktop app on a project directory.
//!
//! The directory travels as a plain positional argument. If the app is
//! already running, its single-instance guard forwards the argument to the
//! live window (which focuses and enters the project); otherwise the app
//! boots straight into it. `OXEN_HARNESS_APP` points the launcher at a
//! non-standard binary or `.app` bundle (a dev build, a custom install).

use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

use crate::theme::Ui;

/// Bundle identifier of the released macOS app (tauri.conf.json `identifier`);
/// `open -b` finds the .app wherever it is installed.
const MACOS_BUNDLE_ID: &str = "ai.oxen.harness";
/// Binary name of the released desktop app (tauri.conf.json `mainBinaryName`
/// — deliberately not `oxen-harness`, which is this CLI).
const APP_BINARY: &str = "oxen-harness-app";

pub(crate) fn run_ui(path: Option<PathBuf>, ui: &Ui) -> Result<()> {
    let dir = path.unwrap_or_else(|| PathBuf::from("."));
    let dir = dir
        .canonicalize()
        .with_context(|| format!("no such directory: {}", dir.display()))?;
    if !dir.is_dir() {
        bail!("not a directory: {}", dir.display());
    }
    let dir = dir.display().to_string();

    let override_app = std::env::var("OXEN_HARNESS_APP")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(installed_windows_app);
    let (program, args) = launch_plan(&dir, override_app.as_deref(), std::env::consts::OS);

    if program == "open" {
        // `open` is a short-lived launcher, not the app: wait for it so a
        // missing bundle ("app not installed") surfaces as a real error.
        let output = Command::new(&program)
            .args(&args)
            .output()
            .context("could not run `open`")?;
        if !output.status.success() {
            bail!(
                "could not open the desktop app: {}\n{}",
                String::from_utf8_lossy(&output.stderr).trim(),
                install_hint()
            );
        }
    } else {
        let mut cmd = Command::new(&program);
        cmd.args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // The app must survive this terminal closing.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        cmd.spawn()
            .with_context(|| format!("could not launch `{program}`\n{}", install_hint()))?;
    }

    println!(
        "{} {}",
        ui.green("✓ opening the desktop app in"),
        ui.cream(&dir)
    );
    Ok(())
}

/// What to exec: the program and its arguments, given the (canonical) project
/// directory, an optional override (env var or detected Windows install), and
/// the platform. Pure so the per-platform shapes are testable anywhere.
fn launch_plan(dir: &str, override_app: Option<&str>, os: &str) -> (String, Vec<String>) {
    if let Some(app) = override_app {
        // A .app bundle can't be exec'd directly — route it through `open`.
        // `-n` forces a fresh process so the argv is always delivered; the
        // running instance receives it via the single-instance guard.
        if os == "macos" && app.ends_with(".app") {
            return (
                "open".into(),
                vec![
                    "-n".into(),
                    "-a".into(),
                    app.into(),
                    "--args".into(),
                    dir.into(),
                ],
            );
        }
        return (app.into(), vec![dir.into()]);
    }
    match os {
        "macos" => (
            "open".into(),
            vec![
                "-n".into(),
                "-b".into(),
                MACOS_BUNDLE_ID.into(),
                "--args".into(),
                dir.into(),
            ],
        ),
        // Linux: the .deb/.rpm install the binary on PATH. Windows reaches
        // here only when the default install location probe found nothing —
        // PATH is still worth a try.
        _ => (APP_BINARY.into(), vec![dir.into()]),
    }
}

/// The released Windows installer (NSIS, per-user) puts the app under
/// %LOCALAPPDATA% without touching PATH — probe the default spot.
#[cfg(windows)]
fn installed_windows_app() -> Option<String> {
    let local = std::env::var("LOCALAPPDATA").ok()?;
    let exe = PathBuf::from(local)
        .join("oxen-harness")
        .join(format!("{APP_BINARY}.exe"));
    exe.is_file().then(|| exe.display().to_string())
}

#[cfg(not(windows))]
fn installed_windows_app() -> Option<String> {
    None
}

fn install_hint() -> String {
    "is the desktop app installed? For a dev build or custom location, set \
     OXEN_HARNESS_APP to the app binary (or .app bundle) and retry."
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_launches_by_bundle_id_with_a_fresh_process() {
        let (program, args) = launch_plan("/work/demo", None, "macos");
        assert_eq!(program, "open");
        assert_eq!(args, ["-n", "-b", MACOS_BUNDLE_ID, "--args", "/work/demo"]);
    }

    #[test]
    fn linux_execs_the_app_binary_from_path() {
        let (program, args) = launch_plan("/work/demo", None, "linux");
        assert_eq!(program, APP_BINARY);
        assert_eq!(args, ["/work/demo"]);
    }

    #[test]
    fn an_override_binary_is_exec_d_directly() {
        let (program, args) = launch_plan("/work/demo", Some("/opt/dev/oxen-harness-app"), "linux");
        assert_eq!(program, "/opt/dev/oxen-harness-app");
        assert_eq!(args, ["/work/demo"]);
    }

    #[test]
    fn an_override_app_bundle_goes_through_open_on_macos() {
        let (program, args) = launch_plan(
            "/work/demo",
            Some("/Applications/oxen-harness.app"),
            "macos",
        );
        assert_eq!(program, "open");
        assert_eq!(
            args,
            [
                "-n",
                "-a",
                "/Applications/oxen-harness.app",
                "--args",
                "/work/demo"
            ]
        );
    }
}
