//! Estimating whether a model will run on the detected hardware, and picking the
//! best quantization that fits.
//!
//! The math is intentionally conservative: it's far better to label a model
//! "tight" (or steer the user to a smaller quant) than to let `llama-server`
//! crash on load. Footprint = weight bytes + KV cache for the *served* context +
//! a fixed runtime/compute overhead. We size against [`HardwareProfile::usable_budget`].

use serde::Serialize;

use crate::hardware::HardwareProfile;

/// A quantization preset. Ordered best-quality (largest) first in [`QUANTS`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quant {
    /// Canonical name as it appears in GGUF filenames, e.g. `Q4_K_M`.
    pub name: &'static str,
    /// Effective bits per weight, for sizing the weights.
    pub bits_per_weight: f64,
}

/// The quant ladder we consider, highest quality (largest) first. `Q4_K_M` is
/// the consumer sweet spot and the baseline the curated catalog ships at.
pub const QUANTS: &[Quant] = &[
    Quant { name: "Q8_0", bits_per_weight: 8.5 },
    Quant { name: "Q6_K", bits_per_weight: 6.6 },
    Quant { name: "Q5_K_M", bits_per_weight: 5.7 },
    Quant { name: "Q4_K_M", bits_per_weight: 4.9 },
    Quant { name: "Q3_K_M", bits_per_weight: 3.9 },
    Quant { name: "IQ3_XS", bits_per_weight: 3.3 },
];

/// The default context window we plan to serve a local model with. The KV cache
/// scales with this, so we budget against it rather than the model's (often far
/// larger) native maximum.
pub const PLANNED_CONTEXT: u32 = 8192;

/// How well a model is expected to run on the given hardware.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Fit {
    /// Comfortable — fits with headroom to spare.
    Good,
    /// Will load but leaves little room; may be slow or pressure memory.
    Tight,
    /// Won't fit in the usable budget.
    TooBig,
}

/// A candidate download for a model at a particular quant.
#[derive(Debug, Clone)]
pub struct QuantCandidate {
    pub quant: String,
    /// On-disk weight size in bytes (exact from the source, or estimated).
    pub weight_bytes: u64,
}

/// KV-cache + runtime overhead (bytes) for serving `context` tokens. ~128 KB per
/// token covers a GQA model's fp16 KV conservatively, plus ~1.2 GB for compute
/// buffers and the runtime itself.
pub fn kv_overhead(context: u32) -> u64 {
    context as u64 * 131_072 + 1_200_000_000
}

/// Total memory (bytes) to run `weight_bytes` of weights at `context` tokens.
pub fn footprint(weight_bytes: u64, context: u32) -> u64 {
    weight_bytes.saturating_add(kv_overhead(context))
}

/// Verdict for a given weight size against a memory `budget`.
pub fn fit_for(weight_bytes: u64, context: u32, budget: u64) -> Fit {
    let need = footprint(weight_bytes, context);
    if need <= budget / 10 * 7 {
        Fit::Good
    } else if need <= budget {
        Fit::Tight
    } else {
        Fit::TooBig
    }
}

/// Convenience: fit a candidate against a hardware profile at the planned context.
pub fn fit_on(profile: &HardwareProfile, weight_bytes: u64) -> Fit {
    fit_for(weight_bytes, PLANNED_CONTEXT, profile.usable_budget)
}

/// Estimate a model's weight size at `bpw` bits/weight given its known size at a
/// reference quant. Used to fill in a quant ladder for the curated catalog,
/// where only the `Q4_K_M` size is published.
pub fn scale_weight_bytes(reference_bytes: u64, reference_bpw: f64, target_bpw: f64) -> u64 {
    if reference_bpw <= 0.0 {
        return reference_bytes;
    }
    (reference_bytes as f64 * (target_bpw / reference_bpw)) as u64
}

/// A conservative window cap for a model whose native context we couldn't read.
/// We'd rather serve less than over-commit KV cache against an unknown limit.
pub const UNKNOWN_NATIVE_CAP: u32 = 32_768;

/// Choose the largest context window to serve a model with that still fits the
/// memory `budget` (weights + KV cache), capped by the model's `native` window.
/// Floors at 2048 so we always serve *something*.
///
/// The window climbs as high as the model *and* the machine's memory allow:
/// the KV cache (see [`kv_overhead`]) is the real limiter, so a small model on a
/// large-memory machine can serve far more than the old flat 32K. When `native`
/// is unknown (0) we fall back to [`UNKNOWN_NATIVE_CAP`].
pub fn plan_context(budget: u64, weight_bytes: u64, native: u32) -> u32 {
    const LADDER: &[u32] = &[
        1_048_576, 524_288, 262_144, 131_072, 65_536, 32_768, 16_384, 8_192, 4_096, 2_048,
    ];
    let cap = if native == 0 { UNKNOWN_NATIVE_CAP } else { native };
    for &c in LADDER {
        if c <= cap && footprint(weight_bytes, c) <= budget {
            return c;
        }
    }
    2048
}

/// Choose the best-quality quant that fits the `budget` at `context`. Candidates
/// must be ordered best-first (largest). Returns the first that is at least
/// `Tight`; if none fit, returns the smallest so we still suggest *something*
/// (the UI flags it as too big).
pub fn pick_quant<'a>(
    candidates: &'a [QuantCandidate],
    context: u32,
    budget: u64,
) -> Option<&'a QuantCandidate> {
    candidates
        .iter()
        .find(|c| fit_for(c.weight_bytes, context, budget) != Fit::TooBig)
        .or_else(|| candidates.last())
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn fit_buckets_by_budget() {
        // ~2.27 GB of KV + overhead is added on top of weights at 8k context.
        let budget = 12 * GIB;
        // 5 GB weights → ~7.3 GB → under 70% of 12 GB → Good.
        assert_eq!(fit_for(5 * GIB, PLANNED_CONTEXT, budget), Fit::Good);
        // 9 GB weights → ~11.3 GB → between 70% and 100% → Tight.
        assert_eq!(fit_for(9 * GIB, PLANNED_CONTEXT, budget), Fit::Tight);
        // 20 GB weights → well over budget → TooBig.
        assert_eq!(fit_for(20 * GIB, PLANNED_CONTEXT, budget), Fit::TooBig);
    }

    #[test]
    fn pick_quant_takes_best_that_fits() {
        // Best-first ladder of the same model at decreasing quants.
        let candidates = vec![
            QuantCandidate { quant: "Q8_0".into(), weight_bytes: 20 * GIB },
            QuantCandidate { quant: "Q5_K_M".into(), weight_bytes: 12 * GIB },
            QuantCandidate { quant: "Q4_K_M".into(), weight_bytes: 9 * GIB },
            QuantCandidate { quant: "Q3_K_M".into(), weight_bytes: 6 * GIB },
        ];
        // 16 GB budget → Q8 too big; Q5_K_M (~15 GB footprint) fits tight and is
        // the highest-quality that fits, so it wins over the smaller Q4_K_M.
        let pick = pick_quant(&candidates, PLANNED_CONTEXT, 16 * GIB).unwrap();
        assert_eq!(pick.quant, "Q5_K_M");
    }

    #[test]
    fn pick_quant_falls_back_to_smallest_when_nothing_fits() {
        let candidates = vec![
            QuantCandidate { quant: "Q8_0".into(), weight_bytes: 40 * GIB },
            QuantCandidate { quant: "Q3_K_M".into(), weight_bytes: 24 * GIB },
        ];
        let pick = pick_quant(&candidates, PLANNED_CONTEXT, 8 * GIB).unwrap();
        assert_eq!(pick.quant, "Q3_K_M");
    }

    #[test]
    fn plan_context_climbs_with_memory_up_to_native() {
        // A small 9B-ish model (~6 GB weights) with a 1M native window on a
        // big-memory machine should climb well past the old 32K lid.
        let weights = 6 * GIB;
        // ~96 GB usable: KV at 131072 tokens ≈ 17.2 GB + 1.2 GB + 6 GB ≈ 24 GB,
        // and 262144 ≈ 41 GB — both fit, so it should reach at least 131072.
        let ctx = plan_context(96 * GIB, weights, 1_048_576);
        assert!(ctx >= 131_072, "expected a large window, got {ctx}");
    }

    #[test]
    fn plan_context_never_exceeds_native_window() {
        // Plenty of memory, but the model only supports 8K natively.
        let ctx = plan_context(256 * GIB, 2 * GIB, 8_192);
        assert_eq!(ctx, 8_192);
        // A non-ladder native value clamps down to the largest ladder rung that
        // fits under it.
        let ctx = plan_context(256 * GIB, 2 * GIB, 40_000);
        assert_eq!(ctx, 32_768);
    }

    #[test]
    fn plan_context_caps_unknown_native_conservatively() {
        // Unknown native (0) + huge memory must not over-commit: cap at 32K.
        let ctx = plan_context(256 * GIB, 2 * GIB, 0);
        assert_eq!(ctx, UNKNOWN_NATIVE_CAP);
    }

    #[test]
    fn plan_context_shrinks_when_memory_is_tight() {
        // A 1M-native model on a small machine is limited by KV memory, not the
        // native window: an 8 GB budget can't hold a large window.
        let ctx = plan_context(8 * GIB, 4 * GIB, 1_048_576);
        assert!(ctx <= 32_768, "tight memory should keep the window small, got {ctx}");
    }

    #[test]
    fn scaling_tracks_bits_per_weight() {
        // Q4_K_M (4.9 bpw) ~5 GB → Q8_0 (8.5 bpw) should be ~1.73x.
        let q4 = 5 * GIB;
        let q8 = scale_weight_bytes(q4, 4.9, 8.5);
        assert!(q8 > q4);
        let ratio = q8 as f64 / q4 as f64;
        assert!((ratio - 8.5 / 4.9).abs() < 0.01);
    }
}
