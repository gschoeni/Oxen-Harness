//! Oxen connection settings, shared by the CLI and desktop app.
//!
//! The non-secret part — the host override — lives in
//! `~/.oxen-harness/connection.json` (versioned, safe to share). The secrets —
//! the Oxen API key and the Brave Search key — live in `~/.oxen-harness/.env`
//! and are read from the process environment, never written into the JSON.
//!
//! Both fields fall back: a blank host resolves from `OXEN_*` env / the `oxen`
//! CLI login / the default endpoint; a blank key resolves from the environment
//! (which [`harness_config::secrets::load`] populates from `.env` at startup).

use harness_config::{paths, secrets};
use harness_llm::auth::{self, DEFAULT_OXEN_HOST};
use harness_llm::OxenClient;
use harness_tools::web::BRAVE_API_KEY_ENV;
use serde::{Deserialize, Serialize};

use crate::RuntimeError;

/// Schema version for `connection.json`.
pub const SCHEMA_VERSION: u32 = 1;

/// Persisted, non-secret connection settings. A blank `host` means "resolve from
/// env / CLI login / default".
///
/// The `api_key`/`brave_api_key` fields exist only to migrate pre-`.env`
/// installs: older files stored keys here in plaintext. [`load`] moves any it
/// finds into `.env` and clears them, and they're never serialized back (so new
/// files carry only `host`).
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ConnectionConfig {
    #[serde(default)]
    pub host: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub api_key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub brave_api_key: String,
}

/// What the Settings page renders: saved + resolved values plus context.
#[derive(Debug, Clone, Serialize)]
pub struct ConnectionView {
    pub host: String,
    pub api_key: String,
    pub brave_api_key: String,
    /// The default Oxen host, shown as the host field's placeholder.
    pub default_host: String,
    /// Whether a key already resolves from env / CLI login, so a blank field works.
    pub env_key_available: bool,
}

/// Read the saved settings, migrating any legacy plaintext keys into `.env` on
/// the way (a one-time effect). Call once at startup so the migration runs and
/// the keys reach the process environment.
pub fn load() -> ConnectionConfig {
    let mut cfg: ConnectionConfig = crate::config::load_or_default(paths::connection_file());

    let mut migrated = false;
    if !cfg.api_key.trim().is_empty() {
        let _ = secrets::set(auth::API_KEY_ENV, cfg.api_key.trim());
        cfg.api_key.clear();
        migrated = true;
    }
    if !cfg.brave_api_key.trim().is_empty() {
        let _ = secrets::set(BRAVE_API_KEY_ENV, cfg.brave_api_key.trim());
        cfg.brave_api_key.clear();
        migrated = true;
    }
    if migrated {
        let _ = write(&cfg);
    }
    cfg
}

/// Atomically persist the (non-secret) settings and snapshot the config repo.
pub fn write(cfg: &ConnectionConfig) -> Result<(), RuntimeError> {
    crate::config::write_and_snapshot(
        &paths::connection_file()?,
        SCHEMA_VERSION,
        cfg,
        "Update connection settings",
    )
}

/// Save the host (non-secret) and the keys (to `.env`). An empty key clears it.
pub fn save(host: &str, api_key: &str, brave_api_key: &str) -> Result<(), RuntimeError> {
    write(&ConnectionConfig {
        host: host.trim().to_string(),
        ..Default::default()
    })?;
    secrets::set(auth::API_KEY_ENV, api_key.trim())?;
    secrets::set(BRAVE_API_KEY_ENV, brave_api_key.trim())?;
    Ok(())
}

/// Persist just the Brave Search key to `.env` (and the current process).
pub fn set_brave_key(key: &str) -> Result<(), RuntimeError> {
    secrets::set(BRAVE_API_KEY_ENV, key.trim())?;
    Ok(())
}

/// Persist just the Oxen API key to `.env` (and the current process), leaving the
/// host override untouched. Used to authenticate a running chat inline after a
/// 401, without rewriting `connection.json` or starting a fresh session.
pub fn set_oxen_key(key: &str) -> Result<(), RuntimeError> {
    secrets::set(auth::API_KEY_ENV, key.trim())?;
    Ok(())
}

/// Persist just the provider host override to `connection.json`, leaving the
/// keys in `.env` untouched. An empty host clears the override (back to env /
/// CLI login / the default endpoint). Used by the CLI's `/auth host` and read
/// by both hosts through [`effective_base_url`].
pub fn set_oxen_host(host: &str) -> Result<(), RuntimeError> {
    let mut cfg = load();
    cfg.host = host.trim().to_string();
    write(&cfg)
}

/// The effective base URL: the saved host override, else env / CLI / default.
pub fn effective_base_url(cfg: &ConnectionConfig) -> String {
    match cfg.host.trim() {
        "" => auth::resolve_base_url(),
        host => auth::base_url_from_host(host),
    }
}

/// The effective Oxen API key: the environment (populated from `.env` / the
/// `oxen` CLI login) for `base_url`'s host, or empty if nothing resolves.
pub fn effective_api_key(base_url: &str) -> String {
    auth::resolve_api_key_for_base_url(base_url).unwrap_or_default()
}

/// The effective Brave Search key from the environment (empty = web search off).
pub fn effective_brave_key() -> String {
    secrets::get(BRAVE_API_KEY_ENV).unwrap_or_default()
}

/// The Brave key override to hand the tool registry: `Some` when set, else `None`
/// (the tool then falls back to `BRAVE_API_KEY` itself).
pub fn brave_key_override() -> Option<String> {
    secrets::get(BRAVE_API_KEY_ENV)
}

/// Build an Oxen client honoring the saved host and resolved key.
pub fn build_client(model: &str) -> Result<OxenClient, RuntimeError> {
    let cfg = load();
    let base_url = effective_base_url(&cfg);
    match effective_api_key(&base_url) {
        key if key.is_empty() => {
            OxenClient::connect(base_url, model).map_err(|e| RuntimeError::Client(e.to_string()))
        }
        key => Ok(OxenClient::new(base_url, key, model)),
    }
}

/// Settings for the desktop Settings page, pre-filled with resolved values.
pub fn view() -> ConnectionView {
    let cfg = load();
    let base_url = effective_base_url(&cfg);
    let api_key = effective_api_key(&base_url);
    ConnectionView {
        host: auth::host_from_base_url(&base_url),
        env_key_available: !api_key.is_empty(),
        api_key,
        brave_api_key: effective_brave_key(),
        default_host: DEFAULT_OXEN_HOST.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::with_temp_home;

    #[test]
    fn migrates_legacy_plaintext_keys_into_env_and_clears_them() {
        with_temp_home(|| {
            // A pre-.env connection.json with keys stored in plaintext.
            let path = paths::connection_file().unwrap();
            std::fs::write(
                &path,
                r#"{"host":"localhost:3001","api_key":"secret-oxen","brave_api_key":"secret-brave"}"#,
            )
            .unwrap();

            let cfg = load();
            assert_eq!(cfg.host, "localhost:3001");
            // Keys moved to the environment...
            assert_eq!(std::env::var(auth::API_KEY_ENV).unwrap(), "secret-oxen");
            assert_eq!(std::env::var(BRAVE_API_KEY_ENV).unwrap(), "secret-brave");
            // ...the .env file...
            let env_body = std::fs::read_to_string(paths::env_file().unwrap()).unwrap();
            assert!(env_body.contains("OXEN_API_KEY=secret-oxen"));
            assert!(env_body.contains("BRAVE_API_KEY=secret-brave"));
            // ...and were scrubbed from connection.json.
            let json = std::fs::read_to_string(&path).unwrap();
            assert!(
                !json.contains("secret-oxen"),
                "keys must not remain: {json}"
            );
            assert!(!json.contains("secret-brave"));
            assert!(json.contains("localhost:3001"));
        });
    }

    #[test]
    fn set_oxen_key_writes_only_the_key_leaving_host_untouched() {
        with_temp_home(|| {
            // A saved host override that the inline key entry must not disturb.
            save("myhost:9000", "", "").unwrap();

            set_oxen_key("sk-inline").unwrap();

            let base_url = effective_base_url(&load());
            assert_eq!(effective_api_key(&base_url), "sk-inline");
            // The host override is preserved (no fresh connection was started).
            let json = std::fs::read_to_string(paths::connection_file().unwrap()).unwrap();
            assert!(json.contains("myhost:9000"));
            assert!(!json.contains("sk-inline"), "key leaked into json: {json}");
        });
    }

    #[test]
    fn set_oxen_host_updates_only_the_host_leaving_keys_untouched() {
        with_temp_home(|| {
            set_oxen_key("sk-keep").unwrap();

            set_oxen_host("localhost:3001").unwrap();

            let cfg = load();
            assert_eq!(cfg.host, "localhost:3001");
            let base_url = effective_base_url(&cfg);
            assert_eq!(base_url, "http://localhost:3001/api/ai");
            // The key survives the host change.
            assert_eq!(effective_api_key(&base_url), "sk-keep");

            // An empty host clears the override.
            set_oxen_host("").unwrap();
            assert!(load().host.is_empty());
        });
    }

    #[test]
    fn save_writes_host_to_json_and_keys_to_env() {
        with_temp_home(|| {
            save("myhost:9000", "k-oxen", "k-brave").unwrap();
            let json = std::fs::read_to_string(paths::connection_file().unwrap()).unwrap();
            assert!(json.contains("myhost:9000"));
            assert!(!json.contains("k-oxen"), "secret leaked into json: {json}");
            assert_eq!(effective_brave_key(), "k-brave");
        });
    }
}
