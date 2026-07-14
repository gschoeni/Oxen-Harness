# Architecture

How `oxen-harness` fits together, and where to reach when you want to extend it.

For a one-line-per-file index see [`DOCUMENT-MAP.md`](DOCUMENT-MAP.md); this
document explains the *shape* of the system and its seams.

## The layers

The workspace is a stack of focused crates. Every crate depends only on ones
below it, so the dependency graph is acyclic and each layer is testable on its
own:

```
                        ┌─────────────┐   ┌────────────────────┐
  front ends            │ harness-cli │   │ app/  (Tauri v2)   │
                        └──────┬──────┘   └─────────┬──────────┘
                               │                    │
                               │                    └───────────────┐
  orchestration  ┌─────────────┴─────┐  ┌────────────────┐  ┌───────┴─────────┐
                 │    harness-loop   │  │ harness-review │  │ harness-runtime │  (shared config:
                 │ (goal/verify loop)│  │ (find→verify→  │  │                 │   connection, models,
                 └─────────┬─────────┘  │  report steps) │  └───┬─────────────┘   tool prefs)
                           │            └───────┬────────┘      │
  agent                 ┌──┴────────────────────┴───────────────┴─┐
                        │             harness-agent               │  (the turn loop: llm + tools
                        │      llm + tools + store + budget       │   + store; stream, dispatch,
                        └───┬───────────┬───────────┬─────────────┘   compact)
                            │           │           │
  capabilities   ┌──────────┴─┐ ┌───────┴─────┐ ┌───┴─────────┐ ┌────────────────┐
                 │ harness-llm│ │harness-tools│ │harness-store│ │ harness-theme  │
                 │  (client)  │ │  (Tool reg) │ │  (SQLite)   │ │ harness-local  │
                 └──────┬─────┘ └──────┬──────┘ └──────┬──────┘ │ harness-oxen   │
                        │              │               │        │harness-compress│
                        │              │               │        └──────┬─────────┘
  foundation      ┌─────┴──────────────┴───────────────┴───────────────┴─────┐
                  │  harness-core (Message/Role, slug, format_bytes)          │
                  │  harness-config (~/.oxen-harness paths, atomic versioned  │
                  │                  config IO, .env secrets)                 │
                  └──────────────────────────────────────────────────────────┘
```

- **`harness-core`** — the leaf. Provider-agnostic `Message`/`Role`, the pinned
  Oxen.ai defaults, and the tiny helpers that would otherwise be copied around:
  `text::{slug, ellipsize, collapse_ws, tail_chars}`, `fmt::{format_bytes,
  human_tokens}`, and `json::first_object` (lenient pull-the-JSON-out-of-a-model-reply).
- **`harness-config`** — the single source of truth for where state lives
  (`~/.oxen-harness/…`), atomic + schema-versioned JSON IO, and `.env` secrets.
- **capabilities** — each an independent, self-contained skill: the LLM client
  and streaming (`harness-llm`), the built-in tools (`harness-tools`), verbatim
  history + fine-tuning export (`harness-store`), themes (`harness-theme`),
  local llama.cpp models (`harness-local`), Oxen versioning (`harness-oxen`),
  and reversible tool-output compression (`harness-compress`).
- **`harness-agent`** — the turn loop that wires an LLM client, a tool registry,
  and a store together, plus token budgeting and context compaction. Also home
  to the **fleet**: `fleet::run_fleet` runs N detached subagents in parallel
  (semaphore-capped, one multiplexed event stream, per-task outcomes), and the
  `spawn_agents` tool exposes that to the model from any turn — hosts inject a
  `FleetSink` to render the lanes.
- **`harness-loop`** / **`harness-review`** / **`harness-runtime`** — the
  goal/verify iteration loop on top of the agent; the configurable code-review
  pipeline (ordered prompt steps — a parallel three-lens find, then verify →
  report by default — every reviewer on an isolated side agent, ending in
  structured findings); and the front-end-agnostic configuration both UIs share.
- **front ends** — the interactive CLI, and the Tauri desktop app (a separate
  workspace under `app/` so the core verification loop stays fast). Both drive
  the *same* `harness-agent`, so behavior can't drift between them.

## The lifecycle of a turn

```
user input ─▶ harness-cli ─▶ Agent::run_turn
                                 │
                                 ├─▶ budget check → compact transcript if needed
                                 ├─▶ harness-llm: stream a chat completion
                                 │        └─ emits AgentEvent::{Token, ToolStart, ToolEnd, …}
                                 ├─▶ for each tool call: ToolRegistry::invoke
                                 │        └─ result appended to the transcript
                                 ├─▶ harness-store: persist every message verbatim
                                 └─▶ loop until the model stops calling tools
```

The agent communicates with the front end through a stream of `AgentEvent`s
(tokens, tool starts/ends, usage), so the CLI and desktop app render the same
run in their own idioms. Tool failures come back as ordinary tool-result
messages, so the model can read the error and self-correct within the turn.

## Extending it

The crate seams are designed so common extensions touch one place:

| To add… | Do this |
|---|---|
| **A tool** | Implement the [`TypedTool`](crates/harness-tools/src/lib.rs) trait (typed args struct; doc comments become the model-facing schema), expose a `*_TOOL` name constant, register it with `with_typed` in the `ToolRegistry`, and add its name to the registry completeness test. Full recipe: ["Adding a built-in tool"](AGENTS.md#adding-a-built-in-tool) in AGENTS.md. |
| **A skill** | No code: drop a `SKILL.md` folder into `~/.oxen-harness/skills/` (global) or `<repo>/.oxen-harness/skills/` (project), or use Settings → Skills in the desktop app. Parsing + the `skill` tool live in [`harness-tools/src/skill.rs`](crates/harness-tools/src/skill.rs); discovery/prefs in [`harness-runtime/src/skills.rs`](crates/harness-runtime/src/skills.rs). See ["Extending the agent"](README.md#extending-the-agent). |
| **A built-in theme** | Add a factory in [`harness-theme/src/builtins.rs`](crates/harness-theme/src/builtins.rs) (overlay a small patch on `Theme::default()`) and list it in `all()`. Theme *data* all lives in that module. |
| **A config file** | Define a serde struct and lean on `harness-runtime`'s `config::{load_or_default, write_and_snapshot}`; you get atomic writes + Oxen snapshotting for free. |
| **A cloud model** | Add an entry to `harness_runtime::models::builtins()`. |
| **A local model** | No Rust required: add an exact GGUF entry to [`harness-local/assets/catalog.json`](crates/harness-local/assets/catalog.json), or install/override one in `~/.oxen-harness/local-models.json`. Set `derive_quants: true` only when the repo publishes the standard Q8→Q3 filename ladder; native formats default to the one declared file. Loading/merging lives in [`harness-local/src/catalog.rs`](crates/harness-local/src/catalog.rs). |
| **A theme/loop field** | Add the field (serde `default`) — partial-override loading means existing files keep working. |
| **A slash command** | Three synchronized spots in `harness-cli`: a `Command` variant + its parse arm in [`repl.rs`](crates/harness-cli/src/repl.rs), a dispatch arm in [`repl_loop.rs`](crates/harness-cli/src/repl_loop.rs) calling your new [`commands/`](crates/harness-cli/src/commands/mod.rs) module, and a `SLASH_COMMANDS` entry in [`live/mod.rs`](crates/harness-cli/src/live/mod.rs) (a test fails if the three drift). Model a command with subcommands on [`commands/loops.rs`](crates/harness-cli/src/commands/loops.rs). |
| **A review step** | No code: edit the pipeline in `~/.oxen-harness/code-review.json` or Settings → Code review — steps are ordered prompts (placeholders: `{{target}}`, `{{diff}}`, `{{previous}}`, `{{max_findings}}`), and any step can carry parallel `agents`. Defaults + schema live in [`harness-review/src/config.rs`](crates/harness-review/src/config.rs). |
| **A subagent fan-out** | Call [`harness_agent::fleet::run_fleet`](crates/harness-agent/src/fleet.rs) with `SubagentTask`s and a spawn source (`\|\| agent.side_agent()`), render its `FleetEvent`s; or let the model do it — the `spawn_agents` tool ([`fleet_tool.rs`](crates/harness-agent/src/fleet_tool.rs)) is registered by both hosts, with a `FleetSink` per host for the lanes display. |

## Conventions

- **Errors** are per-crate `thiserror` enums; capability errors flow up into the
  agent/runtime enums via `#[from]`.
- **Config is never a hard failure** — a missing or unreadable file reads back as
  defaults, so a fresh install just works.
- **Tests live beside the code** they cover, in `#[cfg(test)] mod tests`.
- **Lints**: a shared [`[workspace.lints]`](Cargo.toml) policy every crate
  inherits — `unsafe_code` is a warning (the single audited FFI call is
  annotated), and `dbg!`/`todo!` are denied. `cargo clippy --workspace
  --all-targets` is warning-free.
