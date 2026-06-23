//! Launching and supervising a local `llama-server` process.
//!
//! `llama-server` (from llama.cpp) serves an OpenAI-compatible API, so once it
//! is running the rest of the harness talks to it exactly like any other
//! endpoint — just pointed at `http://127.0.0.1:<port>/v1` with a throwaway key.
//! [`LocalServer`] picks a free port, starts the process against a GGUF file,
//! waits for the model to load (polling `/health`), and kills the process when
//! dropped so a session never leaks a background server.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, Command};

use crate::LocalError;

/// Environment variable pointing directly at a `llama-server` binary.
pub const LLAMA_SERVER_ENV: &str = "LLAMA_SERVER";

/// Default context window passed to `llama-server` (keeps KV-cache memory
/// reasonable; the model can be configured for more).
const DEFAULT_CONTEXT: u32 = 8192;
/// How long to wait for the model to load and the server to report healthy.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(180);

fn exe_name() -> &'static str {
    if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}

/// Locate the `llama-server` binary: the `LLAMA_SERVER` override, then `PATH`,
/// then common install locations (e.g. Homebrew) that aren't always on the PATH
/// of an app launched from the GUI rather than a shell.
pub fn llama_server_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(LLAMA_SERVER_ENV) {
        let path = PathBuf::from(p);
        if path.is_file() {
            return Some(path);
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        if let Some(found) = find_on_path(&path, exe_name()) {
            return Some(found);
        }
    }
    common_bin_dirs()
        .into_iter()
        .map(|dir| dir.join(exe_name()))
        .find(|candidate| candidate.is_file())
}

fn find_on_path(path: &OsStr, exe: &str) -> Option<PathBuf> {
    std::env::split_paths(path)
        .map(|dir| dir.join(exe))
        .find(|candidate| candidate.is_file())
}

/// Well-known package-manager `bin` directories that may hold `brew`,
/// `llama-server`, etc. but are frequently missing from a GUI app's `PATH`.
fn common_bin_dirs() -> Vec<PathBuf> {
    if cfg!(target_os = "macos") {
        vec!["/opt/homebrew/bin".into(), "/usr/local/bin".into()]
    } else if cfg!(target_os = "linux") {
        vec![
            "/home/linuxbrew/.linuxbrew/bin".into(),
            "/usr/local/bin".into(),
        ]
    } else {
        Vec::new()
    }
}

/// Platform-specific guidance for installing `llama-server`.
pub fn install_hint() -> String {
    let how = if cfg!(target_os = "macos") {
        "Install it with `brew install llama.cpp`"
    } else if cfg!(target_os = "linux") {
        "Install llama.cpp (e.g. your package manager) or download a release \
         from https://github.com/ggml-org/llama.cpp/releases"
    } else {
        "Download a release from https://github.com/ggml-org/llama.cpp/releases"
    };
    format!("{how}, or set {LLAMA_SERVER_ENV}=/path/to/llama-server.")
}

/// Locate the Homebrew `brew` binary on `PATH` or in its usual locations.
fn find_brew() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PATH") {
        if let Some(found) = find_on_path(&path, "brew") {
            return Some(found);
        }
    }
    common_bin_dirs()
        .into_iter()
        .map(|dir| dir.join("brew"))
        .find(|candidate| candidate.is_file())
}

/// The command that would install `llama-server` on this machine, if known.
/// Currently this means Homebrew (macOS / Linuxbrew); other platforms point the
/// user at [`install_hint`] instead.
fn install_command() -> Option<(PathBuf, Vec<String>)> {
    let brew = find_brew()?;
    Some((brew, vec!["install".into(), "llama.cpp".into()]))
}

/// Whether the app can install `llama-server` for the user automatically.
pub fn can_auto_install() -> bool {
    install_command().is_some()
}

/// Install `llama-server` via the detected package manager, forwarding each line
/// of output to `on_line` so the UI can show live progress. On success returns
/// the path to the freshly installed binary.
pub async fn install_llama_server<F>(mut on_line: F) -> Result<PathBuf, LocalError>
where
    F: FnMut(&str),
{
    let (program, args) = install_command().ok_or_else(|| {
        LocalError::Install(format!(
            "automatic install isn't supported here. {}",
            install_hint()
        ))
    })?;

    on_line(&format!("$ {} {}", program.display(), args.join(" ")));

    let mut child = Command::new(&program)
        .args(&args)
        // Keep Homebrew non-interactive and snappy.
        .env("HOMEBREW_NO_AUTO_UPDATE", "1")
        .env("NONINTERACTIVE", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            LocalError::Install(format!("could not start `{}`: {e}", program.display()))
        })?;

    // Merge stdout + stderr into one ordered-ish line stream for the callback.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    if let Some(out) = child.stdout.take() {
        spawn_line_reader(out, tx.clone());
    }
    if let Some(err) = child.stderr.take() {
        spawn_line_reader(err, tx.clone());
    }
    drop(tx);
    while let Some(line) = rx.recv().await {
        on_line(&line);
    }

    let status = child.wait().await.map_err(LocalError::Io)?;
    if !status.success() {
        return Err(LocalError::Install(format!(
            "`{} {}` exited with {status}",
            program.display(),
            args.join(" ")
        )));
    }

    llama_server_path().ok_or_else(|| {
        LocalError::Install(
            "install finished but `llama-server` still wasn't found — \
             you may need to restart the app or set LLAMA_SERVER."
                .to_string(),
        )
    })
}

/// Forward each line read from `reader` into `tx` until EOF or the channel drops.
fn spawn_line_reader<R>(reader: R, tx: tokio::sync::mpsc::UnboundedSender<String>)
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
}

/// A running `llama-server` instance bound to one model.
pub struct LocalServer {
    child: Child,
    base_url: String,
    port: u16,
}

impl LocalServer {
    /// Start `llama-server` for `model_path`, serving it under `alias`, and wait
    /// until it reports healthy (the model is loaded).
    pub async fn start(model_path: &Path, alias: &str) -> Result<Self, LocalError> {
        let binary =
            llama_server_path().ok_or_else(|| LocalError::LlamaServerMissing(install_hint()))?;
        let port = find_free_port()?;

        let child = Command::new(&binary)
            .arg("-m")
            .arg(model_path)
            .args(["--host", "127.0.0.1"])
            .args(["--port", &port.to_string()])
            .args(["-a", alias])
            .args(["-c", &DEFAULT_CONTEXT.to_string()])
            // Offload to GPU when the build supports it (ignored on CPU-only).
            .args(["-ngl", "99"])
            // Enable the model's chat template so tool calling works.
            .arg("--jinja")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| LocalError::Server(format!("spawning {}: {e}", binary.display())))?;

        let mut server = Self {
            child,
            base_url: format!("http://127.0.0.1:{port}/v1"),
            port,
        };
        server.await_healthy().await?;
        Ok(server)
    }

    /// The OpenAI-compatible base URL to point an LLM client at.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The port the server is listening on.
    pub fn port(&self) -> u16 {
        self.port
    }

    async fn await_healthy(&mut self) -> Result<(), LocalError> {
        let health = format!("http://127.0.0.1:{}/health", self.port);
        let client = reqwest::Client::new();
        let deadline = tokio::time::Instant::now() + HEALTH_TIMEOUT;

        loop {
            // If the process already exited, surface that immediately.
            if let Ok(Some(status)) = self.child.try_wait() {
                return Err(LocalError::Server(format!(
                    "llama-server exited early ({status})"
                )));
            }
            if let Ok(resp) = client.get(&health).send().await {
                if resp.status().is_success() {
                    return Ok(());
                }
            }
            if tokio::time::Instant::now() >= deadline {
                let _ = self.child.start_kill();
                return Err(LocalError::Server(
                    "timed out waiting for the model to load".to_string(),
                ));
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }
}

impl Drop for LocalServer {
    fn drop(&mut self) {
        // Best-effort: stop the background server when the session ends.
        let _ = self.child.start_kill();
    }
}

fn find_free_port() -> Result<u16, LocalError> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn install_hint_is_actionable() {
        let hint = install_hint();
        assert!(hint.contains(LLAMA_SERVER_ENV));
        if cfg!(target_os = "macos") {
            assert!(hint.contains("brew install llama.cpp"));
        }
    }

    #[test]
    fn install_command_is_brew_when_present() {
        // We can't guarantee brew exists in CI, but when it does the command
        // must be `brew install llama.cpp`, and `can_auto_install` must agree.
        assert_eq!(can_auto_install(), install_command().is_some());
        if let Some((program, args)) = install_command() {
            assert_eq!(program.file_name().unwrap(), "brew");
            assert_eq!(args, vec!["install".to_string(), "llama.cpp".to_string()]);
        }
    }

    #[test]
    fn common_bin_dirs_match_platform() {
        let dirs = common_bin_dirs();
        if cfg!(target_os = "macos") {
            assert!(dirs.iter().any(|d| d.ends_with("homebrew/bin")));
        } else if cfg!(target_os = "linux") {
            assert!(dirs
                .iter()
                .any(|d| d.to_string_lossy().contains("linuxbrew")));
        }
    }

    #[test]
    fn finds_binary_on_a_constructed_path() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join(exe_name());
        std::fs::write(&exe, b"#!/bin/sh\n").unwrap();
        let path_var = OsString::from(dir.path());
        assert_eq!(find_on_path(&path_var, exe_name()), Some(exe));

        let empty = tempfile::tempdir().unwrap();
        assert_eq!(
            find_on_path(&OsString::from(empty.path()), exe_name()),
            None
        );
    }

    #[test]
    fn free_port_is_nonzero() {
        assert!(find_free_port().unwrap() > 0);
    }
}
