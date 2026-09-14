//! Spend limits for generation: a per-output ceiling and a per-tool-call
//! ceiling. Anything at or under both runs without asking; anything over
//! (or with an unknown price) goes to the user through the host's sink.

use crate::MediaKind;

/// The user's limits. `None` means "always ask" for that dimension.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MediaBudget {
    pub per_generation_usd: Option<f64>,
    pub per_run_usd: Option<f64>,
}

/// What a tool call is about to cost, as far as the catalog can tell.
#[derive(Debug, Clone, PartialEq)]
pub struct SpendEstimate {
    pub model: String,
    pub kind: MediaKind,
    /// How many outputs the call asks for.
    pub count: u32,
    /// Estimated cost of one output, when the catalog prices the model.
    pub per_output_usd: Option<f64>,
    /// `per_output_usd × count`.
    pub total_usd: Option<f64>,
    /// A one-line description of the request (duration, resolution, …).
    pub detail: String,
    /// Why the user is being asked, when they are.
    pub reason: String,
}

impl SpendEstimate {
    /// `$0.62` / `about $0.62` / `unknown cost`, for prompts and logs.
    pub fn total_label(&self) -> String {
        match self.total_usd {
            Some(usd) => format!("about {}", fmt_usd(usd)),
            None => "unknown cost".to_string(),
        }
    }

    /// The question a host puts to the user.
    pub fn question(&self) -> String {
        let what = if self.count == 1 {
            format!("1 {}", self.kind)
        } else {
            format!("{} {}s", self.count, self.kind)
        };
        let detail = if self.detail.is_empty() {
            String::new()
        } else {
            format!(" ({})", self.detail)
        };
        format!(
            "Generate {what} with {}{detail} for {}? {}",
            self.model,
            self.total_label(),
            self.reason
        )
    }
}

/// The budget's answer for one call.
#[derive(Debug, Clone, PartialEq)]
pub enum BudgetVerdict {
    /// Under every limit: run it.
    Within,
    /// Over a limit, or unpriced: ask first, for this reason.
    Ask(String),
}

impl MediaBudget {
    /// Judge an estimate against the limits.
    pub fn check(&self, per_output: Option<f64>, count: u32) -> BudgetVerdict {
        let Some(each) = per_output else {
            return BudgetVerdict::Ask(
                "The catalog has no price for this model, so the cost can't be estimated."
                    .to_string(),
            );
        };
        let total = each * count as f64;
        match self.per_generation_usd {
            Some(limit) if each <= limit => {}
            Some(limit) => {
                return BudgetVerdict::Ask(format!(
                    "Each output is {} — over your {} per-generation limit.",
                    fmt_usd(each),
                    fmt_usd(limit)
                ))
            }
            None => {
                return BudgetVerdict::Ask("Your settings ask before every generation.".to_string())
            }
        }
        match self.per_run_usd {
            Some(limit) if total <= limit => BudgetVerdict::Within,
            Some(limit) => BudgetVerdict::Ask(format!(
                "This run totals {} — over your {} per-run limit.",
                fmt_usd(total),
                fmt_usd(limit)
            )),
            None => BudgetVerdict::Ask("Your settings ask before every run.".to_string()),
        }
    }
}

/// `$0.01`, `$1.38`, `$12.50` — two decimals, or four when the amount is
/// under a cent so a $0.004 draft doesn't read as free.
pub fn fmt_usd(usd: f64) -> String {
    if usd > 0.0 && usd < 0.01 {
        format!("${usd:.4}")
    } else {
        format!("${usd:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget(gen: Option<f64>, run: Option<f64>) -> MediaBudget {
        MediaBudget {
            per_generation_usd: gen,
            per_run_usd: run,
        }
    }

    #[test]
    fn under_both_limits_runs() {
        assert_eq!(
            budget(Some(0.25), Some(1.0)).check(Some(0.01), 4),
            BudgetVerdict::Within
        );
    }

    #[test]
    fn over_per_generation_asks_with_the_amounts() {
        let v = budget(Some(0.25), Some(10.0)).check(Some(0.62), 1);
        let BudgetVerdict::Ask(reason) = v else {
            panic!("expected ask")
        };
        assert!(
            reason.contains("$0.62") && reason.contains("$0.25"),
            "{reason}"
        );
    }

    #[test]
    fn over_per_run_asks_even_when_each_is_cheap() {
        let v = budget(Some(0.25), Some(0.5)).check(Some(0.2), 4);
        assert!(matches!(v, BudgetVerdict::Ask(r) if r.contains("$0.80")));
    }

    #[test]
    fn unpriced_and_unset_limits_always_ask() {
        assert!(matches!(
            budget(Some(1.0), Some(1.0)).check(None, 1),
            BudgetVerdict::Ask(_)
        ));
        assert!(matches!(
            budget(None, Some(1.0)).check(Some(0.01), 1),
            BudgetVerdict::Ask(_)
        ));
        assert!(matches!(
            budget(Some(1.0), None).check(Some(0.01), 1),
            BudgetVerdict::Ask(_)
        ));
    }

    #[test]
    fn usd_formatting() {
        assert_eq!(fmt_usd(0.004), "$0.0040");
        assert_eq!(fmt_usd(0.01), "$0.01");
        assert_eq!(fmt_usd(1.375), "$1.38");
        assert_eq!(fmt_usd(0.0), "$0.00");
    }

    #[test]
    fn question_reads_naturally() {
        let e = SpendEstimate {
            model: "kling-video-o3-omni".into(),
            kind: MediaKind::Video,
            count: 2,
            per_output_usd: Some(0.73),
            total_usd: Some(1.46),
            detail: "5s, 1080p".into(),
            reason: "Over the per-run limit.".into(),
        };
        let q = e.question();
        assert!(
            q.starts_with(
                "Generate 2 videos with kling-video-o3-omni (5s, 1080p) for about $1.46?"
            ),
            "{q}"
        );
    }
}
