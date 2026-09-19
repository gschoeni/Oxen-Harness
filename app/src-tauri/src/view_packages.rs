//! Native package surfaces. Child views can invoke only the scoped bridge.

use crate::{preview::Bounds, state::AppState};
use harness_runtime::{
    documents::Documents,
    view_packages::{self, Package},
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};
use tauri::{
    AppHandle, LogicalPosition, LogicalSize, Manager, Webview, WebviewBuilder, WebviewUrl,
};

/// App commands are unavailable to embedded web content. A package's one
/// bridge still verifies its live instance and grants for every request.
pub(crate) fn command_allowed(label: &str, command: &str) -> bool {
    label == "main" || (label.starts_with("view-") && command == "view_bridge")
}

struct Lease {
    session: String,
    package: Package,
    project: Documents,
}
static LEASES: OnceLock<Mutex<HashMap<String, Arc<Lease>>>> = OnceLock::new();
fn leases() -> &'static Mutex<HashMap<String, Arc<Lease>>> {
    LEASES.get_or_init(|| Mutex::new(HashMap::new()))
}
fn lease(label: &str) -> Result<Arc<Lease>, String> {
    leases()
        .lock()
        .map_err(|e| e.to_string())?
        .get(label)
        .cloned()
        .ok_or_else(|| "view instance is closed or unavailable".into())
}

fn still_installed(lease: &Lease) -> Result<(), String> {
    if !view_packages::installed()?
        .iter()
        .any(|p| p.manifest.id == lease.package.manifest.id && p.digest == lease.package.digest)
    {
        return Err("view package was removed or updated; reopen the view".into());
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn view_packages_request(
    app: AppHandle,
    action: String,
    source: Option<String>,
    digest: Option<String>,
    id: Option<String>,
) -> Result<Value, String> {
    match action.as_str() {
        "list" => Ok(json!(view_packages::installed()?)),
        "inspect" => Ok(json!(view_packages::inspect(std::path::Path::new(
            source.as_deref().ok_or("missing source directory")?
        ))?)),
        "install" => {
            let installed = view_packages::install(
                std::path::Path::new(source.as_deref().ok_or("missing source directory")?),
                digest.as_deref().ok_or("missing approved digest")?,
            )
            .await?;
            revoke_instances(&app, &installed.manifest.id)?;
            Ok(json!(installed))
        }
        "remove" => {
            let id = id.as_deref().ok_or("missing package id")?;
            view_packages::remove(id).await?;
            revoke_instances(&app, id)?;
            Ok(Value::Null)
        }
        _ => Err("unknown package operation".into()),
    }
}

fn revoke_instances(app: &AppHandle, id: &str) -> Result<(), String> {
    let labels: Vec<_> = leases()
        .lock()
        .map_err(|e| e.to_string())?
        .iter()
        .filter(|(_, lease)| lease.package.manifest.id == id)
        .map(|(label, _)| label.clone())
        .collect();
    for label in labels {
        view_package_close(app.clone(), label)?;
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn view_package_mount(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    session: String,
    id: String,
    path: Option<String>,
    bounds: Bounds,
    theme: Option<Value>,
) -> Result<String, String> {
    let package = view_packages::installed()?
        .into_iter()
        .find(|p| p.manifest.id == id)
        .ok_or("view package is not installed")?;
    view_packages::assets(&package)?;
    let project = state.documents(&session)?;
    if let Some(path) = &path {
        if !package.manifest.permissions.allows("read", path) {
            return Err(format!("view is not granted access to {path}"));
        }
        project.resolve(path).map_err(|e| e.to_string())?;
    }
    let label = format!("view-{}", uuid::Uuid::new_v4());
    let address = format!(
        "viewasset://localhost/{label}/bundle/{}",
        package.manifest.entry
    );
    let url: tauri::Url = address.parse().map_err(|e| format!("view URL: {e}"))?;
    let context = json!({"session":session,"workspace":project.root(),"theme":theme,"target":{"view":format!("package:{}",package.manifest.id),"path":path}});
    let init = format!(
        "window.__OXEN_VIEW_CONTEXT__={context};\n{}",
        include_str!("../../src/workbench-sdk/browser.js")
    );
    let guard_label = label.clone();
    let builder = WebviewBuilder::new(&label, WebviewUrl::CustomProtocol(url))
        .initialization_script(init)
        .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
        .on_navigation(move |url| {
            let own_scheme = url.scheme() == "viewasset"
                || ((url.scheme() == "http" || url.scheme() == "https")
                    && url.host_str() == Some("viewasset.localhost"));
            own_scheme && url.path().starts_with(&format!("/{guard_label}/bundle/"))
        });
    leases().lock().map_err(|e| e.to_string())?.insert(
        label.clone(),
        Arc::new(Lease {
            session,
            package,
            project,
        }),
    );
    let window = app.get_window("main").ok_or("main window is unavailable")?;
    if let Err(e) = window.add_child(
        builder,
        LogicalPosition::new(bounds.x, bounds.y),
        LogicalSize::new(bounds.width.max(1.0), bounds.height.max(1.0)),
    ) {
        leases().lock().map_err(|e| e.to_string())?.remove(&label);
        return Err(format!("mount view package: {e}"));
    }
    Ok(label)
}

#[tauri::command]
pub(crate) fn view_package_move(
    app: AppHandle,
    label: String,
    bounds: Bounds,
    visible: bool,
) -> Result<(), String> {
    let instance = lease(&label)?;
    still_installed(&instance)?;
    let view = app
        .get_webview(&label)
        .ok_or("native view is unavailable")?;
    view.set_position(LogicalPosition::new(bounds.x, bounds.y))
        .map_err(|e| e.to_string())?;
    view.set_size(LogicalSize::new(
        bounds.width.max(1.0),
        bounds.height.max(1.0),
    ))
    .map_err(|e| e.to_string())?;
    if visible { view.show() } else { view.hide() }.map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn view_package_close(app: AppHandle, label: String) -> Result<(), String> {
    leases().lock().map_err(|e| e.to_string())?.remove(&label);
    if let Some(view) = app.get_webview(&label) {
        view.close().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn view_bridge(
    webview: Webview,
    state: tauri::State<'_, AppState>,
    action: String,
    mut payload: Value,
) -> Result<Value, String> {
    let instance = lease(webview.label())?;
    still_installed(&instance)?;
    let permissions = &instance.package.manifest.permissions;
    let path = payload
        .get("path")
        .and_then(Value::as_str)
        .map(str::to_owned);
    match action.as_str() {
        "read" | "save" | "asset" => {
            let path = path.as_deref().ok_or("missing document path")?;
            if !permissions.allows(&action, path) {
                return Err(format!("view permission denied: {action} {path}"));
            }
            instance.project.resolve(path).map_err(|e| e.to_string())?;
            if action == "asset" {
                let scheme = if cfg!(target_os = "windows") {
                    "http://viewasset.localhost"
                } else {
                    "viewasset://localhost"
                };
                let mut url: tauri::Url = scheme.parse().map_err(|e| format!("asset URL: {e}"))?;
                url.set_path(&format!("/{}/project/{path}", webview.label()));
                return Ok(json!(url.to_string()));
            }
        }
        "open" => {
            if let Some(path) = &path {
                if !permissions.allows("read", path) && !permissions.allows("asset", path) {
                    return Err("opened resource is outside the view grant".into());
                }
            }
        }
        "add_context" => {}
        "run" => {
            if !permissions.action("workflow.run") {
                return Err("this view has no workflow execution grant".into());
            }
            let path = path.as_deref().ok_or("missing workflow path")?;
            if !permissions.allows("read", path) {
                return Err("workflow is outside this view's document grant".into());
            }
            let document = instance.project.read(path).map_err(|e| e.to_string())?;
            if payload
                .get("revision")
                .and_then(Value::as_str)
                .is_some_and(|revision| revision != document.revision)
            {
                return Err("graph changed on disk; reload before running".into());
            }
            let graph = harness_runtime::workflow::Workflow::parse(&document.content)?;
            payload["revision"] = json!(document.revision);
            for node in graph
                .nodes
                .iter()
                .filter(|n| n.kind == "image_input" || n.kind == "video_input")
            {
                if !permissions.allows("asset", node.text("path")) {
                    return Err(format!(
                        "workflow reference is outside this view's asset grant: {}",
                        node.text("path")
                    ));
                }
            }
        }
        "status" | "cancel" => {
            if !permissions.action("workflow.run") {
                return Err("this view has no workflow execution grant".into());
            }
            let engine = state.workbench(&instance.session).await?;
            let run = engine.status(payload["id"].as_str().ok_or("missing run id")?)?;
            if !permissions.allows("read", &run.graph_path) {
                return Err("run is outside this view's document grant".into());
            }
        }
        "latest" => {
            if !permissions.allows("read", path.as_deref().ok_or("missing path")?) {
                return Err("document access denied".into());
            }
        }
        "report" => {
            payload["view"] = json!(format!("package:{}", instance.package.manifest.id));
            if let Some(path) = path {
                if !permissions.allows("read", &path) {
                    return Err("reported path is outside this view's grant".into());
                }
            }
        }
        "list" | "models" => {}
        _ => return Err(format!("view action is not available: {action}")),
    }
    state
        .workbench_request(&instance.session, &action, payload)
        .await
}

pub(crate) fn protocol(
    context: tauri::UriSchemeContext<'_, tauri::Wry>,
    request: tauri::http::Request<Vec<u8>>,
) -> tauri::http::Response<Vec<u8>> {
    let response = (|| -> Result<(Vec<u8>, &'static str), String> {
        let path = percent_encoding::percent_decode_str(request.uri().path())
            .decode_utf8()
            .map_err(|e| e.to_string())?;
        let mut parts = path.trim_start_matches('/').splitn(3, '/');
        let label = parts.next().ok_or("missing view")?;
        let area = parts.next().ok_or("missing asset area")?;
        let relative = parts.next().ok_or("missing asset path")?;
        if context.webview_label() != label {
            return Err("cross-view asset access denied".into());
        }
        let instance = lease(label)?;
        still_installed(&instance)?;
        let file = match area {
            "bundle" => view_packages::assets(&instance.package)?
                .resolve(relative)
                .map_err(|e| e.to_string())?,
            "project" => {
                if !instance
                    .package
                    .manifest
                    .permissions
                    .allows("asset", relative)
                {
                    return Err("asset access denied".into());
                }
                instance
                    .project
                    .resolve(relative)
                    .map_err(|e| e.to_string())?
            }
            _ => return Err("unknown view asset area".into()),
        };
        let metadata = std::fs::metadata(&file).map_err(|e| e.to_string())?;
        if !metadata.is_file() || metadata.len() > 100 * 1024 * 1024 {
            return Err("view asset is not a regular file or exceeds 100 MiB".into());
        }
        let mime = match file.extension().and_then(|e| e.to_str()).unwrap_or("") {
            "html" => "text/html; charset=utf-8",
            "js" | "mjs" => "text/javascript; charset=utf-8",
            "css" => "text/css; charset=utf-8",
            "json" => "application/json",
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "webp" => "image/webp",
            "svg" => "image/svg+xml",
            "mp4" => "video/mp4",
            "webm" => "video/webm",
            "woff2" => "font/woff2",
            _ => "application/octet-stream",
        };
        Ok((std::fs::read(file).map_err(|e| e.to_string())?, mime))
    })();
    let (status, body, mime) = match response {
        Ok((body, mime)) => (200, body, mime),
        Err(error) => (403, error.into_bytes(), "text/plain; charset=utf-8"),
    };
    let mut response = tauri::http::Response::new(body);
    *response.status_mut() = if status == 200 {
        tauri::http::StatusCode::OK
    } else {
        tauri::http::StatusCode::FORBIDDEN
    };
    response
        .headers_mut()
        .insert("Content-Type", tauri::http::HeaderValue::from_static(mime));
    response.headers_mut().insert("Content-Security-Policy",tauri::http::HeaderValue::from_static("default-src 'none'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; media-src 'self' blob:; font-src 'self'; connect-src ipc: http://ipc.localhost; frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'"));
    response.headers_mut().insert(
        "X-Content-Type-Options",
        tauri::http::HeaderValue::from_static("nosniff"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn child_views_cannot_call_application_commands() {
        assert!(command_allowed("main", "workbench_request"));
        assert!(command_allowed("view-test", "view_bridge"));
        for label in ["view-test", "link-browser", "preview-session"] {
            for command in [
                "run_turn",
                "workbench_request",
                "fs_read_file",
                "view_packages_request",
            ] {
                assert!(
                    !command_allowed(label, command),
                    "{label} unexpectedly allowed {command}"
                );
            }
        }
        assert!(!command_allowed("link-browser", "view_bridge"));
    }
}
