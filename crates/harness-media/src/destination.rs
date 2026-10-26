//! Where on the hub generations are stored: which `namespace/repo` the queue
//! writes them to (`target_namespace` / `target_repo`), what the key may
//! pick from, and whether a choice will work.
//!
//! The hub's rules (`MediaGeneration.authorize_destination`): a generation
//! may go to any repository — existing or not, it is created on first use —
//! in the key's own namespace or in an organization where the user is an
//! `admin` or `editor`; or to an *existing* repository anywhere the user has
//! been added to with write access (repo role `admin` or `editor`). An
//! organization pays for what lands in it.
//!
//! The endpoints, under the hub's `/api` root:
//!
//! - `GET /users/me` → `{ user: { username } }`
//! - `GET /users/{username}/orgs` → `{ organizations: [{ name, access_level }] }`
//! - `GET /repos/{namespace}?page=&page_size=` → `{ repositories: [{ name }], total_pages }`
//! - `GET /user/repos?page=&page_size=` → repositories the user was added to
//! - `GET /repos/{namespace}/{name}` → `{ access_level, repository: { namespace, name } }`

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::hub::{body, send, HubError, HubRepos};

/// Page size for repository listings (the hub caps it at 100).
const PAGE_SIZE: u32 = 100;
/// A namespace with more repositories than this is listed in part; the
/// rest can still be typed by name.
const MAX_PAGES: u32 = 10;

/// Roles that may create repositories in an organization, and that may
/// write to a repository the user was added to.
const WRITE_ROLES: &[&str] = &["owner", "admin", "editor"];

/// How the key's user relates to a namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NamespaceKind {
    /// The key's own account.
    User,
    /// An organization where the user may create repositories.
    Org,
    /// Someone else's namespace holding repositories the user was added to:
    /// only those existing repositories are on offer.
    Shared,
}

impl NamespaceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Org => "org",
            Self::Shared => "shared",
        }
    }
}

/// A namespace generations can go to, with the repositories in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubNamespace {
    pub name: String,
    pub kind: NamespaceKind,
    /// The user's organization role (`admin`, `editor`); `None` otherwise.
    pub role: Option<String>,
    /// Whether a new repository name works here (the hub creates it).
    pub can_create: bool,
    /// Repository names, sorted case-insensitively.
    pub repos: Vec<String>,
}

/// A destination that will work, in the hub's own casing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DestinationCheck {
    pub namespace: String,
    pub repo: String,
    /// False when the hub will create the repository on the first generation.
    pub exists: bool,
}

/// Why a destination won't work.
#[derive(Debug, thiserror::Error)]
pub enum DestinationError {
    #[error("`{0}` is not a repository: use namespace/name (letters, digits, `-`, `_`, `.`)")]
    Malformed(String),
    #[error(
        "you can't write to `{namespace}/{repo}` (your access: {access}); \
         it needs the admin or editor role"
    )]
    ReadOnly {
        namespace: String,
        repo: String,
        access: String,
    },
    #[error("`{namespace}/{repo}` doesn't exist, and new repositories can only go in {allowed}")]
    CannotCreate {
        namespace: String,
        repo: String,
        allowed: String,
    },
    #[error(transparent)]
    Hub(#[from] HubError),
}

/// Parse `namespace/name` (surrounding whitespace and slashes tolerated;
/// nested paths, empty halves, and other characters are rejected).
pub fn parse_destination(raw: &str) -> Option<(String, String)> {
    let raw = raw.trim().trim_matches('/');
    let (ns, name) = raw.split_once('/')?;
    let valid = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    (valid(ns) && valid(name)).then(|| (ns.to_string(), name.to_string()))
}

impl HubRepos {
    /// Every namespace the key can send generations to, own account first,
    /// then organizations, then namespaces it only has shared repositories in.
    pub async fn destinations(&self) -> Result<Vec<HubNamespace>, HubError> {
        let username = self.whoami().await?;
        let mut out = vec![HubNamespace {
            name: username.clone(),
            kind: NamespaceKind::User,
            role: None,
            can_create: true,
            repos: Vec::new(),
        }];
        for (name, role) in self.organizations(&username).await? {
            if WRITE_ROLES.contains(&role.as_str()) {
                out.push(HubNamespace {
                    name,
                    kind: NamespaceKind::Org,
                    role: Some(role),
                    can_create: true,
                    repos: Vec::new(),
                });
            }
        }
        for ns in &mut out {
            ns.repos = self
                .paged_repos(&format!("repos/{}", ns.name))
                .await?
                .into_iter()
                .map(|(_, name)| name)
                .collect();
        }

        let mut shared: Vec<HubNamespace> = Vec::new();
        for (namespace, name) in self.paged_repos("user/repos").await? {
            if let Some(own) = out
                .iter_mut()
                .find(|n| n.name.eq_ignore_ascii_case(&namespace))
            {
                own.repos.push(name);
            } else if let Some(other) = shared
                .iter_mut()
                .find(|n| n.name.eq_ignore_ascii_case(&namespace))
            {
                other.repos.push(name);
            } else {
                shared.push(HubNamespace {
                    name: namespace,
                    kind: NamespaceKind::Shared,
                    role: None,
                    can_create: false,
                    repos: vec![name],
                });
            }
        }
        shared.sort_by_key(|n| n.name.to_lowercase());
        out.extend(shared);
        for ns in &mut out {
            ns.repos.sort_by_key(|r| r.to_lowercase());
            ns.repos.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        }
        Ok(out)
    }

    /// Whether generations can go to `raw` (`namespace/name`): an existing
    /// repository the user can write to, or a new name in a namespace where
    /// the hub will create it.
    pub async fn check_destination(&self, raw: &str) -> Result<DestinationCheck, DestinationError> {
        let (namespace, repo) =
            parse_destination(raw).ok_or_else(|| DestinationError::Malformed(raw.trim().into()))?;
        if let Some((namespace, repo, access)) = self.repo_access(&namespace, &repo).await? {
            return if WRITE_ROLES.contains(&access.as_str()) {
                Ok(DestinationCheck {
                    namespace,
                    repo,
                    exists: true,
                })
            } else {
                Err(DestinationError::ReadOnly {
                    namespace,
                    repo,
                    access,
                })
            };
        }
        let username = self.whoami().await?;
        let mut creatable = vec![username.clone()];
        creatable.extend(
            self.organizations(&username)
                .await?
                .into_iter()
                .filter(|(_, role)| WRITE_ROLES.contains(&role.as_str()))
                .map(|(name, _)| name),
        );
        match creatable
            .iter()
            .find(|ns| ns.eq_ignore_ascii_case(&namespace))
        {
            Some(ns) => Ok(DestinationCheck {
                namespace: ns.clone(),
                repo,
                exists: false,
            }),
            None => Err(DestinationError::CannotCreate {
                namespace,
                repo,
                allowed: creatable
                    .iter()
                    .map(|ns| format!("`{ns}`"))
                    .collect::<Vec<_>>()
                    .join(", "),
            }),
        }
    }

    /// The user's organizations as `(name, role)`.
    async fn organizations(&self, username: &str) -> Result<Vec<(String, String)>, HubError> {
        let v: Value = self
            .get_json(&format!("users/{username}/orgs"), &[])
            .await?;
        Ok(v.get("organizations")
            .and_then(Value::as_array)
            .map(|orgs| {
                orgs.iter()
                    .filter_map(|o| {
                        let name = o.get("name")?.as_str()?.to_string();
                        let role = o
                            .get("access_level")
                            .and_then(Value::as_str)
                            .unwrap_or("viewer")
                            .to_string();
                        Some((name, role))
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// `(namespace, name)` of every repository a paged listing returns, up
    /// to [`MAX_PAGES`] pages.
    async fn paged_repos(&self, path: &str) -> Result<Vec<(String, String)>, HubError> {
        let mut out = Vec::new();
        for page in 1..=MAX_PAGES {
            let v: Value = self
                .get_json(
                    path,
                    &[
                        ("page", page.to_string()),
                        ("page_size", PAGE_SIZE.to_string()),
                    ],
                )
                .await?;
            let rows = v
                .get("repositories")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default();
            out.extend(rows.iter().filter_map(|r| {
                Some((
                    r.get("namespace")?.as_str()?.to_string(),
                    r.get("name")?.as_str()?.to_string(),
                ))
            }));
            let pages = v.get("total_pages").and_then(Value::as_u64).unwrap_or(1);
            if u64::from(page) >= pages || rows.is_empty() {
                break;
            }
        }
        Ok(out)
    }

    /// `(namespace, name, access_level)` of an existing repository the key
    /// can see, in the hub's casing; `None` when it doesn't exist (or is
    /// hidden from this key).
    async fn repo_access(
        &self,
        namespace: &str,
        repo: &str,
    ) -> Result<Option<(String, String, String)>, HubError> {
        let res = send(
            self.http()
                .get(format!("{}/repos/{namespace}/{repo}", self.api_root()))
                .bearer_auth(self.api_key())
                .timeout(Duration::from_secs(30)),
            true,
        )
        .await?;
        if matches!(res.status().as_u16(), 403 | 404) {
            return Ok(None);
        }
        let v: Value = body(res).await?;
        let found = v.get("repository");
        let field = |key: &str, fallback: &str| {
            found
                .and_then(|r| r.get(key))
                .and_then(Value::as_str)
                .unwrap_or(fallback)
                .to_string()
        };
        let access = v
            .get("access_level")
            .and_then(Value::as_str)
            .unwrap_or("none")
            .to_string();
        Ok(Some((
            field("namespace", namespace),
            field("name", repo),
            access,
        )))
    }

    async fn get_json(&self, path: &str, query: &[(&str, String)]) -> Result<Value, HubError> {
        let res = send(
            self.http()
                .get(format!("{}/{path}", self.api_root()))
                .query(query)
                .bearer_auth(self.api_key())
                .timeout(Duration::from_secs(30)),
            true,
        )
        .await?;
        body(res).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Matcher;
    use serde_json::json;

    fn repos(server: &mockito::Server) -> HubRepos {
        HubRepos::new(
            reqwest::Client::new(),
            format!("{}/api", server.url()),
            "key",
        )
    }

    async fn me(server: &mut mockito::Server, orgs: Value) -> Vec<mockito::Mock> {
        vec![
            server
                .mock("GET", "/api/users/me")
                .with_body(json!({"user": {"username": "greg"}}).to_string())
                .create_async()
                .await,
            server
                .mock("GET", "/api/users/greg/orgs")
                .with_body(json!({ "organizations": orgs }).to_string())
                .create_async()
                .await,
        ]
    }

    #[test]
    fn parses_namespace_slash_name_only() {
        assert_eq!(
            parse_destination(" /ox/art-1.v2/ "),
            Some(("ox".into(), "art-1.v2".into()))
        );
        for bad in ["ox", "ox/", "/art", "ox/a/b", "ox/a b", "o x/a", ""] {
            assert_eq!(parse_destination(bad), None, "{bad}");
        }
    }

    #[tokio::test]
    async fn destinations_list_own_writable_orgs_and_shared_repos() {
        let mut server = mockito::Server::new_async().await;
        let _me = me(
            &mut server,
            json!([
                {"name": "Studio", "access_level": "editor"},
                {"name": "Watchers", "access_level": "viewer"},
            ]),
        )
        .await;
        let _own = server
            .mock("GET", "/api/repos/greg")
            .match_query(Matcher::UrlEncoded("page".into(), "1".into()))
            .with_body(
                json!({"repositories": [{"namespace": "greg", "name": "playground"}], "total_pages": 1})
                    .to_string(),
            )
            .create_async()
            .await;
        let _org = server
            .mock("GET", "/api/repos/Studio")
            .match_query(Matcher::Any)
            .with_body(
                json!({"repositories": [{"namespace": "Studio", "name": "shots"}], "total_pages": 1})
                    .to_string(),
            )
            .create_async()
            .await;
        // Two pages of shared repos; one lands in an org already listed.
        let _p1 = server
            .mock("GET", "/api/user/repos")
            .match_query(Matcher::UrlEncoded("page".into(), "1".into()))
            .with_body(
                json!({"repositories": [
                    {"namespace": "ox", "name": "playground"},
                    {"namespace": "studio", "name": "Archive"},
                ], "total_pages": 2})
                .to_string(),
            )
            .create_async()
            .await;
        let _p2 = server
            .mock("GET", "/api/user/repos")
            .match_query(Matcher::UrlEncoded("page".into(), "2".into()))
            .with_body(
                json!({"repositories": [{"namespace": "ox", "name": "Demo"}], "total_pages": 2})
                    .to_string(),
            )
            .create_async()
            .await;

        let got = repos(&server).destinations().await.unwrap();
        let summary: Vec<_> = got
            .iter()
            .map(|n| (n.name.as_str(), n.kind, n.can_create, n.repos.clone()))
            .collect();
        assert_eq!(
            summary,
            vec![
                (
                    "greg",
                    NamespaceKind::User,
                    true,
                    vec!["playground".to_string()]
                ),
                (
                    "Studio",
                    NamespaceKind::Org,
                    true,
                    vec!["Archive".to_string(), "shots".to_string()]
                ),
                (
                    "ox",
                    NamespaceKind::Shared,
                    false,
                    vec!["Demo".to_string(), "playground".to_string()]
                ),
            ]
        );
        assert_eq!(got[1].role.as_deref(), Some("editor"));
    }

    #[tokio::test]
    async fn an_existing_repo_needs_write_access() {
        let mut server = mockito::Server::new_async().await;
        let _w = server
            .mock("GET", "/api/repos/OX/Playground")
            .with_body(
                json!({"access_level": "editor", "repository": {"namespace": "ox", "name": "playground"}})
                    .to_string(),
            )
            .create_async()
            .await;
        let _r = server
            .mock("GET", "/api/repos/ox/public-data")
            .with_body(
                json!({"access_level": "viewer", "repository": {"namespace": "ox", "name": "public-data"}})
                    .to_string(),
            )
            .create_async()
            .await;
        let r = repos(&server);
        assert_eq!(
            r.check_destination("OX/Playground").await.unwrap(),
            DestinationCheck {
                namespace: "ox".into(),
                repo: "playground".into(),
                exists: true
            }
        );
        let err = r.check_destination("ox/public-data").await.unwrap_err();
        assert!(matches!(err, DestinationError::ReadOnly { .. }), "{err}");
        assert!(err.to_string().contains("viewer"), "{err}");
    }

    #[tokio::test]
    async fn a_new_repo_is_allowed_only_where_the_hub_will_create_it() {
        let mut server = mockito::Server::new_async().await;
        let _me = me(
            &mut server,
            json!([
                {"name": "Studio", "access_level": "admin"},
                {"name": "Watchers", "access_level": "viewer"},
            ]),
        )
        .await;
        let _missing = server
            .mock("GET", Matcher::Regex(r"^/api/repos/[^/]+/fresh$".into()))
            .with_status(404)
            .create_async()
            .await;
        let r = repos(&server);
        assert_eq!(
            r.check_destination("studio/fresh").await.unwrap(),
            DestinationCheck {
                namespace: "Studio".into(),
                repo: "fresh".into(),
                exists: false
            }
        );
        assert!(!r.check_destination("greg/fresh").await.unwrap().exists);
        let err = r.check_destination("Watchers/fresh").await.unwrap_err();
        assert!(
            err.to_string().contains("`greg`, `Studio`"),
            "names where it could go: {err}"
        );
    }

    #[tokio::test]
    #[ignore = "live hub; OXEN_API_KEY=… cargo test -p harness-media live_destinations -- --ignored --nocapture"]
    async fn live_destinations_list_and_check() {
        let key = std::env::var("OXEN_API_KEY").expect("OXEN_API_KEY");
        let hub = HubRepos::connect(harness_core::DEFAULT_BASE_URL, key);
        let namespaces = hub.destinations().await.unwrap();
        for ns in &namespaces {
            println!(
                "{} {:?} create={} repos={}",
                ns.name,
                ns.kind,
                ns.can_create,
                ns.repos.len()
            );
        }
        let own = &namespaces[0];
        assert_eq!(own.kind, NamespaceKind::User);
        let fresh = hub
            .check_destination(&format!("{}/harness-destination-check", own.name))
            .await
            .unwrap();
        println!("{fresh:?}");
        if let Some(shared) = namespaces.iter().find(|n| n.kind == NamespaceKind::Shared) {
            if let Some(repo) = shared.repos.first() {
                println!(
                    "{:?}",
                    hub.check_destination(&format!("{}/{repo}", shared.name))
                        .await
                );
            }
        }
    }

    #[tokio::test]
    async fn malformed_names_never_reach_the_hub() {
        let server = mockito::Server::new_async().await;
        let err = repos(&server)
            .check_destination("just-a-name")
            .await
            .unwrap_err();
        assert!(matches!(err, DestinationError::Malformed(_)), "{err}");
    }
}
