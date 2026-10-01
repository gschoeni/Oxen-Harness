//! Backoff for every HTTP call the harness makes: the model endpoint, the
//! hub's media queue and repos, Brave search, Hugging Face, arbitrary
//! `web_fetch` pages. Deliberately tiny — `reqwest` plus two parsers — so
//! the tool crates can share it without linking the whole LLM client.
//!
//! Three things a "just double the wait" loop gets wrong, all handled here:
//!
//! - **A 429 is not a 502.** A rate limit is the server metering this key or
//!   shedding load; it clears on the server's clock, not ours. Retrying a
//!   second later mostly burns the attempt budget. Rate limits therefore get
//!   a longer floor and more attempts than a random blip.
//! - **`Retry-After` is authoritative.** When the server says how long to
//!   wait, guessing is worse than obeying: the wait is the hint exactly,
//!   never shortened. Both header forms (delay-seconds and an HTTP-date)
//!   are parsed, plus the `retry-after-ms` variant some gateways send. A
//!   hint longer than the schedule is willing to nap for ends the retries
//!   instead, so the failure reaches the user rather than a silent sleep.
//! - **Retries need jitter.** N lanes rate-limited at once and all sleeping
//!   exactly 2s hit the server together again 2s later — the thundering
//!   herd. Every *computed* wait is randomized over its upper half.
//!
//! [`Backoff`] is the schedule; [`send_with_retry`] drives a
//! [`reqwest::RequestBuilder`] through it for callers without their own
//! loop (tools, the media hub). The agent's model-call loop keeps its own
//! loop — it emits progress events and honors the turn's cancel token — but
//! draws its waits from the same [`Backoff`] math.

use std::time::{Duration, SystemTime};

use reqwest::header::{HeaderMap, HeaderValue, RETRY_AFTER};

/// Why a failed attempt is worth another try.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transient {
    /// HTTP 429: the server is metering this key or shedding load. Clears on
    /// the server's schedule, so it waits longer and gets more attempts.
    RateLimited,
    /// A provider hiccup (5xx, 408, 529) or a transport failure (connect
    /// error, timeout, dropped stream). Usually clears within seconds.
    Blip,
}

/// The retry schedule: how many attempts each kind of failure gets and how
/// long the waits between them are. `max_attempts` counts the first try.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backoff {
    /// Attempts for a [`Transient::Blip`], first try included.
    pub max_attempts: u32,
    /// First wait after a blip; doubles per attempt.
    pub base_delay: Duration,
    /// Attempts for a [`Transient::RateLimited`], first try included.
    pub rate_limit_max_attempts: u32,
    /// First wait after a rate limit; doubles per attempt.
    pub rate_limit_base_delay: Duration,
    /// The longest any computed wait may be. Past a minute, waiting longer
    /// doesn't make a provider recover sooner — and a doubling schedule a few
    /// misconfigured attempts deep would otherwise sleep for hours.
    pub max_delay: Duration,
    /// The longest server-sent `Retry-After` this schedule obeys. The server
    /// knows its own quota window better than `max_delay` does, so the model
    /// call lets it exceed the computed clamp; a tool call that blocks a turn
    /// the user is watching keeps it short. A hint past this bound stops the
    /// retries — see [`Backoff::delay_for`].
    pub max_retry_after: Duration,
}

/// A `Retry-After: 0` still means "not right now": the request just failed,
/// and re-sending it in the same millisecond only trips the limit again.
const MIN_HINTED_DELAY: Duration = Duration::from_millis(500);

impl Default for Backoff {
    /// The model-call schedule: blips get 4 tries at 1s → 2s → 4s; rate
    /// limits get 6 tries at 2s → 4s → 8s → 16s → 32s (about a minute of
    /// patience, which covers a per-minute quota window), and a server's
    /// own hint is obeyed up to two minutes.
    fn default() -> Self {
        Self {
            max_attempts: 4,
            base_delay: Duration::from_secs(1),
            rate_limit_max_attempts: 6,
            rate_limit_base_delay: Duration::from_secs(2),
            max_delay: Duration::from_secs(60),
            max_retry_after: Duration::from_secs(120),
        }
    }
}

impl Backoff {
    /// A shorter schedule for tool calls and hub lookups, where a stalled
    /// request blocks a turn the user is watching: blips get 3 tries
    /// (1s → 2s), rate limits 4 (2s → 4s → 8s), and nothing — a server hint
    /// included — waits over 20s.
    pub fn brief() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_secs(1),
            rate_limit_max_attempts: 4,
            rate_limit_base_delay: Duration::from_secs(2),
            max_delay: Duration::from_secs(20),
            max_retry_after: Duration::from_secs(20),
        }
    }

    /// A schedule with near-zero waits and no extra patience for rate
    /// limits, so retry tests run instantly and count attempts exactly. A
    /// hint is still obeyed (tests send millisecond ones), up to a second.
    pub fn instant(max_attempts: u32) -> Self {
        Self {
            max_attempts,
            base_delay: Duration::from_millis(1),
            rate_limit_max_attempts: max_attempts,
            rate_limit_base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(1),
            max_retry_after: Duration::from_secs(1),
        }
    }

    /// How many attempts (first try included) a failure of this kind gets.
    pub fn attempts_for(&self, kind: Transient) -> u32 {
        match kind {
            Transient::RateLimited => self.rate_limit_max_attempts,
            Transient::Blip => self.max_attempts,
        }
    }

    /// How long to wait after the `attempt`-th try (1-based) failed, before
    /// jitter. A server-sent `retry_after` wins outright, floored at half a
    /// second; otherwise the kind's base doubles per attempt — base, 2×base,
    /// 4×base, … — clamped to `max_delay`. `None` when the server asked for
    /// more than `max_retry_after`: the caller should stop retrying and
    /// surface the failure rather than nap past its own promise.
    pub fn delay_for(
        &self,
        attempt: u32,
        kind: Transient,
        retry_after: Option<Duration>,
    ) -> Option<Duration> {
        if let Some(hint) = retry_after {
            return (hint <= self.max_retry_after).then_some(hint.max(MIN_HINTED_DELAY));
        }
        let base = match kind {
            Transient::RateLimited => self.rate_limit_base_delay,
            Transient::Blip => self.base_delay,
        };
        Some(
            base.saturating_mul(2u32.saturating_pow(attempt.saturating_sub(1)))
                .min(self.max_delay),
        )
    }

    /// The actual wait to sleep after the `attempt`-th failure: a computed
    /// delay is `jittered` so concurrent retriers don't wake in lockstep,
    /// while a server's hint is obeyed exactly — shortening it would only
    /// re-send before the server said it may. `None` means stop retrying,
    /// as for [`Backoff::delay_for`].
    pub fn wait_after(
        &self,
        attempt: u32,
        kind: Transient,
        retry_after: Option<Duration>,
    ) -> Option<Duration> {
        let delay = self.delay_for(attempt, kind, retry_after)?;
        Some(if retry_after.is_some() {
            delay
        } else {
            jittered(delay)
        })
    }
}

/// Spread a wait over its upper half — "equal jitter": half the delay is
/// kept, the other half is drawn at random — so concurrent retriers don't
/// wake in lockstep. Never waits longer than `delay`, never less than half.
fn jittered(delay: Duration) -> Duration {
    let half = delay / 2;
    half + half.mul_f64(fastrand::f64())
}

/// Whether an HTTP status is worth retrying, and as what. 408 (the server
/// gave up waiting for us), 429, and every 5xx (including 529 overloaded)
/// qualify; auth, credits, and malformed requests don't — retrying can't
/// fix them.
pub fn transient_status(status: u16) -> Option<Transient> {
    match status {
        429 => Some(Transient::RateLimited),
        408 | 500..=599 => Some(Transient::Blip),
        _ => None,
    }
}

/// The server's wait hint from a response's headers: `Retry-After` as
/// delay-seconds or an HTTP-date (RFC 7231 §7.1.3), else `retry-after-ms`.
/// `None` when neither is present or parseable.
pub fn retry_after_from_headers(headers: &HeaderMap) -> Option<Duration> {
    let text = |v: &HeaderValue| v.to_str().ok().map(str::to_owned);
    headers
        .get(RETRY_AFTER)
        .and_then(text)
        .and_then(|v| parse_retry_after(&v, SystemTime::now()))
        .or_else(|| {
            headers
                .get("retry-after-ms")
                .and_then(text)
                .and_then(|v| v.trim().parse::<u64>().ok())
                .map(Duration::from_millis)
        })
}

/// Parse one `Retry-After` value: a non-negative integer number of seconds,
/// or an HTTP-date, whose distance from `now` is the wait (a date already
/// past is a zero wait, not an error).
pub fn parse_retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim();
    if let Ok(secs) = value.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    let when = httpdate::parse_http_date(value).ok()?;
    Some(when.duration_since(now).unwrap_or(Duration::ZERO))
}

/// Send `request`, retrying transient failures per `backoff`.
///
/// A transport error (connect failure, timeout) or a transient status (see
/// [`transient_status`]) is retried after a jittered wait — the server's
/// `Retry-After` when it sent one. Any other response is returned as-is,
/// success or not, for the caller's own status handling. A `Retry-After`
/// longer than the schedule tolerates also returns the response as-is: the
/// caller reports the rate limit instead of blocking on it.
///
/// `safe_to_repeat` says whether re-sending could duplicate a side effect. A
/// GET, or a POST the server rejected with 429 before doing anything, is safe
/// to repeat; a POST that timed out or drew a 5xx may have half-happened,
/// so with `safe_to_repeat: false` only rate limits and connect failures
/// (which never reached the server) are retried.
///
/// A request whose body can't be cloned (a streaming multipart upload) is
/// sent exactly once.
pub async fn send_with_retry(
    request: reqwest::RequestBuilder,
    backoff: &Backoff,
    safe_to_repeat: bool,
) -> Result<reqwest::Response, reqwest::Error> {
    let mut attempt: u32 = 1;
    loop {
        let Some(this_try) = request.try_clone() else {
            return request.send().await;
        };
        let wait = match this_try.send().await {
            Ok(resp) => match transient_status(resp.status().as_u16()) {
                Some(kind) if safe_to_repeat || kind == Transient::RateLimited => {
                    let hint = retry_after_from_headers(resp.headers());
                    let wait = (attempt < backoff.attempts_for(kind))
                        .then(|| backoff.wait_after(attempt, kind, hint))
                        .flatten();
                    match wait {
                        Some(wait) => wait,
                        None => return Ok(resp),
                    }
                }
                _ => return Ok(resp),
            },
            Err(e) if is_transient_transport(&e, safe_to_repeat) => {
                let wait = (attempt < backoff.attempts_for(Transient::Blip))
                    .then(|| backoff.wait_after(attempt, Transient::Blip, None))
                    .flatten();
                match wait {
                    Some(wait) => wait,
                    None => return Err(e),
                }
            }
            Err(e) => return Err(e),
        };
        tokio::time::sleep(wait).await;
        attempt += 1;
    }
}

/// Whether a `reqwest` failure is worth re-sending for. A malformed request
/// (`is_builder`) never is. A connect failure never reached the server, so
/// it is safe even for a request with side effects; a timeout or a failure
/// mid-body may have, so those are retried only when `safe_to_repeat`.
fn is_transient_transport(e: &reqwest::Error, safe_to_repeat: bool) -> bool {
    if e.is_builder() || e.is_redirect() {
        return false;
    }
    e.is_connect() || safe_to_repeat
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limits_wait_longer_and_get_more_attempts_than_blips() {
        let b = Backoff::default();
        assert_eq!(b.attempts_for(Transient::Blip), 4);
        assert_eq!(b.attempts_for(Transient::RateLimited), 6);
        assert_eq!(
            b.delay_for(1, Transient::Blip, None),
            Some(Duration::from_secs(1))
        );
        assert_eq!(
            b.delay_for(1, Transient::RateLimited, None),
            Some(Duration::from_secs(2))
        );
        assert_eq!(
            b.delay_for(5, Transient::RateLimited, None),
            Some(Duration::from_secs(32))
        );
    }

    #[test]
    fn delay_doubles_then_clamps() {
        let b = Backoff::default();
        assert_eq!(
            b.delay_for(2, Transient::Blip, None),
            Some(Duration::from_secs(2))
        );
        assert_eq!(
            b.delay_for(3, Transient::Blip, None),
            Some(Duration::from_secs(4))
        );
        // A deep (or misconfigured) attempt count must never produce an
        // hours-long sleep — the clamp holds even where 2^n overflows.
        assert_eq!(
            b.delay_for(10, Transient::Blip, None),
            Some(Duration::from_secs(60))
        );
        assert_eq!(
            b.delay_for(u32::MAX, Transient::RateLimited, None),
            Some(Duration::from_secs(60))
        );
    }

    #[test]
    fn a_server_hint_beats_the_schedule_but_is_bounded() {
        let b = Backoff::default();
        let hint = Some(Duration::from_secs(7));
        assert_eq!(
            b.delay_for(1, Transient::RateLimited, hint),
            Some(Duration::from_secs(7))
        );
        // Obeyed past the schedule's own clamp: the server knows its window.
        assert_eq!(
            b.delay_for(1, Transient::RateLimited, Some(Duration::from_secs(90))),
            Some(Duration::from_secs(90))
        );
        // …but a nap past the bound is refused, not silently shortened: the
        // caller stops and reports the limit instead.
        assert_eq!(
            b.delay_for(1, Transient::RateLimited, Some(Duration::from_secs(3600))),
            None
        );
        // "Retry-After: 0" is still not "immediately".
        assert_eq!(
            b.delay_for(1, Transient::RateLimited, Some(Duration::ZERO)),
            Some(MIN_HINTED_DELAY)
        );
    }

    #[test]
    fn the_brief_schedule_never_naps_past_twenty_seconds_even_on_a_hint() {
        // A tool call blocks a turn the user is watching and can't be
        // cancelled mid-sleep, so a page saying "come back in 2 minutes"
        // ends the retries rather than parking the turn.
        let b = Backoff::brief();
        assert_eq!(
            b.delay_for(1, Transient::RateLimited, Some(Duration::from_secs(20))),
            Some(Duration::from_secs(20))
        );
        assert_eq!(
            b.delay_for(1, Transient::RateLimited, Some(Duration::from_secs(120))),
            None
        );
        assert_eq!(
            b.wait_after(1, Transient::RateLimited, Some(Duration::from_secs(120))),
            None
        );
    }

    #[test]
    fn a_hinted_wait_is_obeyed_exactly_while_computed_waits_are_jittered() {
        let b = Backoff::default();
        let hint = Some(Duration::from_secs(17));
        for _ in 0..50 {
            // Jittering the server's hint downward would re-send before the
            // server said it may — and burn the attempt on another 429.
            assert_eq!(
                b.wait_after(1, Transient::RateLimited, hint),
                Some(Duration::from_secs(17))
            );
            let computed = b
                .wait_after(3, Transient::RateLimited, None)
                .expect("a computed wait is always available");
            assert!(
                computed >= Duration::from_secs(4) && computed <= Duration::from_secs(8),
                "{computed:?}"
            );
        }
    }

    #[test]
    fn jitter_stays_within_the_upper_half() {
        let delay = Duration::from_secs(8);
        for _ in 0..200 {
            let j = jittered(delay);
            assert!(j >= Duration::from_secs(4) && j <= delay, "{j:?}");
        }
        assert_eq!(jittered(Duration::ZERO), Duration::ZERO);
    }

    #[test]
    fn transient_statuses_are_the_retryable_ones() {
        assert_eq!(transient_status(429), Some(Transient::RateLimited));
        assert_eq!(transient_status(408), Some(Transient::Blip));
        assert_eq!(transient_status(500), Some(Transient::Blip));
        assert_eq!(transient_status(502), Some(Transient::Blip));
        assert_eq!(transient_status(529), Some(Transient::Blip));
        assert_eq!(transient_status(400), None);
        assert_eq!(transient_status(401), None);
        assert_eq!(transient_status(402), None);
        assert_eq!(transient_status(404), None);
    }

    #[test]
    fn retry_after_parses_seconds_and_http_dates() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        assert_eq!(parse_retry_after("12", now), Some(Duration::from_secs(12)));
        assert_eq!(parse_retry_after(" 3 ", now), Some(Duration::from_secs(3)));
        let later = httpdate::fmt_http_date(now + Duration::from_secs(45));
        assert_eq!(
            parse_retry_after(&later, now),
            Some(Duration::from_secs(45))
        );
        let earlier = httpdate::fmt_http_date(now - Duration::from_secs(45));
        assert_eq!(parse_retry_after(&earlier, now), Some(Duration::ZERO));
        assert_eq!(parse_retry_after("soon", now), None);
        assert_eq!(parse_retry_after("-5", now), None);
    }

    #[test]
    fn headers_prefer_retry_after_then_the_ms_variant() {
        let mut h = HeaderMap::new();
        assert_eq!(retry_after_from_headers(&h), None);
        h.insert("retry-after-ms", HeaderValue::from_static("1500"));
        assert_eq!(
            retry_after_from_headers(&h),
            Some(Duration::from_millis(1500))
        );
        h.insert(RETRY_AFTER, HeaderValue::from_static("4"));
        assert_eq!(retry_after_from_headers(&h), Some(Duration::from_secs(4)));
        h.insert(RETRY_AFTER, HeaderValue::from_static("garbage"));
        // Unparseable Retry-After falls through to the ms variant.
        assert_eq!(
            retry_after_from_headers(&h),
            Some(Duration::from_millis(1500))
        );
    }

    #[tokio::test]
    async fn send_with_retry_obeys_retry_after_then_succeeds() {
        let mut server = mockito::Server::new_async().await;
        let limited = server
            .mock("GET", "/thing")
            .with_status(429)
            .with_header("retry-after-ms", "1")
            .with_body("slow down")
            .expect(2)
            .create_async()
            .await;
        let client = reqwest::Client::new();
        let backoff = Backoff::instant(5);
        // mockito serves mocks in creation order until each is exhausted.
        let ok = server
            .mock("GET", "/thing")
            .with_status(200)
            .with_body("fine")
            .expect(1)
            .create_async()
            .await;
        let resp = send_with_retry(
            client.get(format!("{}/thing", server.url())),
            &backoff,
            true,
        )
        .await
        .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        assert_eq!(resp.text().await.unwrap(), "fine");
        limited.assert_async().await;
        ok.assert_async().await;
    }

    #[tokio::test]
    async fn a_retry_after_past_the_schedule_returns_the_rate_limit_at_once() {
        let mut server = mockito::Server::new_async().await;
        let limited = server
            .mock("GET", "/thing")
            .with_status(429)
            .with_header("retry-after", "120")
            .with_body("come back later")
            .expect(1) // never re-sent: the schedule won't nap that long
            .create_async()
            .await;
        let client = reqwest::Client::new();
        let started = std::time::Instant::now();
        let resp = send_with_retry(
            client.get(format!("{}/thing", server.url())),
            &Backoff::instant(5),
            true,
        )
        .await
        .unwrap();
        assert_eq!(resp.status().as_u16(), 429);
        assert!(started.elapsed() < Duration::from_secs(1));
        limited.assert_async().await;
    }

    #[tokio::test]
    async fn send_with_retry_returns_the_last_response_when_attempts_run_out() {
        let mut server = mockito::Server::new_async().await;
        let down = server
            .mock("GET", "/thing")
            .with_status(503)
            .expect(3)
            .create_async()
            .await;
        let client = reqwest::Client::new();
        let resp = send_with_retry(
            client.get(format!("{}/thing", server.url())),
            &Backoff::instant(3),
            true,
        )
        .await
        .unwrap();
        assert_eq!(resp.status().as_u16(), 503);
        down.assert_async().await;
    }

    #[tokio::test]
    async fn a_post_with_side_effects_retries_only_rate_limits() {
        let mut server = mockito::Server::new_async().await;
        let flaky = server
            .mock("POST", "/create")
            .with_status(500)
            .expect(1) // never re-sent: it may have half-happened
            .create_async()
            .await;
        let client = reqwest::Client::new();
        let resp = send_with_retry(
            client.post(format!("{}/create", server.url())).body("x"),
            &Backoff::instant(3),
            false,
        )
        .await
        .unwrap();
        assert_eq!(resp.status().as_u16(), 500);
        flaky.assert_async().await;

        let limited = server
            .mock("POST", "/create-limited")
            .with_status(429)
            .expect(3) // a 429 was rejected before doing anything: safe
            .create_async()
            .await;
        let resp = send_with_retry(
            client
                .post(format!("{}/create-limited", server.url()))
                .body("x"),
            &Backoff::instant(3),
            false,
        )
        .await
        .unwrap();
        assert_eq!(resp.status().as_u16(), 429);
        limited.assert_async().await;
    }

    #[tokio::test]
    async fn non_transient_responses_come_back_immediately() {
        let mut server = mockito::Server::new_async().await;
        let denied = server
            .mock("GET", "/thing")
            .with_status(401)
            .expect(1)
            .create_async()
            .await;
        let client = reqwest::Client::new();
        let resp = send_with_retry(
            client.get(format!("{}/thing", server.url())),
            &Backoff::instant(5),
            true,
        )
        .await
        .unwrap();
        assert_eq!(resp.status().as_u16(), 401);
        denied.assert_async().await;
    }
}
