//! The machine-readable result of a review, parsed leniently from the final
//! step's reply.
//!
//! The shape (file + line + priority + verdict + failure scenario) is what
//! makes findings actionable: a fixing agent can be pointed at "finding 2" and
//! know exactly where to look and what to reproduce. Parsing is lenient — the
//! final step is still a model — and a reply that isn't valid JSON degrades to
//! a raw-text report rather than an error.

use serde::{Deserialize, Serialize};

/// One review finding, ranked and located.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    /// One-line, imperative statement of the defect.
    #[serde(default)]
    pub title: String,
    /// Repo-relative path.
    #[serde(default)]
    pub file: String,
    /// 1-indexed anchor line, when known.
    #[serde(default)]
    pub line: Option<u32>,
    /// 0 = drop everything … 3 = nice to have (Codex's P0–P3 scale).
    #[serde(default)]
    pub priority: Option<u8>,
    /// `CONFIRMED` or `PLAUSIBLE` when a verify step ran.
    #[serde(default)]
    pub verdict: Option<String>,
    /// Why this is a bug and when it bites (≤ one paragraph).
    #[serde(default)]
    pub body: String,
    /// Concrete inputs/state → wrong outcome.
    #[serde(default)]
    pub failure_scenario: String,
}

impl Finding {
    /// `path/to/file.rs:42`, or just the path when no line is known.
    pub fn location(&self) -> String {
        match self.line {
            Some(line) if !self.file.is_empty() => format!("{}:{line}", self.file),
            _ => self.file.clone(),
        }
    }
}

/// The parsed final report: findings plus the overall verdict, with the raw
/// final-step text kept as the fallback for replies that didn't parse.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ReviewReport {
    #[serde(default)]
    pub findings: Vec<Finding>,
    /// "correct" / "incorrect", when the report step gave one.
    #[serde(default)]
    pub overall_correctness: Option<String>,
    #[serde(default)]
    pub overall_explanation: Option<String>,
    /// The final step's verbatim reply (shown when parsing failed).
    #[serde(default)]
    pub raw: String,
    /// Whether `findings` came from parsed JSON (vs. a raw-text fallback).
    #[serde(default)]
    pub parsed: bool,
}

impl ReviewReport {
    /// Parse the final step's reply. Tries the whole text as JSON, then the
    /// outermost `{…}` (stray prose/fences), then falls back to raw text.
    pub fn parse(raw: &str) -> Self {
        let mut report = harness_core::json::first_object(raw)
            .and_then(|value| serde_json::from_value::<ReviewReport>(value).ok())
            .map(|mut r| {
                r.parsed = true;
                r
            })
            .unwrap_or_default();
        report.raw = raw.to_string();
        report
    }

    /// Render the report as the markdown block both hosts show and inject
    /// into the conversation — numbered so a follow-up "fix 1 and 3" is
    /// unambiguous.
    pub fn to_markdown(&self) -> String {
        if !self.parsed {
            return format!(
                "## Code review\n\nThe report step returned unstructured output:\n\n{}",
                self.raw.trim()
            );
        }
        let mut out = String::new();
        if self.findings.is_empty() {
            out.push_str("## Code review: no findings\n\nNothing qualifying survived verification — the change looks clean.");
        } else {
            out.push_str(&format!(
                "## Code review: {} finding{}\n",
                self.findings.len(),
                if self.findings.len() == 1 { "" } else { "s" }
            ));
            for (i, f) in self.findings.iter().enumerate() {
                let priority = f.priority.map(|p| format!("[P{p}] ")).unwrap_or_default();
                let verdict = f
                    .verdict
                    .as_deref()
                    .map(|v| format!(" — {v}"))
                    .unwrap_or_default();
                out.push_str(&format!(
                    "\n{}. **{}{}** `{}`{}\n",
                    i + 1,
                    priority,
                    f.title,
                    f.location(),
                    verdict,
                ));
                if !f.body.is_empty() {
                    out.push_str(&format!("   {}\n", f.body));
                }
                if !f.failure_scenario.is_empty() {
                    out.push_str(&format!("   *Failure scenario:* {}\n", f.failure_scenario));
                }
            }
        }
        if let Some(correctness) = &self.overall_correctness {
            let explanation = self.overall_explanation.as_deref().unwrap_or_default();
            out.push_str(&format!(
                "\n**Overall:** the change looks {correctness}. {explanation}\n"
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_full_report_amid_prose_and_fences() {
        let raw = r#"Here is the report:
```json
{"findings":[{"title":"Fix off-by-one in pager","file":"src/pager.rs","line":42,"priority":1,"verdict":"CONFIRMED","body":"The loop drops the last page.","failure_scenario":"10 items, page size 5 → page 2 empty"}],"overall_correctness":"incorrect","overall_explanation":"One confirmed bug."}
```"#;
        let report = ReviewReport::parse(raw);
        assert!(report.parsed);
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].location(), "src/pager.rs:42");
        assert_eq!(report.overall_correctness.as_deref(), Some("incorrect"));

        let md = report.to_markdown();
        assert!(md.contains("1. **[P1] Fix off-by-one in pager** `src/pager.rs:42` — CONFIRMED"));
        assert!(md.contains("*Failure scenario:*"));
        assert!(md.contains("looks incorrect"));
    }

    #[test]
    fn empty_findings_render_as_a_clean_review() {
        let report = ReviewReport::parse(r#"{"findings":[],"overall_correctness":"correct"}"#);
        assert!(report.parsed);
        assert!(report.to_markdown().contains("no findings"));
    }

    #[test]
    fn unparseable_reply_degrades_to_raw_text() {
        let report = ReviewReport::parse("I looked around and it seems fine.");
        assert!(!report.parsed);
        assert!(report.findings.is_empty());
        assert!(report.to_markdown().contains("it seems fine"));
    }
}
