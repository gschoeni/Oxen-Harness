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
- `src/` — the frontend: **React 19 + TypeScript**, bundled by **Vite** (which
  gives hot-module reload in `tauri dev`). The chat history lives in the left
  sidebar (**＋ New chat**, past conversations, **⚙ Settings** in the footer); the
  Settings page holds the session info plus the **Local models**, **Theme**, and
  light/dark controls. When the agent needs a decision, a **question modal** pops up
  with multiple-choice options (radio / checkbox) plus a free-text row. You can also
  **queue messages**: keep typing while the agent works and each message stacks above
  the composer; the next is sent automatically when the current turn finishes.

### Frontend layout (how to extend)

The structure mirrors the sibling `ArxivDiver` app — feature folders plus a thin
shared layer:

```
src/
  main.tsx, App.tsx        # entry + shell (sidebar | chat + overlays)
  lib/
    ipc.ts                 # typed wrappers over every Tauri command/event —
                           #   components import from here, never call invoke()
    types.ts               # wire types mirroring the Rust structs (serde shape)
    store.ts               # zustand store: mode, theme, session, history, modals
    color.ts, format.ts    # pure helpers (accent derivation, byte/time formatting)
  components/ui/           # design-system primitives (Button, Modal, Markdown…)
  styles/                  # tokens.css (the only source of color/space) + global
  features/
    history/Sidebar.tsx    # chat-history list + new chat + settings entry
    chat/                  # Chat orchestration; thread.ts = pure stream reducers;
                           #   ThreadItem, Composer, Queue are presentational
    settings/  models/  themes/  questions/   # one folder per overlay
  test/                    # Vitest setup + a controllable mock of lib/ipc
```

To **add a Tauri command**: implement it in `src-tauri/src/lib.rs`, add a typed
wrapper in `lib/ipc.ts` (+ a type in `types.ts`), then call it from a feature.
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
