# Project Status & Roadmap

**Purpose:** Where we are, what's next, what's done. Pull this in for any working session.
**Updated:** 2026-06-21

---

## Phase Overview

| Phase | Goal | Status |
|-------|------|--------|
| **0** | Scaffold: workspace, crates, first green tests, KB, license | ✅ Complete |
| **2** | `harness-tools`: fs read/write/edit/search, sandboxed shell, git | ✅ Complete |
| **3** | `harness-store`: SQLite history (verbatim) + JSONL export | ✅ Complete |
| **1** | `harness-llm`: Oxen client — tool-calling types, `liboxen` auth, SSE streaming | In progress |
| **4** | `harness-core`: wire the agent (Ralph) loop together | Not started |
| **5** | `harness-cli`: interactive streaming REPL | Not started |
| **6** | `harness-tauri`: cross-platform desktop app | Not started |

> Build order note: independent crates (tools, store) are built before the
> `liboxen`-heavy LLM client to keep each phase fast to verify and to isolate
> the heavy build.

---

## Phase 0 — Scaffold

**Status:** ✅ Complete

- [x] Workspace `Cargo.toml` with shared workspace deps
- [x] Five crate skeletons (`core`, `llm`, `tools`, `store`, `cli`)
- [x] First green tests in each crate (role wire format, URL builder, tool trait, JSONL export)
- [x] Apache-2.0 `LICENSE`, `.gitignore`, `rust-toolchain.toml`
- [x] `README.md` + `AGENTS.md` (Ralph loop as the dev process)
- [x] Knowledge base filled in (`00`/`02`/`03`/`04`/`DOCUMENT-MAP`)
- [x] Verification loop green (`fmt`, `clippy`, tests)
- [x] `git init` + initial commit

---

## Phase 2 — harness-tools

**Status:** ✅ Complete (17 tests passing)

- [x] `Workspace` sandbox: path resolution rejecting escapes outside the root
- [x] `Tool` trait, `ToolRegistry` (dispatch by name), OpenAI tool definitions
- [x] fs tools: `read_file`, `write_file`, `edit_file` (unique-match), `search_files`
- [x] `run_shell`: command execution pinned to workspace root
- [x] `git`: status / diff / log / commit

---

## Phase 3 — harness-store

**Status:** ✅ Complete (7 tests passing)

- [x] SQLite schema: `sessions` + `messages` (verbatim `raw_json`, per-session `seq`)
- [x] `create_session` / `append_message` (any serializable message) / `messages`
- [x] Tool-call messages stored and read back verbatim
- [x] `export_jsonl` (one verbatim message per line) for fine-tuning
- [x] Persists across reopen

---

## Phase 1 — harness-llm

**Status:** In progress

- [ ] OpenAI-compatible request/response types (incl. `tools`, `tool_calls`, `tool_choice`)
- [ ] Auth resolution via `liboxen` (`AuthConfig::auth_token_for_host`) + `OXEN_API_KEY` override
- [ ] Non-streaming chat completion call (mocked with `mockito` in tests)
- [ ] SSE streaming of assistant tokens
- [ ] Tool-call parsing (`finish_reason == "tool_calls"`)

---

## Future Phases (Summary)

Tools (Phase 2) and the verbatim SQLite store (Phase 3) are independent and can
be built in parallel after Phase 1. Phase 4 wires the loop; Phase 5 ships the
REPL; Phase 6 adds the Tauri app.

---

## Infrastructure TODOs (Cross-Phase)

- [ ] CI workflow running the verification loop (fmt + clippy + tests) on push.
- [ ] Document the `cmake` / C++ toolchain prereq prominently (needed by `liboxen`).
- [ ] Decide config file location/format (`~/.config/oxen-harness/config.toml`).
