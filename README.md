# oxen-harness 🐂

[![CI](https://github.com/gschoeni/oxen-harness/actions/workflows/ci.yml/badge.svg)](https://github.com/gschoeni/oxen-harness/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

An open source, hackable agentic coding harness — like Claude Code or Codex, built in Rust and powered by [Oxen.ai](https://oxen.ai).

`oxen-harness` runs an objective-check-driven agent loop against any model exposed through the Oxen.ai OpenAI-compatible chat completions API, with first-class tool calling for editing code, running commands, and driving git. Every turn is persisted so you can later export your sessions and fine-tune a model on your own coding traces.

> **Status:** Core complete — streaming REPL, Oxen.ai client, sandboxed tools, verbatim SQLite history, the agent loop, local models (llama.cpp), and a scaffolded Tauri desktop app are all built and tested. See [`02-status.md`](02-status.md).

## Why

- **Hackable & open source (Apache-2.0).** A small, readable Rust workspace you can fork and extend.
- **Extend it at three levels, no forking required.** Teach the agent reusable workflows with [skills](#adding-a-skill) (a markdown file — no code), connect your own HTTP endpoints as [custom tools](#adding-a-tool) from the desktop app's Settings, or add a built-in tool in Rust with a [start-to-finish recipe](#adding-a-tool).
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
| `harness-tools` | The `TypedTool` trait + built-in tools: read/write/edit files, glob find, regex search, sandboxed shell, git, Brave web search, interactive questions, canvas documents, plans, skills, and user-defined HTTP tools |
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

## Contributing

New here? This is a small, layered Rust workspace — you can hold the whole thing
in your head in an afternoon. Here's the fast path in.

### Get it running

```bash
git clone https://github.com/gschoeni/oxen-harness && cd oxen-harness
export OXEN_API_KEY=sk-...        # or log in with the `oxen` CLI
cargo run -p harness-cli          # build, then drop into the REPL
```

No key handy? `cargo run -p harness-cli -- --local qwen3-0.6b` runs a tiny model
fully offline (see [Running models locally](#running-models-locally-llamacpp)).

### Where to look first

Read these in order — it follows a single prompt from keypress to reply, and by
the last file the architecture clicks:

1. **[`ARCHITECTURE.md`](ARCHITECTURE.md)** — the map: how the crates stack, the
   lifecycle of a turn, and a table of "to add X, touch Y". **Start here.**
2. **[`crates/harness-tools/src/lib.rs`](crates/harness-tools/src/lib.rs)** — the
   `TypedTool` trait and `ToolRegistry`. The smallest complete concept in the
   codebase, and the thing you're most likely to extend; each tool (fs, shell,
   git, web, canvas, …) lives in its own file beside it. There's a full recipe
   in [Adding a tool](#adding-a-tool) below.
3. **[`crates/harness-agent/src/lib.rs`](crates/harness-agent/src/lib.rs)** —
   `Agent::run_turn`, the loop that wires the LLM client, the tools, and the
   history store together. This is the heart, and it reads top-to-bottom: *make
   room in the context → stream the reply → run any tool calls → repeat*.
4. **[`crates/harness-llm/src/types.rs`](crates/harness-llm/src/types.rs)** — the
   wire types (`ChatMessage`, `ToolCall`, streaming events) that flow through
   that loop.
5. **[`crates/harness-cli/src/main.rs`](crates/harness-cli/src/main.rs)** — how a
   real session boots: resolve the model + endpoint, build the tools, create the
   agent, then hand off to the REPL. The desktop front end in [`app/`](app/)
   drives the *same* `harness-agent` (see [`app/README.md`](app/README.md)).

Every crate's `src/lib.rs` opens with a `//!` comment stating what it owns, and
[`DOCUMENT-MAP.md`](DOCUMENT-MAP.md) is the one-line-per-file index for the full
layering. Tests live in a `#[cfg(test)] mod tests` right beside the code they
cover, so they double as usage examples.

### Extending the agent: tools vs skills

Two concepts cover everything the agent can be taught, and the split is worth
internalizing before you add either:

- **Tools are what the agent can *do*** — read a file, run a command, search
  the web, call your API. Every tool's name, description, and schema are sent
  to the model on **every request**, and the model calls one whenever it needs
  that ability.
- **Skills are what the agent *knows how to do*** — a workflow, a house style,
  a procedure, written as markdown instructions. Skills ride on a single
  built-in `skill` **tool**: the model is shown only each skill's name and
  one-line description, and when a request matches it calls
  `skill("<name>")` to pull the full instructions into the conversation.
  That's the whole interaction — a skill never runs code itself; once loaded,
  its instructions guide which *tools* the agent uses.

| You want the agent to… | Add a… | How | Cost per request |
|---|---|---|---|
| Follow your release-notes format, review checklist, deploy runbook | **Skill** (markdown, no code) | Settings → Skills, or drop a `SKILL.md` folder | One line (name + description) |
| Call your internal API or webhook | **Custom tool** (no code) | Settings → Tools → New tool (HTTP POST) | Its name + description + schema |
| Do something new on the machine (parse a format, drive a CLI…) | **Built-in tool** (Rust) | The recipe below | Its name + description + schema |

Rule of thumb: if it could be a wiki page for a new teammate, it's a skill; if
it needs to *execute*, it's a tool. Both are managed in the desktop app's
Settings (each page links to the other), both apply to new and resumed chats,
and the CLI honors the same configuration.

### Adding a tool

Tools are what the agent can *do* — read files, run commands, search the web.
Each one is a small Rust type in
[`crates/harness-tools/src/`](crates/harness-tools/src), one file per concern.
A tool has three parts: a **name** the model calls, a **description** telling
the model when to use it, and a **typed args struct** whose doc comments become
the JSON Schema the model reads. The schema is derived from the struct, so the
advertised interface and what your code parses can never drift.

(Not writing Rust? You can also add a no-code tool — a name, a description, and
an HTTP endpoint that receives the arguments as a JSON POST — straight from the
desktop app under **Settings → Tools → New tool**.)

Here's a complete built-in tool, start to finish:

**1. Create `crates/harness-tools/src/word_count.rs`:**

```rust
//! `word_count` — count the words in a workspace file.

use async_trait::async_trait;
use serde::Deserialize;

use crate::sandbox::Workspace;
use crate::{ToolError, TypedTool};

/// Tool name for [`WordCountTool`].
pub const WORD_COUNT_TOOL: &str = "word_count";

/// Arguments to `word_count`. Field doc comments are shown to the model.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct WordCountArgs {
    /// Path relative to the workspace root.
    pub path: String,
}

/// Count words in a file, confined to the workspace sandbox.
pub struct WordCountTool {
    workspace: Workspace,
}

impl WordCountTool {
    pub fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }
}

#[async_trait]
impl TypedTool for WordCountTool {
    const NAME: &'static str = WORD_COUNT_TOOL;
    type Args = WordCountArgs;

    fn description(&self) -> &str {
        "Count the words in a UTF-8 text file. Use it when the user asks how \
         long a document is."
    }

    async fn run(&self, args: WordCountArgs) -> Result<String, ToolError> {
        let path = self.workspace.resolve(&args.path)?;
        let text = tokio::fs::read_to_string(&path).await?;
        Ok(format!("{} words", text.split_whitespace().count()))
    }
}
```

**2. Register it** in [`crates/harness-tools/src/lib.rs`](crates/harness-tools/src/lib.rs):
add `pub mod word_count;` to the module list, then one line in
`default_for_workspace_with_web_key`:

```rust
.with_typed(word_count::WordCountTool::new(workspace.clone()))
```

**3. Add its name** to the `default_registry_contains_every_shipped_tool` test
in the same file (the test fails with a clear message until you do — that's it
catching the register-it step for you).

**4. Run `cargo test -p harness-tools`.** Done. The tool now shows up in both
front ends, appears on the desktop app's **Settings → Tools** page (where users
can toggle it or reword its description), and the model can call it in the next
new chat. Dispatch is by name — nothing else needs to change.

A few conventions worth copying from the existing tools:

- **Write field doc comments for the model, not for rustdoc** — they are the
  schema descriptions the model reads. Put defaults and units in them
  ("Timeout in milliseconds (default 120000)").
- **Optional arguments are `Option<T>`** (or `#[serde(default)]`); required ones
  are plain fields. Enums (`#[serde(rename_all = "snake_case")]`) become JSON
  `enum`s, and invalid values are rejected before your `run` is ever called.
- **All file access goes through `Workspace::resolve`** so a tool can't escape
  the project directory.
- **Tests live right beside the tool** in a `#[cfg(test)] mod tests` — invoke it
  with `tool.invoke(serde_json::json!({...}))` exactly as the model would (see
  `fs.rs` for examples).
- **Schemas are resent on every model call**, so keep descriptions tight; a
  budget test in `lib.rs` fails if the tool block balloons.

**Editing an existing tool:** change the behavior in `run`, or change the
interface by editing the args struct — the schema updates itself. The
`description()` string is prompt engineering: it's how the model decides *when*
to use the tool, so edit it deliberately. Two caveats: a tool's `NAME` is its
stable id (renaming one orphans users' saved preferences in `tools.json`), and
tools that need UI — like `ask_user_question` and `canvas` — define only their
data and a host trait in this crate, with each front end (CLI, desktop)
implementing and registering its own version.

### Adding a skill

Tools are what the agent can *do*; **skills are what it knows how to do well**.
A skill is a reusable set of instructions — release notes in your house style, a
code-review checklist, a deploy procedure — that the agent loads on demand. No
code involved: a skill is a folder holding a `SKILL.md` (the same shape as
Claude Code skills):

```markdown
---
name: release-notes
description: Writes release notes from the git log in our house style.
---

# Writing release notes

1. Run `git log --oneline` since the last tag.
2. Group changes into Added / Fixed / Changed.
3. One crisp line per change — no commit hashes, no filler.
```

Three ways to add one:

- **Desktop app**: **Settings → Skills → New skill.** Fill in the name, the
  one-line description, and the instructions; choose whether it's available in
  every project or just this one. Edit, toggle, and delete from the same page.
- **Drop a folder in** `~/.oxen-harness/skills/<name>/SKILL.md` for a global
  skill — it's picked up on the next chat.
- **Commit one to a repo** at `.oxen-harness/skills/<name>/SKILL.md` — everyone
  who opens that project gets it (a project skill shadows a global one with the
  same name). This repo ships one:
  [`add-a-tool`](.oxen-harness/skills/add-a-tool/SKILL.md) teaches the agent to
  extend itself following the recipe above — open oxen-harness *in*
  oxen-harness and ask for a new capability.

**How skills and tools interact** (the Claude Code pattern, *progressive
disclosure*): the model is shown only each skill's name and one-line description
— a few tokens per skill, carried by a single built-in `skill` tool. When a
request matches a description, the model calls `skill("release-notes")` and the
full instructions enter the conversation right when they're needed. The
description is therefore the trigger — write it like "does X, use when Y". A
skill's folder can hold supporting files (templates, scripts, examples) next to
the `SKILL.md`; the instructions can point the agent at them.

**Referencing tools from a skill**: name the tool in backticks, exactly as it
appears on the Tools page — "run \`git\` with operation=status", "read it with
\`read_file\`". There's no special syntax beyond that; the model connects the
name to the registered tool when the skill loads. The desktop editor makes the
convention hard to get wrong: typing a backtick autocompletes the registered
tool names, known references render as highlighted chips in the preview, and a
backticked snake_case name that matches no tool gets a typo warning.

Skills work in both the CLI and the desktop app, and changes apply to new and
resumed chats. The machinery lives in
[`crates/harness-tools/src/skill.rs`](crates/harness-tools/src/skill.rs)
(parsing + the `skill` tool) and
[`crates/harness-runtime/src/skills.rs`](crates/harness-runtime/src/skills.rs)
(discovery, preferences, authoring).

### The dev loop

This project is built with **The Ralph Wiggum loop** — a tight,
objective-check-driven cycle where tests and files (not conversation) hold state.
A change is "green" only when all three checks pass:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run          # or: cargo test --workspace
```

See [`AGENTS.md`](AGENTS.md) for the full contributor protocol — write the test
in the same change, keep commits small and reasoned, and do a polish pass before
moving on.

## License

[Apache-2.0](LICENSE).

---

<p align="center">Powered by <a href="https://oxen.ai">Oxen.ai</a> 🐂</p>
