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

Keep changes small and incremental; commit logically. When you add a capability,
add the test in the same iteration.

## The checks (verification loop)

Run these and read the real output before declaring done:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run          # or: cargo test --workspace
```

A change is "green" only when all three pass.

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

## Knowledge base entry points

| File | Load when |
|------|-----------|
| `DOCUMENT-MAP.md` | First — index of everything |
| `00-project-brief.md` | Always — orient any session |
| `02-status.md` | Active work — phases, tasks, what's next |
| `03-decisions.md` | Implementation — decisions + rationale |
| `04-backlog.md` | Planning — ideas and future exploration |
