# Contributing to oxen-harness

Thanks for pulling up a wagon. This is a small, layered Rust workspace — you
can hold the whole thing in your head in an afternoon, and the docs below are
laid out to get you there fast.

## Orient yourself

1. [`ARCHITECTURE.md`](ARCHITECTURE.md) — how the crates stack, the lifecycle
   of a turn, and a "to add X, touch Y" table. **Start here.**
2. [`AGENTS.md`](AGENTS.md) — the contributor protocol: the verification loop,
   project conventions, a codebase reading order, and a start-to-finish
   recipe for adding a built-in tool.
3. [`DOCUMENT-MAP.md`](DOCUMENT-MAP.md) — the index of every file, including
   the working knowledge base (`00-project-brief.md`, `02-status.md`,
   `03-decisions.md`, `04-backlog.md`).

## Build and verify

All you need is [Rust](https://www.rust-lang.org/tools/install) (the pinned
toolchain in `rust-toolchain.toml` installs itself on first build):

```bash
cargo run -p harness-cli        # run the CLI

cargo fmt --all -- --check      # the verification loop — all three must pass
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace          # or: cargo nextest run
```

The desktop app under [`app/`](app/README.md) is a separate Cargo project with
its own loop (see [`AGENTS.md`](AGENTS.md#the-checks-verification-loop)).

## What makes a good change here

- **Green checks.** CI runs the same fmt + clippy + test loop on every push.
- **Tests beside the code.** New behavior comes with tests in a
  `#[cfg(test)] mod tests` next to what they cover — they double as usage
  examples.
- **Comments explain intent**, not what the code literally does.
- **Docs move with the code.** If you change how something works, update the
  crate's `//!` header or the README section that describes it; if you make a
  load-bearing decision, add a line to `03-decisions.md` with the *why*.

Not sure where your idea fits? Common extensions (a tool, a skill, a theme, a
model, a config file) each touch exactly one seam — the table in
[`ARCHITECTURE.md`](ARCHITECTURE.md#extending-it) points at it.

## License

By contributing you agree your work is licensed under
[Apache-2.0](LICENSE), the project license.
