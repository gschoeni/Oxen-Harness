# On-device image generation

Status: image generation shipped 2026-10-12 (CLI + desktop + HTTP host share it).
Video is out of scope for now; the engine seam is where a video engine goes.

## What runs

Three commercially licensed (Apache-2.0) models, all through
stable-diffusion.cpp's `sd-server` on Metal — no Python anywhere:

| Model (`local/…`) | Weights (on disk) | Steps | 1024², warm | Notes |
|---|---|---|---|---|
| `iris-3b-turbo` | 6.0 GB bf16 + 2.5 GB Qwen3-VL-4B Q4_K_M | 4 | **~21 s** | Pixel space (no VAE); fastest; no references |
| `flux-2-klein-4b` | 2.5 GB Q4_0 + 0.2 GB VAE + 2.5 GB Qwen3-4B Q4_K_M | 4 | **~37 s** | Edits/combines up to 4 reference images |
| `z-image-turbo` | 3.9 GB Q4_K + 0.3 GB VAE + (shared Qwen3-4B) | 8 | **~110 s** | Photoreal; slowest |

Klein and Z-Image share one Qwen3-4B text encoder file. Everything is
~17 GB on disk when all three are pulled.

## Benchmarks (36 GB M4 Max, macOS 26.6, sd.cpp master-959-d637cfb)

The machine was already under heavy load (≈12 GB in the compressor, ≈22 GB
swap, ~10 GB free), which is the realistic case on a dev laptop. A watchdog
killed the server on critical pressure, <12% free, or +6 GB swap.

| Run | Sampling | Decode | Total | Peak footprint |
|---|---|---|---|---|
| klein Q8, weights resident | 7.5 s/it | — | killed by watchdog (critical pressure at decode) | 7.9 GB |
| klein Q8, `--mmap --vae-tiling` | 7.7 s/it | 21 s (256 px tiles) | 52 s | 0.67 GB |
| klein Q8, no flash attention | 8.4 s/it | — | 60 s | 0.69 GB |
| **klein Q4_0, `--mmap`** | **7.1 s/it** | **8 s** | **37 s** | **0.63 GB** |
| klein Q4_0, `--vae-conv-direct` | 7.1 s/it | 52–60 s | 85–90 s | 0.72 GB |
| **Z-Image-Turbo Q4_K, shared Qwen3-4B** | **12.5 s/it ×8** | **8 s** | **110 s** | **0.71 GB** |
| Iris-3B base Q4_K_M, CFG 3, 20 steps | 11.8 s/it | none | 247 s | 0.95 GB |
| **Iris-3B Turbo bf16** | **5.0 s/it** | none | **21 s** | **0.86 GB** |

Takeaways that are now code:

- **`--mmap` always.** File-backed weight pages are evictable, so macOS
  drops and re-reads them instead of swapping the user's apps; the
  footprint fell from 7.9 GB to ~0.7 GB with no speed cost.
- **No VAE tiling, no direct conv** — both made decoding much slower.
- **Q4_0 over Q8 for klein**: same speed (compute-bound), same look,
  1.8 GB less disk.
- **Iris base is too slow to ship** (not distilled, CFG doubles the work);
  Turbo is the fastest model here.
- Quality checked by eye on the same prompt/seed; the edit test turned a
  daytime fox into the same fox under the northern lights.

## Shape

- `harness-media` knows only the `LocalMedia` trait (`local.rs`): models as
  catalog rows with `local/` ids, `generate(job, progress)`, `cancel(id)`.
  Local models skip the key, upload, spend prompt and hub queue, and share
  the batch/report/save tail with hosted ones. With no API key, an installed
  local model stands in for a hosted default (and says so).
- `harness-media-local` is the provider: catalog → store → engine, with the
  service queueing GPU jobs and holding cancel tokens. A new runtime is a
  `MediaEngine` impl plus one registration line; catalog entries name it.
  "Write our own" (e.g. an MLX- or Metal-backed Rust engine) plugs in there.
- Lifecycle: the server starts on the first local generation (never at
  boot), stays warm 5 minutes, swaps when another model is asked for, is
  refused under critical memory pressure, is recorded in the pid registry
  (reaped at the next boot if a host dies), and has a lifeline watcher that
  stops it within ~2 s of its host exiting.

## Memory guard and offline (2026-10-13)

Every local model load (chat via llama-server, images via sd-server) is
checked and watched — `harness_local::memory`:

- **Before:** refuse at critical pressure or when a model can't fit at all;
  warn, with the cost in GB pushed to swap, when it's bigger than what's free.
- **While running:** a watchdog per server, twice a second. Stops on critical
  pressure, <5% kernel-free, 6 GB of swap growth, or warn-level pressure with
  2 GB of growth. The newest model is stopped first; the others wait and
  re-measure from that point. Reasons go out as `memory.notice`.
- **After:** a lifeline watcher stops a server within ~2 s of its host exiting
  (verified with `kill -9`), on top of the boot-time pid reaper.

Offline rehearsals (all network blocked by dead-port proxies, 36 GB M4 Max
with ~20 GB of apps open):

| Combo | Result |
|---|---|
| Gemma 4 26B Q3 alone (CLI + desktop) | ✅ edits files, answers; loads in ~15 s |
| Qwen3.5 4B + Iris Turbo (CLI + desktop) | ✅ image in 27–37 s, reply after |
| Qwen3.5 4B + FLUX.2 klein edit | ✅ 1m54s |
| Gemma 4 26B + Iris or klein, apps open | ⚠ guard stops the image job (critical pressure); Gemma survives and explains |

Bugs found by these runs and fixed: both models stopped at once; free-% misread
mapped weights; the stop reason lost to two races; local chat models sent
images they can't read (llama-server has no vision projector → every later
request rejected); the desktop host rejected catalog ids.

## Left to do

- **Faster sampling**: sd.cpp reaches ~5 effective TFLOPS here. Worth
  trying: klein bf16 weights (Iris bf16 ran faster per parameter than the
  quantized models), and an MLX-backed engine behind `MediaEngine`.
- **Chat + image on one 36 GB machine with apps open**: let the image job
  borrow memory from an idle chat model (unload it for the render, reload
  after) instead of the guard stopping the render.
- **Video**: a `video` kind entry plus an engine; `generate_video` already
  routes `local/` ids the same way.
