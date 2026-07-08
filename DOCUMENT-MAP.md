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
  AGENTS.md                  — The Ralph Wiggum dev loop + project conventions.
  CONTRIBUTING.md            — Contributor front door: orientation, build/verify, what a good change looks like.
  README.md                  — Public-facing repo README.
  LICENSE                    — Apache-2.0.
  Cargo.toml                 — Cargo workspace manifest.
  rust-toolchain.toml        — Pinned toolchain + components.
  crates/
    harness-core/            — Shared message/role types and defaults (leaf crate).
    harness-config/          — Single source for ~/.oxen-harness paths; atomic + schema-versioned config IO; .env secrets (dotenvy).
    harness-llm/             — Oxen.ai chat client: tool calling + SSE; lightweight auth; attachment store (content-addressed on-disk files) + hydration.
    harness-compress/        — Reversible context compression for tool output: JSON-array crushing, log/line collapsing, CCR store (`<<ccr:hash>>` markers resolved by retrieve_original).
    harness-tools/           — TypedTool trait (schemas derived from typed args), fs read/write/edit, glob find, regex search, sandboxed shell, git, Brave web search, ask_user_question, canvas, update_plan, skills (SKILL.md loaded on demand), custom HTTP tools.
    harness-store/           — SQLite history (verbatim) + JSONL export; rusqlite_migration schema versioning; rich session metadata.
    harness-oxen/            — Version config/data + export/share traces via the `oxen` CLI (testable Runner shell-out; no liboxen).
    harness-local/           — Local models: Qwen3 GGUF catalog, downloads + disk tracking, llama-server launcher.
    harness-theme/           — Configurable themes (palette + voice): built-ins, TOML/JSON load/save with partial overrides, active-theme store.
    harness-agent/           — The agent (Ralph) loop (llm + tools + store).
    harness-runtime/         — Front-end-agnostic services shared by CLI/desktop: connection settings + secrets (.env), cloud-model catalog, tool prefs + custom tools, skill discovery/prefs/authoring, opt-in Oxen versioning of ~/.oxen-harness.
    harness-loop/            — Goal-driven, self-verifying loops (discover→verify→iterate): LoopSpec/Verify, runner, journal, shareable store + built-ins.
    harness-review/          — Configurable code-review pipeline: ordered prompt steps (find→verify→report default), diff targets (uncommitted / vs base branch), isolated side-agent runner, structured findings.
    harness-cli/             — The `oxen-harness` interactive REPL binary (incl. picker, live sticky-bottom composer, /theme + theme subcommand, /loop + loop subcommand, /code-review, `trace export` to share a conversation via Oxen, `oxen` subcommand to version config).
  app/                       — Tauri v2 desktop app (separate project, excluded
                               from the core workspace). src-tauri/ = Rust bridge,
                               dist/ = chat UI. See app/README.md.
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
