//! Real generations on this machine, through the same service the tools
//! use. Ignored by default: they need the weights and runtime
//! (`oxen-harness media pull <id>`) and take minutes.
//!
//!   cargo test -p harness-media-local --test real_models -- --ignored --nocapture

use std::sync::{Arc, Mutex};
use std::time::Instant;

use harness_media::{LocalJob, LocalMedia, LocalProgress, MediaKind};
use serde_json::json;

fn job(id: &str, model: &str, prompt: &str, params: serde_json::Value) -> LocalJob {
    LocalJob {
        id: id.into(),
        model: model.into(),
        kind: MediaKind::Image,
        prompt: prompt.into(),
        params: params.as_object().cloned().unwrap_or_default(),
        refs: vec![],
    }
}

fn out_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("harness-media-local-real");
    std::fs::create_dir_all(&dir).expect("out dir");
    dir
}

#[tokio::test]
#[ignore = "runs real models; needs `oxen-harness media pull`"]
async fn every_installed_model_renders_and_reports_progress() {
    let service = harness_media_local::service().expect("service");
    let installed: Vec<String> = service
        .models()
        .into_iter()
        .filter(|m| m.local.as_ref().is_some_and(|l| l.installed))
        .map(|m| m.id)
        .collect();
    assert!(!installed.is_empty(), "pull a model first");
    for id in installed {
        let steps = Arc::new(Mutex::new(Vec::new()));
        let heard = steps.clone();
        let started = Instant::now();
        let out = service
            .generate(
                job(
                    &format!("local-real-{}", id.replace('/', "-")),
                    &id,
                    "A lighthouse on a cliff at dusk, a hand-painted sign reading \"OXEN\"",
                    json!({"aspect_ratio": "1:1", "seed": 42}),
                ),
                Arc::new(move |p| heard.lock().expect("heard").push(p)),
            )
            .await
            .unwrap_or_else(|e| panic!("{id}: {e}"));
        let secs = started.elapsed().as_secs_f64();
        assert!(out.bytes.starts_with(b"\x89PNG"), "{id} returned a PNG");
        let (w, h) = image_size(&out.bytes);
        assert_eq!((w, h), (1024, 1024), "{id}");
        let steps = steps.lock().expect("steps").clone();
        assert!(
            steps
                .iter()
                .any(|p| matches!(p, LocalProgress::Step { .. })),
            "{id} reported sampling steps: {steps:?}"
        );
        let path = out_dir().join(format!("{}.png", id.replace('/', "-")));
        std::fs::write(&path, &out.bytes).expect("write");
        println!(
            "{id}: {secs:.1}s, warm={}, → {}",
            out.record["warm"],
            path.display()
        );
    }
    service.unload_all().await;
}

#[tokio::test]
#[ignore = "runs real models; needs `oxen-harness media pull local/flux-2-klein-4b`"]
async fn klein_edits_a_reference_image() {
    let service = harness_media_local::service().expect("service");
    let source = out_dir().join("source.png");
    let made = service
        .generate(
            job(
                "local-real-src",
                "local/flux-2-klein-4b",
                "A red fox sleeping in fresh snow, wildlife photograph",
                json!({"width": 768, "height": 768, "seed": 1}),
            ),
            Arc::new(|_| {}),
        )
        .await
        .expect("source image");
    std::fs::write(&source, &made.bytes).expect("write source");
    let mut edit = job(
        "local-real-edit",
        "local/flux-2-klein-4b",
        "Make it nighttime with the northern lights in the sky above reference image 1",
        json!({"width": 768, "height": 768, "seed": 2}),
    );
    edit.refs = vec![source.clone()];
    let started = Instant::now();
    let out = service
        .generate(edit, Arc::new(|_| {}))
        .await
        .expect("edit");
    assert_eq!(out.record["references"], 1);
    let path = out_dir().join("edit.png");
    std::fs::write(&path, &out.bytes).expect("write");
    println!(
        "klein edit: {:.1}s → {}",
        started.elapsed().as_secs_f64(),
        path.display()
    );
    service.unload_all().await;
}

/// Width and height from a PNG's IHDR chunk.
fn image_size(png: &[u8]) -> (u32, u32) {
    let be = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    (be(&png[16..20]), be(&png[20..24]))
}
