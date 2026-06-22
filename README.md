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
| `harness-tools` | Built-in tools: read/write/edit files, glob find, regex search, sandboxed shell, git status/diff/log/commit |
| `harness-store` | SQLite history (verbatim) + JSONL export for fine-tuning |
| `harness-agent` | The agent (Ralph) loop, wiring the LLM, tools, and store together |
| `harness-cli` | The `oxen-harness` interactive REPL binary |

A cross-platform [Tauri v2](https://v2.tauri.app/) desktop app lives in [`app/`](app/) (a separate project, excluded from this workspace) and reuses `harness-agent`. It needs the Tauri CLI, which is **not** bundled with cargo — install it once, then run from `app/`:

```bash
cargo install tauri-cli --version "^2" --locked   # provides `cargo tauri`
cd app
OXEN_API_KEY=sk-... cargo tauri dev
```

See [`app/README.md`](app/README.md) for the npm-based alternative (`npm install && npm run dev`) and platform webview prerequisites.

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
| Base URL | `--base-url` flag, `OXEN_BASE_URL` env, or `--host`/`OXEN_HOST` (expanded) | `https://hub.oxen.ai/api/ai` |
| Model | `--model` flag | `claude-opus-4-8` |
| Resume | `--resume <SESSION_ID>` flag (id printed on the death screen) | new session |

### Pointing at a different Oxen host

To use a local or self-hosted Oxen server, override the base URL — by host or full URL:

```bash
# Convenience: just the host[:port] (http is used for local hosts, /api/ai appended)
oxen-harness --host localhost:3001
OXEN_HOST=localhost:3001 oxen-harness

# Explicit full base URL (any scheme/path)
oxen-harness --base-url http://localhost:3001/api/ai
OXEN_BASE_URL=http://localhost:3001/api/ai oxen-harness
```

Precedence: `--base-url` > `--host` > `OXEN_BASE_URL` > `OXEN_HOST` > default. The
API key is looked up by the resolved host (e.g. `localhost:3001`), so `OXEN_API_KEY`
or an `oxen` CLI login for that host works automatically. The desktop app honors
the `OXEN_BASE_URL` / `OXEN_HOST` env vars.

## On the trail (the CLI experience)

The REPL borrows its structure from modern coding CLIs (a welcome panel, an
in-place status spinner, transparent tool lines) and its *voice* from the 1980s
**Oregon Trail** game — because oxen pull the wagons on the trail, and Oxen.ai
powers this one. While the model thinks or a tool runs, you'll see trail-flavored
status lines animate in place ("Fording the river…", "Yoking the oxen…", "Sizing
up the situation…"), tools show what they're doing, and errors are reported as
the classic "You have died of dysentery."

```
🐂 trail ❯ add a test for the parser
✶  Caulking the wagon to float across…  (3s)
  ◆ Reading the trail guide  read_file(src/parser.rs)
  └─ 142 lines forded.
```

The menu mirrors the game's title screen (`/help`):

```
You may:
  1. Travel the trail        — just type what you want done
  2. Learn about the trail   — /help
  3. See the Oregon Top Ten  — /export [path]  (save the journey as JSONL)
  4. Trade your oxen         — /model [name]
  5. Make camp / End         — /exit  (or Ctrl-D)
```

Assistant responses are rendered as **streaming Markdown** — headings, **bold**,
*italics*, `inline code`, bullet/numbered lists, blockquotes, links, and fenced
code blocks are formatted live, line by line, as tokens arrive. GFM tables are
buffered and drawn as aligned, box-drawn grids (respecting `:--`/`--:`/`:-:`
column alignment) instead of leaking raw pipes.

### Resuming an expedition

Every session is saved to `~/.oxen-harness/history.sqlite`. When you quit (Ctrl-C,
Ctrl-D, or `/exit`), the tombstone screen engraves your session id and the command
to pick the trail back up:

```
  Your trail journal was saved. Resume this expedition with:
    oxen-harness --resume 8f3c… 
```

Resuming restores the full transcript (so the model keeps its memory) along with
that session's working directory and model. Override either with `--workspace` or
`--model` if you've moved camp.

Color uses 24-bit ANSI and is auto-disabled when output isn't a TTY, when
`NO_COLOR` is set, or for `TERM=dumb`, so piped/redirected output stays clean.

## Development

This project is built with **The Ralph Wiggum loop** — a tight, objective-check-driven cycle where tests and files (not conversation) hold state. See [`AGENTS.md`](AGENTS.md) for the full contributor protocol, and the knowledge base ([`DOCUMENT-MAP.md`](DOCUMENT-MAP.md)) for project context.

## License

[Apache-2.0](LICENSE).
