//! A live probe of prompt-cache behavior and model-limit resolution through a
//! real endpoint.
//!
//! Ignored by default — it spends (a fraction of a cent of) real credits and
//! needs an `OXEN_API_KEY`. Point `OXEN_BASE_URL` wherever the deployment
//! under test lives (defaults to the hub):
//!
//! ```sh
//! OXEN_BASE_URL=http://localhost:3001/api/ai OXEN_API_KEY=... \
//!   cargo test -p harness-agent --test live_cache_probe -- --ignored --nocapture
//! ```
//!
//! It warms the model-limits cache from the endpoint's catalog, builds an
//! agent exactly the way the hosts do (catalog-reported context window and
//! reply ceiling, name-derived fallback otherwise), then drives two ordinary
//! turns and prints each model call's request-log entry — the prefix
//! classification and the usage the provider reported. Deployments differ:
//! older proxies report Anthropic-style `prompt_tokens` that *exclude* cached
//! tokens (a fully cached prompt bills ~3), newer ones report the full count
//! plus `prompt_tokens_details.cached_tokens`; either way, a growing
//! `cached_prompt_tokens_used` is the cache working.

use std::sync::Arc;

use harness_agent::{Agent, AgentConfig};
use harness_llm::OxenClient;
use harness_store::{HistoryStore, SessionMeta};
use harness_tools::ToolRegistry;

#[tokio::test]
#[ignore = "live network + real spend; run explicitly with --ignored"]
async fn two_turns_hit_the_prompt_cache_with_catalog_limits() {
    let model = "claude-sonnet-4-6";
    let base_url =
        std::env::var("OXEN_BASE_URL").unwrap_or_else(|_| "https://hub.oxen.ai/api/ai".to_string());
    let client =
        OxenClient::connect(&base_url, model).expect("OXEN_API_KEY (or oxen auth) must resolve");

    // Hermetic home so the probe never touches the real caches, then warm the
    // limits cache from the endpoint's catalog — the same side-effect path
    // every host-triggered catalog fetch takes.
    let home = tempfile::tempdir().unwrap();
    std::env::set_var(harness_config::paths::BASE_DIR_ENV, home.path());
    let token = std::env::var("OXEN_API_KEY").ok();
    harness_local::source::oxen_search_models(&base_url, "", token.as_deref())
        .await
        .expect("catalog fetch should succeed");

    // Resolve limits the way harness-host / the CLI do at agent build.
    let context_window = harness_local::limits::context_window(model);
    let max_output_tokens = harness_local::limits::max_output_tokens(model);
    println!("catalog limits for {model}: ctx={context_window:?} max_out={max_output_tokens:?}");

    let dir = tempfile::tempdir().unwrap();
    let request_log = dir.path().join("requests.jsonl");
    let store = Arc::new(HistoryStore::open_in_memory().unwrap());
    let session = store
        .create_session(&SessionMeta {
            workspace: dir.path().display().to_string(),
            model: model.into(),
            ..Default::default()
        })
        .unwrap();
    let config = AgentConfig {
        model: model.into(),
        context_window,
        max_output_tokens,
        request_log: Some(request_log.clone()),
        ..AgentConfig::default()
    };
    let mut agent = Agent::new(client, ToolRegistry::new(), store, session, config).unwrap();

    // When the endpoint reports a window, the agent must budget against it
    // (not the name-derived fallback path).
    if let Some(ctx) = context_window {
        assert_eq!(
            agent.context_window(),
            ctx,
            "agent should budget against the catalog-reported window"
        );
    }

    let a = agent
        .run_turn("Reply with one word: what is 2+2?", |_| {})
        .await
        .unwrap();
    let b = agent
        .run_turn("And that times 3? One word.", |_| {})
        .await
        .unwrap();
    println!("turn 1: {a}\nturn 2: {b}");

    let log = std::fs::read_to_string(&request_log).unwrap();
    for line in log.lines() {
        let entry: serde_json::Value = serde_json::from_str(line).unwrap();
        println!(
            "call: est_prompt={} prefix={} billed_prompt={} latency_ms={}",
            entry["est_prompt_tokens"],
            entry["prefix"],
            entry["usage"]["prompt_tokens"],
            entry["latency_ms"],
        );
    }
    println!(
        "cached_prompt_tokens_used={} cache_write_tokens_used={}",
        agent.cached_prompt_tokens_used(),
        agent.cache_write_tokens_used()
    );

    // The second turn extends the first — the request log must classify it as
    // append-only (the cache-friendly shape), whatever the provider billed.
    let last: serde_json::Value = serde_json::from_str(log.lines().last().unwrap()).unwrap();
    assert_eq!(last["prefix"], "append_only");

    std::env::remove_var(harness_config::paths::BASE_DIR_ENV);
}
