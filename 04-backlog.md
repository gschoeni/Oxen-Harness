# Backlog & Future Exploration

**Purpose:** Ideas, links, tools, half-formed thoughts, things to consider later. Pull in during planning sessions, not implementation.
**Updated:** 2026-07-08

---

## Ideas

- **MCP support** — let the harness consume Model Context Protocol servers as additional tools.
- ~~**Sub-agents / parallel tasks**~~ — ✅ shipped as the fleet (2026-07-08): `harness_agent::fleet::run_fleet` + the `spawn_agents` tool, review find fan-out, live lanes in both front ends. Remaining ideas: persistent subagent transcripts (a `kind` column on sessions), depth budgets if recursion is ever wanted.
- **Permission modes** — optional "ask before edit/shell/commit" mode for cautious use, even though edits are allowed by default.
- **Checkpoints / undo** — snapshot the working dir per turn so a bad edit can be rolled back.
- **Session replay** — re-run a recorded JSONL transcript against a different model to compare behavior.
- ~~**Prompt caching**~~ — ✅ shipped (2026-07-17): `cache_control` anchors on the last two content-bearing user/assistant messages (`PromptCacheMode::Auto`, Anthropic-family models), verified ~99% billed-input reduction through hub.oxen.ai; cached-usage parsing, per-call attribution on `usage_events` (M8), `requests.jsonl` cache diagnostics, loop guard, session token budgets, summary-model routing — see `03-decisions.md`. Remaining ideas: surface cache stats in TokenMeter//usage, cache-rate-aware pricing for OpenAI-style endpoints, Settings page for `limits.json`.
- **Switch local models mid-session** — today `--local <id>` is chosen at startup; allow `/model <local-id>` to stop the current `llama-server` and start another without restarting the session. Could also auto-detect GGUFs already in `~/.cache/huggingface`, `~/.lmstudio/models`, or `~/.ollama/models`.
- **Per-model llama-server tuning** — expose context size / GPU layers / quant choice per catalog entry, and a way to add custom (non-catalog) GGUFs.
- **Theme gallery + sharing** — a curated, importable set of community themes (e.g. a small index the CLI/app can browse and pull), beyond the five built-ins.
- **Live theme preview while editing** — apply changes as you tweak a theme file; per-theme palette swatches in the app's theme list.
- **Editable/iterate-on themes via the model** — "make the greens warmer", "calmer voice" follow-ups that patch the active theme instead of regenerating from scratch.
- **Theme the banner ASCII art by vibe** — let generation produce custom `banner_art`/`exit_art`/`wordmark` reliably (currently inherits defaults unless the model fills them in well).

## Tools & Resources to Evaluate

- [Oxen.ai chat completions docs](https://docs.oxen.ai/examples/inference/chat_completions)
- [`liboxen` crate](https://crates.io/crates/liboxen) — auth + data versioning
- [Tauri v2](https://v2.tauri.app/) — desktop app shell
- REPL line editing: `rustyline` vs `reedline`
- CLI args: `clap`
- SQLite: `rusqlite` (bundled) vs `sqlx`
- HTTP/SSE: `reqwest` + `eventsource-stream` (or manual SSE parsing)
- Test HTTP mocking: `mockito` / `wiremock`

## Integration / Architecture Ideas

- Use Oxen data versioning to store exported fine-tuning datasets directly in an Oxen repo.
- Share the agent loop core between the CLI and the Tauri app (Tauri commands call `harness-core`).
- Pluggable tool registry so forks can add custom tools without touching core.
- Vision support — the Oxen API accepts image content; could feed screenshots to the agent.
