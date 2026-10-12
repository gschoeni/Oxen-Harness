//! API key resolution for Oxen.ai.
//!
//! Resolution order:
//! 1. The `OXEN_API_KEY` environment variable (explicit override, also used in
//!    CI and containers).
//! 2. The Oxen auth config file (`~/.config/oxen/auth_config.toml`, or
//!    `$OXEN_CONFIG_DIR/auth_config.toml`), looked up by host. This is the same
//!    file the `oxen` CLI writes on login, so `oxen config --auth ...` /
//!    `oxen login` interoperate without depending on the heavy `liboxen` crate.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::LlmError;
use harness_core::DEFAULT_BASE_URL;

/// Environment variable that overrides the configured Oxen API key.
pub const API_KEY_ENV: &str = "OXEN_API_KEY";

/// Environment variable overriding the Oxen config directory.
pub const CONFIG_DIR_ENV: &str = "OXEN_CONFIG_DIR";

/// Environment variable overriding the full API base URL
/// (e.g. `http://localhost:3001/api/ai`).
pub const BASE_URL_ENV: &str = "OXEN_BASE_URL";

/// Environment variable overriding just the host\[:port\]
/// (e.g. `localhost:3001`); turned into a base URL via [`base_url_from_host`].
pub const HOST_ENV: &str = "OXEN_HOST";

/// The host whose auth token backs the default Oxen.ai inference API.
pub const DEFAULT_OXEN_HOST: &str = "hub.oxen.ai";

const AUTH_CONFIG_FILENAME: &str = "auth_config.toml";

/// Resolve the API base URL: `OXEN_BASE_URL`, else `OXEN_HOST` (expanded), else
/// the default Oxen.ai endpoint.
pub fn resolve_base_url() -> String {
    if let Ok(url) = std::env::var(BASE_URL_ENV) {
        if !url.trim().is_empty() {
            return normalize_base_url(url.trim());
        }
    }
    if let Ok(host) = std::env::var(HOST_ENV) {
        if !host.trim().is_empty() {
            return base_url_from_host(host.trim());
        }
    }
    DEFAULT_BASE_URL.to_string()
}

/// Turn a host (or full URL) into an API base URL.
///
/// - If the value already has a scheme (`http://`/`https://`), it is used as-is
///   (a trailing slash is trimmed).
/// - Otherwise it is treated as `host[:port]`: a `/api/ai` base is built, using
///   `http` for local hosts (localhost / loopback / explicit non-443 port) and
///   `https` elsewhere.
pub fn base_url_from_host(host: &str) -> String {
    let host = host.trim().trim_end_matches('/');
    if host.contains("://") {
        return host.to_string();
    }
    let scheme = if is_local_host(host) { "http" } else { "https" };
    format!("{scheme}://{host}/api/ai")
}

/// Extract the `host[:port]` authority from a base URL, used to look up the
/// matching auth token in the Oxen config.
pub fn host_from_base_url(base_url: &str) -> String {
    let without_scheme = base_url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(base_url);
    without_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(without_scheme)
        .to_string()
}

fn is_local_host(host: &str) -> bool {
    let bare = host.split(':').next().unwrap_or(host);
    // An explicit port (other than 443) strongly implies a local/dev server.
    let has_nondefault_port = host
        .rsplit_once(':')
        .and_then(|(_, port)| port.parse::<u16>().ok())
        .is_some_and(|p| p != 443);
    matches!(
        bare,
        "localhost" | "127.0.0.1" | "0.0.0.0" | "[::1]" | "::1"
    ) || has_nondefault_port
}

fn normalize_base_url(url: &str) -> String {
    url.trim_end_matches('/').to_string()
}

/// Mirror of the relevant parts of Oxen's `auth_config.toml`.
#[derive(Debug, Deserialize)]
struct AuthConfig {
    #[serde(default)]
    host_configs: Vec<HostConfig>,
}

#[derive(Debug, Deserialize)]
struct HostConfig {
    host: String,
    auth_token: Option<String>,
}

/// Resolve the Oxen config directory (`$OXEN_CONFIG_DIR` or `~/.config/oxen`).
fn oxen_config_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var(CONFIG_DIR_ENV) {
        if !dir.trim().is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    dirs::home_dir().map(|home| home.join(".config").join("oxen"))
}

/// Resolve the API key for `host`, checking the env var first, then the config.
pub fn resolve_api_key(host: &str) -> Result<String, LlmError> {
    if let Ok(key) = std::env::var(API_KEY_ENV) {
        if !key.trim().is_empty() {
            return Ok(key);
        }
    }

    let dir = oxen_config_dir().ok_or_else(|| {
        LlmError::Auth("could not determine the home directory for Oxen config".into())
    })?;
    token_from_file(&dir.join(AUTH_CONFIG_FILENAME), host)
}

/// Resolve the API key for the default Oxen.ai host.
pub fn resolve_default_api_key() -> Result<String, LlmError> {
    resolve_api_key(DEFAULT_OXEN_HOST)
}

/// Resolve the API key for whatever host backs `base_url`.
pub fn resolve_api_key_for_base_url(base_url: &str) -> Result<String, LlmError> {
    resolve_api_key(&host_from_base_url(base_url))
}

fn token_from_file(path: &Path, host: &str) -> Result<String, LlmError> {
    let contents = std::fs::read_to_string(path).map_err(|_| {
        LlmError::Auth(format!(
            "no Oxen API key found. Set {API_KEY_ENV}, or log in with the oxen CLI \
             (expected {})",
            path.display()
        ))
    })?;

    let config: AuthConfig = toml::from_str(&contents)
        .map_err(|e| LlmError::Auth(format!("could not parse {}: {e}", path.display())))?;

    config
        .host_configs
        .into_iter()
        .find(|h| h.host == host)
        .and_then(|h| h.auth_token)
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| {
            LlmError::Auth(format!(
                "no Oxen API key for host `{host}`. Set {API_KEY_ENV} or run `oxen config --auth {host} <token>`"
            ))
        })
}

/// The Oxen auth config file the key lookup reads (and [`save_api_key`]
/// writes): `$OXEN_CONFIG_DIR/auth_config.toml`, else
/// `~/.config/oxen/auth_config.toml`.
pub fn auth_config_path() -> Option<PathBuf> {
    oxen_config_dir().map(|dir| dir.join(AUTH_CONFIG_FILENAME))
}

/// The key saved for `host` in the auth config, ignoring `OXEN_API_KEY` —
/// "is this host configured?", not "which key will a request use?". A missing
/// file or host entry is `Ok(None)`; an unreadable or malformed file is an
/// error, so a broken config never reads as "not configured yet".
pub fn configured_api_key(host: &str) -> Result<Option<String>, LlmError> {
    let path = auth_config_path().ok_or_else(|| {
        LlmError::Auth("could not determine the home directory for Oxen config".into())
    })?;
    configured_api_key_at(&path, host)
}

fn configured_api_key_at(path: &Path, host: &str) -> Result<Option<String>, LlmError> {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(LlmError::Auth(format!(
                "could not read {}: {e}",
                path.display()
            )))
        }
    };
    let config: AuthConfig = toml::from_str(&contents)
        .map_err(|e| LlmError::Auth(format!("could not parse {}: {e}", path.display())))?;
    Ok(config
        .host_configs
        .into_iter()
        .find(|h| h.host == host)
        .and_then(|h| h.auth_token)
        .filter(|t| !t.trim().is_empty()))
}

/// Save `key` as `host`'s token in the auth config — the file the `oxen` CLI
/// writes on login — replacing that host's entry if there is one and leaving
/// every other host and setting as it was. Creates the file (and directory)
/// when missing; the file is written owner-only, since it holds secrets.
/// Returns the path written.
pub fn save_api_key(host: &str, key: &str) -> Result<PathBuf, LlmError> {
    let path = auth_config_path().ok_or_else(|| {
        LlmError::Auth("could not determine the home directory for Oxen config".into())
    })?;
    save_api_key_at(&path, host, key)?;
    Ok(path)
}

fn save_api_key_at(path: &Path, host: &str, key: &str) -> Result<(), LlmError> {
    let (host, key) = (host.trim(), key.trim());
    if host.is_empty() || key.is_empty() {
        return Err(LlmError::Auth(
            "both a host and an API key are needed".into(),
        ));
    }
    let fail = |what: &str, e: &dyn std::fmt::Display| {
        LlmError::Auth(format!("could not {what} {}: {e}", path.display()))
    };
    // A generic table rather than [`AuthConfig`], so fields this crate doesn't
    // model survive the rewrite.
    let mut doc: toml::Table = match std::fs::read_to_string(path) {
        Ok(contents) => toml::from_str(&contents).map_err(|e| fail("parse", &e))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => toml::Table::new(),
        Err(e) => return Err(fail("read", &e)),
    };
    doc.entry("default_host")
        .or_insert_with(|| DEFAULT_OXEN_HOST.into());
    let configs = doc
        .entry("host_configs")
        .or_insert_with(|| toml::Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| fail("update", &"`host_configs` is not a list"))?;
    let existing = configs
        .iter_mut()
        .filter_map(toml::Value::as_table_mut)
        .find(|entry| entry.get("host").and_then(toml::Value::as_str) == Some(host));
    match existing {
        Some(entry) => {
            entry.insert("auth_token".into(), key.into());
        }
        None => {
            let mut entry = toml::Table::new();
            entry.insert("host".into(), host.into());
            entry.insert("auth_token".into(), key.into());
            configs.push(toml::Value::Table(entry));
        }
    }
    let text = toml::to_string(&doc).map_err(|e| fail("write", &e))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| fail("create the directory for", &e))?;
    }
    // Write beside the target and rename over it, so an interrupted save
    // never leaves a half-written config behind.
    let tmp = path.with_extension("toml.tmp");
    write_private(&tmp, text.as_bytes()).map_err(|e| fail("write", &e))?;
    std::fs::rename(&tmp, path).map_err(|e| fail("replace", &e))
}

/// Write `bytes` to a new file readable only by its owner (on Unix).
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn saving_a_key_adds_or_replaces_one_host_and_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join(AUTH_CONFIG_FILENAME);

        // A fresh file gets the oxen CLI's shape.
        save_api_key_at(&path, "hub.oxen.ai", " sk-hub ").unwrap();
        assert_eq!(token_from_file(&path, "hub.oxen.ai").unwrap(), "sk-hub");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(r#"default_host = "hub.oxen.ai""#), "{text}");
        assert!(text.contains("[[host_configs]]"), "{text}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        // Another host is added beside it; re-saving one replaces only it.
        save_api_key_at(&path, "localhost:3001", "sk-local").unwrap();
        save_api_key_at(&path, "hub.oxen.ai", "sk-hub-2").unwrap();
        assert_eq!(token_from_file(&path, "hub.oxen.ai").unwrap(), "sk-hub-2");
        assert_eq!(
            token_from_file(&path, "localhost:3001").unwrap(),
            "sk-local"
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches("[[host_configs]]").count(), 2, "{text}");
        assert!(!path.with_extension("toml.tmp").exists());
    }

    #[test]
    fn saving_keeps_settings_this_crate_does_not_model() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(AUTH_CONFIG_FILENAME);
        std::fs::write(
            &path,
            "default_host = \"staging.hub.oxen.ai\"\n\n[[host_configs]]\nhost = \"staging.hub.oxen.ai\"\nauth_token = \"sk-stage\"\nscheme = \"https\"\n",
        )
        .unwrap();
        save_api_key_at(&path, "hub.oxen.ai", "sk-hub").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains(r#"default_host = "staging.hub.oxen.ai""#),
            "{text}"
        );
        assert!(text.contains(r#"scheme = "https""#), "{text}");
        assert_eq!(
            token_from_file(&path, "staging.hub.oxen.ai").unwrap(),
            "sk-stage"
        );
    }

    #[test]
    fn configured_key_tells_missing_from_broken() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(AUTH_CONFIG_FILENAME);
        assert_eq!(configured_api_key_at(&path, "hub.oxen.ai").unwrap(), None);
        save_api_key_at(&path, "hub.oxen.ai", "sk-hub").unwrap();
        assert_eq!(
            configured_api_key_at(&path, "hub.oxen.ai")
                .unwrap()
                .as_deref(),
            Some("sk-hub")
        );
        assert_eq!(
            configured_api_key_at(&path, "localhost:3001").unwrap(),
            None
        );
        std::fs::write(&path, "host_configs = [[[").unwrap();
        assert!(configured_api_key_at(&path, "hub.oxen.ai").is_err());
        assert!(save_api_key_at(&path, "hub.oxen.ai", "sk").is_err());
    }

    fn write_auth_file(token_host: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(AUTH_CONFIG_FILENAME);
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            "default_host = \"{token_host}\"\n\n[[host_configs]]\nhost = \"{token_host}\"\nauth_token = \"sk-from-file\"\n"
        )
        .unwrap();
        (dir, path)
    }

    #[test]
    fn reads_token_for_matching_host_from_file() {
        let (_dir, path) = write_auth_file(DEFAULT_OXEN_HOST);
        let token = token_from_file(&path, DEFAULT_OXEN_HOST).unwrap();
        assert_eq!(token, "sk-from-file");
    }

    #[test]
    fn errors_when_host_not_present() {
        let (_dir, path) = write_auth_file("other.host");
        let err = token_from_file(&path, DEFAULT_OXEN_HOST).unwrap_err();
        assert!(matches!(err, LlmError::Auth(_)));
    }

    #[test]
    fn errors_with_helpful_message_when_file_missing() {
        let err = token_from_file(
            Path::new("/nonexistent/auth_config.toml"),
            DEFAULT_OXEN_HOST,
        )
        .unwrap_err();
        match err {
            LlmError::Auth(msg) => assert!(msg.contains(API_KEY_ENV)),
            other => panic!("expected Auth error, got {other:?}"),
        }
    }

    #[test]
    fn host_expands_to_local_http_base_url() {
        assert_eq!(
            base_url_from_host("localhost:3001"),
            "http://localhost:3001/api/ai"
        );
        assert_eq!(
            base_url_from_host("127.0.0.1:8080"),
            "http://127.0.0.1:8080/api/ai"
        );
    }

    #[test]
    fn bare_remote_host_expands_to_https_base_url() {
        assert_eq!(
            base_url_from_host("hub.oxen.ai"),
            "https://hub.oxen.ai/api/ai"
        );
    }

    #[test]
    fn host_with_scheme_is_used_as_is() {
        assert_eq!(
            base_url_from_host("http://my-server.internal/api/ai/"),
            "http://my-server.internal/api/ai"
        );
    }

    #[test]
    fn extracts_host_from_base_url() {
        assert_eq!(
            host_from_base_url("http://localhost:3001/api/ai"),
            "localhost:3001"
        );
        assert_eq!(
            host_from_base_url("https://hub.oxen.ai/api/ai"),
            "hub.oxen.ai"
        );
    }

    #[test]
    fn host_extraction_strips_path_query_and_missing_scheme() {
        assert_eq!(host_from_base_url("https://h.ai:9/api/ai?x=1#f"), "h.ai:9");
        assert_eq!(host_from_base_url("no-scheme.host/api"), "no-scheme.host");
    }

    #[test]
    fn nondefault_port_implies_a_local_http_server_even_for_remote_hosts() {
        // A hostname with an explicit non-443 port reads as a dev server → http,
        // even when the host itself isn't loopback.
        assert_eq!(
            base_url_from_host("my-box.lan:8080"),
            "http://my-box.lan:8080/api/ai"
        );
        // ...but the standard HTTPS port keeps https.
        assert_eq!(
            base_url_from_host("api.example.com:443"),
            "https://api.example.com:443/api/ai"
        );
    }
}
