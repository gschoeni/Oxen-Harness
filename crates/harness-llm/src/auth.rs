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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

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
