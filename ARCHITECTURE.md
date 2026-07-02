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
  orchestration      ┌─────────┴──────────┐  ┌──────┴───────────┐
                     │    harness-loop     │  │  harness-runtime │  (shared config:
                     │ (goal/verify loop)  │  │                  │   connection, models,
                     └─────────┬───────────┘  └──────┬───────────┘   tool prefs)
                               │                     │
  agent                 ┌──────┴─────────────────────┴──┐
                        │         harness-agent          │  (the turn loop:
                        │   llm + tools + store + budget)│   stream, dispatch, compact)
                        └───┬───────────┬───────────┬────┘
                            │           │           │
  capabilities   ┌──────────┴─┐ ┌───────┴─────┐ ┌───┴─────────┐ ┌──────────────┐
                 │ harness-llm│ │harness-tools│ │harness-store│ │ harness-theme│
                 │  (client)  │ │  (Tool reg) │ │  (SQLite)   │ │  harness-local│
                 └──────┬─────┘ └──────┬──────┘ └──────┬──────┘ │  harness-oxen │
                        │              │               │        └──────┬───────┘
  foundation      ┌─────┴──────────────┴───────────────┴───────────────┴─────┐
                  │  harness-core (Message/Role, slug, format_bytes)          │
                  │  harness-config (~/.oxen-harness paths, atomic versioned  │
                  │                  config IO, .env secrets)                 │
                  └──────────────────────────────────────────────────────────┘
```

- **`harness-core`** — the leaf. Provider-agnostic `Message`/`Role`, the pinned
  Oxen.ai defaults, and dependency-free string/format helpers (`text::slug`,
  `fmt::format_bytes`) that would otherwise be copied around.
- **`harness-config`** — the single source of truth for where state lives
  (`~/.oxen-harness/…`), atomic + schema-versioned JSON IO, and `.env` secrets.
- **capabilities** — each an independent, self-contained skill: the LLM client
  and streaming (`harness-llm`), the built-in tools (`harness-tools`), verbatim
  history + fine-tuning export (`harness-store`), themes (`harness-theme`),
  local llama.cpp models (`harness-local`), and Oxen versioning (`harness-oxen`).
- **`harness-agent`** — the turn loop that wires an LLM client, a tool registry,
  and a store together, plus token budgeting and context compaction.
- **`harness-loop`** / **`harness-runtime`** — the goal/verify iteration loop on
  top of the agent, and the front-end-agnostic configuration both UIs share.
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
| **A tool** | Implement the [`TypedTool`](crates/harness-tools/src/lib.rs) trait (typed args struct; doc comments become the model-facing schema), expose a `*_TOOL` name constant, register it with `with_typed` in the `ToolRegistry`, and add its name to the registry completeness test. Full recipe: ["Adding a tool"](README.md#adding-a-tool) in the README. |
| **A skill** | No code: drop a `SKILL.md` folder into `~/.oxen-harness/skills/` (global) or `<repo>/.oxen-harness/skills/` (project), or use Settings → Skills in the desktop app. Parsing + the `skill` tool live in [`harness-tools/src/skill.rs`](crates/harness-tools/src/skill.rs); discovery/prefs in [`harness-runtime/src/skills.rs`](crates/harness-runtime/src/skills.rs). See ["Adding a skill"](README.md#adding-a-skill). |
| **A built-in theme** | Add a factory in [`harness-theme/src/builtins.rs`](crates/harness-theme/src/builtins.rs) (overlay a small patch on `Theme::default()`) and list it in `all()`. Theme *data* all lives in that module. |
| **A config file** | Define a serde struct and lean on `harness-runtime`'s `config::{load_or_default, write_and_snapshot}`; you get atomic writes + Oxen snapshotting for free. |
| **A cloud model** | Add an entry to `harness_runtime::models::builtins()`. |
| **A local model** | Add a `ModelSpec` to the curated catalog in [`harness-local/src/catalog.rs`](crates/harness-local/src/catalog.rs). |
| **A theme/loop field** | Add the field (serde `default`) — partial-override loading means existing files keep working. |

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
