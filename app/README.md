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

- `src-tauri/` — the Rust backend. `src/lib.rs` exposes two Tauri commands:
  - `run_turn(prompt)` — drives `harness_agent::Agent`, emitting `agent://token`
    and `agent://tool` events as the turn streams, returning the final text.
  - `session_info()` — model, workspace, and session id for the header.
- `dist/` — a dependency-free frontend (HTML/CSS/JS). It uses the global Tauri
  API (`withGlobalTauri`), so no bundler/`npm install` is required for the UI
  itself; only the Tauri CLI is needed to run/build the shell.

The agent is initialized lazily on first command, so the window always opens
even if no API key is configured (you'll see a "not ready" status).

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
