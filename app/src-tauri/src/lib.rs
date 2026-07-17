//! Tauri desktop bridge for oxen-harness.
//!
//! Exposes the agent loop to the web UI: the `run_turn` command drives
//! [`harness_agent::Agent`], emitting `agent://token` and `agent://tool` events
//! as the turn streams, and returning the assistant's final text. The agent is
//! initialized lazily on first use so the window always opens, even without an
//! API key configured.
//!
//! The crate is a thin shell over four concerns:
//!
//! - [`state`] — [`AppState`] and the per-session agent lifecycle: build,
//!   resume, cache, evict. Everything that touches an agent goes through it.
//! - [`bridges`] — the host↔agent bridges that surface agent capabilities
//!   (`ask_user_question`, `canvas`, fleet lanes) as webview events.
//! - [`events`] — every payload emitted to the webview, in one place so the
//!   wire format the frontend parses is auditable at a glance.
//! - [`commands`] — the `#[tauri::command]` handlers, one module per feature
//!   area; see its docs for how to add a command.
//!
//! This file only wires them together: [`run`] builds the Tauri app, seeds
//! [`AppState`] from the persisted selections, registers every command, and
//! shuts the local model server down on exit.

use std::path::PathBuf;

use tauri::{Emitter, Manager, RunEvent};

mod browser;
mod commands;
mod events;
mod preview;
#[cfg(target_os = "macos")]
mod snapshot;
mod state;

use commands::project::read_projects_config;
use state::{launch_dir, AppState};

/// Entry point shared by the binary and mobile targets.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Load ~/.oxen-harness/.env so saved API keys reach the environment before
    // any agent or tool reads them, then migrate any legacy plaintext keys out
    // of connection.json into the .env.
    harness_config::secrets::load();
    let _ = harness_runtime::connection::load();
    // Report a crash from the previous run to the developer error log, then
    // arm the fatal-signal handler for this one (see harness-crash).
    if let Ok(marker) = harness_config::paths::last_crash_file() {
        if let Some(signal) = harness_crash::arm(&marker) {
            let log = harness_config::paths::errors_log().ok();
            harness_agent::errlog::record(
                log.as_deref(),
                "crashed",
                serde_json::json!({ "signal": signal }),
            );
        }
    }
    // Start in the last active project (or the launch directory on first run).
    let initial_project = read_projects_config()
        .active
        .map(PathBuf::from)
        .unwrap_or_else(launch_dir);
    // Start on the model the user last chose: the selected cloud model, plus any
    // persisted local model (its server is started lazily on first use). Both are
    // restored so the dropdown choice survives a restart.
    let initial_model = harness_runtime::models::selected();
    let initial_local = harness_runtime::models::active_local();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        // The main webview IS the app: navigating it to a clicked link would
        // replace the entire UI with that page, with no way back. The frontend
        // intercepts link clicks (see app/src/lib/links.ts); this guard
        // backstops anything that slips through by cancelling the navigation
        // and handing the URL to the link-browser side panel instead. Child
        // webviews (preview-*, the browser pane) enforce their own policies in
        // their per-webview handlers and pass through here.
        .plugin(
            tauri::plugin::Builder::<tauri::Wry>::new("nav-guard")
                .on_navigation(|webview, url| {
                    if webview.label() != "main" {
                        return true;
                    }
                    // The app's own origins: the bundled tauri:// origin in
                    // production, the Vite dev server (loopback) in dev.
                    let own = url.scheme() == "tauri"
                        || matches!(
                            url.host_str(),
                            Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
                        );
                    if !own {
                        let _ = webview.app_handle().emit(
                            "browser://open",
                            events::BrowserOpenPayload {
                                url: url.to_string(),
                            },
                        );
                    }
                    own
                })
                .build(),
        )
        // The shared session service needs the app handle (its event sink and
        // native-preview hooks emit into this window), so state is wired in
        // setup — after the handle exists, before any command can run.
        .setup(move |app| {
            app.manage(AppState::new(
                app.handle().clone(),
                initial_project,
                initial_model,
                initial_local,
            ));
            app.manage(commands::watch::FsWatchState::default());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::turn::run_turn,
            commands::turn::cancel_turn,
            commands::review::run_code_review,
            commands::review::get_code_review_config,
            commands::review::save_code_review_config,
            commands::review::default_code_review_config,
            commands::loops::list_loops,
            commands::loops::loops_path,
            commands::loops::get_loop,
            commands::loops::save_loop,
            commands::loops::import_loop,
            commands::loops::export_loop,
            commands::loops::remove_loop,
            commands::loops::run_loop,
            commands::session::session_info,
            commands::session::list_sessions,
            commands::session::session_messages,
            commands::session::set_review_status,
            commands::session::set_review_status_many,
            commands::session::delete_session,
            commands::session::attachment_data_uri,
            commands::tools::tool_definitions,
            commands::tools::list_tools,
            commands::tools::add_custom_tool,
            commands::tools::remove_custom_tool,
            commands::tools::set_tool_enabled,
            commands::tools::set_tool_description,
            commands::connection::get_compression_mode,
            commands::connection::set_compression_mode,
            commands::session::total_tokens_saved,
            commands::skills::list_skills,
            commands::skills::save_skill,
            commands::skills::delete_skill,
            commands::skills::set_skill_enabled,
            commands::session::export_finetuning,
            commands::session::total_tokens_used,
            commands::session::total_cost_usd,
            commands::session::model_usage_breakdown,
            commands::session::session_cost,
            commands::session::daily_usage,
            commands::session::new_session,
            commands::session::resume_session,
            commands::browser::browser_attach,
            commands::browser::browser_detach,
            commands::browser::browser_close,
            commands::browser::browser_reload,
            commands::browser::open_external,
            commands::preview::preview_attach,
            commands::preview::preview_detach,
            commands::preview::preview_reload,
            commands::preview::preview_stop,
            commands::preview::preview_open_external,
            commands::preview::preview_status,
            commands::preview::preview_statuses,
            commands::preview::preview_restart,
            commands::preview::get_preview_prefs,
            commands::preview::set_preview_auto_verify,
            commands::files::fs_list_dir,
            commands::files::fs_read_file,
            commands::files::fs_write_file,
            commands::files::fs_create_entry,
            commands::watch::fs_watch,
            commands::watch::fs_unwatch,
            commands::project::list_projects,
            commands::project::open_project,
            commands::project::start_project,
            commands::project::update_project,
            commands::project::add_project_context,
            commands::project::remove_project_context,
            commands::project::set_active_project,
            commands::project::get_default_project_location,
            commands::project::set_default_project_location,
            commands::connection::get_connection,
            commands::connection::set_connection,
            commands::connection::configure_brave_key,
            commands::connection::configure_oxen_key,
            commands::turn::retry_turn,
            commands::models::installed_local_models,
            commands::models::install_llama,
            commands::models::detect_hardware,
            commands::models::runtime_status,
            commands::models::install_runtime,
            commands::models::list_model_catalog,
            commands::models::resolve_hf_model,
            commands::models::search_hf_models,
            commands::models::search_oxen_models,
            commands::models::hf_token_present,
            commands::models::set_hf_token,
            commands::models::download_model,
            commands::models::remove_model,
            commands::models::use_local_model,
            commands::models::list_cloud_models,
            commands::models::add_cloud_model,
            commands::models::remove_cloud_model,
            commands::models::set_model,
            commands::models::select_cloud_model_for_new_chats,
            commands::turn::answer_question,
            commands::turn::answer_approval,
            commands::permissions::get_permissions,
            commands::permissions::set_permission_mode,
            commands::permissions::add_permission_rule,
            commands::permissions::remove_permission_rule,
            commands::theme::list_themes,
            commands::theme::active_theme,
            commands::theme::use_theme,
            commands::theme::import_theme,
            commands::theme::export_theme,
            commands::theme::remove_theme,
            commands::theme::new_theme,
            commands::theme::theme_location,
            commands::theme::set_theme_location
        ])
        .build(tauri::generate_context!())
        .expect("error while building oxen-harness desktop app")
        .run(|app, event| {
            // The local `llama-server` runs as a separate child process. On a
            // normal quit (Cmd+Q, window close, app menu) drop it so it doesn't
            // linger after the app is gone — dropping the `LocalServer` kills the
            // child (it spawned with `kill_on_drop`). A SIGKILL of the app itself
            // can't be intercepted, so that case can still orphan the server.
            // `ExitRequested` fires before `Exit`; both are idempotent here
            // (the server slot and the dev-server map are drained the first
            // time), and handling both means an `Exit` without a preceding
            // request still cleans up.
            if let RunEvent::ExitRequested { .. } | RunEvent::Exit = event {
                let state = app.state::<AppState>();
                tauri::async_runtime::block_on(async {
                    state.local_server.lock().await.take();
                    // Dev servers are children too — never outlive the app.
                    state.dev_servers.stop_all().await;
                });
            }
        });
}
