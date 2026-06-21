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

/// Environment variable that overrides the configured Oxen API key.
pub const API_KEY_ENV: &str = "OXEN_API_KEY";

/// Environment variable overriding the Oxen config directory.
pub const CONFIG_DIR_ENV: &str = "OXEN_CONFIG_DIR";

/// The host whose auth token backs the default Oxen.ai inference API.
pub const DEFAULT_OXEN_HOST: &str = "hub.oxen.ai";

const AUTH_CONFIG_FILENAME: &str = "auth_config.toml";

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
}
