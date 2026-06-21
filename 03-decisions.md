# Working Decisions & Rationale

**Purpose:** Currently relevant decisions with enough "why" to be useful during implementation. For full deep-dive analysis, cite the source in each entry.
**Updated:** 2026-06-21

---

## Provider & Models

**Oxen.ai is the only provider** (2026-06-21)
The harness targets the Oxen.ai OpenAI-compatible chat completions API (`https://hub.oxen.ai/api/ai`, endpoint `/chat/completions`). Any model with tool calling works; the provider is fixed but the model is swappable. Default model is `claude-opus-4-8`.
-> *Full context: https://docs.oxen.ai/examples/inference/chat_completions*

## Auth & Config

**Depend on `liboxen` for auth** (2026-06-21)
We use the `liboxen` crate's auth functions, with an `OXEN_API_KEY` env var as an override. Tradeoff: `liboxen` is heavy (pulls in duckdb/rocksdb/polars/aws-sdk and needs `cmake` + a C/C++ toolchain), so it is **isolated to `harness-llm`** to keep other crates light. The upside is consistency with the Oxen ecosystem and a path to data versioning later. Lightweight config-file parsing remains the fallback if build cost becomes painful.

## History & Export

**SQLite, stored verbatim, JSONL export** (2026-06-21)
Conversation messages and tool inputs/outputs are persisted verbatim (no truncation caps) in SQLite, so traces are complete enough to fine-tune on. A JSONL exporter emits one message object per line for dataset building. Verbatim storage means large file/shell outputs are stored in full — acceptable for a local, single-user tool.

## Agent Loop

**Objective-check-driven (Ralph Wiggum) loop** (2026-06-21)
Development follows a tight test-first loop: read spec → write/adjust a test → smallest change → write to disk → run `fmt`/`clippy`/tests → fix root cause → stop on green. The *runtime* agent loop mirrors this: call model → execute any `tool_calls` → append `tool` messages → repeat until `finish_reason` is `stop`/`length`.
-> *Full context: `AGENTS.md`*

## UX & Scope

**CLI first, Tauri later; stream from day one** (2026-06-21)
Ship a `claude`-style interactive REPL with live SSE token streaming first; add the cross-platform Tauri v2 desktop app once the core loop is stable. Sessions are scoped to a single working directory. Shell commands run in a sandboxed working directory where possible, but the model decides. File edits are allowed by default. Cross-platform support is a constraint from day one.

## Tooling

**Single Cargo workspace, focused crates** (2026-06-21)
`harness-core` / `harness-llm` / `harness-tools` / `harness-store` / `harness-cli`. Tests use `mockito` to fake the HTTP endpoint (deterministic, offline). `cargo nextest` is the preferred test runner.
