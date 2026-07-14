# oxen-harness desktop app

A cross-platform [Tauri v2](https://v2.tauri.app/) desktop UI for `oxen-harness`,
with a chat interface similar to Cursor agents / Claude Cowork. It reuses the
same `harness-agent` loop as the CLI, so the agent can read/write/search files,
run shell commands, and use git — scoped to the working directory the app is
launched from.

This is a **separate Cargo project** from the core workspace (the root workspace
`exclude`s it) so the core build/test loop stays fast and free of the webview
toolchain.

## How it works

- `src-tauri/` — the Rust backend, a thin shell over four concerns:
  `state.rs` (the `AppState` and per-session agent lifecycle), `bridges.rs`
  (the host↔agent bridges that surface `ask_user_question` / `canvas` / fleet
  lanes as events), `events.rs` (every webview payload struct, in one place so
  the wire format is auditable at a glance), and `commands/` (the
  `#[tauri::command]` handlers, one module per feature area — its `mod.rs`
  spells out how to add a command). `src/lib.rs` is now just the module map and
  `run()`. The command groups:
  - **Turns** (`commands/turn.rs`) — `run_turn(prompt)` drives `harness_agent::Agent`, emitting
    `agent://token` / `agent://tool` / `agent://usage` events as the turn
    streams; chats run concurrently per session, so several can be mid-turn.
  - **Sessions & projects** — history CRUD (`list_sessions`, `resume_session`,
    `delete_session`, …) plus guided project creation/editing. The recent index
    stays in `~/.oxen-harness/projects.json`, along with the user's preferred
    parent directory for new projects; shareable goals, instructions, and context
    live in each repository's `.oxen-harness/project.json` and `.oxen-harness/context/`.
  - **Tools & skills** — `list_tools` / `set_tool_enabled` /
    `set_tool_description` / `add_custom_tool` / `remove_custom_tool` and
    `list_skills` / `save_skill` / `delete_skill` / `set_skill_enabled`, backing
    the Tools and Skills settings pages (see the root README's "Adding a tool" /
    "Adding a skill").
  - **Models** — the cloud-model catalog and the local-model setup (llama.cpp
    downloads with `models://progress`, one-click `install_llama`, hardware-fit
    annotation).
  - **Bridged tools** — when the agent calls `ask_user_question` the backend
    emits `agent://question` and parks until the UI answers
    (`answer_question`); `canvas` documents stream over `agent://canvas` into
    the side panel.
  - **Code review & fleets** — `run_code_review` drives the shared
    `harness-review` pipeline (find → verify → report) over the working diff or
    a base branch, streaming `review://` progress and injecting the findings
    into the chat; the pipeline's parallel steps — and any `spawn_agents` fleet
    the model launches mid-turn — surface as live `fleet://` lanes you can
    click to watch. `get/save_code_review_config` back the Code-review settings
    page.
  - **Themes, connection, training data** — theme CRUD shared with the CLI via
    `harness-theme`; endpoint/API-key settings; per-chat review status + JSONL
    fine-tuning export.
- `src/` — the frontend: **React 19 + TypeScript**, bundled by **Vite** (which
  gives hot-module reload in `tauri dev`). **Projects** is the navigation root:
  choose a project to open its home (model-selectable, context-aware composer
  plus editable Instructions and Context cards), then work within its scoped sidebar of
  **＋ New chat** and that project's history. The project sidebar leads back to Projects, and
  Settings leads back to the active project from its upper-left rail rather
  than a top-right close action. Settings is a
  full-window surface with pages for **Connection**, **Cloud/Local models**,
  **Tools**, **Skills**, **Code review**, **Appearance**, and **Training
  data**. When the agent needs a decision, a **question modal** pops up with
  multiple-choice options plus a free-text row. You can also **queue
  messages**: keep typing while the agent works and each message stacks above
  the composer; the next is sent automatically when the current turn finishes.
  A **Review** button in the composer runs the code-review pipeline on your
  changes, and while it (or any `spawn_agents` fleet) runs, a **lanes panel**
  above the composer shows each parallel agent — click a lane to watch its
  output stream.

### Frontend layout (how to extend)

The structure mirrors the sibling `ArxivDiver` app — feature folders plus a thin
shared layer:

```
src/
  main.tsx, App.tsx        # entry + project-first shell (sidebar | chat + canvas + overlays)
  lib/
    ipc.ts                 # typed wrappers over every Tauri command/event —
                           #   components import from here, never call invoke()
    types.ts               # wire types mirroring the Rust structs (serde shape)
    store.ts               # zustand store: mode, theme, sessions, threads, modals
    color.ts, format.ts    # pure helpers (accent derivation, byte/time formatting)
  components/ui/           # design-system primitives (Button, Modal, Markdown…)
  styles/                  # tokens.css (the only source of color/space) + global
  features/
    history/Sidebar.tsx    # the active project's chats + new chat + settings
    projects/              # project list, guided creation, and durable project home
    chat/                  # Chat orchestration; thread.ts = pure stream reducers;
                           #   ThreadItem, Composer, Queue are presentational
    canvas/                # the side-panel document viewer
    settings/  tools/  skills/  models/  themes/  logs/  inspector/  questions/
  test/                    # Vitest setup + a controllable mock of lib/ipc
```

To **add a Tauri command**: write the `#[tauri::command]` fn in the right
`src-tauri/src/commands/` module (see `commands/mod.rs`), list it in the
`invoke_handler!` in `src-tauri/src/lib.rs`, add a typed wrapper in `lib/ipc.ts`
(+ a type in `types.ts`), then call it from a feature.
To **add a view/overlay**: create a `features/<name>/` folder with its `.tsx` +
colocated `.css`, and mount it from `App.tsx` (store a boolean in `store.ts` if it
toggles). Keep feature logic out of `components/ui` and reference design tokens —
never hardcode colors/spacing.

### Design system (`src/styles/tokens.css`)

A small, semantic **design-token** layer inspired by the minimal chrome of Claude
and Cursor's agent window:

- **Spacing** is a 4pt scale (`--space-1` … `--space-8`); **radii** and a **type
  scale** keep everything consistent. No hardcoded pixels/colors in components.
- **Light + dark mode** are driven by warm-neutral token sets toggled via
  `[data-theme]` on `<html>` (the light/dark control in Settings; defaults to the OS
  `prefers-color-scheme`, persisted to `localStorage`). `color-scheme` is set so
  native controls and scrollbars match.
- **Themes layer on top of the mode**: `applyThemePalette()` (in `store.ts`, using
  `lib/color.ts`) maps the active theme's palette onto the *accent* tokens
  (`--accent`, `--link`, `--danger`) and sets the brand glyph from the theme's
  prompt icon, while neutrals stay owned by the mode. Hover/focus/tint states derive
  from the accent via `color-mix()`, so a single palette change re-skins the whole
  UI without breaking readability in either mode.

The agent is initialized lazily on first command, so the window always opens
even if no API key is configured (you'll see a "not ready" status). To run local
models from the UI you also need `llama-server`; on macOS (or Linuxbrew) the
**Local models** panel can install it for you with one click, otherwise install it
manually (`brew install llama.cpp`) — see the root README's "Running models
locally" section.

## Prerequisites

- Rust (stable) and the platform webview deps for Tauri v2 — see
  [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/).
  (macOS: Xcode command line tools; Linux: WebKitGTK; Windows: WebView2.)
- The Tauri CLI, via either:
  - `cargo install tauri-cli --version "^2"` → run with `cargo tauri dev`, or
  - `npm install` here (installs `@tauri-apps/cli`) → run with `npm run dev`.
- An Oxen.ai API key in `OXEN_API_KEY`, or be logged in via the `oxen` CLI.

## Run

```bash
# From this app/ directory:
OXEN_API_KEY=sk-... cargo tauri dev
# or
OXEN_API_KEY=sk-... npm run dev
```

## Build a distributable

```bash
cargo tauri build   # or: npm run build
```

> Note: bundling is disabled by default in `tauri.conf.json` (`bundle.active`
> = false). Set it to `true` and add app icons to produce installers.
