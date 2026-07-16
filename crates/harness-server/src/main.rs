//! `oxen-harness-server` — run the agent backend as a standalone HTTP server.
//!
//! ```sh
//! oxen-harness-server [--port 4770] [--host 127.0.0.1] [--token <secret>] [--project <dir>]
//! ```
//!
//! Binds 127.0.0.1 by default (single user, local). Without `--token` a
//! random one is generated and printed at startup. Configuration (connection,
//! models, tools, skills, permissions) comes from the same `~/.oxen-harness`
//! the CLI and desktop app use.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use rand::Rng;

const DEFAULT_PORT: u16 = 4770;

struct Args {
    host: IpAddr,
    port: u16,
    token: Option<String>,
    project: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        host: [127, 0, 0, 1].into(),
        port: DEFAULT_PORT,
        token: None,
        project: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = |name: &str| {
            it.next()
                .ok_or_else(|| format!("{name} requires a value"))
        };
        match flag.as_str() {
            "--port" => args.port = value("--port")?.parse().map_err(|e| format!("bad --port: {e}"))?,
            "--host" => args.host = value("--host")?.parse().map_err(|e| format!("bad --host: {e}"))?,
            "--token" => args.token = Some(value("--token")?),
            "--project" => args.project = Some(PathBuf::from(value("--project")?)),
            "--help" | "-h" => {
                println!(
                    "oxen-harness-server [--port {DEFAULT_PORT}] [--host 127.0.0.1] \
                     [--token <secret>] [--project <dir>]"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown flag {other}")),
        }
    }
    Ok(args)
}

fn random_token() -> String {
    const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::thread_rng();
    (0..32)
        .map(|_| CHARS[rng.gen_range(0..CHARS.len())] as char)
        .collect()
}

#[tokio::main]
async fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    };

    // Load ~/.oxen-harness/.env so saved API keys reach the environment
    // before any agent or tool reads them — same startup the desktop does.
    harness_config::secrets::load();
    let _ = harness_runtime::connection::load();

    // Report a crash from the previous run, then arm the handler for this one
    // — same crash detection every front end gets (see harness-crash).
    if let Ok(marker) = harness_config::paths::last_crash_file() {
        if let Some(signal) = harness_crash::arm(&marker) {
            let log = harness_config::paths::errors_log().ok();
            harness_agent::errlog::record(
                log.as_deref(),
                "crashed",
                serde_json::json!({ "signal": signal }),
            );
            eprintln!("note: the previous run crashed ({signal}) — see errors.jsonl");
        }
    }

    let token = args.token.clone().unwrap_or_else(random_token);
    let addr = SocketAddr::new(args.host, args.port);
    let project = args.project.clone();
    let config = harness_server::ServerConfig {
        token: token.clone(),
        configure: Some(Box::new(move |mut builder| {
            if let Some(project) = project {
                builder = builder.active_project(project);
            }
            builder
        })),
    };

    let handle = match harness_server::serve(addr, config).await {
        Ok(handle) => handle,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };

    println!("oxen-harness server listening on {}", handle.base_url());
    println!("  events : GET  {}/v1/events?token={token}", handle.base_url());
    println!("  api    : Authorization: Bearer {token}");
    if args.token.is_none() {
        println!("  (token generated for this run; pass --token to pin one)");
    }

    // Run until interrupted; dropping the handle stops the listener.
    let _ = tokio::signal::ctrl_c().await;
    drop(handle);
}
