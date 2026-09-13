//! Launching and supervising a local `llama-server` process.
//!
//! `llama-server` (from llama.cpp) serves an OpenAI-compatible API, so once it
//! is running the rest of the harness talks to it exactly like any other
//! endpoint — just pointed at `http://127.0.0.1:<port>/v1` with a throwaway key.
//! [`LocalServer`] picks a free port, starts the process against a GGUF file,
//! waits for the model to load (polling `/health`), and kills the process when
//! dropped so a session never leaks a background server.
//!
//! Dropping only helps when the host exits cleanly. A host that is killed
//! (`kill`, a crash, a force-quit) never runs destructors, and the server —
//! holding a model's worth of memory — is reparented to init and lives on.
//! So every spawn is also recorded in a small registry file
//! (`~/.oxen-harness/runtime/llama-server.pids`) tagged with the owning host's
//! pid, and [`reap_stale_servers`] kills any entry whose owner is gone. Hosts
//! call it at boot and before every new spawn.

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
pub const DEFAULT_CONTEXT: u32 = 8192;
/// How long to wait for the model to load and the server to report healthy.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(180);

fn exe_name() -> &'static str {
    if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}

/// Locate the `llama-server` binary, in precedence order: the `LLAMA_SERVER`
/// override (power users with their own build), then the runtime we manage
/// ourselves, then `PATH` / common install locations (e.g. Homebrew) — which
/// aren't always on the PATH of an app launched from the GUI rather than a shell.
pub fn llama_server_path() -> Option<PathBuf> {
    env_override()
        .or_else(crate::runtime::managed_binary_path)
        .or_else(path_llama_server)
}

/// The explicit `LLAMA_SERVER` override, if it points at a real file.
fn env_override() -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var_os(LLAMA_SERVER_ENV)?);
    path.is_file().then_some(path)
}

/// `llama-server` discovered on `PATH` or in the common package-manager dirs.
fn path_llama_server() -> Option<PathBuf> {
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

/// Coarse phases of bringing a local model online, for progress UI. The first
/// phase ([`LoadPhase::Starting`]) is where a cold first run spends several
/// seconds compiling GPU shaders; [`LoadPhase::LoadingModel`] then scales with
/// the model's size as its weights are read into memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadPhase {
    /// Process spawned; the runtime/GPU backend is initializing.
    Starting,
    /// The model weights are being read into memory.
    LoadingModel,
    /// Healthy and serving requests.
    Ready,
}

/// A running `llama-server` instance bound to one model.
pub struct LocalServer {
    child: Child,
    /// The child's pid, kept so `Drop` can remove its registry entry even
    /// after the process has exited and `Child::id` has gone `None`.
    pid: u32,
    base_url: String,
    port: u16,
    context: u32,
    /// The model id/alias this server is serving (`-a`), so callers can tell
    /// whether a running server is the one they need or a leftover.
    model: String,
}

impl LocalServer {
    /// Start `llama-server` for `model_path` with the default context window.
    pub async fn start(model_path: &Path, alias: &str) -> Result<Self, LocalError> {
        Self::start_with_context(model_path, alias, DEFAULT_CONTEXT, |_| {}).await
    }

    /// Start `llama-server` for `model_path`, serving it under `alias` with a
    /// `context`-token window (sized to the machine's memory by the caller), and
    /// wait until it reports healthy (the model is loaded). `on_status` receives
    /// [`LoadPhase`] updates so a UI can show what the startup is doing.
    pub async fn start_with_context(
        model_path: &Path,
        alias: &str,
        context: u32,
        mut on_status: impl FnMut(LoadPhase),
    ) -> Result<Self, LocalError> {
        let binary =
            llama_server_path().ok_or_else(|| LocalError::LlamaServerMissing(install_hint()))?;
        let port = find_free_port()?;
        let context = context.max(512);

        // Free the memory of any server a dead host left behind before
        // committing this model's worth on top of it.
        let _ = reap_stale_servers();

        on_status(LoadPhase::Starting);
        let mut child = Command::new(&binary)
            .arg("-m")
            .arg(model_path)
            .args(["--host", "127.0.0.1"])
            .args(["--port", &port.to_string()])
            .args(["-a", alias])
            .args(["-c", &context.to_string()])
            // Offload to GPU when the build supports it (ignored on CPU-only).
            .args(["-ngl", "99"])
            // Enable the model's chat template so tool calling works.
            .arg("--jinja")
            .stdout(std::process::Stdio::null())
            // Capture stderr so we can tell when the runtime finishes initializing
            // and the model itself starts loading (for accurate progress phases).
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| LocalError::Server(format!("spawning {}: {e}", binary.display())))?;

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        if let Some(err) = child.stderr.take() {
            spawn_line_reader(err, tx);
        }
        // Register before waiting on health: a host killed mid-load must
        // still leave a trail to the half-loaded server.
        let pid = child.id().unwrap_or(0);
        registry::register(pid, port);

        let mut server = Self {
            child,
            pid,
            base_url: format!("http://127.0.0.1:{port}/v1"),
            port,
            context,
            model: alias.to_string(),
        };
        server.await_healthy(rx, &mut on_status).await?;
        on_status(LoadPhase::Ready);
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

    /// The context window (in tokens) the server was started with. The agent
    /// uses this to budget prompts, since the local window is typically far
    /// smaller than the model's theoretical maximum.
    pub fn context_size(&self) -> u32 {
        self.context
    }

    /// The model id/alias this server is serving.
    pub fn model_id(&self) -> &str {
        &self.model
    }

    /// Whether the server process is still running. A crashed or killed
    /// `llama-server` (e.g. reclaimed under memory pressure) reports `false`,
    /// so callers can start a fresh one instead of reusing a dead endpoint.
    pub fn is_alive(&mut self) -> bool {
        // `Ok(Some(_))` = exited; `Ok(None)` = running; `Err` = can't tell,
        // treat as alive rather than churning a healthy server.
        !matches!(self.child.try_wait(), Ok(Some(_)))
    }

    async fn await_healthy(
        &mut self,
        mut lines: tokio::sync::mpsc::UnboundedReceiver<String>,
        on_status: &mut impl FnMut(LoadPhase),
    ) -> Result<(), LocalError> {
        let health = format!("http://127.0.0.1:{}/health", self.port);
        let client = reqwest::Client::new();
        let deadline = tokio::time::Instant::now() + HEALTH_TIMEOUT;
        let mut announced_loading = false;

        loop {
            // Drain stderr: the "loading model" line marks the end of runtime/GPU
            // init (the slow cold-start phase) and the start of reading weights.
            while let Ok(line) = lines.try_recv() {
                if !announced_loading && line.to_ascii_lowercase().contains("loading model") {
                    announced_loading = true;
                    on_status(LoadPhase::LoadingModel);
                }
            }
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
        // Best-effort: stop the background server when the session ends, and
        // strike it from the registry so a later boot doesn't chase its pid.
        let _ = self.child.start_kill();
        registry::unregister(self.pid);
    }
}

/// Kill `llama-server` processes started by a harness host that has since
/// died without cleaning up (see the module docs). Returns the pids killed.
/// Servers whose owning host is still running — this process, or another
/// host sharing the machine — are left alone. Never fails: a missing or
/// unreadable registry just means there is nothing to reap.
pub fn reap_stale_servers() -> Vec<u32> {
    let Some(path) = registry::path() else {
        return Vec::new();
    };
    registry::reap_at(&path, std::process::id(), process::is_alive, |pid| {
        process::command_name(pid)
            .map(|name| name.contains("llama-server"))
            .unwrap_or(false)
    })
}

/// The on-disk registry of spawned servers: one `<pid> <owner pid> <port>`
/// line each. Every access is best-effort — the registry is a safety net,
/// never a reason a model fails to start.
mod registry {
    use std::io::Write;
    use std::path::{Path, PathBuf};

    pub(super) fn path() -> Option<PathBuf> {
        Some(
            harness_config::paths::base_dir_unchecked()?
                .join("runtime")
                .join("llama-server.pids"),
        )
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) struct Entry {
        pub pid: u32,
        pub owner: u32,
        pub port: u16,
    }

    pub(super) fn parse(contents: &str) -> Vec<Entry> {
        contents
            .lines()
            .filter_map(|line| {
                let mut parts = line.split_whitespace();
                Some(Entry {
                    pid: parts.next()?.parse().ok()?,
                    owner: parts.next()?.parse().ok()?,
                    port: parts.next()?.parse().ok()?,
                })
            })
            .collect()
    }

    fn read(path: &Path) -> Vec<Entry> {
        std::fs::read_to_string(path)
            .map(|s| parse(&s))
            .unwrap_or_default()
    }

    fn write(path: &Path, entries: &[Entry]) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let body: String = entries
            .iter()
            .map(|e| format!("{} {} {}\n", e.pid, e.owner, e.port))
            .collect();
        let _ = std::fs::write(path, body);
    }

    pub(super) fn register(pid: u32, port: u16) {
        let Some(path) = path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = writeln!(f, "{pid} {} {port}", std::process::id());
        }
    }

    pub(super) fn unregister(pid: u32) {
        let Some(path) = path() else { return };
        let entries = read(&path);
        let kept: Vec<Entry> = entries.iter().copied().filter(|e| e.pid != pid).collect();
        if kept.len() != entries.len() {
            write(&path, &kept);
        }
    }

    /// The reaping rule, with the process probes injected so it can be
    /// tested without spawning real servers. An entry is stale when its
    /// owner is neither `self_pid` nor alive; it is killed only if the pid is
    /// alive *and* still a llama-server (pids get reused). Stale entries are
    /// dropped from the file either way; live ones are kept.
    pub(super) fn reap_at(
        path: &Path,
        self_pid: u32,
        is_alive: impl Fn(u32) -> bool,
        is_server: impl Fn(u32) -> bool,
    ) -> Vec<u32> {
        let entries = read(path);
        if entries.is_empty() {
            return Vec::new();
        }
        let mut killed = Vec::new();
        let mut kept = Vec::new();
        for e in entries {
            let owned = e.owner == self_pid || is_alive(e.owner);
            if owned {
                kept.push(e);
                continue;
            }
            if is_alive(e.pid) && is_server(e.pid) {
                super::process::terminate(e.pid);
                killed.push(e.pid);
            }
        }
        write(path, &kept);
        killed
    }
}

/// Minimal process probes for the reaper, via the platform `kill`/`ps`
/// commands so no unsafe FFI is needed. Unix only; elsewhere every process
/// reads as alive and unknown, so nothing is ever killed.
mod process {
    #[cfg(unix)]
    fn kill(pid: u32, signal: &str) -> Option<bool> {
        std::process::Command::new("kill")
            .args([signal, &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .ok()
            .map(|s| s.success())
    }

    /// `kill -0`: succeeds while the process exists (or exists but belongs to
    /// someone else — the shell reports that as failure, which errs toward
    /// leaving other users' processes alone).
    #[cfg(unix)]
    pub(super) fn is_alive(pid: u32) -> bool {
        pid != 0 && kill(pid, "-0").unwrap_or(false)
    }

    #[cfg(not(unix))]
    pub(super) fn is_alive(_pid: u32) -> bool {
        true
    }

    /// The process's executable name (`ps -o comm=`), if it can be read.
    pub(super) fn command_name(pid: u32) -> Option<String> {
        let out = std::process::Command::new("ps")
            .args(["-o", "comm=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!name.is_empty()).then_some(name)
    }

    /// Plain SIGTERM to a pid the caller has just verified is a llama-server.
    #[cfg(unix)]
    pub(super) fn terminate(pid: u32) {
        let _ = kill(pid, "-TERM");
    }

    #[cfg(not(unix))]
    pub(super) fn terminate(_pid: u32) {}
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

    /// A LocalServer wrapped around an arbitrary child process, bypassing the
    /// spawn/health-check path, to test the liveness/identity accessors.
    fn fake_server(child: Child, model: &str) -> LocalServer {
        LocalServer {
            // 0 never matches a registry entry, so dropping a fake server
            // can't touch the real registry file.
            pid: 0,
            child,
            base_url: "http://127.0.0.1:1/v1".to_string(),
            port: 1,
            context: 512,
            model: model.to_string(),
        }
    }

    #[tokio::test]
    async fn is_alive_tracks_the_process_and_model_id_is_kept() {
        let child = Command::new("sleep")
            .arg("30")
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut server = fake_server(child, "muse-glimmer-30b-gguf-q4-k-m");
        assert_eq!(server.model_id(), "muse-glimmer-30b-gguf-q4-k-m");
        assert!(server.is_alive());

        // Kill it (as the OS would under memory pressure) and wait for the
        // exit to be reaped; a dead server must report so.
        server.child.start_kill().unwrap();
        let _ = server.child.wait().await;
        assert!(!server.is_alive());
    }

    #[test]
    fn registry_parses_and_skips_garbage_lines() {
        let entries = registry::parse("101 7 5000\nnot a line\n202 7\n303 9 6000 extra\n");
        // The short line is dropped; the long one parses its leading fields.
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].pid, 101);
        assert_eq!(entries[0].owner, 7);
        assert_eq!(entries[0].port, 5000);
        assert_eq!(entries[1].pid, 303);
    }

    /// A pid whose owner is dead is killed and dropped; one owned by us or by
    /// a live host is kept untouched; a stale entry whose pid was reused by
    /// some other program is dropped but not killed.
    #[test]
    fn reap_kills_only_orphaned_llama_servers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("llama-server.pids");
        std::fs::write(
            &path,
            "100 1000 5000\n\
             200 9999 5001\n\
             300 2000 5002\n\
             400 9998 5003\n\
             500 9997 5004\n",
        )
        .unwrap();
        let alive = |pid: u32| matches!(pid, 100 | 200 | 300 | 400 | 2000);
        // 500's pid is dead already; 400's pid now belongs to something else.
        let is_server = |pid: u32| matches!(pid, 100 | 200 | 300);
        let killed = registry::reap_at(&path, 1000, alive, is_server);
        assert_eq!(killed, vec![200]);
        let left = registry::parse(&std::fs::read_to_string(&path).unwrap());
        let pids: Vec<u32> = left.iter().map(|e| e.pid).collect();
        assert_eq!(pids, vec![100, 300], "own + live-owner entries survive");
    }

    #[test]
    fn reap_with_no_registry_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.pids");
        let killed = registry::reap_at(&path, 1, |_| true, |_| true);
        assert!(killed.is_empty());
        assert!(!path.exists(), "an empty reap must not create the file");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reap_terminates_a_real_orphan_and_spares_a_live_owner() {
        // Two throwaway processes stand in for servers; the probes treat both
        // as llama-servers. One is owned by a pid that certainly exited
        // (a finished `true`), the other by this test process.
        let mut done = std::process::Command::new("true").spawn().unwrap();
        let dead_owner_pid = done.id();
        done.wait().unwrap();

        let mut orphan = Command::new("sleep").arg("30").spawn().unwrap();
        let mut owned = Command::new("sleep")
            .arg("30")
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("llama-server.pids");
        std::fs::write(
            &path,
            format!(
                "{} {} 5000\n{} {} 5001\n",
                orphan.id().unwrap(),
                dead_owner_pid,
                owned.id().unwrap(),
                std::process::id()
            ),
        )
        .unwrap();

        let killed = registry::reap_at(&path, std::process::id(), process::is_alive, |_| true);
        assert_eq!(killed, vec![orphan.id().unwrap()]);
        // SIGTERM lands: the orphan exits, the owned one keeps running.
        let status = tokio::time::timeout(Duration::from_secs(5), orphan.wait())
            .await
            .expect("orphan should exit after SIGTERM")
            .unwrap();
        assert!(!status.success());
        assert!(matches!(owned.try_wait(), Ok(None)));
    }
}
