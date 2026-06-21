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
