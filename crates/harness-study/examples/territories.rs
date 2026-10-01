//! Print the territories a workspace splits into: `cargo run -p harness-study
//! --example territories -- <dir>`. Handy when tuning the split thresholds.

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    for t in harness_study::territories::discover(std::path::Path::new(&root)) {
        println!("{:<40} {:<28} {} files", t.id, t.name, t.files.len());
    }
}
