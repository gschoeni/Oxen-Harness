//! Generation speed of one model call, for the live `tok/s` readouts.
//!
//! A [`ThroughputMeter`] rides a single streamed call. While the reply
//! streams it estimates the output so far from every generated character
//! (visible text, tool-call arguments, and a thinking model's reasoning, which
//! counts against the same decode budget) and reports a reading at most every
//! [`EMIT_EVERY`]. When the call ends, [`ThroughputMeter::finish`] re-states
//! the reading with the endpoint's exact completion count when it sent one.
//! Tokenizers pack very different amounts of text into a token, so the live
//! estimate divides by a characters-per-token ratio the agent learns from
//! each model's exact counts ([`learned_chars_per_token`]); until it has one,
//! the generic [`budget::CHARS_PER_TOKEN`].
//!
//! The rate is the decode rate, `tokens / (last token − first token)`: the
//! wait before the first token (queueing, prompt processing) is reported
//! separately as time-to-first-token, so a long prompt doesn't read as a slow
//! model and a cloud model's network hop doesn't hide how fast it decodes.

use std::time::{Duration, Instant};

use crate::budget;

/// The minimum gap between live readings: frequent enough to feel live,
/// sparse enough that the event stream stays cheap for every host.
pub const EMIT_EVERY: Duration = Duration::from_millis(250);

/// Below this decode window a rate is noise — one buffered chunk can
/// "arrive" at any speed, and providers often open with a few tiny chunks —
/// so no rate is reported.
pub const MIN_RATE_WINDOW: Duration = Duration::from_millis(500);

/// One reading of a model call's generation speed. Integer milliseconds keep
/// it `Eq` like every other event; [`Self::tokens_per_sec`] derives the rate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Throughput {
    /// The model that is answering (a fallback model once the chain moved).
    pub model: String,
    /// Tokens generated so far: estimated while streaming, the endpoint's own
    /// count on the final reading when it reported one.
    pub output_tokens: usize,
    /// How long the model has been generating, first token to latest.
    pub decode_ms: u64,
    /// The wait from sending the request to the first generated token.
    pub ttft_ms: u64,
    /// Whether `output_tokens` is the endpoint's exact count.
    pub exact: bool,
    /// The call finished: this is its last reading.
    pub done: bool,
}

impl Throughput {
    /// Output tokens per second of decoding, once the window is long enough
    /// to mean something.
    pub fn tokens_per_sec(&self) -> Option<f64> {
        let window = Duration::from_millis(self.decode_ms);
        (window >= MIN_RATE_WINDOW && self.output_tokens > 0)
            .then(|| self.output_tokens as f64 / window.as_secs_f64())
    }
}

/// Times one model call; see the module docs.
#[derive(Debug)]
pub struct ThroughputMeter {
    model: String,
    chars_per_token: f64,
    sent: Instant,
    first: Option<Instant>,
    last: Option<Instant>,
    chars: usize,
    last_emit: Option<Instant>,
}

impl ThroughputMeter {
    /// Start timing a call to `model` whose request went out at `sent`,
    /// estimating live counts at `chars_per_token`.
    pub fn start(model: &str, chars_per_token: f64, sent: Instant) -> Self {
        Self {
            model: model.to_string(),
            chars_per_token,
            sent,
            first: None,
            last: None,
            chars: 0,
            last_emit: None,
        }
    }

    /// Count `generated` (any streamed output) arriving at `now`. Returns a
    /// reading when one is due.
    pub fn observe(&mut self, generated: &str, now: Instant) -> Option<Throughput> {
        if generated.is_empty() {
            return None;
        }
        self.first.get_or_insert(now);
        self.last = Some(now);
        self.chars += generated.chars().count();
        let due = self
            .last_emit
            .is_none_or(|at| now.saturating_duration_since(at) >= EMIT_EVERY);
        if !due {
            return None;
        }
        self.last_emit = Some(now);
        Some(self.reading(self.estimate(), false, false))
    }

    /// Characters generated so far, for learning the model's ratio.
    pub fn generated_chars(&self) -> usize {
        self.chars
    }

    fn estimate(&self) -> usize {
        (self.chars as f64 / self.chars_per_token).round() as usize
    }

    /// The call's final reading: the endpoint's completion count when it
    /// reported one (it includes reasoning), else the streamed estimate.
    /// `None` when nothing was generated, e.g. a stopped call.
    pub fn finish(&self, reported_completion_tokens: Option<usize>) -> Option<Throughput> {
        self.first?;
        let (tokens, exact) = match reported_completion_tokens.filter(|t| *t > 0) {
            Some(tokens) => (tokens, true),
            None => (self.estimate(), false),
        };
        Some(self.reading(tokens, exact, true))
    }

    fn reading(&self, output_tokens: usize, exact: bool, done: bool) -> Throughput {
        let ms = |d: Duration| d.as_millis() as u64;
        let (decode_ms, ttft_ms) = match (self.first, self.last) {
            (Some(first), Some(last)) => (
                ms(last.saturating_duration_since(first)),
                ms(first.saturating_duration_since(self.sent)),
            ),
            _ => (0, 0),
        };
        Throughput {
            model: self.model.clone(),
            output_tokens,
            decode_ms,
            ttft_ms,
            exact,
            done,
        }
    }
}

/// Characters per token for live estimates of `model`: what its last exact
/// count measured, else the generic ratio. `learned` is the agent's memory
/// of `(model, ratio)`; another model's ratio says nothing about this one.
pub fn learned_chars_per_token(learned: Option<&(String, f64)>, model: &str) -> f64 {
    learned
        .filter(|(m, _)| m == model)
        .map_or(budget::CHARS_PER_TOKEN as f64, |(_, ratio)| *ratio)
}

/// The ratio a finished call measured, when it says enough: an exact count
/// over a reply long enough that a few odd tokens can't skew it.
pub fn measured_chars_per_token(reading: &Throughput, generated_chars: usize) -> Option<f64> {
    const MIN_TOKENS: usize = 32;
    (reading.exact && reading.output_tokens >= MIN_TOKENS && generated_chars > 0)
        .then(|| generated_chars as f64 / reading.output_tokens as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    #[test]
    fn readings_are_throttled_and_time_decoding_from_the_first_token() {
        let t0 = Instant::now();
        let mut meter = ThroughputMeter::start("m", 4.0, t0);
        // First token after an 800ms wait: a reading at once, no rate yet.
        let first = meter.observe("abcd", at(t0, 800)).expect("first reading");
        assert_eq!(first.ttft_ms, 800);
        assert_eq!(first.decode_ms, 0);
        assert_eq!(first.tokens_per_sec(), None);
        // Inside the throttle window: counted, not reported.
        assert!(meter.observe("abcd", at(t0, 900)).is_none());
        // 40 chars ≈ 10 tokens over the first 500ms of decoding.
        let r = meter.observe(&"x".repeat(32), at(t0, 1300)).expect("due");
        assert_eq!(r.output_tokens, 10);
        assert_eq!(r.decode_ms, 500);
        assert_eq!(r.tokens_per_sec(), Some(20.0));
        assert!(!r.done && !r.exact);
    }

    #[test]
    fn the_final_reading_prefers_the_endpoint_count() {
        let t0 = Instant::now();
        let mut meter = ThroughputMeter::start("m", 4.0, t0);
        meter.observe("abcd", at(t0, 100));
        meter.observe("abcd", at(t0, 1100));
        let exact = meter.finish(Some(50)).expect("generated");
        assert_eq!(
            (exact.output_tokens, exact.exact, exact.done),
            (50, true, true)
        );
        assert_eq!(exact.tokens_per_sec(), Some(50.0));
        // No (or a zero) count falls back to the streamed estimate.
        let estimated = meter.finish(Some(0)).expect("generated");
        assert_eq!((estimated.output_tokens, estimated.exact), (2, false));
    }

    #[test]
    fn live_estimates_use_the_learned_ratio_for_that_model_only() {
        let learned = ("claude".to_string(), 2.8);
        assert_eq!(learned_chars_per_token(Some(&learned), "claude"), 2.8);
        assert_eq!(learned_chars_per_token(Some(&learned), "gemma"), 4.0);
        assert_eq!(learned_chars_per_token(None, "claude"), 4.0);

        let t0 = Instant::now();
        let mut meter = ThroughputMeter::start("claude", 2.8, t0);
        meter.observe(&"x".repeat(28), at(t0, 100));
        let r = meter.observe(&"x".repeat(28), at(t0, 600)).expect("due");
        assert_eq!(r.output_tokens, 20);
        // A finished call teaches the ratio only from a real count.
        let done = meter.finish(Some(40)).expect("generated");
        assert_eq!(
            measured_chars_per_token(&done, meter.generated_chars()),
            Some(1.4)
        );
        let estimated = meter.finish(None).expect("generated");
        assert_eq!(
            measured_chars_per_token(&estimated, meter.generated_chars()),
            None
        );
    }

    #[test]
    fn nothing_generated_means_no_reading() {
        let t0 = Instant::now();
        let mut meter = ThroughputMeter::start("m", 4.0, t0);
        assert!(meter.observe("", at(t0, 10)).is_none());
        assert!(meter.finish(Some(12)).is_none());
    }

    #[test]
    fn a_burst_too_short_to_time_reports_no_rate() {
        let t0 = Instant::now();
        let mut meter = ThroughputMeter::start("m", 4.0, t0);
        meter.observe("abcd", at(t0, 300));
        meter.observe("abcd", at(t0, 400));
        let r = meter.finish(Some(8)).expect("generated");
        assert_eq!(r.decode_ms, 100);
        assert_eq!(r.tokens_per_sec(), None);
    }
}
