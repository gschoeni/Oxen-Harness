# Document Map

**Purpose:** Central index of all project files — structure, descriptions, and loading guidance.

---

## Directory Structure

```
oxen-harness/
  00-project-brief.md        — Condensed project context. Load this first. (Tier 1)
  02-status.md               — Phase status, TODOs, what's next. (Tier 2A)
  03-decisions.md            — Working decisions & rationale. (Tier 2B)
  04-backlog.md              — Ideas, links, future exploration. (Tier 2C)
  DOCUMENT-MAP.md            — This file. File index and loading strategy.
  ARCHITECTURE.md            — Crate layering, the lifecycle of a turn, and how to extend.
  PROTOCOL.md                — The wire protocol (SSE events + REST) for building UIs on harness-server.
  AGENTS.md                  — The Ralph Wiggum dev loop + project conventions.
  CONTRIBUTING.md            — Contributor front door: orientation, build/verify, what a good change looks like.
  README.md                  — Public-facing repo README.
  LICENSE                    — Apache-2.0.
  Cargo.toml                 — Cargo workspace manifest.
  rust-toolchain.toml        — Pinned toolchain + components.
  crates/
    harness-core/            — Shared message/role types, defaults, bounded stream text, and copied-around helpers (slug/ellipsize/tail_chars, format_bytes/human_tokens, lenient JSON extraction). Leaf crate.
    harness-config/          — Single source for ~/.oxen-harness paths; atomic + schema-versioned config IO; .env secrets (dotenvy).
    harness-llm/             — Oxen.ai chat client: tool calling + SSE; lightweight auth; attachment store (content-addressed on-disk files) + hydration.
    harness-compress/        — Reversible context compression for tool output: JSON-array crushing, log/line collapsing, CCR store (`<<ccr:hash>>` markers resolved by retrieve_original).
    harness-tools/           — TypedTool trait, bounded process/HTTP capture, fs read/write/edit, glob/search, shell, git, web, questions, canvas, plans, skills, and custom HTTP tools.
    harness-store/           — SQLite history (verbatim) + JSONL export; rusqlite_migration schema versioning; rich session metadata.
    harness-oxen/            — Version config/data + export/share traces via the `oxen` CLI (testable Runner shell-out; no liboxen).
    harness-local/           — Local models: extensible GGUF catalog (Qwen3 + Bonsai), downloads + disk tracking, llama-server launcher.
    harness-theme/           — Configurable themes (palette + voice): built-ins, TOML/JSON load/save with partial overrides, active-theme store.
    harness-agent/           — The agent (Ralph) loop (llm + tools + store); the fleet (run_fleet: N parallel detached subagents) + the model-facing spawn_agents tool (FleetSpawner/FleetSink).
    harness-protocol/        — The transport-neutral wire types every UI speaks: the tagged ProtocolEvent enum + command DTOs (serde + JSON Schema); tests/wire.rs is the spec.
    harness-host/            — The transport-agnostic host layer: SessionService (multi-session agent cache, turn driving, question/approval round-trips, model swaps, review/loop runners), generic over an EventSink; the Tauri app and HTTP server are both thin adapters over it.
    harness-server/          — The agent backend as a standalone HTTP server (axum): REST commands + an SSE protocol-event stream with Last-Event-ID replay; bearer-token auth; see PROTOCOL.md.
    harness-runtime/         — Front-end-agnostic services shared by CLI/desktop: connection settings + secrets (.env), cloud-model catalog, tool prefs + custom tools, skill discovery/prefs/authoring, opt-in Oxen versioning of ~/.oxen-harness.
    harness-loop/            — Goal-driven, self-verifying loops (discover→verify→iterate): LoopSpec/Verify, runner, journal, shareable store + built-ins.
    harness-review/          — Configurable code-review pipeline: ordered prompt steps (find→verify→report default), diff targets (uncommitted / vs base branch), isolated side-agent runner (fan-out steps run as a parallel fleet), structured findings.
    harness-cli/             — The `oxen-harness` interactive REPL binary. Slash-command handlers live in commands/ (auth, compression, loops, model, oxen, queue, review, theme, trace); the live sticky-bottom composer in live/; the fleet lanes display in fleet_ui.rs/fleet_sink.rs. Top-level subcommands: theme, loop, trace, oxen.
  app/                       — Tauri v2 desktop app (separate project, excluded
                               from the core workspace). See app/README.md.
    src-tauri/src/           — Rust bridge, a thin adapter over harness-host:
                               lib.rs (module map + run()), state.rs (AppState =
                               SessionService + TauriSink + native-preview hooks),
                               events.rs (the few Tauri-only payloads), commands/
                               (the #[tauri::command] handlers, one module per
                               feature, delegating to the service).
    src/                     — React + TS chat UI (features/, lib/, components/).
  examples/
    web-chat.html            — Dependency-free single-file web client for the HTTP
                               protocol (SSE + REST); the "build your own UI" demo.
  plans/                     — Actionable execution docs. Pull in per-topic.
    archive/                 — Deprecated plans, kept for historical reference.
```

## Loading Guide

Context is finite. Load what's relevant, not everything.

### Tier 1 — Always loaded

| Document | Description |
|----------|-------------|
| `00-project-brief.md` | Vision, goals, architecture, current phase. Enough to orient any conversation. |

### Tier 2 — Pull in for working sessions

| Document | When to pull in |
|----------|-----------------|
| `02-status.md` | Any active work — phase status, TODOs, what's next |
| `03-decisions.md` | Implementation work — current decisions with rationale |
| `04-backlog.md` | Planning sessions — ideas, links, future exploration |
| `AGENTS.md` | Any contribution — the dev loop and conventions to follow |

### Plans — Pull in per-topic

| Document | When to pull in |
|----------|-----------------|
| _(none yet)_ | Phase plans will be added here as work begins |

### Reference — Pull in when you need specifics

| Document | When to pull in |
|----------|-----------------|
| _(none yet)_ | API summaries / research land here |

## Maintenance

When adding a new file to the project, update this document map.
