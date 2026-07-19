---
name: verify
description: Drive the oxen-harness TUI end-to-end offline — canned SSE server + pty harness, no API key or tmux needed.
---

# Verifying the CLI/TUI

Build: `cargo build -p harness-cli` → `target/debug/oxen-harness`.

## Offline turn driving (no real model)

The client speaks OpenAI-style chat completions: POST `{base}/chat/completions`,
SSE `data: {"choices":[{"index":0,"delta":{"content":"…"}}]}` chunks, then a
`finish_reason: "stop"` chunk and `data: [DONE]`. Auth is satisfied by
`OXEN_API_KEY=<anything>` when `--base-url` points at localhost.

1. Stand up a small Python `http.server` that streams a canned reply in ~7-byte
   chunks with tiny sleeps (exercises the line-at-a-time streaming renderer).
2. Launch `oxen-harness --base-url http://127.0.0.1:<port> --model mock-model
   --workspace <scratch>`.

## Driving the TUI (no tmux on this machine)

Use a Python `pty.fork()` harness: set the winsize via `TIOCSWINSZ`, log every
raw byte to a file, and feed the bytes to `pyte` (pip-installed) for screen
snapshots at checkpoints. Keys: `\x15` ctrl-u clears the draft, `\r` submits,
`\x03` ctrl-c stages interrupt/exit (second press at idle opens the exit-time
training-data picker — kill the pid to end the run).

The raw byte log is the evidence for escape-level behavior: grep it for
`\x1b[?2026h`/`l` balance (synchronized output), truecolor spans in code
blocks (syntax highlighting), etc. Screen snapshots are the evidence for
alignment (CJK/emoji in the composer, GFM table borders).

Gotchas
- The vt100 golden tests live in `live/vt_tests.rs` but are CI's job — verify
  at the pty surface instead.
- `NO_COLOR=1` in the child env checks the plain-mode fallbacks.
- The app writes real session state to `~/.oxen-harness`; use a throwaway
  workspace dir and expect a session row per run.
