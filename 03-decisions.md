# Working Decisions & Rationale

**Purpose:** Currently relevant decisions with enough "why" to be useful during implementation. For full deep-dive analysis, cite the source in each entry.
**Updated:** 2026-07-14

---

## Provider & Models

**Oxen.ai is the only provider** (2026-06-21)
The harness targets the Oxen.ai OpenAI-compatible chat completions API (`https://hub.oxen.ai/api/ai`, endpoint `/chat/completions`). Any model with tool calling works; the provider is fixed but the model is swappable. Default model is `claude-opus-4-8`.
-> *Full context: https://docs.oxen.ai/examples/inference/chat_completions*

## Auth & Config

**Projects are the desktop navigation root** (2026-07-11)
The desktop opens on the Projects page. Selecting a directory enters that
project's chats; its sidebar leads back to Projects, and Settings leads back to
that active project through a matching upper-left rail control. Full-window
surfaces do not use top-right close affordances for primary navigation.

*Why:* the workspace determines where agents execute, which chats belong
together, and which project-local skills are visible. Making it the first and
durable piece of context prevents the UI from treating that boundary as an
incidental detail. This is a navigation decision, not a config migration:
connection, model, tool, review, compression, usage, appearance, and training
settings remain global; chats, execution, and project skills retain their
existing workspace scope.

**Projects are durable repo-local agent context, not only recent folders** (2026-07-14)
The global `~/.oxen-harness/projects.json` remains the user's recent-project
index, active-project pointer, and preferred parent directory for creating new
projects. A project's shareable identity now lives in
its own repository at `.oxen-harness/project.json`: display name, goal,
instructions, and a manifest of durable references. External references are
copied content-addressed into `.oxen-harness/context/`; text stays available to
`read_file` on demand, while PDFs/images are attached to the first user prompt
of a new chat. Both desktop and CLI compose this same project section into the
agent prompt.

The desktop treats the project home as getting-started/settings, not the default
return destination. A project without chat history lands there, where its
instructions/context are visible and editable and the user can stage the cloud
or local model for the first chat. An established project resumes its newest
chat directly; a files button in the chat titlebar reopens the project page on
demand. Sending from that page still starts a fresh chat so a persisted
transcript's original system prompt stays truthful rather than being silently
mutated. Existing folder-only projects need no migration file: their directory
basename is the default name and the new fields are empty.

*Why:* the working directory controls execution, but it does not communicate
the user's goal or stable constraints. Keeping this metadata with the codebase
makes project behavior portable across machines/front ends, reviewable in
version control, and explicit at the point a conversation begins.

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
- *M6* repairs the first Usage release's version-5 `model_usage` aggregate
  table into the timestamped `usage_events` ledger. Its totals are preserved as
  `unpriced` events at the former row's update time; the original schema did
  not retain enough information to recover a provider or a cost. This is a
  forward-only repair because released databases already recorded version 5.
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

## File formats & safety

**Theme & loop files are versioned and validated** (2026-06-25)
Themes and loops are public, shareable, and (for themes) model-generated, so
both now carry a `schema_version` (`THEME_SCHEMA_VERSION` / `LOOP_SCHEMA_VERSION`,
`#[serde(default)]` so older files still load). On load, `Style::sanitize` clamps
the enum-ish style fields (`display_transform`, `shadow`, `hero`, `scene`) to
known values and validates CSS lengths (`radius`, `border_width`,
`display_spacing`), falling back to the default for anything malformed — so a
hand-written or LLM-generated theme can't quietly produce broken UI.

**The workspace sandbox is policy, not a security boundary** (2026-06-25)
`sandbox.rs`'s module docs now say this plainly. `Workspace::resolve` is
*lexical* (normalizes `.`/`..` textually, no `canonicalize`), so a symlink inside
the root pointing outside it is not caught; `run_shell` is only cwd-pinned, not
confined. It's a guardrail against an honest agent wandering, not a sandbox
against a malicious one. Real isolation (containers/seccomp, a permission layer)
is tracked separately in `04-backlog.md`.

**Web-search docs reconciled with code** (2026-06-25)
The registry always registers `web_search`; without a key the call returns the
recognizable `WEB_SEARCH_NO_KEY` error the front ends turn into an inline "add
your Brave key" prompt. `web.rs`, the registry doc comment, and the README
(which all still said the tool was *omitted* without a key) were corrected.

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

**Tool schemas derive from typed args structs (`TypedTool` + `schemars`)** (2026-07-01)
Every tool used to carry a hand-written `json!` schema *and* hand-rolled argument
extraction — two disconnected encodings of the same interface that could silently
drift. Tools now implement `TypedTool` (`NAME`, `type Args: Deserialize + JsonSchema`,
`run(Args)`); the schema the model reads is generated from the same struct serde
parses, with doc comments becoming the model-facing field descriptions. A blanket
`impl Tool for T: TypedTool` would collide with runtime-schema tools (user-defined
custom tools) under coherence rules, so the registry wraps via a private adapter
(`with_typed`/`register_typed`) instead. `schema_for` strips generator noise and
compacts documented enums to keep the per-request token overhead pinned by the
existing budget test; a registry-completeness test makes forgot-to-register loud.
The old `args.rs` helpers were deleted. Rejected: staying hand-written (drift risk),
proc-macro codegen (a second thing to learn).

**Skills: SKILL.md + progressive disclosure via one `skill` tool** (2026-07-01)
Reusable know-how (house styles, checklists, procedures) is data, not code — so a
skill is a directory holding a `SKILL.md` (YAML frontmatter `name`/`description` +
markdown body), deliberately the same shape as Claude Code skills so existing ones
port over. The interaction copies Claude Code's progressive disclosure: a single
built-in `skill` tool advertises only each skill's name + one-line description
(a few tokens per skill); the full instructions enter context only when the model
invokes the skill. Discovery is global (`~/.oxen-harness/skills/`) + per-project
(`.oxen-harness/skills/`, committed to the repo — project shadows global by name);
enable/disable prefs live in `skills.json`; the desktop Settings → Skills page and
the CLI `/skills` command surface them. Rejected: injecting all skills into the
system prompt (pays for every skill on every call), a per-skill tool (tool-list
bloat), and a bespoke format (Claude Code compatibility is free adoption).

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

**Run open-weight models locally via `llama-server`** (2026-06-21; revised 2026-07-14)
Added a `harness-local` crate so the agent can run GGUF models on the user's own
machine, fully offline. llama.cpp's `llama-server` was chosen because it exposes
the *same* OpenAI-compatible chat-completions API the harness already speaks — so a
local model is just `OxenClient::new("http://127.0.0.1:<port>/v1", "local", id)`
with a throwaway key, **no client changes**. `--jinja` is passed so the model's
chat template (and thus tool calling) works.

- **Catalog is data, not Rust**: built-ins live in `assets/catalog.json`; users
  add or override entries in `~/.oxen-harness/local-models.json`. It includes
  Qwen3 `Q4_K_M` models from 0.6B → 32B plus 30B-A3B, and the July 2026 Bonsai
  27B release: the 3,803,452,480-byte `Q1_0` binary GGUF and the
  7,585,330,240-byte mainline-compatible `Q2_g64` ternary GGUF, both with a
  262,144-token native window. The standard Q8→Q3 filename ladder is explicitly
  opt-in per entry (`derive_quants`); native/one-off formats get exactly their
  published file. This keeps adding a model a JSON-only change without making
  guessed URLs part of the UI.
- **Low-bit discovery is first-class**: Q1/Q2/PQ2 names parse like conventional
  quants, while optional `mmproj` vision projectors and DSpark speculative
  drafters are filtered from the generic standalone-model picker.
- **Downloads are managed in-process** (streamed via `reqwest` to a `.part` file,
  atomically renamed) rather than delegated to `llama-server --hf`, *specifically*
  so the CLI and UI can show real download progress and per-model disk usage — the
  feature the user asked for. Weights live in `~/.oxen-harness/models/`.
- **`llama-server` resolution** prefers `LLAMA_SERVER`, then the desktop-managed
  runtime, then `PATH`/Homebrew. The managed Apple Silicon build is deliberately
  pinned (now llama.cpp `b10002`, new enough for Bonsai Q1 and mainline Q2 on
  CPU/Metal); other platforms get an actionable install hint rather than a
  cryptic spawn error.
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

**Context overflow compacts instead of erroring** (2026-07-05)
When the calibrated estimate says the next request won't fit the window, the
agent frees context in two stages before giving up: prune stale tool output
(cheap, no model call), then summarize the oldest turns into a single note via
`Agent::complete` — cut on a *user-turn boundary* so no tool result is orphaned
from its call. Only if a compacted transcript still can't fit does
`ContextWindowExceeded` surface. Compaction mutates only the in-memory
transcript; the store keeps the verbatim record, so exports and resume are
unaffected. The client-side token estimate is **calibrated** against the real
prompt sizes the endpoint reports (`token_ratio`), so the budget check tracks
reality rather than the crude 4-chars/token heuristic.

**Reversible compression over summarization for stale tool output** (2026-07-05)
`harness-compress` shapes *outbound requests only*: stale tool results are
crushed (JSON-array sampling, log/line collapsing) and replaced with a digest
plus a `<<ccr:hash>>` marker; the `retrieve_original` tool restores any marker,
so nothing the model might need is unrecoverable — unlike summarization, which
is lossy and needs a model call. Three modes (`off`/`audit`/`on`) because
trust is earned: `audit` runs the identical pipeline and reports would-be
savings without changing a byte on the wire. The most recent tool results are
always protected (the model is still working with them), as are errors, small
outputs, and `retrieve_original` results (re-compressing them would loop).
The in-memory transcript and the store always keep the originals.

**Transient model-call failures retry with backoff inside the agent** (2026-07-06)
Provider 5xx, rate limits, and streams that die mid-reply are facts of life on
any endpoint; making every host implement recovery would drift. `stream_reply`
retries per `RetryPolicy` (default 4 attempts, 1s → 2s → 4s), emitting
`AgentEvent::Retrying` so UIs show the pause as a hiccup rather than a hang.
Retrying a dead stream is safe because nothing is persisted until a reply
assembles — the UI may show some text twice, which the event explains.
Exhausted retries return `RetriesExhausted{attempts, model, endpoint, source}`
so the failure is debuggable from the message alone; non-transient errors
(auth, credits, bad request) fail fast and keep their original shape so the
hosts' 401 handling (`/auth` card) still matches. Recovery on top: `/retry`
re-drives a transcript that ends mid-turn via `Agent::continue_turn` (no user
message re-appended, so history and fine-tuning exports stay clean), and
`--continue` reopens the newest session.

**Loop gates are named and conditional (`run_when`)** (2026-07-06)
A loop's `verify` became a list of *named* gates, each with `run_when`:
`always`, or `on_change` with glob patterns checked against a git content
snapshot of the workspace. A docs-only pass no longer pays for the full test
suite, while failed or blocked gates always re-run regardless of change
detection (a gate must never stay red because nothing changed). Single-gate
TOML files still load (serde back-compat), so shared loops keep working.

**Big files split by module-per-concern, fields stay private** (2026-07-07)
When a file outgrows ~1000 lines it splits into submodules by concern, not by
kind: `harness-agent` became `error`/`event`/`config` + an `agent/` directory
whose `turn` and `compression` children hold the loop and compression `impl`
blocks — *child* modules, so `Agent`'s fields stay private to the module tree
(Rust privacy is module-scoped; descendants see an ancestor's private items,
siblings don't). The CLI's `live/` and `main.rs` split the same way
(`turn`/`events`/`paint`/`completion`; `endpoint`/`turn`/`repl_loop`/`*_cmd`).
Public APIs are preserved via re-exports at the old paths; tests move beside
the code they exercise with shared `cfg(test)` `test_support` helpers.

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

**Subagents are one fan-out level deep** (2026-07-08)
`spawn_agents` lets the model run 2-6 parallel subagents, but a subagent can
never spawn its own fleet: the `FleetSpawner` snapshots the tool registry
*before* the fleet tool registers, and `Agent::side_agent` strips it again.
Recursion would turn one prompt into an unbounded tree of model calls — a cost
and debugging hazard with no compelling use we could name. Revisit only with a
depth budget and per-tree token accounting.

**Fleet transcripts are ephemeral; the display is the record** (2026-07-08)
Subagents run on in-memory stores: you watch their lanes live (TUI focus keys,
desktop panel) and only their *results* land in the session — as labeled tool
output or the injected review exchange. Persisting every subagent transcript
would bloat history.sqlite with reasoning nobody re-reads and clutter the
sidebar with sessions that aren't conversations. If post-hoc inspection is ever
needed, the seam is a `kind` column on sessions, not a behavior change.

**Usage is a timestamped call ledger; spend is a catalog-rate estimate** (2026-07-11)
Usage is captured inside `Agent`, immediately after a model call has a settled
provider count (or the calibrated fallback). This is the only layer that sees
every request: ordinary turns, repeated tool-loop prompts, detached review/fleet
agents, and one-shot completions. Detached transcripts stay ephemeral, but they
inherit the parent history store as a separate aggregate-usage destination.

`harness-store` persists one `usage_events` row per call with model, endpoint
source, input/output tokens, and timestamp. Append-only events make lifetime,
per-model, and local-calendar daily aggregation exact without reconstructing
dates from chat messages. Dollar values are not persisted as fact: model
catalog rates can change. Both front ends price any recorded model that appears
with token rates in the *configured endpoint's* catalog, so a self-hosted
Oxen-compatible endpoint can advertise its own prices. Models absent from that
catalog remain `—` rather than being silently called free.

**One process-wide `FleetHub` coordinates the CLI's fleet display** (2026-07-08)
The `spawn_agents` sink and the live composer must agree on who paints the
lanes (a background painter fights raw-mode composers for the cursor). Rather
than threading a handle through six call sites, `FleetHub::global()` is a
singleton slot: the sink publishes state, `set_live` says who owns the
terminal, and whichever display is active reads it. A global is defensible
here because the terminal itself is a process-wide singleton; the review
pipeline still uses a local hub since it owns both state and painter.

**The review pipeline runs on isolated side agents** (2026-07-08)
Each `/code-review` step gets a fresh `side_agent` (fleet lanes too): the
verifier must judge the finders' candidates against the *code*, not be
anchored by the finders' reasoning, and a review must never pollute the
session's context window. Step outputs thread through `{{previous}}` only.

**Durable history is unbounded; active memory is not** (2026-07-12)
The SQLite transcript remains verbatim and authoritative, while active agent
context has a resident ceiling and saves compacted context checkpoints. Cold
resume loads the checkpoint plus newer rows rather than materializing every
historical JSON value. Attachments, event payloads, desktop threads, inactive
sessions, and streaming channels have independent bounds because they serve
different model and UX constraints.

**Large producers are drained through bounded projections** (2026-07-12)
Child stdout/stderr and HTTP bodies are consumed incrementally while retaining
only a useful head and tail; process readers drain concurrently to avoid pipe
deadlocks. Durable history remains the source of truth, while transient UI
events carry capped display copies. Compression originals use workspace disk
storage with only a bounded CCR index in memory.
