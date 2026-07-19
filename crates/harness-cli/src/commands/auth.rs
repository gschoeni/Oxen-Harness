//! `/auth` — set the API key and provider endpoint from inside the REPL.
//!
//! `/auth` walks two prompt cards: the provider base URL, pre-filled with the
//! current one (e.g. `https://hub.oxen.ai/api/ai`) so Enter just accepts it —
//! or edit it to `localhost:3001`, another Oxen server, or any
//! OpenAI-compatible provider's base URL (`https://openrouter.ai/api/v1`,
//! `https://api.openai.com/v1`, …) — then a masked key entry (typed characters
//! echo as `•`) so a pasted key never lands in the terminal scrollback or the
//! prompt history. The key is persisted to `~/.oxen-harness/.env` and the
//! endpoint to `~/.oxen-harness/connection.json` — the same places the desktop
//! app stores them — and a client carrying both is swapped into the running
//! agent, so the current conversation moves over immediately without a restart.
//!
//! Direct forms for scripts (at the cost of appearing in the terminal):
//! `/auth <key>` sets the key, `/auth host <host-or-url>` moves providers, and
//! `/auth <host-or-url> <key>` sets both. A single argument that reads as an
//! endpoint (a URL, `host:port`, or localhost) counts as a host, not a key —
//! so `/auth localhost:3001` never overwrites a saved key.

use std::io;

use anyhow::Result;
use harness_agent::Agent;
use harness_llm::OxenClient;

use crate::picker::{card_input, CardInput, CardInputSpec};
use crate::theme::Ui;

/// The parsed forms of `/auth`'s argument.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AuthArgs {
    /// `/auth` — prompt for the host, then the key.
    Interactive,
    /// `/auth host` — prompt for just the host.
    HostPrompt,
    /// `/auth host <host>` — set the host directly.
    Host(String),
    /// `/auth <key>` — set the key directly.
    Key(String),
    /// `/auth <host> <key>` — set both directly.
    HostAndKey(String, String),
}

/// Whether a bare argument reads as an endpoint rather than an API key. Keys
/// (JWTs, `sk-…`) never contain `:` or `/`; hosts and URLs almost always do.
fn looks_like_endpoint(s: &str) -> bool {
    s.contains(':') || s.contains('/') || s.starts_with("localhost") || s.starts_with("127.")
}

fn parse_args(rest: Option<String>) -> AuthArgs {
    let Some(rest) = rest else {
        return AuthArgs::Interactive;
    };
    let mut parts = rest.split_whitespace();
    match (parts.next(), parts.next()) {
        (Some("host"), None) => AuthArgs::HostPrompt,
        (Some("host"), Some(h)) => AuthArgs::Host(h.to_string()),
        (Some(h), Some(k)) => AuthArgs::HostAndKey(h.to_string(), k.to_string()),
        // The command's own help teaches endpoints, so a lone URL/host is a
        // move-providers request with the `host` keyword forgotten — treating
        // it as a key would overwrite the real one with garbage.
        (Some(h), None) if looks_like_endpoint(h) => AuthArgs::Host(h.to_string()),
        (Some(k), None) => AuthArgs::Key(k.to_string()),
        (None, _) => AuthArgs::Interactive,
    }
}

/// Handle `/auth [host|<key>|<host> <key>]`: collect a host and/or key (from
/// the arguments or the prompt cards), persist them, and swap a client carrying
/// them into the running agent in place. In the card flow, Enter on a blank
/// value skips that piece (leaving it as it is); Esc at *any* card cancels the
/// whole flow — including a host confirmed on an earlier card — so backing out
/// never leaves the session pointed at a provider it has no key for.
pub(crate) fn handle_repl(rest: Option<String>, agent: &mut Agent, ui: &Ui) -> Result<()> {
    let current_base = agent.base_url().to_string();
    let current_host = harness_llm::host_from_base_url(&current_base);

    let cancelled = |ui: &Ui| {
        println!("  {}", ui.dim("cancelled — nothing changed."));
        Ok(())
    };
    let (host, key) = match parse_args(rest) {
        AuthArgs::Interactive => {
            let host = match read_base_url(ui, &current_base)? {
                CardInput::Cancelled => return cancelled(ui),
                CardInput::Skipped => None,
                CardInput::Entered(h) => Some(h),
            };
            // Ask for the key against the host the session will actually use.
            let key_host = host
                .as_deref()
                .map(display_host)
                .unwrap_or_else(|| current_host.clone());
            let key = match read_masked_key(ui, &key_host)? {
                CardInput::Cancelled => return cancelled(ui),
                CardInput::Skipped => None,
                CardInput::Entered(k) => Some(k),
            };
            (host, key)
        }
        AuthArgs::HostPrompt => match read_base_url(ui, &current_base)? {
            CardInput::Cancelled => return cancelled(ui),
            CardInput::Skipped => (None, None),
            CardInput::Entered(h) => (Some(h), None),
        },
        AuthArgs::Host(h) => (Some(h), None),
        AuthArgs::Key(k) => (None, Some(k)),
        AuthArgs::HostAndKey(h, k) => (Some(h), Some(k)),
    };

    apply(agent, ui, host, key, &current_base)
}

/// Persist whatever was entered and swap a matching client into the agent.
fn apply(
    agent: &mut Agent,
    ui: &Ui,
    host: Option<String>,
    key: Option<String>,
    current_base: &str,
) -> Result<()> {
    let key = key.map(|k| k.trim().to_string()).filter(|k| !k.is_empty());
    // Entering the endpoint the session already uses is not a move.
    let host = host
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty() && harness_llm::base_url_from_host(h) != current_base);

    if host.is_none() && key.is_none() {
        println!(
            "  {}",
            ui.dim("skipped — nothing changed. Run /auth anytime to set a provider or key.")
        );
        return Ok(());
    }

    let base_url = host
        .as_deref()
        .map(harness_llm::base_url_from_host)
        .unwrap_or_else(|| current_base.to_string());
    let shown_host = harness_llm::host_from_base_url(&base_url);

    // The endpoint override is non-secret: it lands in connection.json, where
    // the desktop Settings and the next CLI start both pick it up.
    if host.is_some() {
        match harness_runtime::connection::set_oxen_host(&base_url) {
            Ok(()) => println!(
                "  {} {}",
                ui.green(&format!("🌐 ✓ provider set to {base_url}")),
                ui.dim("— saved for future sessions and the desktop app."),
            ),
            Err(e) => println!(
                "  {} {}",
                ui.green(&format!("🌐 ✓ provider set to {base_url} for this session")),
                ui.dim(&format!("(couldn't persist it: {e})")),
            ),
        }
    }

    // Persist an entered key to `.env` + this process (the same store the
    // desktop app uses); without one, run with whatever already resolves for
    // the (possibly new) host — env, `.env`, or the `oxen` CLI login.
    let resolved_key = match &key {
        Some(k) => {
            let persisted = harness_runtime::connection::set_oxen_key(k);
            match persisted {
                Ok(()) => println!(
                    "  {} {}",
                    ui.green("🔑 ✓ saved"),
                    ui.dim(&format!(
                        "authenticated with {shown_host} — this chat picks it up on the next message."
                    )),
                ),
                Err(e) => println!(
                    "  {} {}",
                    ui.green("🔑 ✓ set for this session"),
                    ui.dim(&format!("(couldn't persist it: {e})")),
                ),
            }
            Some(k.clone())
        }
        None => {
            let resolved = harness_llm::auth::resolve_api_key_for_base_url(&base_url).ok();
            if resolved.is_none() {
                println!(
                    "  {} {}",
                    ui.red("⚠"),
                    ui.dim(&format!(
                        "no API key found for {shown_host} — run /auth to add one."
                    )),
                );
            }
            resolved
        }
    };

    // Swap a client carrying the new endpoint/key into the running agent —
    // same model, same transcript — and carry it to the fleet spawner too, so
    // spawn_agents lanes launched after this don't keep using the old one.
    let client = OxenClient::new(&base_url, resolved_key.unwrap_or_default(), agent.model());
    agent.set_client(client.clone());
    crate::endpoint::update_fleet_endpoint(Some(&client), None);
    Ok(())
}

/// The clean `host[:port]` a host entry resolves to (handles full URLs too).
fn display_host(host: &str) -> String {
    harness_llm::host_from_base_url(&harness_llm::base_url_from_host(host))
}

/// Offer the masked key card at startup, when no API key resolves at all (so
/// the session couldn't even connect). On save the key is persisted the same
/// way `/auth` does; the caller then builds its client with the returned key.
/// Returns `None` when skipped/cancelled or stdin isn't a terminal.
pub(crate) fn prompt_for_missing_key(ui: &Ui, base_url: &str) -> Option<String> {
    let host = harness_llm::host_from_base_url(base_url);
    println!();
    println!(
        "  {} {}",
        ui.accent("🔑"),
        ui.cream(&format!("No API key found for {host}."))
    );
    let key = read_masked_key(ui, &host)
        .ok()
        .and_then(CardInput::entered)?;
    match harness_runtime::connection::set_oxen_key(&key) {
        Ok(()) => println!("  {}", ui.green("✓ saved — setting out on the trail…")),
        Err(e) => println!(
            "  {} {}",
            ui.green("✓ set for this session"),
            ui.dim(&format!("(couldn't persist it: {e})")),
        ),
    }
    Some(key)
}

/// A dim one-line nudge to run `/auth`, shown under an authentication failure —
/// `Some` only when `err` looks like an Oxen 401.
pub(crate) fn auth_hint(ui: &Ui, err: &str) -> Option<String> {
    is_auth_error(err).then(|| {
        format!(
            "  {}",
            ui.dim("Your API key is missing or invalid — run /auth to set it.")
        )
    })
}

/// Whether an agent error message reads as an authentication failure.
pub(crate) fn is_auth_error(err: &str) -> bool {
    err.contains("(401)") || err.to_lowercase().contains("must be authenticated")
}

/// Read a provider base URL through the shared input card (unmasked — endpoints
/// aren't secrets), pre-filled with the current one so Enter just accepts it.
fn read_base_url(ui: &Ui, current_base: &str) -> io::Result<CardInput> {
    let help = [
        "Enter keeps it, or edit it — a bare host like localhost:3001 works too.".to_string(),
        "Any OpenAI-compatible endpoint, e.g. https://openrouter.ai/api/v1 \
         or https://api.openai.com/v1"
            .to_string(),
    ];
    card_input(
        ui,
        &CardInputSpec {
            header: "Authenticate",
            title: "Where should requests go?",
            help: &help,
            prompt: "url ❯",
            initial: current_base,
            mask: false,
            collapse_label: "🌐 Provider:",
        },
    )
}

/// Read an API key with masked echo through the shared input card
/// ([`crate::picker::card_input`]): every character renders as `•`, and the
/// collapsed echo line reports only the entered length — the key never lands
/// in the terminal scrollback.
fn read_masked_key(ui: &Ui, host: &str) -> io::Result<CardInput> {
    let title = format!("Paste your API key for {host}");
    // Only Oxen servers have a /settings page to point at; other providers
    // (OpenRouter, OpenAI, …) just get told where the key is stored.
    let help = [if host.ends_with("oxen.ai") {
        format!("Get one at https://{host}/settings — stored in ~/.oxen-harness/.env")
    } else if host.starts_with("localhost") || host.starts_with("127.0.0.1") {
        format!("Get one at http://{host}/settings — stored in ~/.oxen-harness/.env")
    } else {
        "Stored in ~/.oxen-harness/.env".to_string()
    }];
    card_input(
        ui,
        &CardInputSpec {
            header: "Authenticate",
            title: &title,
            help: &help,
            prompt: "key ❯",
            initial: "",
            mask: true,
            collapse_label: "🔑 API key:",
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_errors_are_recognized() {
        assert!(is_auth_error(
            "Oxen API error (401): You must be authenticated to perform this action."
        ));
        assert!(is_auth_error("You must be authenticated to do that"));
        assert!(!is_auth_error("Oxen API error (500): boom"));
        assert!(!is_auth_error("connection refused"));
    }

    #[test]
    fn hint_only_fires_on_auth_errors() {
        let ui = Ui::plain();
        assert!(auth_hint(&ui, "Oxen API error (401): nope").is_some());
        assert!(auth_hint(&ui, "Oxen API error (429): slow down").is_none());
    }

    #[test]
    fn args_parse_into_their_forms() {
        assert_eq!(parse_args(None), AuthArgs::Interactive);
        assert_eq!(
            parse_args(Some("sk-abc123".into())),
            AuthArgs::Key("sk-abc123".into())
        );
        assert_eq!(parse_args(Some("host".into())), AuthArgs::HostPrompt);
        assert_eq!(
            parse_args(Some("host localhost:3001".into())),
            AuthArgs::Host("localhost:3001".into())
        );
        assert_eq!(
            parse_args(Some("hub.oxen.ai sk-abc".into())),
            AuthArgs::HostAndKey("hub.oxen.ai".into(), "sk-abc".into())
        );
    }

    #[test]
    fn a_bare_endpoint_reads_as_a_host_not_a_key() {
        // Forgetting the `host` keyword must never overwrite the saved key.
        for arg in [
            "localhost:3001",
            "127.0.0.1:8080",
            "https://openrouter.ai/api/v1",
            "openrouter.ai/api/v1",
        ] {
            assert_eq!(
                parse_args(Some(arg.into())),
                AuthArgs::Host(arg.into()),
                "{arg} should read as a host"
            );
        }
        // Keys (JWTs contain dots, never colons or slashes) still read as keys.
        for arg in ["sk-abc123", "eyJhbGciOi.eyJzdWIi.SflKxwRJ"] {
            assert_eq!(
                parse_args(Some(arg.into())),
                AuthArgs::Key(arg.into()),
                "{arg} should read as a key"
            );
        }
    }

    #[test]
    fn host_entries_normalize_for_display() {
        assert_eq!(display_host("hub.oxen.ai"), "hub.oxen.ai");
        assert_eq!(display_host("localhost:3001"), "localhost:3001");
        assert_eq!(
            display_host("http://localhost:3001/api/ai"),
            "localhost:3001"
        );
    }
}
