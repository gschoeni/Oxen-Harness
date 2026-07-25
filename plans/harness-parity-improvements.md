# Harness improvements — lessons from oh-my-pi and pi

> **Status (branch `harness-parity-improvements`, 2026-07-24).** Phases 1 and 2
> complete, plus 3.3a and 3.4 — nine of the twelve sequenced items. All green
> (`cargo fmt`, `cargo clippy --workspace --all-targets -D warnings`, `cargo
> test --workspace`, plus `tsc --noEmit` and `vitest` for the desktop app).
> See [Progress log](#progress-log) at the bottom for what shipped, what
> changed versus this plan, and what's left.

Concrete, prioritized changes to oxen-harness derived from reading two other
coding-agent harnesses:

- **oh-my-pi (`omp`)** — <https://github.com/can1357/oh-my-pi>. A fork of Mario
  Zechner's Pi, "batteries included": 13 TS packages + 6 Rust crates, 32 tools,
  LSP/DAP integration, content-hash editing, stream rules, cross-session memory.
  Its `docs/` directory is unusually good — near-spec-level writeups of every
  subsystem, worth reading directly.
- **pi** — <https://github.com/earendil-works/pi>. Leaner: 6 TS packages, ~12
  tools, no permission system (they punt to containers), strong on harness
  hygiene — multi-edit, per-file mutation queues, durable/resumable harness
  state, supply-chain hardening.

Reference clones used while writing this: `oh-my-pi` and `pi` cloned shallow
into the session scratchpad. Re-clone as needed; nothing here depends on them
at build time.

Everything below is a *native* implementation proposal — same posture as
[`context-compression.md`](./context-compression.md), where headroom was
reference material, not a dependency.

---

## Scorecard: where we stand

| Capability | oxen-harness | omp | pi |
| --- | --- | --- | --- |
| Project context files (AGENTS.md/CLAUDE.md) | ❌ none | ✅ + inherits Cursor/Cline/Copilot rules | ✅ |
| Read-before-edit / stale-write guard | ❌ | ✅ snapshot store + hash tags | ⚠️ mutation queue only |
| Multi-edit per call | ❌ 1 replacement | ✅ multi-hunk patch language | ✅ `edits[]` |
| Structural (outline) reads | ❌ | ✅ tree-sitter summaries | ❌ |
| Truncation → retrievable artifact | ⚠️ CCR for compression only | ✅ | ✅ |
| Model roles / fallback chains | ⚠️ `summary_model` only | ✅ 5 roles + chains | ⚠️ |
| Cross-session memory | ❌ | ✅ Hindsight | ❌ |
| Stream-level correction rules | ⚠️ 3 hardcoded nudges | ✅ TTSR (regex + AST) | ❌ |
| LSP / diagnostics | ❌ | ✅ 14 ops + DAP debugger | ❌ |
| Subagent isolation | ❌ shared workspace | ✅ worktrees + APFS/btrfs clones | ❌ |
| Persistent shell / PTY | ❌ `sh -c` per call | ✅ embedded bash + PTY | ⚠️ |
| Permission gate | ✅ classify + approve + breakers | ⚠️ approval modes | ❌ none |
| Prompt-cache economics | ✅ anchors + request log | ⚠️ | ⚠️ |
| Reversible context compression | ✅ `harness-compress` | ⚠️ snapcompact | ❌ |
| Typed tool schemas + budget test | ✅ | ❌ | ❌ |
| Protocol/server split | ✅ `harness-protocol`/`-server` | ✅ RPC/ACP | ✅ |

We are ahead on context economics, permissions, and structure. We are behind on
**file-editing safety, context-efficient reads, and project knowledge**.

---

## Phase 1 — the cheap wins (target: one week)

### 1.1 Project context files (AGENTS.md / CLAUDE.md)

**Problem.** The model gets no project knowledge whatsoever. Our system prompt
(`crates/harness-agent/src/prompt.rs:70`) is a fixed policy block plus
`environment_section()` — literally one line naming the cwd
(`prompt.rs:46`). This repo ships an `AGENTS.md`, a `CLAUDE.md`-shaped
`ARCHITECTURE.md`, and `03-decisions.md`, and our own agent cannot see any of
them unless it happens to read them. Every competitor loads these
automatically; it is the single largest quality-per-line gap we have.

**Reference.** omp `docs/context-files.md` + `docs/rulebook-matching-pipeline.md`:
discovers `AGENTS.md`, `.clinerules`, Cursor `.mdc`, Copilot `applyTo` files,
buckets them into *always-apply* (injected into the system prompt) and
*rulebook* (glob-scoped, injected as a reminder when a matching path is
touched), dedupes by name with first-wins precedence. pi has the same idea in
`packages/coding-agent/src/core/prompt-templates.ts`.

**Design.**

New module `crates/harness-agent/src/context_files.rs` (or a small
`harness-context` crate if the hosts need it independently — start in-agent):

```rust
pub struct ProjectContext {
    /// Files found, nearest-last (so the closest file wins on conflict).
    pub files: Vec<ContextFile>,
}
pub struct ContextFile {
    pub path: PathBuf,
    pub scope: ContextScope,   // Global | Repo | Directory
    pub body: String,          // clipped
    pub applies_to: Vec<String>, // globs from frontmatter; empty = always
}
pub fn discover(workspace: &Path) -> ProjectContext;
pub fn render_always_apply(&self) -> String;   // system-prompt suffix
pub fn rules_for_path(&self, rel: &Path) -> Vec<&ContextFile>;
```

Discovery order (each optional, all deduped by canonical path):

1. `~/.oxen-harness/AGENTS.md` — user-global.
2. Walk from workspace root down to the session cwd, collecting `AGENTS.md`,
   `CLAUDE.md`, `.oxen-harness/AGENTS.md` at each level.
3. Optional inheritance (behind a setting, default on): `.cursor/rules/*.mdc`
   and `.clinerules` — parse YAML frontmatter for `description`/`globs`/
   `alwaysApply`. This is pure upside for users migrating from another tool and
   costs ~50 lines.

Injection:

- Always-apply bodies are appended to the system prompt at agent construction,
  **after** the fixed policy block and before `environment_section()`. That
  keeps them inside the stable cache prefix that `crates/harness-agent/src/cache.rs`
  anchors, so they are billed once per session, not per turn.
- Glob-scoped rules are *not* in the system prompt. They are injected as a
  one-shot `<system-reminder>` on the tool result the first time a matching
  path is read or edited — same delivery channel as our existing nudges in
  `prompt.rs:185-210`. Track injected rule names per session so a rule fires
  once (omp calls this `repeatMode: once`).

Budget rails (learn from omp's `summaryInjectionTokenLimit: 5000`):

- Per-file clip at 16 KB with a `… [context file truncated; read <path> for the
  rest]` marker — the model can always `read_file` the full text.
- Total always-apply budget 32 KB; past that, nearest-scope files win and the
  rest degrade to "these files exist, read them if relevant" one-liners.
- Emit the resolved list in the request log so an oversized AGENTS.md is
  visible as a cache-prefix cost, not a mystery.

**Files touched.**

- `crates/harness-agent/src/context_files.rs` (new, ~250 lines + tests)
- `crates/harness-agent/src/prompt.rs` — `system_prompt_with_env` takes the
  rendered block.
- `crates/harness-cli/src/endpoint.rs:245` and
  `crates/harness-host/src/service.rs:544` — both `AgentConfig` builders call
  `discover()`.
- CLI: a `/context` slash command listing what was loaded and its token cost.
- Desktop: show the loaded files in the Inspector drawer.

**Tests.** Discovery precedence (global < repo < directory); frontmatter glob
parsing; clip markers; total-budget degradation; a golden test that the
rendered prompt is byte-identical across two constructions (cache stability).

**Risk.** Low. Worst case is prompt bloat, which the budget rails and the
request log make visible.

**Effort.** ~1 day including the Cursor/Cline inheritance.

---

### 1.2 Read-before-edit and stale-write guard

**Problem.** `EditFileTool::run` (`crates/harness-tools/src/fs.rs:280`) reads
the file, does a string replace, and writes. There is no check that the model
ever read the file, and no check that the file changed between the read and the
edit. Three real failure modes:

1. The model edits from memory/assumption and silently rewrites something it
   never saw.
2. The user (or a formatter, or `cargo fmt` from a prior `run_shell`) changes
   the file mid-turn; the model's edit lands on stale content and clobbers it.
3. **`spawn_agents` makes this worse.** `FleetSpawner` snapshots the session's
   `ToolRegistry` (`crates/harness-agent/src/fleet_tool.rs:44`), which is rooted
   at the *same* `Workspace`. N lanes doing read-modify-write on the same file
   interleave with no serialization and last-write-wins.

We already solved the equivalent problem once — the data-grid editor does
surgical mtime-guarded cell edits — but the model-facing tools don't.

**Reference.**

- omp `packages/hashline/src/snapshots.ts` + `docs/tools/read.md` step 10:
  every hashline-eligible read records a whole-file snapshot into a session
  `FileSnapshotStore` (30 paths × 4 versions, files >4 MiB skipped). An edit
  resolves its 4-hex tag against that store, verifies the live file still
  hashes the same, and refuses (or 3-way merges) on mismatch.
- pi `packages/coding-agent/src/core/tools/file-mutation-queue.ts`: a global
  `Map<realpath, Promise>` chain so mutations to one file serialize while
  different files stay parallel. 61 lines, trivially portable.

**Design.**

New `crates/harness-tools/src/fs/snapshots.rs`:

```rust
/// Per-session record of what the model has actually seen on disk.
pub struct FileSnapshots { inner: Mutex<LruMap<PathBuf, Snapshot>> }
pub struct Snapshot { pub hash: u64, pub len: u64, pub mtime: SystemTime, pub read_at: Instant }

impl FileSnapshots {
    pub fn record(&self, path: &Path, contents: &str);
    pub fn verify(&self, path: &Path, current: &str) -> VerifyResult;
}
pub enum VerifyResult {
    Fresh,            // matches what we handed the model
    NeverRead,        // no snapshot for this path
    Changed { .. },   // on-disk content differs from the snapshot
}
```

Wiring:

- `ReadFileTool` records a snapshot on every successful full or partial read.
  (Partial reads record the *whole file* hash — the point is change detection,
  not the window.)
- `EditFileTool` and `WriteFileTool` (when the target exists) call `verify`
  before mutating:
  - `NeverRead` → `ToolError::InvalidArguments("read <path> before editing it")`.
    This is a real behavioral change; gate it behind a setting
    (`edit.requireRead`, default **on**) so it can be flipped off for scripted
    flows.
  - `Changed` → error naming what changed:
    `"<path> changed on disk since you read it (was 412 lines, now 418) — re-read it, then re-apply your edit"`.
    This message matters: our `edit_diagnostics.rs` already proves that a good
    error stops the retry-blind loop.
- Both tools run their whole read-modify-write inside a per-path async mutex
  (`DashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>`, keyed by canonicalized
  path), so concurrent fleet lanes serialize instead of racing.
- The store is per-`Workspace` (an `Arc` handed to the fs tools at registry
  build), so a subagent sharing the workspace shares the snapshot state — which
  is exactly what we want for lane safety.

**Snapshot-store bounds.** 64 paths, LRU, skip files >4 MiB (omp uses 30×4 and
4 MiB). Hash with `xxhash`/`ahash` over the LF-normalized content, not a
cryptographic hash — this is change detection, not integrity.

**Files touched.** `crates/harness-tools/src/fs/snapshots.rs` (new),
`fs.rs` (read/write/edit), `lib.rs` (share the store through
`default_for_workspace_*`), settings for `edit.requireRead`.

**Tests.** Edit without read → refused; edit after read → allowed; external
modification between read and edit → refused with the diff summary; two
concurrent edits to one path serialize and both land; concurrent edits to
different paths run in parallel; LRU eviction re-arms `NeverRead` gracefully
(evicted ≠ hard failure — treat eviction as `NeverRead` and say "re-read").

**Risk.** Medium-low. The `NeverRead` rejection changes model behavior; the
setting and a clear error message contain it. Watch for false positives from
tools that legitimately rewrite files (our own `canvas` writes, loop gates
running formatters) — those are host-side writes, not `edit_file`, so they only
matter as `Changed` on a *subsequent* model edit, which is the correct signal.

**Effort.** ~1.5 days.

---

### 1.3 Multi-edit in one call

**Problem.** `EditFileArgs` (`fs.rs:256`) is one `old_string`/`new_string` pair.
A rename touching six call sites in one file is six tool calls, six model
round-trips, and six full-context re-bills. It is also six chances for the
model to drift.

**Reference.** pi `packages/coding-agent/src/core/tools/edit.ts:33-53` — the
schema is `{path, edits: [{oldText, newText}]}` with explicit prompt language:
*"Each edit is matched against the original file, not incrementally. Do not
include overlapping or nested edits."* Application and diffing live in
`edit-diff.ts` (560 lines: `applyEditsToNormalizedContent`, `computeEditsDiff`,
`generateUnifiedPatch`, BOM/line-ending preservation).

**Design.**

Extend `EditFileArgs` additively so existing single-edit calls keep working:

```rust
pub struct EditFileArgs {
    pub path: String,
    /// Single-edit form (kept for compatibility and simple cases).
    pub old_string: Option<String>,
    pub new_string: Option<String>,
    /// Batch form: each edit matched against the ORIGINAL content.
    pub edits: Option<Vec<Replacement>>,
    #[serde(default)] pub replace_all: bool,
}
```

Semantics (all-or-nothing):

1. Normalize once (strip BOM, record line ending, convert to LF) — pi's
   `normalizeToLF`/`restoreLineEndings`/`stripBom`. We currently don't do this
   at all, so a CRLF file plus an LF `old_string` fails to match today with a
   confusing error.
2. Resolve every `oldText` against the **original** content, collecting byte
   ranges. Reject if any is missing (per-edit `edit_diagnostics` message
   naming the index), ambiguous without `replace_all`, or **overlapping** with
   another edit's range.
3. Apply from the end backwards so offsets stay valid; write once; restore the
   original line ending and BOM.
4. Return a compact summary: `edited src/foo.rs (3 edits, +12/-4 lines)` plus
   the unified diff when small. The diff in the result is what lets the model
   self-check without re-reading.

Note the interaction with 1.2: verification happens once for the batch, and the
whole batch runs inside the per-path mutex.

**Prompt.** Update the tool description and the "Read before you write" bullet
in `prompt.rs:124` to say: prefer one `edit_file` with several `edits` over
several calls.

**Tests.** Batch applies in order-independent fashion; overlap rejected with a
message naming both indices; one bad edit rolls back all (file unchanged on
disk); CRLF file round-trips with CRLF preserved; BOM preserved; `replace_all`
interacts sanely with the batch form.

**Risk.** Low — additive schema, existing behavior preserved.

**Effort.** ~1 day (the diff/patch rendering is the bulk).

---

### 1.4 Truncation → retrievable artifact

**Problem.** We truncate and discard. `read_file` caps at 100 000 chars
(`fs.rs:36`), `run_shell` caps stdout/stderr at 30 000 chars each
(`shell.rs:23`). The model gets `… [output capped]` and its only recourse is to
re-run the command with different flags — usually re-paying for the whole
thing.

**Reference.** omp `docs/tools/read.md` "Limits & Caps": every truncated result
carries `details.meta.truncation` with the shown range, total lines/bytes,
**next offset**, and an `artifactId` pointing at the full body in session
artifact storage. pi does the same in `core/tools/truncate.ts` +
`output-accumulator.ts`.

**Design.** We already built the retrieval half of this:
`crates/harness-compress/src/ccr.rs` stores originals keyed by content hash and
`retrieve_original` (`crates/harness-tools/src/retrieve.rs`) hands them back.
Reuse it rather than inventing artifact storage.

- Add `harness_compress::CcrStore::put_overflow(&str) -> Handle` (it already
  does content-hash keying; this is a naming/API convenience).
- `read_file` and `run_shell` (and `task_output`, `web_fetch`, `search_files`)
  route their overflow through the store and end the result with:
  `… [12 480 of 88 210 lines shown; next offset 12 481; full output: <<ccr:a1b2c3>> — retrieve_original]`
- Only active when compression mode ≠ `Off`? **No** — decouple. Truncation
  overflow should be retrievable regardless of the compression setting.
  Restructure so the `CcrStore` is always constructed (it is cheap and bounded)
  and only the *compressor* is gated by `CompressionMode`.
- Persist consideration: the store is in-memory FIFO today, so a handle can
  expire. The expiry message already exists and is fine; the v2 SQLite store
  from `context-compression.md` would make handles survive restarts.

**Tests.** Truncated read yields a resolvable handle; `retrieve_original`
returns the exact dropped bytes; handle expiry returns the friendly message;
the "next offset" figure is correct so a follow-up `read_file` with that offset
is contiguous.

**Effort.** ~0.5 day.

---

## Phase 2 — context efficiency and routing (target: second week)

### 2.1 Structural reads (tree-sitter outlines)

**Problem.** `read_file` on a 2 000-line file dumps 2 000 lines. The model
usually needs the shape plus two functions. This is the single biggest source
of avoidable prompt growth in a long session — and it compounds, because those
lines are then resent on every subsequent turn until compaction eats them.

**Reference.** omp `docs/tools/read.md` "Local text files": with no explicit
selector, `summarizeCode()` returns declarations with bodies elided, replacing
spans with `…` (or a merged `{ … }` brace line), and appends a footer naming
concrete re-read ranges:
`[…NNln elided; re-read needed ranges, e.g. <path>:5-16,40-80]`.
Guards: file ≤2 MiB and ≤20 000 lines; explicit line ranges bypass it entirely.
Their `pi-ast` crate carries 50+ grammars for this.

**Design.**

`tree-sitter 0.25` is already a workspace dependency (`Cargo.toml:74`), used by
`harness-permissions` for bash command classification — so the machinery and
build story are proven in-tree. Add grammars incrementally:
`tree-sitter-rust`, `-typescript`, `-javascript`, `-python`, `-go`. Unknown
extension → current behavior, verbatim.

New `crates/harness-tools/src/fs/outline.rs`:

```rust
pub struct Outline { pub lines: Vec<OutlineLine>, pub elided_spans: Vec<(usize, usize)> }
pub fn summarize(path: &Path, source: &str) -> Option<Outline>;
```

Keep: module/imports header, every top-level and nested declaration line
(fn/struct/enum/impl/class/def/type/const), doc comments attached to kept
declarations, and any line matching a search pattern when the read came from a
`search_files` follow-up. Elide bodies over N (say 4) lines.

Behavior on `ReadFileTool`:

- No `offset`/`limit`, file is outline-eligible, and rendered outline is ≤60%
  of the full text → return the outline plus the re-read footer.
- Any explicit `offset`/`limit` → verbatim (never surprise a targeted read).
- New arg `full: bool` to force verbatim.
- Never outline a file below ~200 lines; the savings don't justify the round
  trip.

**Interaction with 1.2 (important).** An outline read must still snapshot the
*whole file* hash, and must record that the model has only *seen* the outline.
Two options; I recommend (b):

  (a) Treat an outline read as a valid "has read" for the edit gate — simple,
      slightly unsafe (the model may edit inside a span it never saw).
  (b) Track seen line ranges per path. An edit whose `old_string` resolves
      inside an elided span is refused with
      `"that range was elided in your read — read <path>:120-190 first"`.
      This is exactly omp's "Elided regions are UNSEEN" rule, and it turns a
      whole class of hallucinated edits into a caught error.

**Tests.** Rust/TS/Python fixtures produce stable outlines (golden files);
elided-span edit is refused with the range hint; explicit ranges bypass
outlining; unsupported language falls through verbatim; a measured
before/after token count on a representative repo file.

**Effort.** ~2–3 days including per-language golden tests. Highest
context-savings-per-hour of anything in this document.

### 2.2 Model roles and fallback chains

**Problem.** One model does everything. `AgentConfig` has exactly one escape
hatch, `summary_model` (`crates/harness-agent/src/config.rs:113`). Fleet lanes,
review find/verify steps (`harness-review`), plan generation, loop gate
judgments, and session titling all burn the session model. And when
`RetryPolicy` (`config.rs:19`) exhausts its four attempts, the turn just fails —
even if another configured model is healthy.

**Reference.** omp `docs/models.md`: roles `default`, `smol`, `slow`, `plan`,
`commit`, `advisor`, each resolvable to `provider/model:thinking-level`, with
resolution falling back role → `default` → session model → first registry
model. `docs/non-compaction-retry-policy.md` step 8: on a retryable error, if
`retry.modelFallback` is enabled, suppress the current model for a cooldown and
walk a configured fallback chain, forcing delay to 0 on a model switch.

**Design.**

- Extend `models.json` (`harness-config/paths.rs:64`) with a `roles` map:
  `{"smol": "...", "plan": "...", "review": "...", "summary": "..."}`.
  `summary_model` becomes the `summary` role (migrate, keep reading the old key).
- `AgentConfig` gains `roles: ModelRoles` with a `resolve(Role) -> String` that
  falls back to the session model. Call sites: `fleet_tool.rs` (lane model),
  `harness-review` step agents, `compact.rs` summarizer, title generation.
- Fallback chain: `retry.fallback_models: Vec<String>`. After
  `RetryPolicy::max_attempts` on a transient error, switch model, reset the
  attempt counter once, and emit an `AgentEvent` so the UI can say
  "retrying on <model>". Cooldown the failed model for the session (or N
  minutes) so we don't ping-pong.
- Surface both in Settings → Models, next to the existing catalog UI.

**Tests.** Role resolution precedence; fallback fires only on transient errors
(never on a 400/validation error); cooldown prevents immediate re-selection;
event emitted exactly once per switch.

**Effort.** ~1.5 days.

### 2.3 Persistent shell sessions

**Problem.** `ShellTool` spawns a fresh `sh -c` per call
(`crates/harness-tools/src/shell.rs:170`). So `cd subdir` does nothing for the
next call, `source .venv/bin/activate` is a no-op, `export` is lost, and any
command that expects a TTY (a prompt, a pager, a progress bar) either hangs
until timeout or produces mangled output. Models routinely paper over this by
chaining `cd x && ...` on every call — wasted tokens and a source of quoting
bugs.

**Reference.** omp `docs/bash-tool-runtime.md` + `crates/pi-shell` (vendored
`brush-core`, an embedded bash) — persistent sessions with custom builtins,
abort support, and optional PTY allocation per call.

**Design.** We do *not* need to embed bash. A middle path gets 80% of the value:

- `ShellSession` holds `cwd: PathBuf` and `env: BTreeMap<String, String>`,
  persisted per session (and per fleet lane).
- Each call still spawns `sh -c`, but with the session's cwd/env, and appends a
  trailer that reports the post-command state:
  `; __rc=$?; pwd >&3; env -0 >&3; exit $__rc` over an extra fd, parsed and
  folded back into the session. Cheap, no PTY, no shell embedding, and `cd`
  and `export` now persist.
- Optional `pty: bool` arg using `portable-pty` for commands that need a TTY.
  Defer if we want to keep Phase 2 small.

**Risk.** The env round-trip must not leak secrets into the transcript — parse
it, don't print it. Cap the env size we retain and skip `_`/`SHLVL`/`PWD`
noise.

**Effort.** ~1 day for cwd/env persistence; +1 day for PTY.

---

## Phase 3 — differentiating features

### 3.1 Stream rules (a TTSR-lite)

**Problem.** We have exactly three corrective nudges, all hardcoded and all
*post-hoc*: `INTENT_NUDGE`, `PLAN_STALL_NUDGE`, `LOOP_NUDGE`
(`prompt.rs:185-210`). They fire after a full turn has streamed and been billed.
Users cannot add their own ("never use `unwrap()` in this repo", "don't touch
`generated/`") without editing the system prompt, which costs prefix tokens on
every request whether or not the situation ever arises.

**Reference.** omp `docs/ttsr-injection-lifecycle.md` — the most interesting
idea in either repo. Rules stay *dormant* (zero prompt cost) until a regex (or
ast-grep pattern) matches the live token stream; the stream is aborted
mid-generation, a `<system-interrupt reason="rule_violation" rule="..">` block
is injected, and generation resumes from that point. Key details worth copying:

- **Scopes**: `text`, `thinking`, `tool` (matched against the reconstructed
  tool-argument snapshot, not raw deltas — so a rule can catch a bad `edit`
  *before it is applied*).
- **Interrupt vs. reminder**: `interruptMode: never` doesn't abort; for
  tool-source matches it prepends the reminder to that tool's result instead.
  Cheaper and less disruptive.
- **`contextMode`**: `discard` drops the partial assistant message before
  retrying; `keep` leaves it and appends the reminder.
- **Repeat policy**: `once` or `after-gap` with a turn-count gap, and the
  injected-rule set is persisted so resume doesn't re-nag.
- **Glob gating**: a rule can require the stream context to name a matching path.

**Design.**

New `crates/harness-agent/src/rules.rs` + `~/.oxen-harness/rules.json` and
`<workspace>/.oxen-harness/rules.json`:

```jsonc
{
  "rules": [{
    "name": "no-unwrap",
    "when": "\\.unwrap\\(\\)",          // regex
    "scope": ["tool"],                   // text | thinking | tool
    "globs": ["**/*.rs"],
    "message": "This repo forbids `.unwrap()` in non-test code — return a Result or use `expect` with a reason.",
    "interrupt": true,
    "repeat": { "mode": "after-gap", "gap": 5 }
  }]
}
```

Integration points in `crates/harness-agent/src/agent/turn.rs` (which already
streams deltas and coalesces them for the UI):

1. Maintain a per-scope rolling buffer during the stream; for tool-call deltas,
   accumulate the partial JSON and match against the reconstructed argument
   text (omp's `matcherDigest`).
2. On an interrupting match: cancel the in-flight stream, drop or keep the
   partial per `contextMode`, append a hidden system message, and re-issue.
3. On a non-interrupting match: stash it, and prepend the reminder to the
   matched tool's result.
4. Persist injected rule names alongside session state so `--resume` doesn't
   re-fire a `once` rule.

Then **reimplement our three existing nudges as built-in rules**, so there is
one mechanism instead of four ad-hoc ones. Skip ast-grep initially; regex plus
globs covers the realistic cases, and we can add tree-sitter patterns once 2.1
lands the grammars.

**Risk.** Medium. Aborting mid-stream interacts with retry, cancellation, and
our `StreamBatch` coalescing — regression risk concentrated in `turn.rs`, our
largest file (2 345 lines). Mitigate with the offline pty/canned-SSE harness in
the `verify` skill: rules are exactly the kind of thing that needs a
deterministic stream to test against. Also cap injections per turn (omp
effectively does this via repeat policy) so a badly-written rule can't loop.

**Effort.** ~3 days.

### 3.2 Cross-session project memory

**Problem.** Every session starts cold. Everything learned about a repo — that
the tests need a feature flag, that a subsystem is mid-migration, that an
approach was already tried and rejected — evaporates.

**Reference.** omp `docs/memory.md` (Hindsight). Two phases, both background:
per-session extraction (a model reads a past session and extracts durable
signal: decisions, constraints, resolved failures, recurring workflows), then
consolidation across sessions into `MEMORY.md` + a compact `memory_summary.md`
injected at startup + generated `skills/` playbooks. Details worth copying:

- **Disabled by default**, opt-in via config. Correct default for a trust-
  sensitive feature.
- **Lease + heartbeat** so two processes starting at once don't double-run.
- **Secret redaction** before anything is written to disk.
- **Skip rules**: sessions too recent (`minRolloutIdleHours: 12`), too old
  (`maxRolloutAgeDays: 30`), currently active, or beyond a per-startup cap.
- **Explicitly heuristic framing** in the injected prompt: prefer repo state and
  user instruction over memory; treat conflicting memory as stale; cite the
  memory artifact when it changes the plan.
- Phase 2 runs on the cheap (`smol`) model.

**Why this fits us particularly well.** We already have (a) a full session store
(`harness-store`), (b) the training-data review pipeline with per-chat
`review_status`, (c) a skills system with the exact `SKILL.md` shape omp
generates into, and (d) Oxen. Memory as a *versioned, diffable, shareable*
artifact — `oxen log` on your project's accumulated agent knowledge, memory that
a team can review in a PR — is a genuinely differentiated story that neither
reference can tell.

**Design sketch.** New `crates/harness-memory`:

- `extract.rs` — per-session extraction against `HistoryStore`, with a
  `memory_jobs` table for the queue and processed-watermarks.
- `consolidate.rs` — cross-session synthesis producing
  `<workspace>/.oxen-harness/memory/MEMORY.md`, `summary.md`, and
  `skills/<name>/SKILL.md` (which our existing skill loader picks up for free —
  `harness-tools/src/skill.rs:135`).
- `redact.rs` — token/secret patterns, reusing `harness-config/secrets.rs`.
- Injection: `summary.md` appended to the system prompt through the same path
  as 1.1 (context files), with the same clip budget and the heuristic framing.
- Optional: commit the memory directory to Oxen on change, so history is
  first-class.

Do this **after** 1.1 ships, because it reuses the injection path, and after
2.2, because it should run on the `smol` role.

**Effort.** ~4–5 days. Highest ceiling, highest cost.

### 3.3 Post-edit diagnostics (the pragmatic slice of LSP)

**Problem.** After an edit we know nothing until a build runs, and builds are
slow enough that models skip them. omp's LSP integration (14 ops) plus a real
debugger (28 DAP ops) is their clearest capability edge over every other
harness, including Claude Code.

**Design (staged).**

- **Stage A — checker-on-edit (cheap, ~1 day).** Per-language configured check
  command (`cargo check --message-format=json` scoped to the file's crate,
  `tsc --noEmit`, `ruff`, `go vet`). After a successful `edit_file`, run it
  debounced in the background; attach new diagnostics for that file to the edit
  result: `edited src/foo.rs (2 edits) — 1 new error: expected `;`, found `}` at 41:9`.
  Reuse the `harness-loop` gate infrastructure, which already knows how to run
  and parse project commands with `run_when` globs.
- **Stage B — a real `lsp` tool (~1 week).** Spawn `rust-analyzer`/`tsserver`
  per workspace, expose `diagnostics`, `definition`, `references`, `hover`,
  `rename`. `rename` in particular is a genuine superpower: it updates
  re-exports and barrel files that regex-based edits miss.

Stage A alone changes the edit→verify loop materially and is worth doing in
Phase 2 if there's room.

### 3.4 Worktree isolation for fleet lanes

**Problem.** Fleet subagents share one workspace (see 1.2). Even with the
per-path mutex, parallel *editing* lanes will produce interleaved, mutually
inconsistent changes. Today the honest guidance is "only fan out read-only
work" — and nothing enforces it.

**Reference.** omp `crates/pi-iso` — workspace isolation via APFS clones, btrfs
reflinks, overlayfs, or projfs depending on platform, with `task` subagents
fanning out into isolated worktrees and returning schema-validated results.

**Design (pragmatic).** Skip the filesystem-clone matrix; use git.

- `spawn_agents` gains `isolation: "shared" | "worktree"` (default `shared`).
- `worktree` → `git worktree add <tmp> --detach HEAD` per lane, root each
  lane's `Workspace` there, and on completion return `git diff` plus a
  patch the parent can apply. Auto-remove an unchanged worktree.
- Refuse `worktree` outside a git repo with a clear message.

This mirrors the Agent-tool isolation model the user already knows from Claude
Code, and it makes parallel implementation fleets actually safe.

**Effort.** ~1.5 days.

---

## Phase 4 — smaller items worth queueing

- **Magic keywords** (omp `docs/magic-keywords.md`). Standalone lowercase words
  in a prompt add a hidden per-turn instruction: `ultrathink` (raise reasoning
  effort), `orchestrate` (multi-agent contract). We already have effort settings
  and `spawn_agents`; this is a prompt-preprocessing pass plus editor
  highlighting. Matching rules matter — code fences, inline code, and
  inflections (`orchestrated`, `orchestrate.ts`) must not trigger. ~0.5 day.
- **`read_file` polymorphism.** omp's single `read` handles directories, images,
  PDFs, notebooks, SQLite, archives, URLs, and internal `pr://`/`issue://`
  schemes via one `path` string plus a selector grammar (`:50-100`, `:raw`,
  `:5-16,960-973`). We have most of the *rendering* already (viewer, data grid,
  attachments) but the model can't reach it. Highest-value slices, in order:
  (1) directory listing instead of an error, (2) multi-range selectors,
  (3) PDF/image into context, (4) our existing Parquet/CSV path.
- **`git` tool depth.** omp's `github` tool + `pr://`/`issue://` caching (SQLite,
  soft TTL 5 min / hard 7 days, stale-hit + background refresh) is a good model
  for making PR review cheap. Ours is `crates/harness-tools/src/git.rs`, 173
  lines.
- **Session tree / branching** (omp `docs/tree.md`): navigate to any prior entry
  and continue from there, with an auto-generated branch summary for the
  abandoned path. We have linear sessions; our store's schema would need parent
  pointers. Good UX, non-trivial.
- **Supply-chain hardening** (pi README). Exact-pinned direct deps,
  `min-release-age`, lockfile as ground truth with a pre-commit guard,
  `cargo audit` on a schedule, `cargo-vet` or `cargo-deny`. We're a Rust shop so
  the npm specifics don't port, but the *posture* does — and the desktop app
  does have an npm tree.
- **Publish sessions as training data.** pi actively solicits public OSS agent
  sessions (`pi-share-hf` → Hugging Face). We have the better version of this
  half-built: per-chat keep/reject review and JSONL export. Pointing that at
  Oxen datasets, with an explicit opt-in and secret redaction, is a natural
  extension of the training-data builder.

---

## Explicitly not doing (and why)

- **Hashline / content-hash patch language.** The 61%-token claim is credible
  and it is the most technically interesting thing in omp, but it is a whole
  subsystem: a grammar, a snapshot store with 3-way-merge recovery, a
  tree-sitter block resolver, and ~200 lines of model-facing prompt that every
  request pays for. It also couples `read` output format to `edit` input format,
  so both tools change together forever. Items 1.2 + 1.3 + 2.1 defend against
  most of the same failure modes (stale anchors, whitespace battles, retyped
  keepers, oversized reads) at a fraction of the cost. Revisit only if
  edit-token spend still looks bad with those in place — and measure first.
- **32 tools in one namespace.** omp ships `debug` (28 DAP ops), `browser`,
  `tts`, `generate_image`, `ssh`, `eval`. Our schema-budget test
  (`harness-tools/src/lib.rs:648`, ceiling 11 500 chars) exists precisely to
  stop this, and that ceiling is a feature. Anything new should either replace
  something or justify its permanent prefix cost.
- **Embedded bash.** Vendoring `brush-core` to get a persistent shell is a large
  maintenance surface; the fd-based cwd/env round-trip in 2.3 gets most of the
  benefit.
- **Collab/relay sessions.** Fun (`/collab` with QR codes, client-side encrypted
  relay), orthogonal to agent quality, and we already have the HTTP protocol
  layer if we ever want it.

---

## Suggested sequencing

| Order | Item | Effort | Payoff |
| --- | --- | --- | --- |
| 1 | 1.1 Project context files | 1d | Very high |
| 2 | 1.2 Read-before-edit + stale guard | 1.5d | Very high (correctness) |
| 3 | 1.3 Multi-edit | 1d | High |
| 4 | 1.4 Retrievable truncation | 0.5d | Medium |
| 5 | 2.1 Structural reads | 2–3d | Very high (context) |
| 6 | 2.2 Model roles + fallback | 1.5d | High (cost + resilience) |
| 7 | 3.3a Post-edit diagnostics | 1d | High |
| 8 | 2.3 Persistent shell | 1d | Medium |
| 9 | 3.4 Worktree fleet isolation | 1.5d | High (unlocks edit fleets) |
| 10 | 3.1 Stream rules | 3d | Medium-high (differentiating) |
| 11 | 3.2 Cross-session memory | 4–5d | High (differentiating) |
| 12 | 3.3b Full LSP tool | 5d | Very high, expensive |

Phase 1 (items 1–4, ~4 days) is the part I'd commit to without further
discussion: all four are additive, testable in isolation, and fix things that
are plainly missing rather than merely different.

## Open questions

1. **`edit.requireRead` default.** On is safer and matches Claude Code; it will
   occasionally annoy on trivial one-line fixes. Recommend on, with the error
   message telling the model exactly what to do.
2. **Outline reads default-on or opt-in?** Default-on saves the most tokens but
   changes what every `read_file` returns. Suggest shipping behind a setting
   defaulted **on** with the elided-span edit guard (2.1b) as the safety net,
   and an `A/B` measurement using the request log before flipping.
3. **Memory storage location.** `<workspace>/.oxen-harness/memory/` (visible,
   committable, reviewable — fits the Oxen story) vs. `~/.oxen-harness/memory/
   <project-hash>/` (private by default, omp's choice). Recommend workspace with
   a `.gitignore` hint and an explicit "commit this?" prompt.
4. **Do we want rule inheritance from Cursor/Cline/Copilot?** ~50 lines, pure
   migration upside, small ongoing format-drift cost.

---

## Progress log

Branch: `harness-parity-improvements` (off `main`). Paused 2026-07-24 mid-2.2.
Working tree clean; every check green at the last commit.

### Landed

| Item | Commit | Notes |
| --- | --- | --- |
| 1.1 Project context files | `fa6e0c4` | `harness-runtime/src/context_files.rs`; both hosts wired; `/context` in the CLI |
| 1.2 Read-before-edit + stale guard + per-path lock | `2640bd9` | `harness-tools/src/fs/state.rs`; also carries path-scoped rule injection |
| 1.3 Multi-edit | `b2d99cb` | `edits[]` in `EditFileArgs`; CRLF/BOM preservation fell out of it |
| 1.4 Retrievable truncation | `255afc4` | Spills the complete task log to the CCR store; `retrieve_original` now always registered |
| 2.1 Structural reads | `3c337e8` | `harness-tools/src/fs/outline.rs` + seen-range tracking + elided-edit refusal |
| 2.2 Model roles + fallback | `faf1f17` | `ModelRoles` (smol/summary) + `RetryPolicy.fallback_models`; `switching_to` on the retry event |
| 2.3 Persistent shell | `a4c138a` | `harness-tools/src/shell/session.rs` — cwd + env carried between calls |
| 3.3a Post-edit diagnostics | `aaf5680` | `harness-tools/src/fs/syntax.rs` — tree-sitter syntax regression check |
| 3.4 Worktree fleet isolation | `29ef48d` | `harness-agent/src/worktree.rs` + `isolate_edits` on `spawn_agents` |
| Self-review fixes | `b71e678` | Two real holes in the seen-range bookkeeping (see below) |
| Knowledge base | `fa6ffb4` | DOCUMENT-MAP / 02-status / 03-decisions brought up to date |

### Where the implementation differs from the plan above

- **1.1** — the plan proposed `harness-agent`; it landed in `harness-runtime`
  instead, next to the existing `project::prompt_section` both hosts already
  call, so no new dependency edges. It also discovers Copilot's
  `.github/copilot-instructions.md`, which the plan didn't list.
- **1.1/1.2 split** — glob-scoped rules were going to need turn-loop surgery.
  They didn't: `FileState` (1.2) already hooks every read/write, so a rule
  rides along on the first touch of a file it governs and never repeats.
- **1.4** — `read_file` deliberately does *not* spill to the store. Its
  overflow is the file itself, still on disk, and `offset` is the better
  affordance. The real data loss was `run_shell`, whose complete log existed
  on disk right up until `take_streams` deleted it. Also: the tool-schema
  budget was raised twice (11.5K → 12K); the test comment records why, and the
  next tool should replace one rather than raise it again.
- **2.1** — hides *function bodies* rather than keeping a per-language list of
  declarations worth showing. Simpler, less fragile, and it leaves types,
  constants, and imports (the interface) visible. No `full: bool` argument was
  added — an explicit `offset`/`limit` already forces a verbatim read and the
  schema budget is tight.

### What the self-review caught

Worth recording, because both were invisible to the tests that existed and both
widened what the model was allowed to edit:

- A read whose window fell past the end of the file — or which the 100k-char
  cap cut short — recorded the *requested* window as seen rather than the
  rendered one. `read_file offset=50` on a two-line file claimed lines 2..50.
- Every applied edit re-recorded the file as seen **in full**, so one edit
  inside a ten-line window handed back sight of the whole file, undoing
  exactly what windowed and outlined reads withhold. Seen ranges now remap
  into post-edit coordinates (`state::remap_seen`).

The lesson generalizes: the outline feature's safety rests entirely on this
bookkeeping, so any future change to how reads report themselves needs a test
at the `unseen_around` level, not just at the tool level.

### Still to do

| Item | Effort | Note |
| --- | --- | --- |
| 3.1 Stream rules (TTSR-lite) | 3d | Biggest remaining differentiator; concentrated risk in `turn.rs` — use the offline canned-SSE harness in the `verify` skill |
| 3.2 Cross-session memory | 4–5d | Open decision in this plan: where memory lives (workspace vs `~/.oxen-harness`) |
| 3.3b Full LSP tool | 5d | The syntax check shipped; types/rename need a language server per workspace |
| Phase 4 | — | Magic keywords, `read_file` polymorphism, git/PR caching, session tree, supply-chain posture, session publishing |

Two follow-ups the shipped work created:

- **`edit.requireRead` has no setting.** The plan promised one; the gate ships
  on with no way to turn it off. If it proves annoying, the flag already exists
  on `FileState` (`gated()`/`ungated()`) — it only needs a pref and a host wire.
- **Roles and fallbacks have no UI.** They live in `limits.json` alongside
  `max_session_tokens` and `summary_model`, which were already hand-edited
  only. A Settings → Models section would cover all four at once.
- **The tool-schema budget is at ~11.9K of 12K.** It was raised twice in this
  work (batch edits, always-on `retrieve_original`). The next tool should
  replace one rather than raise it again.

### Known unrelated flake

`harness-preview` `watch::tests::one_edit_batch_becomes_one_reload` fails
under load (a cold run, or the whole preview suite together) and passes when
run alone. Verified pre-existing by stashing this branch's changes and
reproducing on a clean tree — not caused by this work, and deliberately left
alone rather than fixed inside an unrelated feature branch.

One attempt was made and reverted; what it established, for whoever picks it
up:

- `notify` registers its OS watch on a background thread, measured at **~400ms
  on macOS**. The test writes immediately after `spawn`, so under load those
  writes land before anything is listening and are never reported. A probe
  that sleeps 500ms before a single write fires reliably (~400ms later:
  FSEvents latency plus the 300ms debounce).
- Nudging *continuously* to detect readiness is self-defeating — the debounce
  absorbs changes until things go quiet, so a steady stream of warm-up writes
  keeps the callback from ever firing. A readiness probe needs one write per
  round followed by a quiet second.
- Even with registration proven live first, the measured burst still fails to
  produce a callback within 5s when the whole preview suite runs together.
  That last part is unexplained and is where the next attempt should start —
  possibly FSEvents coalescing across the two write groups in one directory,
  which would argue for a fresh tempdir (or a subdirectory) for the measured
  burst.
