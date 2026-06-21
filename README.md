# oxen-harness 🐂

An open source, hackable agentic coding harness — like Claude Code or Codex, built in Rust and powered by [Oxen.ai](https://oxen.ai).

`oxen-harness` runs an objective-check-driven agent loop against any model exposed through the Oxen.ai OpenAI-compatible chat completions API, with first-class tool calling for editing code, running commands, and driving git. Every turn is persisted so you can later export your sessions and fine-tune a model on your own coding traces.

> **Status:** Early development (Phase 0 — scaffold). The interactive REPL, LLM client, tools, and history store land over the next phases. See [`02-status.md`](02-status.md).

## Why

- **Hackable & open source (Apache-2.0).** A small, readable Rust workspace you can fork and extend.
- **Bring your own model.** Anything with a chat completions endpoint and tool calling — default is `claude-opus-4-8` via Oxen.ai.
- **Your data, exportable.** Full conversation + tool-call history in SQLite, with a JSONL exporter for fine-tuning.
- **Two front ends.** A `claude`-style interactive CLI first, then a cross-platform [Tauri v2](https://v2.tauri.app/) desktop app.

## Architecture

A single Cargo workspace of focused crates:

| Crate | Responsibility |
|-------|----------------|
| `harness-core` | Shared domain types (messages, roles) and defaults |
| `harness-llm` | Oxen.ai chat completions client: tool calling + SSE streaming, lightweight auth |
| `harness-tools` | Built-in tools: read/write/edit/search files, sandboxed shell, git status/diff/log/commit |
| `harness-store` | SQLite history (verbatim) + JSONL export for fine-tuning |
| `harness-agent` | The agent (Ralph) loop, wiring the LLM, tools, and store together |
| `harness-cli` | The `oxen-harness` interactive REPL binary |

A `harness-tauri` desktop app is added once the core + CLI loop is stable.

## Requirements

- Rust (stable) via [rustup](https://www.rust-lang.org/tools/install)
- An Oxen.ai API key in `OXEN_API_KEY` — or log in with the [`oxen`](https://docs.oxen.ai/getting-started/cli) CLI, which writes `~/.config/oxen/auth_config.toml` (read automatically)

## Quick start

```bash
# Build everything
cargo build

# Run the CLI (prints a banner during Phase 0)
cargo run -p harness-cli

# Run the verification loop
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run   # or: cargo test
```

## Configuration

| Setting | Source | Default |
|---------|--------|---------|
| API key | `OXEN_API_KEY` env, or `~/.config/oxen/auth_config.toml` (`$OXEN_CONFIG_DIR` to override) | — (required) |
| Base URL | config | `https://hub.oxen.ai/api/ai` |
| Model | config / flag | `claude-opus-4-8` |

## Development

This project is built with **The Ralph Wiggum loop** — a tight, objective-check-driven cycle where tests and files (not conversation) hold state. See [`AGENTS.md`](AGENTS.md) for the full contributor protocol, and the knowledge base ([`DOCUMENT-MAP.md`](DOCUMENT-MAP.md)) for project context.

## License

[Apache-2.0](LICENSE).
