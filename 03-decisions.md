# Working Decisions & Rationale

**Purpose:** Currently relevant decisions with enough "why" to be useful during implementation. For full deep-dive analysis, cite the source in each entry.
**Updated:** 2026-06-23

---

## Provider & Models

**Oxen.ai is the only provider** (2026-06-21)
The harness targets the Oxen.ai OpenAI-compatible chat completions API (`https://hub.oxen.ai/api/ai`, endpoint `/chat/completions`). Any model with tool calling works; the provider is fixed but the model is swappable. Default model is `claude-opus-4-8`.
-> *Full context: https://docs.oxen.ai/examples/inference/chat_completions*

## Auth & Config

**Lightweight auth; no `liboxen` dependency** (2026-06-21, revised)
Auth resolves from `OXEN_API_KEY`, falling back to parsing the Oxen
`auth_config.toml` directly (`$OXEN_CONFIG_DIR` or `~/.config/oxen/`, looked up
by host `hub.oxen.ai`). This is the same file the `oxen` CLI writes on login, so
`oxen login` interoperates — without taking the dependency.

*Why revised:* we initially chose a hard `liboxen` dependency, but empirically
`liboxen` does not compile with `default-features = false` (its source imports
`duckdb` unconditionally — 74 compile errors), so a real dependency forces the
full bundled **DuckDB + RocksDB C++ build**: a multi-minute first compile plus a
`cmake`/C++ toolchain prereq on every platform — all just to read an API token
from a TOML file. We dropped it in favor of a ~40-line parser with identical
behavior. `liboxen` can return later behind an optional feature for genuine data
versioning (see `04-backlog.md`).

**Configurable API host** (2026-06-21)
The base URL is resolvable, not hardcoded. Precedence: `--base-url` > `--host` >
`OXEN_BASE_URL` env > `OXEN_HOST` env > default `https://hub.oxen.ai/api/ai`. A
bare host (`localhost:3001`) expands to `http://<host>/api/ai` for local/loopback
hosts or explicit non-443 ports, `https://<host>/api/ai` otherwise; a value with a
scheme is used as-is. The auth token is looked up by the *resolved* host (e.g.
`localhost:3001`), matching how the `oxen` CLI keys tokens, so self-hosted/dev
servers work with `OXEN_API_KEY` or a per-host login. Helpers live in
`harness-llm::auth` (`resolve_base_url`, `base_url_from_host`, `host_from_base_url`).

**Config lives in `harness-config`; secrets live in `.env`** (2026-06-25)
The `~/.oxen-harness` base dir and every file under it (`history.sqlite`,
`connection.json`, `projects.json`, `config.toml`, `themes/`, `loops/`,
`models/`, `canvas/`, `.env`) is resolved in one place — `harness-config::paths`
— overridable with `OXEN_HARNESS_DIR` for tests/sandboxes. It previously lived in
~8 copies across crates and the two front-ends and could drift.
- *JSON config is atomic + versioned.* Writes go through a temp-file+rename
  (`atomic_write`) so an interrupted write never leaves a torn file, and each
  file carries a `schema_version` via a flattened `Versioned<T>` envelope. Files
  written before versioning read back as version `0` (`UNVERSIONED`), so a future
  migration can detect and upgrade them. `#[serde(flatten)]` keeps the JSON shape
  backward compatible (the CLI's raw-JSON reads of `connection.json` still work).
- *Secrets are not versioned.* API keys move out of `connection.json` into
  `~/.oxen-harness/.env`, loaded into the process env at startup with `dotenvy`
  (never overriding a var already set). This keeps the JSON config safe to commit
  to an Oxen repo and share. The `.env` is written `0600`. (Migration of existing
  plaintext keys out of `connection.json` lands with the runtime extraction.)
-> *Why `.env` over the OS keychain: portable across the CLI + desktop app + cloud/headless runs, and trivially scriptable, without a platform-specific secret-store dependency.*

## History & Storage

**Versioned history DB via `rusqlite_migration`** (2026-06-25)
The history store no longer creates its schema with bare `CREATE TABLE IF NOT
EXISTS`; it runs an ordered migration chain tracked by SQLite's `user_version`.
- *M1* is the original schema, declared with `IF NOT EXISTS` so a database
  created before migrations existed (tables present, `user_version` still 0)
  adopts the chain without error rather than colliding.
- *M2* replaces the plain `(session_id, seq)` index with a **UNIQUE** one
  (one row per sequence number) and adds session-metadata columns: `provider`,
  `base_url`, `mode` (local/cloud), `context_window`, `system_prompt_version`,
  `theme`, `transcript_version`. New columns default to empty/NULL so existing
  rows migrate without backfill.
-> *Why: changing `ChatMessage`, adding columns, or repairing derived fields was
  unsafe once users had real history; and resuming an old session was ambiguous
  with only `workspace` + `model` recorded as local models/tools/providers evolve.*

**Derived `content`/title handles multimodal messages** (2026-06-25)
`append_message` populated the queryable `content` column only when the
top-level `content` was a JSON string, so a user message with an attachment
(content serialized as a `Parts` array) recorded `NULL` and lost its title.
`derive_content_text` now flattens the `text` parts of an array so titles come
from the words the user typed. Kept as a JSON walk in `harness-store` rather than
calling `ChatMessage::content_text()` so the store stays decoupled from
`harness-llm`.

**Attachments stored on disk, hydrated for sending** (2026-06-25)
Image/PDF attachments were base64-encoded inline in the message JSON, so
`history.sqlite` and JSONL exports ballooned with every screenshot. They now go
through `AttachmentStore`: bytes are written content-addressed (sha256) to
`<project_root>/.oxen-harness/attachments/<hash>.<ext>` and the message records
only a path *relative to the project root*. `Agent::outbound_messages`
re-inflates those references to `data:` URIs (`hydrate_content`) just before each
provider request, so the wire format to the model is unchanged.
- *Why under the project root:* attachments are versioned by Oxen alongside the
  code the conversation is about, and the relative path stays stable across
  machines/clones.
- *Back-compat:* references already inline (`data:`) or remote (`http(s):`) — old
  transcripts — pass through hydration untouched; a missing file becomes a short
  text note rather than a broken request.
- *Gated by `AgentConfig::attachment_root`:* `None` keeps the legacy inline
  behavior (used by tests). Text/video/opaque attachments are still inlined as
  before (their content is small text or just a note).
- Neither front end displayed attachment images inline (the desktop `ContentPart`
  is text-only; the terminal can't show images), so storing paths is display-neutral.

**Oxen versioning via the `oxen` CLI, not `liboxen`** (2026-06-25)
`harness-oxen` versions everything outside the history DB — config, project
data, and shareable conversation traces — by shelling out to the `oxen` binary
(`init`/`add`/`commit`/`set-remote`/`push`/`clone`), confirming the earlier
"no `liboxen` dependency" call: `liboxen` drags in Polars + Arrow + the AWS SDK +
a bundled DuckDB C++ build, while the verbs we need are the stable CLI surface,
and the CLI shares `~/.config/oxen` auth so `oxen config --auth` already enables
push/share.
- *Testability:* all commands go through a `Runner` trait; tests inject a
  `FakeRunner` to assert the exact argv and parse scripted exits without the
  binary installed. `SystemRunner` is the production `std::process::Command`.
- *Graceful degradation:* a missing binary surfaces as `OxenError::NotInstalled`
  with an install hint; callers treat versioning as optional, never fatal.
- *Traces:* `oxen-harness trace export <session> [--out DIR] [--push URL]`
  bundles the session's JSONL transcript with the attachment files it references
  (kept at their project-relative paths so the clone hydrates), commits, and
  optionally pushes to an Oxen hub repo to share.
- *Config-dir versioning* (committing `~/.oxen-harness`) reuses `Oxen::snapshot`
  and is wired where config writes are centralized — the runtime layer.

**Shared runtime for connection/secrets; agent loop stays in `harness-agent`** (2026-06-25)
The review flagged the Tauri bridge accreting app logic that could drift from the
CLI. The genuinely duplicated, drift-prone piece was *connection resolution*: the
CLI (`brave.rs`) and desktop each parsed `connection.json` and built clients
their own way. `harness-runtime::connection` now owns it — one resolution path
both front ends call (`build_client`, `brave_key_override`, `view`) — and the
desktop's `ConnectionConfig`/`ConnectionView`/`configured_client`/`effective_*`
were deleted in favor of it.
- *Secrets moved to `.env`:* keys no longer live in `connection.json`.
  `connection::load()` (run at startup by both front ends) migrates any legacy
  plaintext keys into `~/.oxen-harness/.env` and scrubs them from the JSON, so
  the versioned config is safe to share. `save()` writes the host to JSON and the
  keys to `.env`.
- *Config versioning:* `config_repo` snapshots `~/.oxen-harness` with Oxen after
  connection/projects writes (no-op until opted in via `oxen-harness oxen init`).
- *Scope:* the agent **loop** is already shared (`Agent::run_turn_with_attachments`
  — both front ends call it), so it can't drift. The desktop's concurrent
  per-session agent *cache* (the agents map / evict / current) stays in the Tauri
  layer because it's a desktop concern (the CLI is single-session); it isn't
  duplicated logic. Unifying agent *construction* (tools+config+session) is a
  worthwhile follow-up but was left out of this pass to avoid a blind refactor of
  the desktop session manager (no headless way to runtime-test it here).

## Wire contracts

**Rust↔TS wire types: ts-rs codegen + serde golden tests** (2026-06-25)
`app/src/lib/types.ts` hand-mirrored the `harness-llm` wire types (`ChatMessage`,
the untagged `MessageContent`, the tagged `ContentPart`, …), which was brittle —
exactly the fragile boundary the review flagged.
- *Codegen:* the wire types derive `ts_rs::TS` behind an optional `ts` feature
  (so normal builds don't pull `ts-rs`). `cargo test -p harness-llm --features ts
  -- --ignored generate_bindings` writes `app/src/lib/bindings.ts`; `types.ts`
  re-exports from it instead of declaring its own. A `bindings_are_up_to_date`
  test (run under `--features ts`) fails if a Rust type changed without
  regenerating. The generated `ContentPart` is now an accurate discriminated
  union (the old hand TS was `{type, text?}`); `thread.ts` narrows on it.
- *Golden serde tests* (always on, no feature) pin the exact JSON: text content
  as a bare string, attachments as a tagged-part array, tool calls using the
  `type` field with content omitted, tool results round-tripping. These guard the
  wire shape regardless of the TS toolchain.
- *Scope:* only the `harness-llm` message types are generated (the fragile,
  shared boundary). Other TS types (Theme, ModelStatus, Tauri payloads) stay
  hand-written; generating them would mean `ts-rs` derives across many crates for
  little gain.

## Tooling parity

**Essential tool set modeled on Claude Code (no MCP, no orchestration/network)** (2026-06-21)
After researching Claude Code's built-in tools (Read, Write, Edit, Bash, Glob,
Grep, plus Task/TodoWrite/WebFetch/WebSearch), we matched the *file + shell*
primitives a strong coding agent needs and deliberately stopped there:

- `read_file` returns `cat -n` line numbers with `offset`/`limit` and truncation
  caps (2000 lines / 2000 chars per line) — mirrors Claude's `Read` so models can
  cite/edit by line and read large files in chunks. Edit args must exclude the prefix.
- `find_files` = Claude's `Glob` (glob via `globset`, gitignore-aware, newest-first).
- `search_files` upgraded from literal substring to a `Grep`-style **regex** search
  (`regex` crate) with `content`/`files_with_matches`/`count` modes + `glob`/`path` filters.
- `run_shell` got a `timeout_ms` (default 120s) and a 30k-char output cap.

*Deliberately skipped* to keep the codebase simple: `TodoWrite`/`Task` (orchestration
+ session state), `WebFetch`/`NotebookEdit`, and anything MCP. New deps were limited
to `regex` + `globset` (both already in the `ignore` transitive tree). MCP remains a
future opt-in (see `04-backlog.md`).

**Web search via Brave (`web_search`)** (2026-06-21)
Added a `web_search` tool backed by the [Brave Search API](https://brave.com/search/api/)
so the agent can research docs, current events, and unfamiliar errors. Brave was chosen
for a clean JSON API and a no-credit-card free tier. The key comes from `BRAVE_API_KEY`
(or `BRAVE_SEARCH_API_KEY`); reqwest (already in the tree via `harness-llm`) was promoted
to a workspace dependency rather than adding a new HTTP client. The tool is registered in
`default_for_workspace` **only when a key is present**, so the model is never shown a
capability it cannot use. We kept it to a single search call (no `WebFetch`/HTML scraping)
to stay simple — snippet text is returned with Brave's `<strong>` highlight tags stripped.

**Interview the user via `ask_user_question`** (2026-06-21)
Added an `ask_user_question` tool so the agent can stop and ask the user to choose
between genuinely ambiguous approaches instead of guessing. We mirrored Claude Code's
`AskUserQuestion` shape — 1–4 questions, each with a short `header`, the full
`question`, 2–4 `{label, description}` options, and a `multiSelect` flag — because it's
a well-tested schema models already understand, and the host always supplies the
free-text "type your own" escape hatch (so the model must **not** add an "Other" option).

Rendering is host-specific, so `harness-tools` owns only the data types and a
`QuestionAsker` trait; each front end implements it. The **CLI** asker (`harness-cli`)
draws an interactive picker with `crossterm` — the one terminal dependency we added,
since `rustyline` can't build arbitrary key-driven pickers and hand-rolling
cross-platform raw-mode key parsing is error-prone. It runs on a `spawn_blocking`
thread (so the async agent loop never stalls), restores the terminal via an RAII guard,
treats `Ctrl-C` in raw mode as cancel, and returns `None` for non-TTY sessions so the
tool tells the model to proceed with sensible defaults. The **desktop app** asker emits
an `agent://question` event and parks on a `oneshot` channel until the `answer_question`
command delivers the user's selection from the question card. The CLI renderer
special-cases the tool name to suppress the usual tool line + spinner while the picker
owns the screen.

## Attachments (drag-and-drop)

**Dropped files become content parts; text documents are inlined** (2026-06-23)
Both front ends let the user drag files into the chat. `harness-llm::Attachment`
reads a file, classifies it, and serializes it the way the model expects: images
as `image_url` data URIs, PDFs as `file` data URIs, **text documents inlined as
text**, and anything the model can't read natively (video, opaque binaries) as a
short text note so the transcript still records the drop. Files are capped at
20 MiB; inlined document text is further capped at `MAX_TEXT_CHARS` (100k chars)
so a large file can't swamp the context window.

- *Text vs binary is sniffed, not extension-mapped.* `from_extension` only names
  the media types; any other file is decided by `from_bytes` via a cheap
  text/binary heuristic (valid UTF-8 + no NUL byte). This catches Markdown, CSV,
  JSON, source, and extension-less files (a bare `README`) without a brittle
  allowlist, and treats true binaries as opaque notes.
- *CLI extraction is path-shaped to avoid hijacking edit prompts.* The CLI has
  no separate attach affordance, so `harness-cli::attach` tokenizes the prompt
  line (shell-style, honoring quotes/escaped spaces) and pulls out file paths.
  **Media** files attach however they're referenced, but a non-media file is only
  attached when written as an **absolute path** — the signature of a terminal
  drag-and-drop — so typed *relative* references like `README.md` or
  `src/main.rs` stay in the prompt for the agent's file tools instead of being
  swallowed. The desktop app passes dropped paths explicitly, so it has no such
  ambiguity.
- *Live composer uses bracketed paste.* The sticky-bottom composer runs the
  terminal in raw mode, where a drag-drop would otherwise arrive as a fragile
  burst of keystrokes. It enables bracketed paste (`\x1b[?2004h`, disabled on
  drop) and handles `Event::Paste` by inserting the block into the focused
  single-line editor (newlines flattened to spaces), so a dropped path lands
  intact in one event.

## Theming

**Configurable, shareable themes (palette + voice) in `harness-theme`** (2026-06-21)
The CLI/app personality was hardcoded Oregon Trail. We extracted a `harness-theme`
crate so a **theme is data**, shared by both front ends. A `Theme` is `Meta` +
`Palette` (7 terminal foreground colors + app `background`/`surface`/`border`, each
a `#rrggbb` `Color`) + `Voice` (prompt icon/label, spinner glyphs, thinking phrases,
per-tool verbs with a `default` fallback, "death"/quit lines, banner art/wordmark/
labels, help items, and the exit screen). Built-ins: **Oregon Trail** (the `Default`,
preserving the original look verbatim), **Midnight**, **Synthwave**.

*Format & partial overrides:* themes serialize to a single self-contained **TOML**
file (also readable as JSON) — trivial to export/import/share. Loading is tolerant:
the file is parsed to a `serde_json::Value`, **deep-merged over the serialized
default**, then deserialized — so a file may set only a few fields and inherit the
rest. This makes hand-written and model-generated themes robust to omissions without
per-field `serde` defaults or duplicate "patch" structs.

*Persistence:* everything lives under `~/.oxen-harness/` (alongside history/models,
not a separate `~/.config` tree): `config.toml` records the active theme slug;
`themes/<slug>.toml` holds installed themes. An installed file **shadows** a built-in
of the same slug, so users can fork a built-in by saving over its name.

*CLI integration:* `Ui` now carries an `Arc<Theme>` (it became `Clone`, not `Copy`)
and resolves all colors/phrases from it; the spinner owns `Vec<String>` verbs.
`/theme` reuses the `ask_user_question` picker (extracted into a shared `picker.rs`).
A top-level `oxen-harness theme …` subcommand covers list/use/export/import/path/remove
for scripting and dotfiles; switches hot-swap the live UI.

*Vibe-coding:* `/theme new` (CLI) and the app's 🎨 panel run a short interview, then
ask the model — via `Theme::generation_system_prompt()` (schema + the default theme as
a worked example) — to emit a complete theme. `Theme::from_model_output` strips code
fences/prose, tries TOML then JSON, and only injects a fallback name when the model
omitted one (so a nameless generation can't silently shadow a built-in). The CLI uses
the session's `Agent` (a new `Agent::complete` one-shot that isn't persisted to the
transcript); the app uses the same via a `new_theme` command. The app applies the
palette to its CSS variables and uses the voice phrases for status + tool chips.

## Local models (llama.cpp)

**Run open-weight models locally via `llama-server`** (2026-06-21)
Added a `harness-local` crate so the agent can run Qwen3 models on the user's own
machine, fully offline. llama.cpp's `llama-server` was chosen because it exposes
the *same* OpenAI-compatible chat-completions API the harness already speaks — so a
local model is just `OxenClient::new("http://127.0.0.1:<port>/v1", "local", id)`
with a throwaway key, **no client changes**. `--jinja` is passed so the model's
chat template (and thus tool calling) works.

- **Catalog** = curated Qwen3 GGUFs at `Q4_K_M` (the consumer sweet spot: ~4.5
  bits/weight, near-FP16 quality at ~28% size), 0.6B → 32B plus the 30B-A3B MoE,
  from the de-facto-standard `bartowski` repos. All repo/file URLs were
  HEAD-verified against Hugging Face before shipping.
- **Downloads are managed in-process** (streamed via `reqwest` to a `.part` file,
  atomically renamed) rather than delegated to `llama-server --hf`, *specifically*
  so the CLI and UI can show real download progress and per-model disk usage — the
  feature the user asked for. Weights live in `~/.oxen-harness/models/`.
- **`llama-server` is detected, not bundled** (`LLAMA_SERVER` override or `PATH`);
  when missing we fail fast with a platform-specific install hint (`brew install
  llama.cpp`, a release download, or the env override) instead of a cryptic error.
- **Server lifecycle**: a free port is picked, the process spawned, `/health`
  polled until the model loads, and the process killed on `Drop` so a session never
  leaks a background server. Selected at startup via `--local`; mid-session
  switching is deferred (see `04-backlog.md`).
- New deps stayed minimal: `reqwest` (already in the tree) gained `stream`, plus
  `futures-util` + `dirs`. `mockito` (dev) tests the streaming download loop.

## History & Export

**SQLite, stored verbatim, JSONL export** (2026-06-21)
Conversation messages and tool inputs/outputs are persisted verbatim (no truncation caps) in SQLite, so traces are complete enough to fine-tune on. A JSONL exporter emits one message object per line for dataset building. Verbatim storage means large file/shell outputs are stored in full — acceptable for a local, single-user tool.

## Agent Loop

**Objective-check-driven (Ralph Wiggum) loop** (2026-06-21)
Development follows a tight test-first loop: read spec → write/adjust a test → smallest change → write to disk → run `fmt`/`clippy`/tests → fix root cause → stop on green → commit with a clear message. At the **end of each feature** there's a separate **review/refactor pass**: the LLM critiques the diff for modularity / maintainability / readability / idiomatic / pragmatic code, the agent applies the worthwhile fixes, the checks are re-run, and that cleanup lands as its own commit. The *runtime* agent loop mirrors the build loop: call model → execute any `tool_calls` → append `tool` messages → repeat until `finish_reason` is `stop`/`length`.
-> *Full context: `AGENTS.md`*

**Bounded by context window, not a fixed iteration cap** (2026-06-22, revised)
The runtime loop has **no max-iterations limit**. Instead it budgets against the
model's **context window**: before each model call it estimates the prompt's
token cost (transcript + tool definitions) and stops with `ContextWindowExceeded`
if it would overflow `window − response_reserve`. Token counts are estimated
client-side (`harness-agent::budget`, ~4 chars/token) so it works for every
endpoint — remote or local — without bundling a tokenizer. The window is derived
from the model name (`context_window_for`), or set explicitly via
`AgentConfig.context_window` — local `llama-server` sessions pass the server's
real `--ctx-size` (`LocalServer::context_size`), which is far smaller than the
model's theoretical maximum. The agent tracks cumulative `tokens_used`, and the
CLI prints a subtle `🧭 context X / Y tokens (Z%)` trailer after each turn.

*Why revised:* a hard 25-iteration cap killed long-but-legitimate tool-heavy
turns ("died of measles: reached max iterations"). The real constraint on a
chat-completions loop is the context window, so we bound on that instead.

**Goal-driven loops live in their own `harness-loop` crate** (2026-06-22)
A *loop* (distinct from a single agent turn) hands the agent a job, a gate that
decides when it's done, and a give-up rule, then drives DISCOVER → QUESTION →
PLAN → EXECUTE → VERIFY → ITERATE. Design choices:

- **The gate is the point.** [`Verify`] is either a **command** (shell exit 0 =
  pass — objective, the model can't talk past it) or a **rubric** (a *separate*
  strict checker turn via `Agent::complete` scores 1–10 against the criteria;
  passes only if every score clears the threshold). When a loop has no command,
  it falls back to the rubric but the CLI nudges toward a command. We deliberately
  recompute pass/fail from the scores rather than trusting the checker's own
  "pass" field, and fail *closed* on unparseable checker output (never a silent pass).
- **State that learns.** A [`LoopJournal`] records each pass (summary + verify
  outcome) and is fed back as a digest so the agent doesn't repeat a failed
  approach; it's persisted per-iteration under `loops/runs/<slug>.json` for resume.
- **Two stop conditions** beyond success: an iteration cap (default 8) and an
  optional cumulative token budget — so a loop can't run all night for nothing.
- **Reuse, not a parallel stack.** The runner drives the existing `Agent`
  (tools + `ask_user_question` already work for the QUESTION phase) and forwards
  `AgentEvent`s wrapped in `LoopEvent`, so the CLI renders a loop identically to a
  normal turn (shared `render::TurnRenderer`). Loops are shareable TOML in
  `harness-loop`'s `LoopStore`, mirroring the theme store; built-ins ship with the
  `default` "make the checks green" loop this repo runs on itself.
- **Scope:** core engine + CLI now (`oxen-harness loop …` and in-REPL `/loop …`);
  Tauri UI is a follow-up.

## UX & Scope

**CLI first, Tauri later; stream from day one** (2026-06-21)
Ship a `claude`-style interactive REPL with live SSE token streaming first; add the cross-platform Tauri v2 desktop app once the core loop is stable. Sessions are scoped to a single working directory. Shell commands run in a sandboxed working directory where possible, but the model decides. File edits are allowed by default. Cross-platform support is a constraint from day one.

**Oregon-Trail-themed CLI, hand-rolled styling** (2026-06-21)
The REPL's structure follows modern coding CLIs (welcome panel, in-place status
spinner, transparent tool lines) and its voice is the 1980s Oregon Trail game —
a natural fit since *oxen* pull the trail's wagons and Oxen.ai powers this one.
All flavor + rendering lives in `harness-cli/src/theme.rs`. Styling is hand-rolled
(24-bit ANSI, a tiny block-letter "figlet", a time-seeded xorshift for phrase
picks, a background-thread spinner) rather than pulling `owo-colors`/`ratatui`/
`indicatif` — it keeps deps minimal and the build fast, and the surface is small.
Color auto-disables for non-TTY / `NO_COLOR` / `TERM=dumb` so piped output is clean.

## Tooling

**Single Cargo workspace, focused crates** (2026-06-21)
`harness-core` (base types) / `harness-llm` / `harness-tools` / `harness-store` /
`harness-local` (local models) / `harness-theme` / `harness-agent` (orchestration
loop) / `harness-loop` (goal-driven self-verifying loops, atop `harness-agent`) /
`harness-cli`. Tests use `mockito` to fake HTTP endpoints (deterministic, offline).
`cargo nextest` is the preferred test runner.

**Orchestration lives in `harness-agent`, not `harness-core`** (2026-06-21)
The loop depends on the llm/tools/store crates, which all depend on `harness-core`
for shared types. Putting the loop in `core` would create a dependency cycle, so a
dedicated `harness-agent` crate sits above them. `harness-core` stays a leaf of
shared domain types.

**`HistoryStore` is `Send + Sync`** (2026-06-21)
The SQLite connection is wrapped in a `Mutex` so the store can be shared via `Arc`
across threads (the agent loop today, the Tauri app later).
