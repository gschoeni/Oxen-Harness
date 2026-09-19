fn main() {
    // tauri_build embeds the app/window icons through `generate_context!`, which
    // caches them under a content hash in OUT_DIR and never tells cargo about
    // the source files. Without this, editing icons/* leaves a stale icon in
    // dev builds until something else forces a recompile.
    println!("cargo:rerun-if-changed=icons");
    tauri_build::build();
}
