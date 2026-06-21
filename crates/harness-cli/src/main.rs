//! `oxen-harness` command-line entry point.
//!
//! Phase 5 turns this into the interactive, streaming REPL. For now it prints
//! a banner so the workspace produces a runnable binary end to end.

use harness_core::{DEFAULT_BASE_URL, DEFAULT_MODEL};

fn main() -> anyhow::Result<()> {
    println!("oxen-harness {}", env!("CARGO_PKG_VERSION"));
    println!("provider : Oxen.ai ({DEFAULT_BASE_URL})");
    println!("model    : {DEFAULT_MODEL}");
    println!("status   : scaffolding (Phase 0) — interactive REPL lands in Phase 5");
    Ok(())
}
