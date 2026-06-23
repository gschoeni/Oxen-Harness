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
  AGENTS.md                  — The Ralph Wiggum dev loop + project conventions.
  README.md                  — Public-facing repo README.
  LICENSE                    — Apache-2.0.
  Cargo.toml                 — Cargo workspace manifest.
  rust-toolchain.toml        — Pinned toolchain + components.
  crates/
    harness-core/            — Shared message/role types and defaults (leaf crate).
    harness-llm/             — Oxen.ai chat client: tool calling + SSE; lightweight auth.
    harness-tools/           — fs read/write/edit, glob find, regex search, sandboxed shell, git, Brave web search, ask_user_question (clarifying questions).
    harness-store/           — SQLite history (verbatim) + JSONL export.
    harness-local/           — Local models: Qwen3 GGUF catalog, downloads + disk tracking, llama-server launcher.
    harness-theme/           — Configurable themes (palette + voice): built-ins, TOML/JSON load/save with partial overrides, active-theme store.
    harness-agent/           — The agent (Ralph) loop (llm + tools + store).
    harness-cli/             — The `oxen-harness` interactive REPL binary (incl. picker, /theme + theme subcommand).
  app/                       — Tauri v2 desktop app (separate project, excluded
                               from the core workspace). src-tauri/ = Rust bridge,
                               dist/ = chat UI. See app/README.md.
  plans/                     — Actionable execution docs. Pull in per-topic.
    archive/                 — Deprecated plans, kept for historical reference.
  reference/                 — Look-up material. Pull in when in the weeds.
  output/                    — Finished or near-finished deliverables.
  ongoing/                   — Operational runbooks. Living docs for recurring processes.
  journal/                   — Historical archives & templates.
  scratch/                   — Working artifacts. Graduate to plans/ or reference/ when complete.
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
