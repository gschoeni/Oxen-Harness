//! The `User-Agent` every outbound request carries, so the hub (and anyone
//! else we call) can tell harness traffic apart and see which front end and
//! release sent it: `ox-harness/0.0.7 (cli; macos; aarch64)`.
//!
//! The product token deliberately avoids the word "oxen": the hub treats any
//! agent containing it (case-insensitively) as the Oxen CLI, switching its
//! auth handling and version-gating the request with an HTTP 426.

use std::sync::OnceLock;

/// The product token every request leads with.
pub const PRODUCT: &str = "ox-harness";

static CLIENT_NAME: OnceLock<&'static str> = OnceLock::new();

/// Name the front end this process is (`cli`, `desktop`, `server`), so its
/// requests say where they came from. Call once at startup, before any
/// client is built; the first name set wins.
pub fn set_client_name(name: &'static str) {
    CLIENT_NAME.get_or_init(|| name);
}

/// The `User-Agent` for this process.
pub fn user_agent() -> String {
    format_user_agent(CLIENT_NAME.get().copied())
}

fn format_user_agent(client: Option<&str>) -> String {
    let version = env!("CARGO_PKG_VERSION");
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    match client {
        Some(client) => format!("{PRODUCT}/{version} ({client}; {os}; {arch})"),
        None => format!("{PRODUCT}/{version} ({os}; {arch})"),
    }
}

/// A [`reqwest::ClientBuilder`] that already sends [`user_agent`], for
/// callers that need more settings (timeouts, redirects).
pub fn client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder().user_agent(user_agent())
}

/// A plain [`reqwest::Client`] that sends [`user_agent`].
pub fn client() -> reqwest::Client {
    client_builder()
        .build()
        // The builder only fails if the TLS backend can't initialize, and then
        // the default client can't either; this keeps constructors infallible
        // the same way `reqwest::Client::new` does.
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_product_version_and_client() {
        let ua = format_user_agent(Some("desktop"));
        assert!(
            ua.starts_with(&format!(
                "ox-harness/{} (desktop; ",
                env!("CARGO_PKG_VERSION")
            )),
            "{ua}"
        );
        assert!(
            ua.ends_with(&format!("{})", std::env::consts::ARCH)),
            "{ua}"
        );
        assert!(!format_user_agent(None).contains("()"));
    }

    #[test]
    fn never_reads_as_the_oxen_cli() {
        for client in [None, Some("cli"), Some("desktop"), Some("server")] {
            let ua = format_user_agent(client);
            assert!(
                !ua.to_lowercase().contains("oxen"),
                "hub would 426 this: {ua}"
            );
        }
    }

    #[tokio::test]
    async fn clients_send_it() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/")
            .match_header("user-agent", user_agent().as_str())
            .create_async()
            .await;
        client().get(server.url()).send().await.expect("send");
        mock.assert_async().await;
    }
}
