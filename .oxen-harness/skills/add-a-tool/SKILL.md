---
name: add-a-tool
description: Adds a new built-in tool to this repo's agent following the TypedTool recipe. Use when asked to give the agent a new capability, tool, or ability.
---

# Adding a built-in tool to oxen-harness

Follow these steps exactly — the codebase enforces most of them with tests.

1. **Create the module** `crates/harness-tools/src/<tool_name>.rs`:
   - A `pub const <NAME>_TOOL: &str = "<tool_name>"` name constant
     (lowercase + underscores).
   - An args struct deriving `serde::Deserialize` + `schemars::JsonSchema`.
     Doc comments on each field become the schema descriptions the model
     reads — write them for the model (include defaults and units).
     Optional arguments are `Option<T>`; enums use
     `#[serde(rename_all = "snake_case")]`.
   - The tool struct implementing `TypedTool` (`const NAME`, `type Args`,
     `description()`, `async fn run`). If it touches files, take a
     `Workspace` in the constructor and resolve every path through
     `workspace.resolve(..)` so it can't escape the sandbox.
   - A `#[cfg(test)] mod tests` beside the code, invoking the tool with
     `tool.invoke(serde_json::json!({...}))` like the model would.
     Copy the shape of `plan.rs` (simple) or `fs.rs` (workspace-rooted).

2. **Register it** in `crates/harness-tools/src/lib.rs`:
   - Add `pub mod <tool_name>;` to the module list (alphabetical).
   - Add `.with_typed(<tool_name>::<Tool>::new(...))` in
     `default_for_workspace_with_web_key`.
   - Add the name constant to the `default_registry_contains_every_shipped_tool`
     test's expected list (it is sorted alphabetically and fails loudly with
     instructions if you skip this).

3. **Verify**: `cargo test -p harness-tools`. Watch for the
   `default_tool_definitions_stay_within_budget` test — schemas are resent on
   every model call, so keep the description tight. Then
   `cargo fmt --all` and `cargo clippy --workspace --all-targets -- -D warnings`.

## Host-surface tools (the tool shows something in the UI)

Tools whose effect is a host surface — a panel, a picker, a viewer (like
`ask_user_question`, `canvas`, `open_file`) — follow a different recipe.
**Read `crates/harness-tools/src/viewer.rs` first: it is the documented
reference implementation.** The shape:

1. In `harness-tools`: a plain data struct (what to show), a `…Sink` trait
   (`async fn …(&self, data) -> Result<Option<String>, ToolError>` — the
   `Option<String>` is a host note appended to the model-visible result), and
   a `TypedTool` that validates arguments (paths through `Workspace`) and
   forwards to the sink. The result text tells the model what the user now
   sees, so it doesn't re-paste content into chat.
2. Do NOT add it to `default_for_workspace_with_web_key` or the completeness
   test — host-surface tools register per front end.
3. Desktop (`app/src-tauri`): a `Tauri…Sink` in `bridges.rs` emitting a
   session-tagged `agent://…` event (payload struct in `events.rs`), a
   `Null…Sink` for `settings_registry`, registration in `state.rs::finish_tools`;
   frontend: payload type in `types.ts`, `on…` wrapper in `ipc.ts`, an
   `ingest…` store action keyed by session, subscribed in `agentEvents.ts`.
4. CLI (`crates/harness-cli`): either a sink that degrades usefully (canvas
   writes a file and opens the browser) or — when the terminal simply lacks
   the surface — don't register the tool at all (`open_file`). Never register
   a sink that silently does nothing.
5. Gate the system prompt: if the prompt should mention the tool, add a flag to
   `harness-agent/src/prompt.rs::OptionalTools` (derived from the registry via
   `from_registry`, so hosts advertise exactly what they registered).

The full walkthrough with a complete example lives in the README's
"Adding a tool" section.
