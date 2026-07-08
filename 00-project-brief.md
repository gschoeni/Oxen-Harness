# oxen-harness — Project Brief

**Purpose:** Condensed project context for all conversations. Vision, goals, current state, key decisions.
**Created:** 2026-06-21 · **Updated:** 2026-07-07

---

## Vision

`oxen-harness` is an open source, hackable agentic coding harness — like Claude Code or Codex (OpenHands is a close comp) — written in Rust and powered by Oxen.ai. It runs an agent loop against any chat-completions model with tool calling, and records every turn so you can later fine-tune a model on your own coding traces.

## Goals

- A `claude`-style **interactive CLI** that edits code, runs commands, and drives git via tool calling.
- A cross-platform **Tauri v2 desktop app** (UI like Cursor agents / Claude Cowork), sharing the same agent core.
- **Bring-your-own-model**: any Oxen.ai chat-completions model with tool calling; default `claude-opus-4-8`.
- **Exportable history**: full SQLite log of conversations + tool calls, with a JSONL exporter for fine-tuning.
- Stay small, readable, and forkable (Apache-2.0).

## Architecture / Approach

Single Cargo workspace of focused crates (layers + lifecycle: `ARCHITECTURE.md`):

- `harness-core` — shared message/role types, defaults, and string/format helpers (leaf crate).
- `harness-config` — `~/.oxen-harness` paths, atomic + schema-versioned JSON config IO, `.env` secrets.
- `harness-llm` — Oxen.ai chat completions client (tool calling + SSE streaming); lightweight auth (env var or `auth_config.toml`); on-disk attachment store + hydration.
- `harness-tools` — the `TypedTool` trait (schemas derive from typed args structs) + built-ins: read (line-numbered)/write/edit files, glob find, regex search, sandboxed shell (with timeout), git, Brave web search, `ask_user_question`, canvas documents, `update_plan`, skills, and user-defined HTTP tools.
- `harness-compress` — reversible context compression for stale tool output (off/audit/on; `<<ccr:hash>>` markers restored by `retrieve_original`).
- `harness-store` — SQLite history (verbatim) + JSONL export for fine-tuning.
- `harness-local` — local models: curated Qwen3 GGUF catalog, managed downloads + disk tracking, `llama-server` launcher (OpenAI-compatible, no key).
- `harness-theme` — configurable, shareable themes (palette + voice + style) used by both front ends: five built-ins (Oregon Trail/Midnight/Synthwave/New York Times/Cupertino), TOML/JSON load+save with partial overrides, and the active-theme store under `~/.oxen-harness/`.
- `harness-oxen` — version config/data + export/share conversation traces via the `oxen` CLI.
- `harness-agent` — the agent (Ralph) loop, wiring llm + tools + store together, with token budgeting, compaction, retry, and compression.
- `harness-loop` — goal-driven, self-verifying loops (gates, journal, stop conditions) atop the agent.
- `harness-runtime` — front-end-agnostic runtime services shared by the CLI and desktop app (connection, model catalog, tool prefs, skills).
- `harness-cli` — the `oxen-harness` interactive REPL binary (live sticky-bottom composer, pickers, themes, loops, traces).

The agent loop: call model → if `finish_reason == tool_calls`, execute tools, append `tool` messages, repeat → stop on `stop`/`length`.

## Current Phase

**All core phases shipped (0–9) plus the extensibility, compression, recovery,
code-review, and fleet pushes:** the Oxen client, the sandboxed tool set, the
verbatim SQLite store, the agent loop (with context compaction, reversible
compression, and transient-failure retry), self-verifying loops with
conditional gates, a configurable code-review pipeline, a parallel-subagent
fleet (`spawn_agents` from any turn + `/code-review` fan-out, with live lanes
in both front ends), themes, local models, skills/custom tools, and both front
ends (CLI + Tauri desktop app) are built and tested — 528 Rust tests + 163
frontend tests passing, CI green on push. See `02-status.md` for the full
ledger.

## Key Decisions

- **Oxen.ai is the default provider** (OpenAI-compatible API); models are swappable, default `claude-opus-4-8`. **Local models** (Qwen3 via llama.cpp) are a first-class alternative through `--local <id>` / `models` subcommands / the desktop UI.
- **Lightweight auth** (no `liboxen` dependency): `OXEN_API_KEY` or parse `auth_config.toml` directly. (`liboxen` won't build without its heavy DuckDB/RocksDB tree — see `03-decisions.md`.)
- **SQLite history, stored verbatim**, with a JSONL exporter for fine-tuning.
- **SSE streaming in the REPL from day one**; single working directory per session; cross-platform from day one.
- **Themeable + shareable**: the CLI/app personality is data (palette + voice + style), persisted under `~/.oxen-harness/`, exportable/importable as a single TOML file, and creatable by "vibe" via the model. Default is Oregon Trail; five ship built in.
- Built with **The Ralph Wiggum loop** (tests-first, objective checks). Details in `03-decisions.md`.
