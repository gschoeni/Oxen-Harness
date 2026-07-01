//! In-place animated status spinners (no-op when color is disabled).
//!
//! Two shapes share one rendering: [`Spinner`] owns a background thread that
//! updates a line in place (for cooked-mode commands), while [`LiveSpinner`] is
//! driven one frame at a time from the async composer loop (where a background
//! thread would fight the composer for the cursor).

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::almanac::{seed, xorshift};

use super::{paint, Rgb, Ui};

/// An animated status spinner running on a background thread.
///
/// Mirrors Claude Code's approach: it updates a single line in place with ANSI
/// escapes (`\r` + clear-to-end-of-line) and hides the cursor while spinning,
/// so there is no flicker and no scrollback spam.
pub struct Spinner {
    inner: Option<Inner>,
}

struct Inner {
    stop: Arc<AtomicBool>,
    handle: thread::JoinHandle<()>,
}

/// The colors + glyphs the spinner needs, captured so the background thread is
/// self-contained (doesn't borrow the `Ui`).
struct SpinnerStyle {
    glyphs: Vec<String>,
    glyph_rgb: Rgb,
    text_rgb: Rgb,
    dim_rgb: Rgb,
}

impl SpinnerStyle {
    /// Capture the colors + glyphs for a UI, or `None` when color/animation is
    /// disabled (piped output, `NO_COLOR`, `TERM=dumb`).
    fn for_ui(ui: &Ui) -> Option<SpinnerStyle> {
        if !ui.animates() {
            return None;
        }
        let pal = &ui.theme().palette;
        let mut glyphs = ui.theme().voice.spinner_glyphs.clone();
        if glyphs.is_empty() {
            glyphs.push("✶".to_string());
        }
        Some(SpinnerStyle {
            glyphs,
            glyph_rgb: pal.title.rgb(),
            text_rgb: pal.text.rgb(),
            dim_rgb: pal.muted.rgb(),
        })
    }
}

impl Spinner {
    /// Start spinning, cycling through `verbs`. Returns a no-op spinner when
    /// color/animation is disabled (e.g. piped output) or there's nothing to show.
    pub fn start(ui: &Ui, verbs: Vec<String>) -> Self {
        Self::start_with_target(ui, verbs, None)
    }

    /// Like [`Spinner::start`] but pins a `target` (file/command/query) after the
    /// verb, so a running tool shows *what* it's working on alongside the timer.
    pub fn start_with_target(ui: &Ui, verbs: Vec<String>, target: Option<String>) -> Self {
        let Some(style) = SpinnerStyle::for_ui(ui).filter(|_| !verbs.is_empty()) else {
            return Spinner { inner: None };
        };
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let handle =
            thread::spawn(move || run_spinner(&stop_thread, &verbs, target.as_deref(), &style));
        Spinner {
            inner: Some(Inner { stop, handle }),
        }
    }

    /// Stop the spinner, clear its line, and restore the cursor.
    pub fn stop(self) {
        if let Some(inner) = self.inner {
            inner.stop.store(true, Ordering::Relaxed);
            let _ = inner.handle.join();
        }
    }
}

/// Build one rendered spinner frame: `glyph  verb… target  (elapsed)`, fully
/// painted. The `target` (a file/command/query) is shown dimmed after the verb
/// when present, so a running tool says *what* it's acting on.
fn spinner_frame(
    style: &SpinnerStyle,
    verbs: &[String],
    target: Option<&str>,
    start: Instant,
    frame: usize,
    verb_idx: usize,
) -> String {
    let glyph = &style.glyphs[frame % style.glyphs.len()];
    let verb = match target {
        Some(t) if !t.is_empty() => format!("{}… {}", verbs[verb_idx], t),
        _ => format!("{}…", verbs[verb_idx]),
    };
    format!(
        "{}  {}  {}",
        paint(glyph, style.glyph_rgb),
        paint(&verb, style.text_rgb),
        paint(&format!("({})", elapsed(start)), style.dim_rgb),
    )
}

fn run_spinner(stop: &AtomicBool, verbs: &[String], target: Option<&str>, style: &SpinnerStyle) {
    let mut out = io::stdout();
    let start = Instant::now();
    let mut s = seed();
    let mut verb_idx = (xorshift(&mut s) as usize) % verbs.len();
    let mut frame = 0usize;

    let _ = write!(out, "\x1b[?25l"); // hide cursor
    let _ = out.flush();

    while !stop.load(Ordering::Relaxed) {
        if frame > 0 && frame % 16 == 0 {
            verb_idx = (verb_idx + 1) % verbs.len();
        }
        let line = spinner_frame(style, verbs, target, start, frame, verb_idx);
        let _ = write!(out, "\r{line}\x1b[K");
        let _ = out.flush();
        frame += 1;
        thread::sleep(Duration::from_millis(110));
    }

    let _ = write!(out, "\r\x1b[K\x1b[?25h"); // clear line, show cursor
    let _ = out.flush();
}

/// A spinner driven a single frame at a time from an async loop, for the live
/// composer (where a background thread writing to stdout would fight the
/// composer for the cursor). It produces a status line on demand instead of
/// owning a thread; the caller decides when to draw and advance it.
pub(crate) struct LiveSpinner {
    style: SpinnerStyle,
    verbs: Vec<String>,
    /// An optional target (a file path, command, query…) shown after the verb so
    /// the indicator reads e.g. `Reading the trail guide… src/lib.rs (3s)`.
    target: Option<String>,
    start: Instant,
    frame: usize,
    verb_idx: usize,
}

impl LiveSpinner {
    /// A spinner cycling `verbs`, or `None` when there's nothing to animate.
    pub(crate) fn new(ui: &Ui, verbs: Vec<String>) -> Option<Self> {
        Self::with_target(ui, verbs, None)
    }

    /// Like [`LiveSpinner::new`] but pins a `target` (file/command/etc.) after the
    /// verb, so a running tool shows *what* it's working on alongside the timer.
    pub(crate) fn with_target(ui: &Ui, verbs: Vec<String>, target: Option<String>) -> Option<Self> {
        if verbs.is_empty() {
            return None;
        }
        let style = SpinnerStyle::for_ui(ui)?;
        let mut s = seed();
        let verb_idx = (xorshift(&mut s) as usize) % verbs.len();
        Some(LiveSpinner {
            style,
            verbs,
            target,
            start: Instant::now(),
            frame: 0,
            verb_idx,
        })
    }

    /// The current frame's status line (glyph + verb + target + elapsed), painted.
    pub(crate) fn line(&self) -> String {
        spinner_frame(
            &self.style,
            &self.verbs,
            self.target.as_deref(),
            self.start,
            self.frame,
            self.verb_idx,
        )
    }

    /// Advance one frame, rotating the verb on the same cadence as the thread.
    pub(crate) fn tick(&mut self) {
        self.frame += 1;
        if self.frame % 16 == 0 {
            self.verb_idx = (self.verb_idx + 1) % self.verbs.len();
        }
    }
}

/// Format an elapsed duration like `7s` or `1m07s`.
fn elapsed(start: Instant) -> String {
    let secs = start.elapsed().as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m{:02}s", secs / 60, secs % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_formats_minutes() {
        assert_eq!(elapsed(Instant::now() - Duration::from_secs(7)), "7s");
        assert_eq!(elapsed(Instant::now() - Duration::from_secs(67)), "1m07s");
    }
}
