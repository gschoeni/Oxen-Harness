# AGENTS.md — How we build oxen-harness

This project is developed with **The Ralph Wiggum loop**: a tight,
objective-check-driven cycle. Never assume success — rely on test/build output,
and persist state in files (code, tests, docs) rather than in your head.

## The loop

Each iteration:

1. **Read the task / spec** (and the relevant `*.md` — start at `DOCUMENT-MAP.md`,
   then `00-project-brief.md`, then `02-status.md`).
2. **Write or update a test that encodes the change in behavior** — *before* the
   code where practical. A test net first makes refactors safe.
3. **Make the smallest change** that moves toward green.
4. **Write to the filesystem directly** — don't hold large diffs in conversation
   (reduces context rot).
5. **Run the checks** (below) and *read the actual output*.
6. **On failure, fix the root cause**, not the symptom; iterate.
7. **Stop when all checks pass** and the requirement is met — then stop editing.
8. **Commit the change** with a clear, concise message that explains the *why*,
   not just the *what*. Keep commits small and logical — one coherent change each.

When you add a capability, add the test in the same iteration.

## The end-of-feature polish pass

A feature isn't done at the first green commit. Once the behavior is complete and
committed, do **one dedicated review/refactor pass** before moving on:

1. **Review** — have the LLM read the feature's diff and critique it for
   **modularity, maintainability, readability, idiomatic Rust / frontend code, and
   pragmatism** (simplicity over cleverness; no over-engineering). Produce a
   concrete list of suggested changes.
2. **Fix** — feed that review back to the agent and apply the changes that are
   genuinely worth it (skip nitpicks that don't improve the code).
3. **Re-verify** — run the full check suite again (`fmt`, `clippy`, tests) and read
   the real output; everything must stay green.
4. **Commit separately** — land this as its own commit (e.g.
   `refactor: polish <feature>`), distinct from the feature commit(s), so the
   behavioral change and the cleanup stay reviewable on their own.

## The checks (verification loop)

Run these and read the real output before declaring done:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run          # or: cargo test --workspace
```

A change is "green" only when all three pass.

The desktop app is a separate Cargo project (the root workspace excludes it), so
changes under `app/` get their own loop:

```bash
cd app/src-tauri && cargo clippy -- -D warnings   # the Tauri bridge
cd app && npx tsc --noEmit && npx vitest run       # the React frontend
```

## Project conventions

- **Provider:** Oxen.ai only. Base URL `https://hub.oxen.ai/api/ai`, default model
  `claude-opus-4-8`. Models are swappable; the provider is not.
- **Files are the source of truth.** Update the knowledge base as you go:
  - `02-status.md` when phase status changes.
  - `03-decisions.md` when you make a load-bearing decision.
  - `DOCUMENT-MAP.md` when you add or rename a file.
- **Crates stay focused.** Heavy dependencies (e.g. `liboxen`) are isolated to the
  crate that needs them (`harness-llm`).
- **No narrating comments.** Comments explain intent/trade-offs, not what the code
  literally does.

## Codebase orientation

Read these in order — it follows a single prompt from keypress to reply, and by
the last file the architecture clicks:

1. **`ARCHITECTURE.md`** — the map: how the crates stack, the lifecycle of a
   turn, and a table of "to add X, touch Y". **Start here.**
2. **`crates/harness-tools/src/lib.rs`** — the `TypedTool` trait and
   `ToolRegistry`. The smallest complete concept in the codebase, and the thing
   you're most likely to extend; each tool (fs, shell, git, web, canvas, …)
   lives in its own file beside it.
3. **`crates/harness-agent/src/lib.rs`** — `Agent::run_turn`, the loop that
   wires the LLM client, the tools, and the history store together. This is the
   heart, and it reads top-to-bottom: *make room in the context → stream the
   reply → run any tool calls → repeat*.
4. **`crates/harness-llm/src/types.rs`** — the wire types (`ChatMessage`,
   `ToolCall`, streaming events) that flow through that loop.
5. **`crates/harness-cli/src/main.rs`** — how a real session boots: resolve the
   model + endpoint, build the tools, create the agent, then hand off to the
   REPL. The desktop front end in `app/` drives the *same* `harness-agent`
   (see `app/README.md`).

Every crate's `src/lib.rs` opens with a `//!` comment stating what it owns, and
`DOCUMENT-MAP.md` is the one-line-per-file index for the full layering. Tests
live in a `#[cfg(test)] mod tests` right beside the code they cover, so they
double as usage examples.

Other useful entry points:

- **Skills machinery**: `crates/harness-tools/src/skill.rs` (SKILL.md parsing +
  the `skill` tool) and `crates/harness-runtime/src/skills.rs` (discovery,
  preferences, authoring). Skills reference tools by their backticked names;
  the desktop editor autocompletes and lints those references.
- **Theme hero scenes**: the scene registry in
  `app/src/features/chat/scenes.tsx` — a new scene is a one-function drop-in.

## Adding a built-in tool

Each tool is a small Rust type in `crates/harness-tools/src/`, one file per
concern. A tool has three parts: a **name** the model calls, a **description**
telling the model when to use it, and a **typed args struct** whose doc comments
become the JSON Schema the model reads. The schema is derived from the struct,
so the advertised interface and what your code parses can never drift. (This
recipe also ships as the `add-a-tool` skill in
`.oxen-harness/skills/add-a-tool/SKILL.md`; user-facing extension points —
skills and no-code HTTP tools — are covered in the README.)

Here's a complete built-in tool, start to finish:

**1. Create `crates/harness-tools/src/word_count.rs`:**

```rust
//! `word_count` — count the words in a workspace file.

use async_trait::async_trait;
use serde::Deserialize;

use crate::sandbox::Workspace;
use crate::{ToolError, TypedTool};

/// Tool name for [`WordCountTool`].
pub const WORD_COUNT_TOOL: &str = "word_count";

/// Arguments to `word_count`. Field doc comments are shown to the model.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct WordCountArgs {
    /// Path relative to the workspace root.
    pub path: String,
}

/// Count words in a file, confined to the workspace sandbox.
pub struct WordCountTool {
    workspace: Workspace,
}

impl WordCountTool {
    pub fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }
}

#[async_trait]
impl TypedTool for WordCountTool {
    const NAME: &'static str = WORD_COUNT_TOOL;
    type Args = WordCountArgs;

    fn description(&self) -> &str {
        "Count the words in a UTF-8 text file. Use it when the user asks how \
         long a document is."
    }

    async fn run(&self, args: WordCountArgs) -> Result<String, ToolError> {
        let path = self.workspace.resolve(&args.path)?;
        let text = tokio::fs::read_to_string(&path).await?;
        Ok(format!("{} words", text.split_whitespace().count()))
    }
}
```

**2. Register it** in `crates/harness-tools/src/lib.rs`: add
`pub mod word_count;` to the module list, then one line in
`default_for_workspace_with_web_key`:

```rust
.with_typed(word_count::WordCountTool::new(workspace.clone()))
```

**3. Add its name** to the `default_registry_contains_every_shipped_tool` test
in the same file (the test fails with a clear message until you do — that's it
catching the register-it step for you).

**4. Run `cargo test -p harness-tools`.** Done. The tool now shows up in both
front ends, appears on the desktop app's **Settings → Tools** page, and the
model can call it in the next new chat. Dispatch is by name — nothing else
needs to change.

Conventions worth copying from the existing tools:

- **Write field doc comments for the model, not for rustdoc** — they are the
  schema descriptions the model reads. Put defaults and units in them
  ("Timeout in milliseconds (default 120000)").
- **Optional arguments are `Option<T>`** (or `#[serde(default)]`); required ones
  are plain fields. Enums (`#[serde(rename_all = "snake_case")]`) become JSON
  `enum`s, and invalid values are rejected before your `run` is ever called.
- **All file access goes through `Workspace::resolve`** so a tool can't escape
  the project directory.
- **Tests live right beside the tool** in a `#[cfg(test)] mod tests` — invoke it
  with `tool.invoke(serde_json::json!({...}))` exactly as the model would (see
  `fs.rs` for examples).
- **Schemas are resent on every model call**, so keep descriptions tight; a
  budget test in `lib.rs` fails if the tool block balloons.

**Editing an existing tool:** change the behavior in `run`, or change the
interface by editing the args struct — the schema updates itself. The
`description()` string is prompt engineering: it's how the model decides *when*
to use the tool, so edit it deliberately. Two caveats: a tool's `NAME` is its
stable id (renaming one orphans users' saved preferences in `tools.json`), and
tools that need UI — like `ask_user_question` and `canvas` — define only their
data and a host trait in this crate, with each front end (CLI, desktop)
implementing and registering its own version.

## Knowledge base entry points

| File | Load when |
|------|-----------|
| `DOCUMENT-MAP.md` | First — index of everything |
| `00-project-brief.md` | Always — orient any session |
| `02-status.md` | Active work — phases, tasks, what's next |
| `03-decisions.md` | Implementation — decisions + rationale |
| `04-backlog.md` | Planning — ideas and future exploration |
