//! The `⚡ tok/s` readout on the pinned meter: how fast the model is
//! generating right now, and how fast its last call ran.
//!
//! The agent reports cumulative readings per model call (see
//! [`harness_agent::throughput`]). The headline number is the call's decode
//! rate; the sparkline beside it plots the *instantaneous* rate between
//! consecutive readings, so a model that stalls or speeds up mid-reply shows
//! it, instead of an average that flattens everything out.

use std::collections::VecDeque;

use harness_agent::Throughput;

use crate::theme::Ui;

/// How many instantaneous samples the sparkline spans (~3s of streaming).
const SPARK_SAMPLES: usize = 12;

/// Eighth-block glyphs, lowest to highest.
const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// The speed state the meter renders; lives across turns so an idle prompt
/// still says how fast the last call ran.
#[derive(Debug, Default)]
pub(crate) struct SpeedGauge {
    latest: Option<Throughput>,
    /// Instantaneous tokens/sec between consecutive readings of a call.
    samples: VecDeque<f64>,
}

impl SpeedGauge {
    /// A gauge showing one finished call's reading.
    pub(crate) fn settled(reading: &Throughput) -> Self {
        Self {
            latest: Some(reading.clone()),
            samples: VecDeque::new(),
        }
    }

    /// Fold in one reading from the agent.
    pub(crate) fn record(&mut self, reading: &Throughput) {
        if let Some(rate) = self
            .latest
            .as_ref()
            .and_then(|prev| moment_rate(prev, reading))
        {
            if self.samples.len() == SPARK_SAMPLES {
                self.samples.pop_front();
            }
            self.samples.push_back(rate);
        }
        // A reading with no rate — a new call's first token, or a whole call
        // too short to time — keeps the last figure on screen rather than
        // blanking the readout (see `Throughput::supersedes`).
        if reading.supersedes(self.latest.as_ref()) {
            self.latest = Some(reading.clone());
        }
    }

    /// The meter segment as `(plain, styled)`, or `None` before any model
    /// call has generated enough to time.
    pub(crate) fn segment(&self, ui: &Ui) -> Option<(String, String)> {
        let latest = self.latest.as_ref()?;
        let rate = format_rate(latest.tokens_per_sec()?);
        if latest.done {
            // A reply that arrived in one burst is timed over the whole call;
            // say so, so it isn't read as decode speed.
            let overall = if latest.rate_is_overall() {
                " overall"
            } else {
                ""
            };
            let ttft = format!("{:.1}s to first token", latest.ttft_ms as f64 / 1000.0);
            let plain = format!("⚡ {rate} tok/s{overall} · {ttft}");
            let styled = format!(
                "{} {}{} {}",
                ui.dim("⚡"),
                ui.cream(&format!("{rate} tok/s")),
                ui.dim(overall),
                ui.dim(&format!("· {ttft}"))
            );
            return Some((plain, styled));
        }
        let spark = sparkline(self.samples.iter().copied());
        let plain = format!("⚡ {rate} tok/s {spark}").trim_end().to_string();
        let styled = format!(
            "{} {}{}",
            ui.accent("⚡"),
            ui.accent(&format!("{rate} tok/s")),
            if spark.is_empty() {
                String::new()
            } else {
                format!(" {}", ui.green(&spark))
            },
        );
        Some((plain, styled))
    }
}

/// Tokens/sec between two live readings of the same call. The final
/// reading swaps in the endpoint's exact count, so it never differences
/// cleanly against an estimate.
fn moment_rate(prev: &Throughput, next: &Throughput) -> Option<f64> {
    let same_call = !prev.done && !next.done && prev.model == next.model;
    let dt = next
        .decode_ms
        .checked_sub(prev.decode_ms)
        .filter(|dt| *dt > 0)?;
    let tokens = next.output_tokens.checked_sub(prev.output_tokens)?;
    same_call.then(|| tokens as f64 * 1000.0 / dt as f64)
}

/// A rate at the precision a glance needs: one decimal below 100, whole
/// numbers above.
fn format_rate(rate: f64) -> String {
    if rate < 100.0 {
        format!("{rate:.1}")
    } else {
        format!("{rate:.0}")
    }
}

/// Scale `samples` onto [`BARS`] against their own peak, so the shape reads
/// the same for a 9 tok/s laptop model and a 200 tok/s cloud one.
fn sparkline(samples: impl Iterator<Item = f64>) -> String {
    let samples: Vec<f64> = samples.collect();
    let peak = samples.iter().copied().fold(0.0_f64, f64::max);
    if peak <= 0.0 {
        return String::new();
    }
    samples
        .iter()
        .map(|s| {
            let level = ((s / peak) * (BARS.len() - 1) as f64).round() as usize;
            BARS[level.min(BARS.len() - 1)]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(tokens: usize, decode_ms: u64, done: bool) -> Throughput {
        Throughput {
            model: "m".into(),
            output_tokens: tokens,
            decode_ms,
            ttft_ms: 800,
            exact: done,
            done,
        }
    }

    fn plain(gauge: &SpeedGauge) -> Option<String> {
        gauge.segment(&Ui::plain()).map(|(plain, _)| plain)
    }

    #[test]
    fn nothing_shows_until_a_rate_exists() {
        let mut gauge = SpeedGauge::default();
        assert_eq!(plain(&gauge), None);
        gauge.record(&reading(1, 0, false));
        assert_eq!(plain(&gauge), None);
    }

    #[test]
    fn a_streaming_call_shows_its_rate_and_a_sparkline_of_the_moment() {
        let mut gauge = SpeedGauge::default();
        gauge.record(&reading(10, 500, false)); // 20 tok/s so far
        gauge.record(&reading(30, 1000, false)); // +20 in 0.5s → 40 now
        gauge.record(&reading(40, 1500, false)); // +10 in 0.5s → 20 now
        assert_eq!(plain(&gauge).as_deref(), Some("⚡ 26.7 tok/s █▅"));
    }

    #[test]
    fn a_finished_call_settles_on_its_exact_rate_and_first_token_wait() {
        let mut gauge = SpeedGauge::default();
        gauge.record(&reading(30, 1000, false));
        gauge.record(&reading(446, 10_000, true));
        assert_eq!(
            plain(&gauge).as_deref(),
            Some("⚡ 44.6 tok/s · 0.8s to first token")
        );
        // The next call's first token (no rate yet) keeps the last figure up.
        gauge.record(&reading(1, 0, false));
        assert_eq!(
            plain(&gauge).as_deref(),
            Some("⚡ 44.6 tok/s · 0.8s to first token")
        );
    }

    #[test]
    fn a_call_too_short_to_time_keeps_the_last_figure_up() {
        let mut gauge = SpeedGauge::default();
        gauge.record(&reading(446, 10_000, true));
        // A quick tool call: done in 200ms, below the rate window.
        gauge.record(&reading(12, 200, true));
        assert_eq!(
            plain(&gauge).as_deref(),
            Some("⚡ 44.6 tok/s · 0.8s to first token")
        );
    }

    #[test]
    fn a_burst_reply_reads_as_an_overall_rate() {
        let mut gauge = SpeedGauge::default();
        let burst = Throughput {
            ttft_ms: 53_000,
            ..reading(106, 0, true)
        };
        gauge.record(&burst);
        assert_eq!(
            plain(&gauge).as_deref(),
            Some("⚡ 2.0 tok/s overall · 53.0s to first token")
        );
    }

    #[test]
    fn fast_rates_drop_the_decimal() {
        assert_eq!(format_rate(212.4), "212");
        assert_eq!(format_rate(8.44), "8.4");
    }
}
