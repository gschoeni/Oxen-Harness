//! A live probe of the model-limits pipeline against a real endpoint.
//!
//! Ignored by default — it needs a reachable Oxen-compatible endpoint. Point
//! it wherever the catalog changes are deployed and run it explicitly:
//!
//! ```sh
//! OXEN_BASE_URL=http://localhost:3001/api/ai \
//!   cargo test -p harness-local --test live_limits_probe -- --ignored --nocapture
//! ```
//!
//! It fetches the catalog through the same code path every host uses, then
//! asserts the side-effect limits cache is populated and prints what a few
//! well-known models reported — the values agents will budget against.

#[tokio::test]
#[ignore = "live network; run explicitly with --ignored against a reachable endpoint"]
async fn catalog_fetch_populates_the_limits_cache() {
    // Hermetic home so the probe never touches the real cache.
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var(harness_config::paths::BASE_DIR_ENV, tmp.path());

    let base_url =
        std::env::var("OXEN_BASE_URL").unwrap_or_else(|_| "https://hub.oxen.ai/api/ai".to_string());
    let token = std::env::var("OXEN_API_KEY").ok();

    let hits = harness_local::source::oxen_search_models(&base_url, "", token.as_deref())
        .await
        .expect("catalog fetch should succeed");
    println!("{} models listed by {base_url}", hits.len());

    let described: Vec<_> = hits.iter().filter(|h| h.context_length.is_some()).collect();
    println!("{} report a context_length", described.len());
    for h in described.iter().take(8) {
        println!(
            "  {}: ctx={:?} max_out={:?}",
            h.id, h.context_length, h.max_output_tokens
        );
    }
    assert!(
        !described.is_empty(),
        "the endpoint reports no context_length for any model — \
         is this the deployment with the new catalog fields?"
    );

    // The fetch must have refreshed the on-disk cache as a side effect, and
    // the cached values must match what the hits carried — this is exactly
    // what hosts read when they build an agent.
    let sample = described[0];
    assert_eq!(
        harness_local::limits::context_window(&sample.id),
        sample.context_length.map(|v| v as usize),
        "cache should hold {}'s reported window",
        sample.id
    );
    assert_eq!(
        harness_local::limits::max_output_tokens(&sample.id),
        sample.max_output_tokens.map(|v| v as usize),
    );
    println!(
        "cache verified: {} → ctx={:?} max_out={:?}",
        sample.id,
        harness_local::limits::context_window(&sample.id),
        harness_local::limits::max_output_tokens(&sample.id),
    );

    std::env::remove_var(harness_config::paths::BASE_DIR_ENV);
}
