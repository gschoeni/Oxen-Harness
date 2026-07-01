# oxen-harness 🐂

An open source, hackable agentic coding harness — like Claude Code or Codex, built in Rust and powered by [Oxen.ai](https://oxen.ai).

`oxen-harness` runs an objective-check-driven agent loop against any model exposed through the Oxen.ai OpenAI-compatible chat completions API, with first-class tool calling for editing code, running commands, and driving git. Every turn is persisted so you can later export your sessions and fine-tune a model on your own coding traces.

> **Status:** Core complete — streaming REPL, Oxen.ai client, sandboxed tools, verbatim SQLite history, the agent loop, local models (llama.cpp), and a scaffolded Tauri desktop app are all built and tested. See [`02-status.md`](02-status.md).

## Why

- **Hackable & open source (Apache-2.0).** A small, readable Rust workspace you can fork and extend.
- **Bring your own model.** Anything with a chat completions endpoint and tool calling — default is `claude-opus-4-8` via Oxen.ai, or run Qwen3 **locally** with llama.cpp (`--local`).
- **Your data, exportable.** Full conversation + tool-call history in SQLite, with a JSONL exporter for fine-tuning.
- **Two front ends.** A `claude`-style interactive CLI first, then a cross-platform [Tauri v2](https://v2.tauri.app/) desktop app.

## Architecture

For the layering, the lifecycle of a turn, and how to extend the harness, see
[`ARCHITECTURE.md`](ARCHITECTURE.md). At a glance, it's a single Cargo workspace
of focused crates:

| Crate | Responsibility |
|-------|----------------|
| `harness-core` | Shared domain types (messages, roles) and defaults |
| `harness-llm` | Oxen.ai chat completions client: tool calling + SSE streaming, lightweight auth |
| `harness-tools` | Built-in tools: read/write/edit files, glob find, regex search, sandboxed shell, git status/diff/log/commit, Brave web search, interactive multiple-choice questions |
| `harness-store` | SQLite history (verbatim) + JSONL export for fine-tuning |
| `harness-local` | Local models: curated Qwen3 GGUF catalog, downloads + disk tracking, `llama-server` launcher |
| `harness-theme` | Configurable, shareable themes (palette + voice): built-ins, TOML/JSON load/save, partial overrides, the active-theme store |
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
| Web search | `BRAVE_API_KEY` env (or `BRAVE_SEARCH_API_KEY`), or `~/.oxen-harness/.env` | always offered; key enables results |
| Local model | `--local <MODEL_ID>` flag (runs llama.cpp instead of a remote endpoint) | remote Oxen.ai |
| Theme | `/theme` in the REPL or `oxen-harness theme use <name>` (persists to `~/.oxen-harness/config.toml`) | Oregon Trail |

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

### Web search (Brave)

The agent can search the web via the [Brave Search API](https://brave.com/search/api/).
The `web_search` tool is always available; without a key a call fails with a
recognizable error that the CLI and desktop app turn into an inline "add your
Brave key" prompt, so you can enable it mid-conversation and retry. The key is
read from the environment, an explicit override, or `~/.oxen-harness/.env`.

```bash
export BRAVE_API_KEY=brave-...   # or BRAVE_SEARCH_API_KEY
oxen-harness
```

You can also paste a key when prompted after a failed search; it's saved to
`~/.oxen-harness/.env` and shared with the desktop app.

### Clarifying questions

When a decision is genuinely ambiguous, the agent can interview you with the
`ask_user_question` tool instead of guessing — mirroring Claude Code's
`AskUserQuestion` (1–4 questions, each with a short header, 2–4 options, and an
optional multi-select). In the CLI this renders an interactive picker: arrow keys
(or `j`/`k`) move, number keys jump, `space` toggles in multi-select, `enter`
confirms, a final row lets you type your own answer, and `esc` cancels. The
desktop app shows the same choices as a question card. Piped/non-interactive
sessions skip the prompt and the agent proceeds with sensible defaults.

## Running models locally (llama.cpp)

Instead of a remote endpoint, `oxen-harness` can run open-weight models on your
own machine via [llama.cpp](https://github.com/ggml-org/llama.cpp)'s
`llama-server`, which speaks the same OpenAI-compatible API — so the agent (and
all its tools) work identically, fully offline, with no API key.

**1. Install `llama-server`** (one time):

```bash
brew install llama.cpp                       # macOS
# Linux/Windows: a release from https://github.com/ggml-org/llama.cpp/releases
# Or point at any build: export LLAMA_SERVER=/path/to/llama-server
```

**2. Browse and download a model.** The curated catalog is the Qwen3 family at
`Q4_K_M` (the consumer sweet spot), from a 0.6B that runs anywhere up to the 32B
and the 30B-A3B mixture-of-experts. Downloads are managed locally so you always
see progress and how much disk each one uses:

```bash
oxen-harness models list            # table of models, sizes, what's downloaded + disk used
oxen-harness models pull qwen3-8b   # download with a live progress bar
oxen-harness models remove qwen3-8b # reclaim the disk
oxen-harness models path            # where GGUFs are stored (~/.oxen-harness/models)
```

**3. Ride it.** `--local` starts `llama-server` for the session (auto-downloading
the model first if needed) and points the agent at it:

```bash
oxen-harness --local qwen3-8b
```

Weights live in `~/.oxen-harness/models/`. Match the model to your hardware
(roughly: 0.6B–4B on a CPU/small GPU, 8B–14B on an 8–12 GB machine, 32B or
30B-A3B on a 24 GB card). The desktop app exposes the same catalog under
**🐂 Local models** — download, see disk usage, and switch models from the UI.

## Theming — make it yours

The whole personality of the harness is a **theme**: a *palette* (named semantic
colors), a *voice* (the prompt, spinner glyphs, "thinking" phrases, per-tool
verbs, exit messages, banner art, labels, and help text), and a *style* (the
desktop app's typography and framing). The CLI and the desktop app both render
from the active theme. Five ship built in: **Oregon Trail** (default, 8-bit
pixel), **Synthwave** (neon outrun grid), **Midnight** (calm IBM Plex),
**New York Times** (broadsheet serif + blackletter masthead), and **Cupertino**
(clean system SF) — and they look genuinely different, not just recolored.

The `[style]` block makes the desktop UI fully themeable: `font_display` /
`font_body` / `font_mono` (any of the bundled faces — PixelHead, PixelRead,
Playfair, Masthead, Orbitron, PlexSans, PlexMono — or a system stack), plus
`radius`, `border_width`, `shadow` (`pixel`/`soft`/`glow`/`none`), `hero`
(`pixel`/`newspaper`/`minimal`), and `scene` (`trail`/`grid`/`none`, the art the
pixel hero draws). Adding a new scene is a one-function drop-in to the scene
registry in `app/src/features/chat/scenes.tsx`.

Themes are a single self-contained TOML file (also readable as JSON), so they're
trivial to **export, import, and share**. Files can be *partial* — override just a
few colors, phrases, or a font and the rest inherits the default. They live in
`~/.oxen-harness/themes/`, and the active one is recorded in
`~/.oxen-harness/config.toml`.

```bash
# Non-interactive (great for dotfiles / sharing)
oxen-harness theme list
oxen-harness theme use Synthwave
oxen-harness theme export Synthwave ./synthwave.toml   # share this file
oxen-harness theme import ./a-friends-theme.toml
oxen-harness theme path
```

Inside the REPL, `/theme` opens an interactive picker; `/theme use <name>`,
`/theme import <path>`, and `/theme export <path>` work too. To **vibe-code** a
brand-new theme, run `/theme new` (optionally with a description): a short
interview asks for the mood, color inspiration, and voice, then the model designs
a complete theme, saves it, and activates it live. The desktop app has the same
controls under **🎨 Theme**, including the generate-by-vibe and import/export flows.

```
🐂 trail ❯ /theme new a cozy autumn cabin, warm ambers and pine
  [Mood]   What overall mood do you want?
  ❯ 1. Cozy & warm   — soft, earthy, inviting
  🎨 Designing your theme with claude-opus-4-8
  ✓ created + activated: Amber Hearth
```

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
  5. Change your colors      — /theme  (select, create, import, export)
  6. Pack the wagon          — /queue add <msg> … then /queue run
  7. Set the wagon rolling   — /loop run [name]  (work until the gate is green)
  8. Make camp / End         — /exit  (or Ctrl-D)
```

### Queuing messages

Instead of waiting for one turn to finish before lining up the next, you can
**stack messages and edit them before they run**.

In an interactive terminal the CLI is **live**: while the agent is thinking,
streaming, or running tools, a composer stays pinned to the bottom row. Keep
typing and press Enter to **stack** each message — the prompt shows the depth
(`[2 queued] ❯ `) — and queued messages **drain automatically, in order**, as
soon as the current turn finishes (you can keep stacking while they run).

Stacked messages render as a **navigable list right above the composer**, each a
one-line preview:

```
  1. write a failing test for the parser
  2. now make it pass, minimally
  3. then refactor for readability
  [3 queued] ❯ ▏
```

Press **↑** to step up into the list (and **↓** to come back down to the
composer); the focused row is highlighted. On a focused row, **Enter** or **e**
opens it for **inline editing** (Enter saves, **Esc** cancels and restores the
original), and **d**, **Delete**, or **Backspace** removes it. The list windows
with an `…(+k more)` indicator when it's taller than the screen allows, and
collapses to just the composer + count on very short terminals. Ctrl-C
interrupts the turn and Ctrl-D (on an empty line) ends the session.

Piped / non-interactive usage is unchanged: it falls back to the classic
blocking prompt, so you still stack and send batches explicitly:

```
/queue add write a failing test for the parser
/queue add now make it pass
/queue                       # list what's stacked
/queue edit 2 now make it pass, minimally
/queue up 2                  # reorder (also: /queue down <n>, /queue rm <n>)
/queue run                   # send them all, in order
```

In the desktop app it's live: keep typing while the agent works and each message
**stacks into a queue above the composer**. Reorder them with ↑/↓, **Edit** any
message inline, or remove it — the next queued message sends automatically as
soon as the current turn finishes.

### Loops (goal → verify → iterate)

A prompt hands the agent an instruction. A **loop** hands it a *job*, a way to
know when the job is done, and a rule for when to give up. Each pass runs:

```
DISCOVER → QUESTION → PLAN → EXECUTE → VERIFY → ITERATE
```

The heart of a loop is **VERIFY** — a gate that can actually *fail* the work, so
the agent makes real progress instead of agreeing with itself on repeat. A loop
also keeps **state** (a journal of what's been tried, fed into the next pass and
saved for resuming) and **stop conditions** (success *and* a hard iteration cap
plus an optional token budget).

Two kinds of gate are supported:

- **Command** — runs a shell command in the workspace; **exit 0 = pass**. The
  strongest, most objective gate (e.g. `cargo test`).
- **Rubric** — a separate, strict checker scores the work 1–10 against your
  criteria and passes only if every score clears a threshold. Used when "done"
  can't be reduced to an exit code (the checker is a fresh turn, so the maker
  isn't grading its own homework).

Run the built-in `default` loop — the exact "make the checks green" gate this
repo runs on itself — straight from the shell, or from inside the REPL:

```bash
# one-shot from the command line (runs, then exits)
oxen-harness loop run default
oxen-harness loop run --goal "make every test in crates/parser pass" --max-iterations 6

# manage your loop library
oxen-harness loop list
oxen-harness loop new            # short interview → saved TOML you can share
oxen-harness loop show green-tests
oxen-harness loop export green-tests ./green.toml
```

```
# inside the REPL
/loop                 # list available loops
/loop run default     # keep working until fmt + clippy + tests are green
/loop goal make the README table render correctly   # ad-hoc, rubric-gated
/loop new             # build + save your own
/loop show default
```

Loops live as shareable TOML under `~/.oxen-harness/loops/` (saving one with a
built-in's name overrides it), and each run's journal is saved alongside so a
later run can pick up where it left off. A few ship built in: `default`,
`green-tests`, and `clean-clippy`.

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

---

<p align="center">Powered by <a href="https://oxen.ai">Oxen.ai</a> 🐂</p>
