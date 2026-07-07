# Context compression — save tokens on every request

Port the key techniques from [headroom](https://github.com/headroomlabs-ai/headroom)
(reference clone at `../headroom`; the canonical algorithms live in its
`crates/headroom-core`) natively into the harness, behind a tri-mode setting so
the savings can be measured before trusting them.

The problem: the agent resends the full transcript on every model call, and the
bulk of it is stale tool output (30k-char shell dumps, file reads, JSON API
results) the model has already acted on. Today that bloat is only reclaimed
*reactively* — `compact.rs` prunes/summarizes when the window overflows. This
feature compresses it *proactively* on every request, and makes the compression
reversible so nothing is ever truly lost to the model.

## Decisions (confirmed)
- **New crate** `harness-compress`: a lean native port, using headroom's Rust
  source as reference — not a dependency on `headroom-core` (which drags in
  ONNX/embeddings/tokenizers and weekly-release churn).
- **v1 scope:** statistical JSON-array crusher ("SmartCrusher-lite"), log/long-
  text line crusher, and CCR (compress-cache-retrieve: originals stored
  locally, `<<ccr:HASH>>` markers inline, a `retrieve_original` tool). v2 ideas
  documented below.
- **Setting:** tri-mode `off | audit | on`, persisted like tool prefs.
  **Audit** runs the compressor and reports would-be savings but sends the
  original request untouched — a risk-free way to measure the difference.

## Architecture

### `crates/harness-compress` (new)
- `detect.rs` — cheap content classification of a tool result: `Json` (parses
  as array/object), `Lines` (long multi-line text: logs, shell output, file
  reads), `Short`/other (passthrough).
- `crush.rs` — the JSON crusher. For arrays of ≥5 objects: per-field stats
  (unique ratios, constants, numeric mean/σ), keep first/last anchors + items
  with error keywords + numeric anomalies (>2σ), dedup identical rows, cap at
  `max_items_after_crush` (15). Never crush arrays of distinct entities with no
  keep-signal (headroom's "unique entities" guard). Dropped rows are stored in
  the CCR store; the output keeps the original array shape plus a trailing
  `{"_ccr_dropped": "<<ccr:HASH N_rows_offloaded>>"}` sentinel.
- `lines.rs` — the log/text crusher: collapse runs of repeated lines
  ("× N repeats"), keep head/tail windows and every error-keyword line, elide
  the middle with a `<<ccr:HASH>>` marker.
- `ccr.rs` — `CcrStore`: in-memory, capacity-bounded (FIFO evict), keyed by
  `sha256(original)[:12 hex]` (matches headroom's row-drop scheme). Shared
  `Arc` between the compressor (writes) and the retrieve tool (reads).
- `lib.rs` — `CompressionMode { Off, Audit, On }`, `CompressConfig`,
  `compress_tool_result()` entry point, `CompressionReport`.

Safety rails (all from headroom, kept in v1):
- Only **tool** messages are touched, and never the most recent
  `keep_recent_tools` (2) of them — same protection as `compact.rs`.
- Error outputs are sacred: content with error keywords (error, exception,
  failed, panic, fatal, timeout, denied, …) near the start is left verbatim.
- Small content (< ~800 chars) is left alone; a compression that doesn't save
  ≥15% is rejected; already-compressed content (`<<ccr:` present) is skipped;
  `retrieve_original` results are never re-compressed (would loop).
- Deterministic: same input → same output (sorted field iteration, no RNG).

### Integration (`harness-agent`)
- `AgentConfig.compression: CompressionMode` (default `Off`).
- When mode ≠ Off the Agent creates an `Arc<CcrStore>`; when mode = On it also
  registers the `retrieve_original` tool into its own registry at construction.
- `stream_reply()` calls a new `prepare_outbound()` which clones the transcript
  (as `outbound_messages()` does today), runs the compressor over eligible tool
  messages, and returns the messages plus a `CompressionReport`.
  - **On:** send the compressed clone. **Audit:** compute the report, send the
    original. **Off:** today's behavior, zero overhead.
  - The persisted transcript and in-memory `self.messages` are **never**
    mutated — exactly the rule the existing compaction follows.
- New `AgentEvent::Compression { mode, saved_tokens, total_saved_tokens }`
  emitted per request when savings > 0 (estimated via the calibrated
  chars/token heuristic, same as the budget meter).
- `retrieve_original` tool (in `harness-tools`, holding `Arc<CcrStore>`): takes
  a `hash`, returns the stored original (or a helpful "expired/unknown hash"
  message). Its description teaches the model what `<<ccr:HASH>>` markers mean.

Note on calibration: in On mode the endpoint reports usage for the *compressed*
prompt, so `token_ratio` calibrates toward what is actually sent — the budget
meter and compaction trigger keep tracking reality.

### Setting (`harness-runtime` + hosts)
- `harness-runtime/src/compression.rs` — `CompressionPrefs { mode }` persisted
  to `~/.oxen-harness/compression.json` (versioned, mirrors `tools.rs`).
- `harness-config/paths.rs` — `compression_file()`.
- CLI: `agent_config()` reads prefs into `AgentConfig`; renderer prints a dim
  per-request savings note (like the Compacted notice).
- Desktop: `get_compression_mode` / `set_compression_mode` Tauri commands, a
  Settings page control (three-way choice with explanations), `agent://compression`
  event → store → savings surfaced next to the TokenMeter; cumulative
  `total_tokens_saved` in the `app_meta` table (same pattern as
  `total_tokens_used`).
- Like tool prefs, the mode is applied at agent build time — new/resumed chats
  pick up a change, not the live one.

### Measuring the difference
1. Set mode to **audit**, work normally: every request reports what compression
   *would have* saved; cumulative counter accrues estimated savings.
2. Flip to **on**: the same reports now reflect applied savings, and the real
   usage (`Usage.prompt_tokens`) visibly drops for the same workload.
3. The Inspector's Raw JSON view shows the stored (uncompressed) transcript;
   compressed variants can be eyeballed via the per-message `<<ccr:` markers in
   what the model echoes, and (v2) a dedicated before/after Inspector toggle.

## v2 roadmap (documented, not built)
- **Retrieval-rate feedback loop** (headroom `compression_feedback.py`): track
  per-tool how often the model calls `retrieve_original` after compression;
  >50% → compress that tool less or not at all; <20% → compress harder. The
  ACON insight: retrieval is the signal you compressed too hard.
- **Adaptive K** (headroom `adaptive_sizer.rs`): pick how many array items to
  keep via a knee-point on the content-diversity curve instead of a fixed 15.
- **Relevance anchoring**: score dropped candidates against the latest user
  message (BM25-lite) and pin relevant items — headroom's query anchors.
- **Code-aware compression**: AST-based body truncation keeping
  imports/signatures/docstring first-lines (headroom's `code_compressor.py`;
  tree-sitter — not yet in their Rust port either).
- **Inspector before/after view**: a toggle in the Raw JSON drawer rendering
  the compressed outbound payload next to the stored transcript.
- **Compaction integration**: let `compact_to_fit` stage 1 call the crusher
  before falling back to blunt eliding, so overflow recovery is also lossless.
- **Persistent CCR store**: spill originals to SQLite (headroom's
  `SqliteCcrStore`) so retrieval works across process restarts.
- **Prompt caching alignment**: if the Oxen endpoint ever exposes provider
  cache controls, only compress content *behind* the cache watermark
  (headroom's live-zone/byte-surgery model exists for this).
