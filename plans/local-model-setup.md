# Local model setup — "download & run a model for me"

Make running a local model one-click: detect the machine, recommend a model that
fits, download a self-managed `llama-server` (no Homebrew), pull the weights, and
launch — all from a full-screen wizard.

## Decisions (confirmed)
- **Model sources:** all four — polished curated catalog, paste any Hugging Face
  GGUF, in-app HF search, Oxen.ai-hosted (plumbing + stub catalog now).
- **Runtime:** auto-download a *pinned* prebuilt `llama-server` into
  `~/.oxen-harness/runtime/`. **macOS-first**, modular for Windows/Linux later.
- **Hardware:** detect RAM/VRAM/accelerator, recommend fit, **auto-pick the best
  quant** that fits (with an advanced override).
- **HF:** support a Hugging Face token for gated/private repos.
- **UI:** full-screen setup wizard.

## Runtime spike — VALIDATED (b9835)
- macOS asset is `llama-{tag}-bin-macos-arm64.tar.gz` (tar.gz, not zip).
- Tarball = one dir `llama-{tag}/` with `llama-server` + all `.dylib`s flat,
  including `libggml-metal.dylib` (Metal build).
- Binary `rpath = @loader_path` → dylibs load co-located. `--version` exits 0.
- curl/reqwest downloads set no `com.apple.quarantine` xattr → no Gatekeeper
  prompt (strip xattr anyway as belt-and-suspenders).

## Architecture (backend in `harness-local`)
- `hardware.rs` — `HardwareProfile { ram_bytes, vram_bytes, accelerator,
  chip_label, usable_budget }`; mac detection via sysctl; conservative fallback.
- `fit.rs` — quant ladder, footprint estimate (weights + KV + overhead),
  `Fit { Good|Tight|TooBig }`, `pick_quant()`.
- `runtime.rs` — pinned version, per-OS asset resolver (mac arm64 done),
  download + extract (.tar.gz) + verify (`--version`) + strip quarantine;
  binary precedence `LLAMA_SERVER env → managed → PATH/brew`.
- `source.rs` (Phase 2/3) — `ModelSource: Curated | HuggingFace | Oxen` →
  `ResolvedModel`; HF sibling listing + quant parse + token; Oxen pull.
- `store.rs` — per-model metadata sidecar so arbitrary models persist + show
  friendly names; server uses a hardware-budgeted context (not hardcoded 8192).

## Tauri commands
`detect_hardware`, `runtime_status`, `install_runtime` (streams progress),
`list_model_catalog` (fit + quant annotated), `resolve_hf_model`,
`search_hf_models`, `set_hf_token`, `installed_local_models`; reuse
`pull_model`/`remove_model`/`use_local_model`/progress events.

## Frontend — full-screen `LocalSetup` wizard
1. Your machine → 2. Choose a model (Recommended / Hugging Face / Oxen, with fit
badges + auto-quant) → 3. Runtime check/install → 4. Download → 5. Ready.
Existing thin modal becomes "Manage local models".

## Phases (each shippable)
- **0 — Backend foundations + runtime spike** ✅ (hardware, fit, runtime; validated b9835 install).
- **1 — Wizard + curated, hardware-aware** ✅ (`LocalSetup` full-screen wizard replaces the modal;
  fit badges + auto-quant; one-click runtime install).
- **2 — Hugging Face** ✅ (paste → resolve + search; quant parse, `HF_TOKEN`, sidecar metadata).
- **3 — Oxen.ai source** ✅ plumbing (`Origin::Oxen` + download URL + UI tab); featured catalog
  stubbed empty until repos land.
- **4 — Polish** — done: hardware-budgeted server context (`plan_context`), manage/remove,
  sidecar persistence, **HF live autocomplete** (debounced `search_hf_models`, keyboard-nav
  combobox merging search + paste), **disk-space stats** (`disk_space` via statvfs → disk bar +
  "not enough space" gating before download). Deferred: download resume, runtime
  update/uninstall, partial GPU offload.

## Implementation notes (as built)
- `harness-local`: `hardware.rs`, `fit.rs`, `runtime.rs`, `source.rs` (ModelRef/Origin, HF
  list/search/parse, Oxen stub), `store.rs` rewritten id-keyed (`{id}.gguf` + `{id}.json` sidecar).
- Binary precedence: `LLAMA_SERVER` env → managed runtime → PATH/Homebrew.
- Tauri commands: `detect_hardware`, `runtime_status`, `install_runtime`, `list_model_catalog`,
  `resolve_hf_model`, `search_hf_models`, `hf_token_present`, `set_hf_token`, `download_model`,
  `installed_local_models`, `use_local_model`, `remove_model`.
- Frontend: `features/models/LocalSetup.tsx` (+ test), opened via the composer model picker
  ("Set up a local model…") and `modelsOpen`. Old `ModelsModal` removed.

## Risks
1. Prebuilt llama.cpp packaging/dylib loading — **validated** for mac arm64.
2. Memory math — bias conservative ("tight" beats a crash on load).
3. HF API shapes / rate limits / token errors (Phase 2).
