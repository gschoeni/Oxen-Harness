//! A self-managed llama.cpp runtime.
//!
//! Rather than asking the user to install Homebrew + llama.cpp, the app downloads
//! a *pinned* prebuilt `llama-server` for this OS/arch into
//! `~/.oxen-harness/runtime/` and runs it directly. macOS (Apple Silicon) is
//! wired up today; other targets report `can_manage = false` and fall back to the
//! Homebrew/PATH discovery in [`crate::server`].
//!
//! Validated against release `b11803`: the macOS arm64 tarball unpacks to a single
//! `llama-{version}/` directory holding `llama-server` plus its `.dylib`s (Metal
//! included); the binary's `@loader_path` rpath loads them in place, so no PATH
//! or env setup is needed.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::LocalError;

/// The llama.cpp release we pin to — a known-good build. Bump deliberately (and
/// surface an "update runtime" action) rather than chasing `latest`.
/// `b11803` is the first release with Recursive Flint support (`FlintForCausalLM`).
pub const PINNED_VERSION: &str = "b11803";

/// SHA-256 of the pinned macOS arm64 release asset, as GitHub publishes it
/// (the asset's `digest`) and as a fresh download hashes. The archive holds
/// a binary we then run, so a download that doesn't match is refused before
/// anything is extracted. Bump together with [`PINNED_VERSION`]:
/// `gh api repos/ggml-org/llama.cpp/releases/tags/<ver> --jq '.assets[] |
/// select(.name | endswith("macos-arm64.tar.gz")) | .digest'`.
const PINNED_MACOS_ARM64_SHA256: &str =
    "742ad51ee1bcf2606b5875d3a45824f8e6fd9000dc4d7488107448e0d40ddd12";

/// Where the managed runtime lives: `~/.oxen-harness/runtime/llama.cpp/`
/// (honoring the `OXEN_HARNESS_DIR` override).
fn runtime_root() -> Option<PathBuf> {
    Some(
        harness_config::paths::base_dir_unchecked()?
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
    if let Err(e) = check_sha256(&archive, PINNED_MACOS_ARM64_SHA256) {
        let _ = std::fs::remove_file(&archive);
        return Err(e);
    }

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
    let client = harness_http::client();
    crate::download::fetch_to_file(
        &client,
        url,
        dest,
        crate::download::FetchOpts::default(),
        |downloaded, total| on_event(RuntimeInstallEvent::Progress { downloaded, total }),
    )
    .await
}

/// Refuse `path` unless its SHA-256 is `expected` (lowercase hex).
fn check_sha256(path: &Path, expected: &str) -> Result<(), LocalError> {
    use sha2::Digest;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = sha2::Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    let actual = format!("{:x}", hasher.finalize());
    if actual == expected {
        Ok(())
    } else {
        Err(LocalError::Install(format!(
            "the downloaded llama.cpp runtime failed its checksum (expected {expected}, got \
             {actual}); refusing to install it"
        )))
    }
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
        let url = download_url("b10002", "llama-b10002-bin-macos-arm64.tar.gz");
        assert_eq!(
            url,
            "https://github.com/ggml-org/llama.cpp/releases/download/b10002/llama-b10002-bin-macos-arm64.tar.gz"
        );
    }

    #[test]
    fn a_download_must_match_the_pinned_checksum() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.tar.gz");
        std::fs::write(&file, b"abc").unwrap();
        // SHA-256 of "abc".
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        check_sha256(&file, abc).unwrap();
        let err = check_sha256(&file, &"0".repeat(64)).unwrap_err();
        assert!(err.to_string().contains("checksum"), "{err}");
        assert_eq!(PINNED_MACOS_ARM64_SHA256.len(), 64);
        assert!(PINNED_MACOS_ARM64_SHA256
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
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

    #[test]
    fn pinned_runtime_postdates_bonsai_27b_upstream_support() {
        let build: u32 = PINNED_VERSION
            .strip_prefix('b')
            .and_then(|value| value.parse().ok())
            .expect("llama.cpp release tags are b<build>");
        assert!(
            build >= 10_002,
            "Bonsai 27B requires a current llama.cpp runtime"
        );
    }

    #[test]
    fn pinned_runtime_postdates_muse_glimmer_upstream_support() {
        let build: u32 = PINNED_VERSION
            .strip_prefix('b')
            .and_then(|value| value.parse().ok())
            .expect("llama.cpp release tags are b<build>");
        assert!(
            build >= 10_353,
            "Muse Glimmer 30B requires llama.cpp b10353+ (LLM_ARCH_MUSE_GLIMMER, PR #26841)"
        );
    }

    #[test]
    fn pinned_runtime_postdates_flint_upstream_support() {
        let build: u32 = PINNED_VERSION
            .strip_prefix('b')
            .and_then(|value| value.parse().ok())
            .expect("llama.cpp release tags are b<build>");
        assert!(
            build >= 11_803,
            "Flint requires llama.cpp b11803+ (FlintForCausalLM, PR #30928)"
        );
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
