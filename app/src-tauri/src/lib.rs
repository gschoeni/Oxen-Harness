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

use tauri::{Manager, RunEvent};
use tokio::sync::Mutex;

mod bridges;
mod commands;
mod events;
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
        .manage(AppState {
            active_project: Mutex::new(initial_project),
            cloud_model: Mutex::new(initial_model),
            local_model: Mutex::new(initial_local),
            ..AppState::default()
        })
        .invoke_handler(tauri::generate_handler![
            commands::turn::run_turn,
            commands::turn::cancel_turn,
            commands::review::run_code_review,
            commands::review::get_code_review_config,
            commands::review::save_code_review_config,
            commands::review::default_code_review_config,
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
            commands::session::new_session,
            commands::session::resume_session,
            commands::project::list_projects,
            commands::project::open_project,
            commands::project::set_active_project,
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
            commands::models::hf_token_present,
            commands::models::set_hf_token,
            commands::models::download_model,
            commands::models::remove_model,
            commands::models::use_local_model,
            commands::models::list_cloud_models,
            commands::models::add_cloud_model,
            commands::models::remove_cloud_model,
            commands::models::set_model,
            commands::turn::answer_question,
            commands::theme::list_themes,
            commands::theme::active_theme,
            commands::theme::use_theme,
            commands::theme::import_theme,
            commands::theme::export_theme,
            commands::theme::remove_theme,
            commands::theme::new_theme
        ])
        .build(tauri::generate_context!())
        .expect("error while building oxen-harness desktop app")
        .run(|app, event| {
            // The local `llama-server` runs as a separate child process. On a
            // normal quit (Cmd+Q, window close, app menu) drop it so it doesn't
            // linger after the app is gone — dropping the `LocalServer` kills the
            // child (it spawned with `kill_on_drop`). A SIGKILL of the app itself
            // can't be intercepted, so that case can still orphan the server.
            if let RunEvent::ExitRequested { .. } | RunEvent::Exit = event {
                let state = app.state::<AppState>();
                tauri::async_runtime::block_on(async {
                    state.local_server.lock().await.take();
                });
            }
        });
}
