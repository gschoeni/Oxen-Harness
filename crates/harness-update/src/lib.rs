//! Release checks and self-update for both front ends.
//!
//! The CLI and the desktop app ship as separate GitHub releases off one repo,
//! told apart by tag prefix (`cli-v0.0.8`, `app-v0.0.8`; see
//! `scripts/release.sh`). This crate asks GitHub's public releases API for
//! the newest published release of a [`Component`], compares it with the
//! running version, and — when asked — downloads the platform's archive,
//! checks it against the release's `SHA256SUMS`, and swaps it in place
//! (`install`). Restarting is the front end's business.
//!
//! The startup check is cached on disk ([`Updater::check`] with a max age),
//! so launching the CLI ten times an hour costs GitHub one request, and an
//! offline launch never waits on the network for long.
//!
//! Trust rests on HTTPS to github.com plus the checksum file published with
//! each release: the same footing as the install instructions in the README.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

mod install;
pub use install::{app_bundle_of, install_app_bundle, install_cli};

/// The GitHub repository releases are published to.
pub const REPO: &str = "gschoeni/Oxen-Harness";

/// GitHub's REST API root.
pub const GITHUB_API: &str = "https://api.github.com";

/// The checksum file every release carries (`<sha256>  <asset name>` lines).
pub const SUMS_ASSET: &str = "SHA256SUMS";

/// How long a startup check's answer is reused before GitHub is asked again.
pub const CHECK_MAX_AGE: Duration = Duration::from_secs(60 * 60);

/// How long a startup check may wait on GitHub before giving up.
const CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// Which product a release belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Component {
    /// The `oxen-harness` command-line binary.
    Cli,
    /// The desktop app (Tauri bundle).
    App,
}

impl Component {
    /// The tag prefix its releases carry, before the version.
    pub fn tag_prefix(self) -> &'static str {
        match self {
            Component::Cli => "cli-v",
            Component::App => "app-v",
        }
    }

    fn cache_name(self) -> &'static str {
        match self {
            Component::Cli => "update-check-cli.json",
            Component::App => "update-check-app.json",
        }
    }

    /// The release archive this machine installs from, or `None` when there
    /// is no in-place update for this platform (the user downloads the
    /// installer from the release page instead).
    pub fn asset_name(self, version: &Version) -> Option<String> {
        let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
        match self {
            Component::Cli => {
                let target = match (os, arch) {
                    ("macos", "aarch64") => "aarch64-apple-darwin",
                    ("macos", "x86_64") => "x86_64-apple-darwin",
                    ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
                    ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
                    // Windows ships a .zip; replacing a running .exe also
                    // needs its own dance. Until then: the release page.
                    _ => return None,
                };
                Some(format!("oxen-harness-{version}-{target}.tar.gz"))
            }
            Component::App => match (os, arch) {
                ("macos", "aarch64") => Some("oxen-harness_aarch64.app.tar.gz".into()),
                ("macos", "x86_64") => Some("oxen-harness_x64.app.tar.gz".into()),
                // Linux/Windows bundles are installers (deb, rpm, msi, …):
                // the package manager owns those files, not the app.
                _ => None,
            },
        }
    }
}

/// A `major.minor.patch` release version. Pre-release tags aren't published,
/// so the comparison needs nothing more.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version(pub u64, pub u64, pub u64);

impl Version {
    /// Parse `0.0.8` or `v0.0.8`; `None` for anything else.
    pub fn parse(text: &str) -> Option<Version> {
        let mut parts = text.trim().trim_start_matches('v').split('.');
        let mut next = || parts.next()?.parse::<u64>().ok();
        let version = Version(next()?, next()?, next()?);
        parts.next().is_none().then_some(version)
    }
}

impl Serialize for Version {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Version {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        Version::parse(&text)
            .ok_or_else(|| serde::de::Error::custom(format!("not a version: {text:?}")))
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// A downloadable file attached to a release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

/// A published release of one component.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub version: Version,
    pub tag: String,
    /// The release page, for notes and manual installers.
    pub page_url: String,
    pub published_at: Option<String>,
    /// Release notes (Markdown), as GitHub generated or the author wrote them.
    pub notes: String,
    pub assets: Vec<Asset>,
}

impl Release {
    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == name)
    }
}

/// What a front end shows: the running version, the newest release, and
/// whether it is an update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateStatus {
    pub current: String,
    /// The newest published version, `None` when there are no releases yet.
    pub latest: Option<String>,
    pub available: bool,
    /// The newest release's page.
    pub release_url: Option<String>,
    pub notes: Option<String>,
    pub published_at: Option<String>,
    /// Whether this platform can install the update in place (otherwise the
    /// release page has the installer).
    pub installable: bool,
    /// When GitHub was last asked, in Unix seconds.
    pub checked_at: u64,
}

impl UpdateStatus {
    fn new(
        component: Component,
        current: Version,
        latest: Option<&Release>,
        checked_at: u64,
    ) -> Self {
        let available = latest.is_some_and(|r| r.version > current);
        UpdateStatus {
            current: current.to_string(),
            latest: latest.map(|r| r.version.to_string()),
            available,
            release_url: latest.map(|r| r.page_url.clone()),
            notes: latest
                .map(|r| r.notes.clone())
                .filter(|n| !n.trim().is_empty()),
            published_at: latest.and_then(|r| r.published_at.clone()),
            installable: latest
                .and_then(|r| component.asset_name(&r.version))
                .is_some(),
            checked_at,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("could not reach GitHub to check for updates: {0}")]
    Network(#[source] reqwest::Error),
    #[error("GitHub answered {status} for {url}")]
    Status { status: u16, url: String },
    #[error("GitHub sent a release list this version can't read: {0}")]
    Parse(#[source] reqwest::Error),
    #[error("not a release version: {0:?}")]
    BadVersion(String),
    #[error("no update for this platform in {tag}: download it from {page}")]
    NoAsset { tag: String, page: String },
    #[error("{tag} has no {SUMS_ASSET} entry for {asset}, so it can't be verified")]
    Unverified { tag: String, asset: String },
    #[error("the download of {asset} is corrupt (checksum mismatch); nothing was installed")]
    ChecksumMismatch { asset: String },
    #[error("could not install the update ({path}): {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{0}")]
    Install(String),
}

/// The raw GitHub API shapes, trimmed to what a release check reads.
#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    html_url: String,
    published_at: Option<String>,
    body: Option<String>,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

/// The newest published release in `list` whose tag belongs to `component`.
fn newest(list: Vec<GhRelease>, component: Component) -> Option<Release> {
    list.into_iter()
        .filter(|r| !r.draft && !r.prerelease)
        .filter_map(|r| {
            let version = Version::parse(r.tag_name.strip_prefix(component.tag_prefix())?)?;
            Some(Release {
                version,
                tag: r.tag_name,
                page_url: r.html_url,
                published_at: r.published_at,
                notes: r.body.unwrap_or_default(),
                assets: r
                    .assets
                    .into_iter()
                    .map(|a| Asset {
                        name: a.name,
                        url: a.browser_download_url,
                        size: a.size,
                    })
                    .collect(),
            })
        })
        .max_by_key(|r| r.version)
}

/// A cached check: the newest release as of `checked_at`.
#[derive(Serialize, Deserialize)]
struct CachedCheck {
    checked_at: u64,
    latest: Option<Release>,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The cache file for `component` under the harness base directory.
pub fn cache_path(base_dir: &Path, component: Component) -> PathBuf {
    base_dir.join(component.cache_name())
}

/// Talks to GitHub's releases API for one repository.
#[derive(Clone)]
pub struct Updater {
    client: reqwest::Client,
    api: String,
    repo: String,
}

impl Default for Updater {
    fn default() -> Self {
        Updater::new(GITHUB_API, REPO)
    }
}

impl Updater {
    /// An updater against `api` (GitHub's, or a test server) for `repo`.
    pub fn new(api: &str, repo: &str) -> Self {
        Updater {
            client: harness_http::client(),
            api: api.trim_end_matches('/').to_string(),
            repo: repo.to_string(),
        }
    }

    /// The newest published release of `component`, straight from GitHub.
    pub async fn latest(&self, component: Component) -> Result<Option<Release>, UpdateError> {
        let url = format!("{}/repos/{}/releases?per_page=30", self.api, self.repo);
        let resp = self
            .client
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .timeout(CHECK_TIMEOUT)
            .send()
            .await
            .map_err(UpdateError::Network)?;
        if !resp.status().is_success() {
            return Err(UpdateError::Status {
                status: resp.status().as_u16(),
                url,
            });
        }
        let list: Vec<GhRelease> = resp.json().await.map_err(UpdateError::Parse)?;
        Ok(newest(list, component))
    }

    /// Compare `current` with the newest release. With a `cache` file, an
    /// answer younger than `max_age` is reused instead of asking GitHub, and a
    /// fresh answer is saved for next time. `Duration::ZERO` always asks.
    pub async fn check(
        &self,
        component: Component,
        current: &str,
        cache: Option<&Path>,
        max_age: Duration,
    ) -> Result<UpdateStatus, UpdateError> {
        let current =
            Version::parse(current).ok_or_else(|| UpdateError::BadVersion(current.to_string()))?;
        let now = now_secs();
        if let Some(cached) = cache.and_then(read_cache) {
            if now.saturating_sub(cached.checked_at) < max_age.as_secs() {
                return Ok(UpdateStatus::new(
                    component,
                    current,
                    cached.latest.as_ref(),
                    cached.checked_at,
                ));
            }
        }
        let latest = self.latest(component).await?;
        if let Some(path) = cache {
            write_cache(
                path,
                &CachedCheck {
                    checked_at: now,
                    latest: latest.clone(),
                },
            );
        }
        Ok(UpdateStatus::new(component, current, latest.as_ref(), now))
    }

    /// Download this platform's archive from `release` and check it against
    /// the release's `SHA256SUMS`. A release without a checksum line for the
    /// archive is refused rather than installed unverified.
    pub async fn download(
        &self,
        component: Component,
        release: &Release,
    ) -> Result<Vec<u8>, UpdateError> {
        let no_asset = || UpdateError::NoAsset {
            tag: release.tag.clone(),
            page: release.page_url.clone(),
        };
        let name = component
            .asset_name(&release.version)
            .ok_or_else(no_asset)?;
        let asset = release.asset(&name).ok_or_else(no_asset)?;
        let unverified = || UpdateError::Unverified {
            tag: release.tag.clone(),
            asset: name.clone(),
        };
        let sums = release.asset(SUMS_ASSET).ok_or_else(unverified)?;
        let sums = String::from_utf8_lossy(&self.fetch(&sums.url).await?).into_owned();
        let expected = expected_sum(&sums, &name).ok_or_else(unverified)?;
        let bytes = self.fetch(&asset.url).await?;
        if sha256_hex(&bytes) != expected {
            return Err(UpdateError::ChecksumMismatch { asset: name });
        }
        Ok(bytes)
    }

    async fn fetch(&self, url: &str) -> Result<Vec<u8>, UpdateError> {
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(UpdateError::Network)?;
        if !resp.status().is_success() {
            return Err(UpdateError::Status {
                status: resp.status().as_u16(),
                url: url.to_string(),
            });
        }
        Ok(resp.bytes().await.map_err(UpdateError::Network)?.to_vec())
    }
}

/// A missing or unreadable cache is just "never checked": the next check
/// asks GitHub and rewrites it.
fn read_cache(path: &Path) -> Option<CachedCheck> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Best-effort: an unwritable cache costs one extra request next launch,
/// never the check itself.
fn write_cache(path: &Path, check: &CachedCheck) {
    let written = serde_json::to_vec(check)
        .map_err(std::io::Error::other)
        .and_then(|json| std::fs::write(path, json));
    if let Err(e) = written {
        eprintln!(
            "note: could not save the update check to {}: {e}",
            path.display()
        );
    }
}

/// The checksum `SHA256SUMS` lists for `name` (`<hex>  <name>` lines; a `*`
/// before the name marks binary mode in some tools).
fn expected_sum(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, file) = line.trim().split_once(char::is_whitespace)?;
        (file.trim().trim_start_matches('*') == name).then(|| hash.to_ascii_lowercase())
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gh_release(tag: &str, draft: bool, assets: &[(&str, &str)]) -> serde_json::Value {
        serde_json::json!({
            "tag_name": tag,
            "draft": draft,
            "prerelease": false,
            "html_url": format!("https://github.com/x/y/releases/tag/{tag}"),
            "published_at": "2026-10-20T00:00:00Z",
            "body": "notes",
            "assets": assets.iter().map(|(name, url)| serde_json::json!({
                "name": name, "browser_download_url": url, "size": 3
            })).collect::<Vec<_>>(),
        })
    }

    #[test]
    fn versions_parse_and_order_numerically() {
        assert_eq!(Version::parse("v0.0.10"), Some(Version(0, 0, 10)));
        assert!(Version(0, 0, 10) > Version(0, 0, 9));
        assert_eq!(Version::parse("0.1"), None);
        assert_eq!(Version::parse("0.1.0-beta"), None);
        assert_eq!(Version::parse("0.1.0.1"), None);
    }

    #[test]
    fn picks_the_newest_published_release_of_the_component() {
        let list: Vec<GhRelease> = serde_json::from_value(serde_json::json!([
            gh_release("app-v0.0.9", false, &[]),
            gh_release("cli-v0.0.9", true, &[]),
            gh_release("cli-v0.0.10", false, &[]),
            gh_release("cli-v0.0.8", false, &[]),
            gh_release("cli-vnope", false, &[]),
        ]))
        .unwrap();
        let cli = newest(list, Component::Cli).unwrap();
        assert_eq!(cli.version, Version(0, 0, 10));
        assert_eq!(cli.tag, "cli-v0.0.10");
    }

    #[test]
    fn reads_the_checksum_line_for_an_asset() {
        let sums = "abc123  oxen-harness-0.0.8-x.tar.gz\nDEF456 *other.tar.gz\n";
        assert_eq!(
            expected_sum(sums, "oxen-harness-0.0.8-x.tar.gz").as_deref(),
            Some("abc123")
        );
        assert_eq!(
            expected_sum(sums, "other.tar.gz").as_deref(),
            Some("def456")
        );
        assert_eq!(expected_sum(sums, "missing"), None);
    }

    #[tokio::test]
    async fn check_reports_an_update_and_reuses_the_cached_answer() {
        let mut server = mockito::Server::new_async().await;
        let list = server
            .mock("GET", "/repos/x/y/releases?per_page=30")
            .with_body(serde_json::json!([gh_release("cli-v0.0.9", false, &[])]).to_string())
            .expect(1)
            .create_async()
            .await;
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_path(dir.path(), Component::Cli);
        let updater = Updater::new(&server.url(), "x/y");

        let status = updater
            .check(Component::Cli, "0.0.8", Some(&cache), CHECK_MAX_AGE)
            .await
            .unwrap();
        assert!(status.available);
        assert_eq!(status.latest.as_deref(), Some("0.0.9"));
        assert_eq!(status.current, "0.0.8");

        // Within the max age the second check never reaches GitHub.
        let again = updater
            .check(Component::Cli, "0.0.9", Some(&cache), CHECK_MAX_AGE)
            .await
            .unwrap();
        assert!(!again.available, "already on the newest release");
        list.assert_async().await;
    }

    #[tokio::test]
    async fn check_surfaces_a_github_error() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/repos/x/y/releases?per_page=30")
            .with_status(403)
            .create_async()
            .await;
        let err = Updater::new(&server.url(), "x/y")
            .check(Component::App, "0.0.8", None, Duration::ZERO)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("GitHub answered 403"), "{err}");
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[tokio::test]
    async fn download_verifies_the_archive_against_sha256sums() {
        let mut server = mockito::Server::new_async().await;
        let name = "oxen-harness-0.0.9-aarch64-apple-darwin.tar.gz";
        let good = format!("{}  {name}\n", sha256_hex(b"abc"));
        server
            .mock("GET", "/sums")
            .with_body(good)
            .create_async()
            .await;
        server
            .mock("GET", "/bad-sums")
            .with_body(format!("{}  {name}\n", sha256_hex(b"xyz")))
            .create_async()
            .await;
        server
            .mock("GET", "/archive")
            .with_body("abc")
            .create_async()
            .await;
        let release = |sums: &str| Release {
            version: Version(0, 0, 9),
            tag: "cli-v0.0.9".into(),
            page_url: "page".into(),
            published_at: None,
            notes: String::new(),
            assets: vec![
                Asset {
                    name: name.into(),
                    url: format!("{}/archive", server.url()),
                    size: 3,
                },
                Asset {
                    name: SUMS_ASSET.into(),
                    url: format!("{}{sums}", server.url()),
                    size: 1,
                },
            ],
        };
        let updater = Updater::new(&server.url(), "x/y");
        assert_eq!(
            updater
                .download(Component::Cli, &release("/sums"))
                .await
                .unwrap(),
            b"abc"
        );
        let err = updater
            .download(Component::Cli, &release("/bad-sums"))
            .await
            .unwrap_err();
        assert!(matches!(err, UpdateError::ChecksumMismatch { .. }), "{err}");

        // No checksum file at all: refused, not installed blind.
        let mut bare = release("/sums");
        bare.assets.retain(|a| a.name != SUMS_ASSET);
        let err = updater.download(Component::Cli, &bare).await.unwrap_err();
        assert!(matches!(err, UpdateError::Unverified { .. }), "{err}");
    }
}
