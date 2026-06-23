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

- `src-tauri/` — the Rust backend. `src/lib.rs` exposes the Tauri commands:
  - `run_turn(prompt)` — drives `harness_agent::Agent`, emitting `agent://token`
    and `agent://tool` events as the turn streams, returning the final text.
  - `session_info()` — model, workspace, and session id for the header.
  - `list_models` / `pull_model` / `remove_model` / `use_local_model` — the local
    model catalog (Qwen3 via llama.cpp): list with disk usage, download (emitting
    `models://progress`), remove, and switch the session to a local model.
  - `install_llama` — installs `llama-server` for the user via Homebrew when it's
    missing, streaming output to the UI over `llama://install`. `list_models`
    reports `can_auto_install` so the panel can offer a one-click **Install
    llama.cpp** button (with a live log) instead of just a manual hint.
  - `answer_question(id, answers)` — delivers the user's choice for a clarifying
    question. When the agent calls `ask_user_question`, the backend emits an
    `agent://question` event and parks until the UI answers.
  - `list_themes` / `active_theme` / `use_theme` / `import_theme` / `export_theme`
    / `remove_theme` / `new_theme` — the theme system (shared with the CLI via
    `harness-theme`): list/switch themes, import/export shareable theme files, and
    vibe-code a brand-new theme with the model.
- `dist/` — a dependency-free frontend (HTML/CSS/JS). It uses the global Tauri
  API (`withGlobalTauri`), so no bundler/`npm install` is required for the UI
  itself; only the Tauri CLI is needed to run/build the shell. The **🐂 Local
  models** button opens a panel to browse, download, and switch to local models.
  When the agent needs a decision, a **question card** pops up with multiple-choice
  options (radio / checkbox) plus a free-text row. The **🎨 Theme** button opens a
  panel to switch themes, import/export themes, and generate a new one from a short
  "vibe" form.

### Design system (`dist/styles.css`)

The UI follows a small, semantic **design-token** layer inspired by the minimal
chrome of Claude and Cursor's agent window:

- **Spacing** is a 4px base scale (`--space-1` … `--space-16`); **radii**
  (`--radius-xs` … `--radius-full`) and a 3-level **type scale** with two weights
  keep everything consistent. No hardcoded pixels/colors in components.
- **Light + dark mode** are driven by warm-neutral token sets toggled via
  `[data-mode]` on `<html>` (the **🌙 / ☀️ toggle** in the sidebar; defaults to the
  OS `prefers-color-scheme`, persisted to `localStorage`). `color-scheme` is set so
  native controls and scrollbars match.
- **Themes layer on top of the mode**: `applyTheme()` maps the active theme's
  palette onto the *accent* tokens (`--accent`, `--link`, `--danger`) and sets the
  brand glyph from the theme's prompt icon, while neutrals stay owned by the mode.
  Hover/focus/tint states derive from the accent via `color-mix()`, so a single
  palette change re-skins buttons, the composer focus ring, links, the user bubble,
  and status indicators without breaking readability in either mode.

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
