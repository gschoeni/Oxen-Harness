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
mod cli_open;
mod commands;
mod events;
#[cfg(target_os = "macos")]
mod menu;
mod preview;
#[cfg(target_os = "macos")]
mod snapshot;
mod state;
mod view_packages;

use commands::project::read_projects_config;
use state::{launch_dir, AppState};

/// Whether the main webview may perform a navigation itself, or must bounce
/// it to the link-browser pane.
///
/// The webview reports every navigation, not just the top-level one: an
/// `<iframe srcDoc>` the UI renders (a canvas document, the editor's HTML
/// preview) arrives as `about:srcdoc`, and an object URL the UI minted as
/// `blob:`. Those never leave the app, so they are the app's own, along with
/// its origins: the bundled `tauri://` origin in production and the Vite dev
/// server (loopback) in dev. Anything else is a page to show in the pane.
fn main_webview_keeps(url: &tauri::Url) -> bool {
    matches!(url.scheme(), "tauri" | "about" | "blob")
        || matches!(
            url.host_str(),
            Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
        )
}

/// Entry point shared by the binary and mobile targets.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() -> Result<(), tauri::Error> {
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
    // A directory passed on the command line (`oxen-harness ui <dir>`) becomes
    // the active project before state is built, so a cold start opens straight
    // into it. Otherwise start in the last active project (or the launch
    // directory on first run).
    let cli_dir = cli_open::dir_from_args(
        std::env::args(),
        &std::env::current_dir().unwrap_or_default(),
    );
    if let Some(dir) = &cli_dir {
        let _ = commands::project::remember_project(dir);
    }
    let initial_project = cli_dir
        .map(PathBuf::from)
        .or_else(|| read_projects_config().active.map(PathBuf::from))
        .unwrap_or_else(launch_dir);
    // Start on the selected cloud model. A previously picked local model is
    // deliberately NOT restored: loading one commits a model's worth of memory
    // (17 GB for a 30B Q4) the moment the window opens, before the user has
    // asked for anything. Local models load only when picked in this run.
    let initial_model = harness_runtime::models::selected();
    // A previous run that was killed (rather than quit) left its llama-server
    // behind, still holding that memory. Reap any such orphan before we
    // could add to it. Ours from this run is not started yet, so this only
    // ever touches servers whose owning host is gone.
    let _ = harness_local::reap_stale_servers();
    let builder = tauri::Builder::default()
        // First, so no other plugin runs in a doomed second instance: when the
        // app is already open, a new launch (`oxen-harness ui <dir>`) forwards
        // its argv + cwd to the running instance and exits.
        .plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
            cli_open::open_from_second_instance(app, &argv, &cwd);
        }))
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
                    let own = main_webview_keeps(url);
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
        );
    // macOS gets a menu bar whether asked or not (Tauri's default when none is
    // set), and that default claims ⌘W for Close Window. Ours leaves the key
    // to the chat tab strip. Other platforms keep no menu bar, as before.
    #[cfg(target_os = "macos")]
    let builder = builder.menu(menu::app_menu);
    builder
        // The shared session service needs the app handle (its event sink and
        // native-preview hooks emit into this window), so state is wired in
        // setup — after the handle exists, before any command can run.
        .setup(move |app| {
            app.manage(AppState::new(
                app.handle().clone(),
                initial_project,
                initial_model,
            ));
            // A cold start's `--open <surface>` waits for the UI to ask.
            if let Some(surface) = cli_open::surface_from_args(std::env::args()) {
                *app.state::<AppState>()
                    .launch_surface
                    .lock()
                    .expect("launch surface poisoned") = Some(surface);
            }
            app.manage(commands::watch::FsWatchState::default());
            Ok(())
        })
        .register_uri_scheme_protocol("viewasset", view_packages::protocol)
        .invoke_handler(|invoke: tauri::ipc::Invoke<tauri::Wry>| {
            let label = invoke.message.webview_ref().label();
            if !view_packages::command_allowed(label, invoke.message.command()) {
                invoke
                    .resolver
                    .reject("This webview has no access to application commands");
                return true;
            }
            let handler: fn(tauri::ipc::Invoke<tauri::Wry>) -> bool = tauri::generate_handler![
                view_packages::view_packages_request,
                view_packages::view_package_mount,
                view_packages::view_package_move,
                view_packages::view_package_close,
                view_packages::view_bridge,
                commands::workbench::workbench_request,
                commands::turn::run_turn,
                commands::turn::cancel_turn,
                commands::turn::cancel_fleet,
                commands::turn::cancel_agent,
                commands::turn::interject_agent,
                commands::turn::list_tasks,
                commands::turn::kill_background_task,
                commands::study::study_profile,
                commands::study::study_batch,
                commands::study::study_answer,
                commands::study::get_model_roles,
                commands::study::set_model_role,
                commands::review::run_code_review,
                commands::review::get_code_review_config,
                commands::review::save_code_review_config,
                commands::review::default_code_review_config,
                commands::session::session_info,
                commands::session::list_sessions,
                commands::session::list_agents,
                commands::session::agent_patch,
                commands::session::follow_up_agent,
                commands::session::apply_agent_patch,
                commands::session::session_messages,
                commands::session::set_review_status,
                commands::session::set_review_status_many,
                commands::session::delete_session,
                commands::session::attachment_path,
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
                commands::session::import_sources_scan,
                commands::session::import_external,
                commands::session::total_tokens_used,
                commands::session::total_cost_usd,
                commands::session::model_usage_breakdown,
                commands::session::session_cost,
                commands::session::session_tree_usage,
                commands::session::daily_usage,
                commands::session::new_session,
                commands::session::resume_session,
                commands::threads::threads_snapshot,
                commands::threads::session_finish,
                commands::threads::session_reopen,
                commands::threads::session_mark_seen,
                commands::threads::rename_session,
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
                commands::media::list_media,
                commands::media::cancel_media,
                commands::media::get_media_prefs,
                commands::media::set_media_prefs,
                commands::media::list_media_models,
                commands::files::fs_list_dir,
                commands::files::fs_read_file,
                commands::files::fs_create_entry,
                commands::files::git_status,
                commands::files::git_diff,
                commands::files::fs_asset_path,
                commands::files::stage_dropped_file,
                commands::watch::fs_watch,
                commands::watch::fs_unwatch,
                commands::project::list_projects,
                commands::project::take_launch_surface,
                commands::project::open_project,
                commands::project::start_project,
                commands::project::update_project,
                commands::project::delete_project,
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
                commands::turn::deliver_pending,
                commands::rules::list_rules,
                commands::rules::list_rule_suggestions,
                commands::rules::save_rules,
                commands::rules::check_rule_pattern,
                commands::models::installed_local_models,
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
                commands::models::cancel_download,
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
                commands::theme::set_theme_location,
                commands::ui::load_ui_state,
                commands::ui::feature_flags,
                commands::ui::save_ui_state
            ];
            handler(invoke)
        })
        .build(tauri::generate_context!())?
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::main_webview_keeps;
    use tauri::Url;

    #[test]
    fn frames_the_ui_renders_stay_in_the_main_webview() {
        for own in [
            "about:srcdoc",
            "about:blank",
            "blob:tauri://localhost/2b8d0c9e-0000-4000-8000-000000000000",
            "tauri://localhost/index.html",
            "http://localhost:1430/",
            "http://127.0.0.1:1430/src/main.tsx",
        ] {
            assert!(main_webview_keeps(&Url::parse(own).unwrap()), "{own}");
        }
    }

    /// Attachments the agent keeps live under `.oxen-harness/attachments/`,
    /// and the Files dock shows dotfiles too. Tauri's scope globs skip
    /// dot-components on Unix unless told otherwise, and `**` alone left every
    /// attachment thumbnail broken on macOS.
    #[test]
    fn asset_protocol_serves_files_under_dot_directories() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let scope = &conf["app"]["security"]["assetProtocol"]["scope"];
        assert_eq!(scope["requireLiteralLeadingDot"], false, "{scope}");
        assert_eq!(scope["allow"][0], "**", "{scope}");
    }

    #[test]
    fn web_pages_go_to_the_link_browser() {
        for page in [
            "https://docs.oxen.ai/",
            "http://example.com/",
            "mailto:g@oxen.ai",
        ] {
            assert!(!main_webview_keeps(&Url::parse(page).unwrap()), "{page}");
        }
    }
}
