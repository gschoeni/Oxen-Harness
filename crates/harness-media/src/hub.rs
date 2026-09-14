//! The hub's repository API, for the few calls the harness makes about
//! repositories themselves (not files in them): who the key belongs to,
//! whether `namespace/name` exists, and creating one.
//!
//! Shared by the media uploader (which needs the account's `playground`) and
//! the `create_repository` tool (which gives a project its own remote). The
//! endpoints, all under the hub's `/api` root:
//!
//! - `GET /users/me` → `{ user: { username } }`
//! - `GET /repos/{namespace}/{name}` → 2xx when it exists and the key can
//!   see it, 403/404 otherwise
//! - `POST /repos/` `{ name, namespace?, description, visibility }` →
//!   creates under `namespace` (a user or an organization the key's user
//!   belongs to); without one, under the key's own user

use std::time::Duration;

use serde_json::Value;

pub use crate::upload::UploadError as HubError;

/// One hub, one key.
#[derive(Clone)]
pub struct HubRepos {
    http: reqwest::Client,
    api_root: String,
    api_key: String,
}

/// What to create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRepo<'a> {
    /// The repository name (no namespace).
    pub name: &'a str,
    /// The user or organization to create it under; `None` = the key's user.
    pub namespace: Option<&'a str>,
    pub description: &'a str,
    pub public: bool,
    /// Mark it a workbench repo (the hub's own uploads convention).
    pub workbench: bool,
}

impl HubRepos {
    /// `api_root` is the hub's `/api` root (see
    /// [`crate::queue::api_root_from_base`]). The client must not send a
    /// User-Agent containing "oxen" (see [`crate::USER_AGENT`]).
    pub fn new(
        http: reqwest::Client,
        api_root: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        Self {
            http,
            api_root: api_root.into(),
            api_key: api_key.into(),
        }
    }

    /// [`HubRepos::new`] from the inference base URL (`…/api/ai`) or the
    /// `/api` root, with a client whose User-Agent the hub won't mistake for
    /// the Oxen CLI.
    pub fn connect(base_url: &str, api_key: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(crate::USER_AGENT)
            .build()
            .unwrap_or_default();
        Self::new(http, crate::queue::api_root_from_base(base_url), api_key)
    }

    /// The username this API key belongs to.
    pub async fn whoami(&self) -> Result<String, HubError> {
        let res = self
            .http
            .get(format!("{}/users/me", self.api_root))
            .bearer_auth(&self.api_key)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| HubError::Http(e.to_string()))?;
        let v: Value = body(res).await?;
        v.get("user")
            .and_then(|u| u.get("username"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| HubError::Json("users/me carried no username".into()))
    }

    /// Whether `namespace/name` exists and this key can see it (a 403 reads
    /// as "not for this key", not as an error — another namespace may be).
    pub async fn repo_exists(&self, namespace: &str, name: &str) -> Result<bool, HubError> {
        let res = self
            .http
            .get(format!("{}/repos/{namespace}/{name}", self.api_root))
            .bearer_auth(&self.api_key)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| HubError::Http(e.to_string()))?;
        match res.status().as_u16() {
            200..=299 => Ok(true),
            401 => Err(HubError::Unauthorized),
            403 | 404 => Ok(false),
            other => {
                let text = res.text().await.unwrap_or_default();
                Err(HubError::Api {
                    status: other,
                    message: format!("checking {namespace}/{name}: {}", error_message(&text)),
                })
            }
        }
    }

    /// Create a repository. The hub answers with the created repository's
    /// JSON; only success matters here.
    pub async fn create_repo(&self, repo: NewRepo<'_>) -> Result<(), HubError> {
        let mut json = serde_json::json!({
            "name": repo.name,
            "description": repo.description,
            "visibility": if repo.public { "public" } else { "private" },
        });
        if let Some(ns) = repo.namespace.map(str::trim).filter(|ns| !ns.is_empty()) {
            json["namespace"] = Value::String(ns.to_string());
        }
        if repo.workbench {
            json["workbench"] = Value::Bool(true);
        }
        let res = self
            .http
            .post(format!("{}/repos/", self.api_root))
            .bearer_auth(&self.api_key)
            .json(&json)
            .timeout(Duration::from_secs(60))
            .send()
            .await
            .map_err(|e| HubError::Http(e.to_string()))?;
        body::<Value>(res).await.map(drop)
    }
}

/// Parse a hub response: 401 → [`HubError::Unauthorized`], any other
/// non-2xx → [`HubError::Api`] with the hub's own message, else the JSON.
pub(crate) async fn body<T: serde::de::DeserializeOwned>(
    res: reqwest::Response,
) -> Result<T, HubError> {
    let status = res.status();
    let text = res
        .text()
        .await
        .map_err(|e| HubError::Http(e.to_string()))?;
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(HubError::Unauthorized);
    }
    if !status.is_success() {
        return Err(HubError::Api {
            status: status.as_u16(),
            message: error_message(&text),
        });
    }
    serde_json::from_str(&text).map_err(|e| {
        HubError::Json(format!(
            "{e} (body: {})",
            text.chars().take(200).collect::<String>()
        ))
    })
}

/// The human-readable reason in a hub error body (`{"error": "..."}` or
/// `{"error": {"detail"|"message"|"title"|"type": "..."}}`), else the raw
/// text, clipped.
pub(crate) fn error_message(text: &str) -> String {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return text.chars().take(300).collect();
    };
    if let Some(err) = v.get("error") {
        if let Some(s) = err.as_str() {
            return s.to_string();
        }
        for key in ["detail", "message", "title", "type"] {
            if let Some(s) = err.get(key).and_then(Value::as_str) {
                return s.to_string();
            }
        }
    }
    text.chars().take(300).collect()
}

/// The web page for `namespace/name` on the hub whose API base is
/// `base_url` (either the `/api/ai` inference base or the `/api` root).
pub fn repo_web_url(base_url: &str, namespace: &str, name: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    let origin = trimmed
        .strip_suffix("/api/ai")
        .or_else(|| trimmed.strip_suffix("/api"))
        .unwrap_or(trimmed);
    format!("{origin}/{namespace}/{name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repos(server: &mockito::Server) -> HubRepos {
        HubRepos::new(reqwest::Client::new(), format!("{}/api", server.url()), "key")
    }

    #[tokio::test]
    async fn whoami_reads_the_username() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/api/users/me")
            .match_header("authorization", "Bearer key")
            .with_body(r#"{"user":{"username":"greg"}}"#)
            .create_async()
            .await;
        assert_eq!(repos(&server).whoami().await.unwrap(), "greg");
    }

    #[tokio::test]
    async fn exists_maps_404_and_403_to_false_and_401_to_unauthorized() {
        let mut server = mockito::Server::new_async().await;
        let _a = server.mock("GET", "/api/repos/ox/a").with_status(200).with_body("{}").create_async().await;
        let _b = server.mock("GET", "/api/repos/ox/b").with_status(404).create_async().await;
        let _c = server.mock("GET", "/api/repos/ox/c").with_status(403).create_async().await;
        let _d = server.mock("GET", "/api/repos/ox/d").with_status(401).create_async().await;
        let r = repos(&server);
        assert!(r.repo_exists("ox", "a").await.unwrap());
        assert!(!r.repo_exists("ox", "b").await.unwrap());
        assert!(!r.repo_exists("ox", "c").await.unwrap());
        assert!(matches!(r.repo_exists("ox", "d").await, Err(HubError::Unauthorized)));
    }

    #[tokio::test]
    async fn create_sends_namespace_and_visibility() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/api/repos/")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::PartialJsonString(r#"{"name":"art","namespace":"ox","visibility":"private"}"#.into()),
            ]))
            .with_status(201)
            .with_body(r#"{"repository":{"name":"art"}}"#)
            .create_async()
            .await;
        repos(&server)
            .create_repo(NewRepo {
                name: "art",
                namespace: Some("ox"),
                description: "d",
                public: false,
                workbench: false,
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn create_surfaces_the_hubs_reason() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/api/repos/")
            .with_status(422)
            .with_body(r#"{"error":{"detail":"name already taken"}}"#)
            .create_async()
            .await;
        let err = repos(&server)
            .create_repo(NewRepo { name: "art", namespace: None, description: "", public: false, workbench: false })
            .await
            .unwrap_err();
        assert!(matches!(err, HubError::Api { status: 422, ref message } if message == "name already taken"), "{err}");
    }

    #[test]
    fn web_url_strips_the_api_suffixes() {
        assert_eq!(repo_web_url("https://hub.oxen.ai/api/ai", "ox", "art"), "https://hub.oxen.ai/ox/art");
        assert_eq!(repo_web_url("https://hub.oxen.ai/api", "ox", "art"), "https://hub.oxen.ai/ox/art");
        assert_eq!(repo_web_url("http://localhost:3001/", "ox", "art"), "http://localhost:3001/ox/art");
    }
}
