# Backlog & Future Exploration

**Purpose:** Ideas, links, tools, half-formed thoughts, things to consider later. Pull in during planning sessions, not implementation.
**Updated:** 2026-06-21

---

## Ideas

- **MCP support** — let the harness consume Model Context Protocol servers as additional tools.
- **Sub-agents / parallel tasks** — spawn scoped agents for isolated subtasks (compare OpenHands).
- **Permission modes** — optional "ask before edit/shell/commit" mode for cautious use, even though edits are allowed by default.
- **Checkpoints / undo** — snapshot the working dir per turn so a bad edit can be rolled back.
- **Session replay** — re-run a recorded JSONL transcript against a different model to compare behavior.
- **Prompt-caching / context compaction** — summarize old turns to control context growth.
- **Switch local models mid-session** — today `--local <id>` is chosen at startup; allow `/model <local-id>` to stop the current `llama-server` and start another without restarting the session. Could also auto-detect GGUFs already in `~/.cache/huggingface`, `~/.lmstudio/models`, or `~/.ollama/models`.
- **Per-model llama-server tuning** — expose context size / GPU layers / quant choice per catalog entry, and a way to add custom (non-catalog) GGUFs.
- **Auto-install `llama-server`** — offer to `brew install llama.cpp` / download a release from within the harness instead of just printing a hint.
- **Theme gallery + sharing** — a curated, importable set of community themes (e.g. a small index the CLI/app can browse and pull), beyond the three built-ins.
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
