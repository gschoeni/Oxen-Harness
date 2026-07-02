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

Tools needing UI (like `ask_user_question`/`canvas`) instead define data + a
host trait here and are registered per front end — read
`crates/harness-tools/src/ask.rs` before attempting one.

The full walkthrough with a complete example lives in the README's
"Adding a tool" section.
