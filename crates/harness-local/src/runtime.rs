//! A self-managed llama.cpp runtime.
//!
//! Rather than asking the user to install Homebrew + llama.cpp, the app downloads
//! a *pinned* prebuilt `llama-server` for this OS/arch into
//! `~/.oxen-harness/runtime/` and runs it directly. macOS (Apple Silicon) is
//! wired up today; other targets report `can_manage = false` and fall back to the
//! Homebrew/PATH discovery in [`crate::server`].
//!
//! Validated against release `b9835`: the macOS arm64 tarball unpacks to a single
//! `llama-{version}/` directory holding `llama-server` plus its `.dylib`s (Metal
//! included); the binary's `@loader_path` rpath loads them in place, so no PATH
//! or env setup is needed.

use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use serde::Serialize;

use crate::LocalError;

/// The llama.cpp release we pin to — a known-good build. Bump deliberately (and
/// surface an "update runtime" action) rather than chasing `latest`.
pub const PINNED_VERSION: &str = "b9835";

/// Where the managed runtime lives: `~/.oxen-harness/runtime/llama.cpp/`.
fn runtime_root() -> Option<PathBuf> {
    Some(
        dirs::home_dir()?
            .join(".oxen-harness")
            .join("runtime")
            .join("llama.cpp"),
    )
}

/// The directory a pinned version unpacks into (`.../llama.cpp/llama-{ver}`).
fn version_dir(version: &str) -> Option<PathBuf> {
    Some(runtime_root()?.join(format!("llama-{version}")))
}

fn exe_name() -> &'static str {
    if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}

/// The release asset filename for this OS/arch, or `None` if we can't manage a
/// runtime here yet. Add Windows/Linux variants alongside this match.
pub fn asset_name(version: &str) -> Option<String> {
    if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
        Some(format!("llama-{version}-bin-macos-arm64.tar.gz"))
    } else {
        None
    }
}

/// Whether the app can auto-download a managed runtime for this platform.
pub fn can_manage() -> bool {
    asset_name(PINNED_VERSION).is_some()
}

/// The GitHub download URL for a release asset.
pub fn download_url(version: &str, asset: &str) -> String {
    format!("https://github.com/ggml-org/llama.cpp/releases/download/{version}/{asset}")
}

/// Path to the managed `llama-server`, if the pinned version is present on disk.
pub fn managed_binary_path() -> Option<PathBuf> {
    let bin = version_dir(PINNED_VERSION)?.join(exe_name());
    bin.is_file().then_some(bin)
}

/// Where a usable `llama-server` is coming from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeSource {
    /// Downloaded and managed by us.
    Managed,
    /// Found on PATH / via Homebrew / via the `LLAMA_SERVER` override.
    System,
    /// No runtime available.
    None,
}

/// The runtime's status for the setup UI.
#[derive(Debug, Clone, Serialize)]
pub struct RuntimeStatus {
    /// Path to a usable `llama-server`, if any.
    pub binary: Option<String>,
    pub source: RuntimeSource,
    /// The version we'd manage / have managed.
    pub managed_version: String,
    /// Whether we can auto-download a runtime for this platform.
    pub can_manage: bool,
}

/// Resolve the runtime status: which `llama-server` (if any) will be used and
/// where it came from. Precedence (in [`crate::server::llama_server_path`]) is
/// `LLAMA_SERVER` env → managed → PATH/Homebrew.
pub fn status() -> RuntimeStatus {
    let binary = crate::server::llama_server_path();
    let managed = managed_binary_path();
    let source = match &binary {
        Some(b) if managed.as_deref() == Some(b.as_path()) => RuntimeSource::Managed,
        Some(_) => RuntimeSource::System,
        None => RuntimeSource::None,
    };
    RuntimeStatus {
        binary: binary.map(|p| p.display().to_string()),
        source,
        managed_version: PINNED_VERSION.to_string(),
        can_manage: can_manage(),
    }
}

/// Progress while installing the managed runtime.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum RuntimeInstallEvent {
    /// A human-readable status line.
    Log { line: String },
    /// Download progress in bytes (`total` is `None` if the server didn't say).
    Progress { downloaded: u64, total: Option<u64> },
}

/// Download, extract, and verify the pinned `llama-server` for this platform,
/// reporting progress via `on_event`. Returns the path to the ready binary.
/// Idempotent: a re-install simply re-extracts over the version directory.
pub async fn install<F>(mut on_event: F) -> Result<PathBuf, LocalError>
where
    F: FnMut(RuntimeInstallEvent),
{
    let version = PINNED_VERSION;
    let asset = asset_name(version).ok_or_else(|| {
        LocalError::Install(format!(
            "no prebuilt llama.cpp runtime for this platform yet. {}",
            crate::server::install_hint()
        ))
    })?;
    let root =
        runtime_root().ok_or_else(|| LocalError::Install("no home directory".to_string()))?;
    std::fs::create_dir_all(&root)?;

    let url = download_url(version, &asset);
    on_event(RuntimeInstallEvent::Log {
        line: format!("Downloading llama.cpp {version}…"),
    });
    let archive = root.join(format!(".{asset}.part"));
    download_to_file(&url, &archive, &mut on_event).await?;

    on_event(RuntimeInstallEvent::Log {
        line: "Extracting…".to_string(),
    });
    extract_tar_gz(&archive, &root)?;
    let _ = std::fs::remove_file(&archive);

    let bin = version_dir(version)
        .map(|d| d.join(exe_name()))
        .filter(|b| b.is_file())
        .ok_or_else(|| {
            LocalError::Install("the runtime archive did not contain llama-server".to_string())
        })?;

    #[cfg(unix)]
    ensure_executable(&bin)?;
    dequarantine(version_dir(version).as_deref());

    on_event(RuntimeInstallEvent::Log {
        line: "Verifying…".to_string(),
    });
    verify(&bin).await?;

    on_event(RuntimeInstallEvent::Log {
        line: "Local runtime ready.".to_string(),
    });
    Ok(bin)
}

/// Stream `url` to `dest`, emitting [`RuntimeInstallEvent::Progress`].
async fn download_to_file<F>(url: &str, dest: &Path, on_event: &mut F) -> Result<(), LocalError>
where
    F: FnMut(RuntimeInstallEvent),
{
    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| LocalError::Download(format!("request failed: {e}")))?;
    if !resp.status().is_success() {
        return Err(LocalError::Download(format!(
            "HTTP {} fetching {url}",
            resp.status().as_u16()
        )));
    }
    let total = resp.content_length();
    let mut file = tokio::fs::File::create(dest).await?;
    let mut downloaded: u64 = 0;
    on_event(RuntimeInstallEvent::Progress { downloaded, total });

    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| LocalError::Download(format!("stream error: {e}")))?;
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
        downloaded += chunk.len() as u64;
        on_event(RuntimeInstallEvent::Progress { downloaded, total });
    }
    tokio::io::AsyncWriteExt::flush(&mut file).await?;
    Ok(())
}

/// Extract a `.tar.gz` into `dest` (pure-Rust, no system `tar` needed).
fn extract_tar_gz(archive: &Path, dest: &Path) -> Result<(), LocalError> {
    let file = std::fs::File::open(archive)?;
    let gz = flate2::read::GzDecoder::new(file);
    let mut tar = tar::Archive::new(gz);
    tar.unpack(dest)
        .map_err(|e| LocalError::Install(format!("extract failed: {e}")))?;
    Ok(())
}

#[cfg(unix)]
fn ensure_executable(path: &Path) -> Result<(), LocalError> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(perms.mode() | 0o755);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}

/// Best-effort: clear the quarantine xattr so Gatekeeper doesn't block the
/// downloaded binary. (reqwest downloads don't set it, but be safe.)
fn dequarantine(_dir: Option<&Path>) {
    #[cfg(target_os = "macos")]
    if let Some(dir) = _dir {
        let _ = std::process::Command::new("xattr")
            .args(["-dr", "com.apple.quarantine"])
            .arg(dir)
            .status();
    }
}

/// Confirm the binary runs (`--version` exits 0), so a corrupt download surfaces
/// here rather than when the user tries to start a chat.
async fn verify(bin: &Path) -> Result<(), LocalError> {
    let out = tokio::process::Command::new(bin)
        .arg("--version")
        .output()
        .await
        .map_err(|e| LocalError::Install(format!("could not run llama-server: {e}")))?;
    if !out.status.success() {
        return Err(LocalError::Install(format!(
            "llama-server --version failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_url_points_at_the_release_asset() {
        let url = download_url("b9835", "llama-b9835-bin-macos-arm64.tar.gz");
        assert_eq!(
            url,
            "https://github.com/ggml-org/llama.cpp/releases/download/b9835/llama-b9835-bin-macos-arm64.tar.gz"
        );
    }

    #[test]
    fn asset_and_can_manage_agree() {
        assert_eq!(can_manage(), asset_name(PINNED_VERSION).is_some());
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            let a = asset_name("bXX").unwrap();
            assert!(a.contains("macos-arm64"));
            assert!(a.ends_with(".tar.gz"));
        }
    }

    /// Real end-to-end install: downloads the pinned runtime into the user's home
    /// and verifies it runs. Network + ~11 MB; run with `--ignored`.
    #[tokio::test]
    #[ignore]
    async fn installs_and_verifies_runtime() {
        if !can_manage() {
            eprintln!("skipping: no managed runtime for this platform");
            return;
        }
        let bin = install(|e| match e {
            RuntimeInstallEvent::Log { line } => eprintln!("{line}"),
            RuntimeInstallEvent::Progress { downloaded, total } => {
                eprintln!("  {downloaded}/{total:?}")
            }
        })
        .await
        .expect("install should succeed");
        assert!(bin.is_file());
        assert_eq!(managed_binary_path().as_deref(), Some(bin.as_path()));
        assert_eq!(status().source, RuntimeSource::Managed);
    }
}
