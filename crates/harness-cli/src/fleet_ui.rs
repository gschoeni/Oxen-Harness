//! Fleet rendering for the terminal: live lanes for N parallel subagents, with
//! the ability to focus one and watch its stream.
//!
//! One shared state, three displays:
//!
//! - [`FleetState`] (behind the [`FleetHub`]) — the lanes: label, status, a
//!   one-line activity readout, a rolling output tail, and token spend. Fed by
//!   whoever runs the fleet (the review pipeline, the `spawn_agents` sink).
//! - [`BlockPainter`] — the cooked-mode display (e.g. `/code-review`): a
//!   background thread repaints the block in place, and owns the keyboard in
//!   raw mode so **1-9 focus a lane** (showing its live output tail), **esc**
//!   returns to the overview, and **ctrl-c** stops the fleet:
//!
//! ```text
//!   ⠧ diff-scan     Reading the trail guide… src/parser.rs   12.3k tok · 8s
//!   ⠇ removed-code  scanning the deleted validation branch…   8.1k tok · 8s
//!   ✓ callers       4 candidates                             15.0k tok · 6s
//!   ── watching diff-scan ──────────────────────────────────────────────
//!     …the enclosing function re-checks the bounds, but the early return
//!     on line 84 skips it when the cache is cold…
//!   1-3 watch a lane · esc overview · ctrl-c stop
//! ```
//!
//! - [`pinned_lines`] — the same block, composed for the live composer's
//!   pinned area (which owns the terminal during interactive turns); there the
//!   composer's key loop drives focus with **alt+1-9 / alt+0**.
//!
//! On terminals without color/animation everything degrades to plain
//! milestone lines printed by the state's owner.

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use harness_agent::fleet::FleetEvent;
use harness_agent::AgentEvent;
use tokio_util::sync::CancellationToken;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::theme::{paint, Rgb, Ui};
use crate::turn::human_tokens;

/// How often the cooked-mode painter repaints (and polls keys).
const TICK: Duration = Duration::from_millis(100);

/// Longest one-line activity readout kept per lane.
const ACTIVITY_TAIL: usize = 120;

/// Longest rolling output tail kept per lane (chars) — enough for the focused
/// pane without hoarding a whole transcript.
const OUTPUT_TAIL: usize = 4_000;

/// Rows of the focused lane's tail shown under the lanes block.
const FOCUS_ROWS: usize = 8;

/// Where one lane's subagent is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LaneStatus {
    /// Waiting on a concurrency slot.
    Queued,
    Running,
    Done,
    Failed,
}

/// One subagent's live display state.
pub(crate) struct LaneState {
    pub(crate) label: String,
    pub(crate) status: LaneStatus,
    /// One-line rolling readout (tool verb + target, or the last words).
    pub(crate) activity: String,
    /// Rolling tail of everything the lane streamed (text + tool lines), for
    /// the focused pane.
    pub(crate) tail: String,
    pub(crate) tokens: usize,
    started: Option<Instant>,
    /// Frozen wall-clock duration once the lane finishes.
    elapsed: Option<Duration>,
}

impl LaneState {
    fn new(label: &str) -> Self {
        Self {
            label: label.to_string(),
            status: LaneStatus::Queued,
            activity: String::new(),
            tail: String::new(),
            tokens: 0,
            started: None,
            elapsed: None,
        }
    }

    /// The lane's elapsed display: live while running, frozen once done.
    fn clock(&self, now: Instant) -> Option<Duration> {
        self.elapsed
            .or_else(|| self.started.map(|s| now.duration_since(s)))
    }
}

/// The lanes of one running fleet, plus which lane the user is watching and
/// the token that stops the fleet.
pub(crate) struct FleetState {
    pub(crate) lanes: Vec<LaneState>,
    pub(crate) focused: Option<usize>,
    pub(crate) cancel: Option<CancellationToken>,
}

impl FleetState {
    pub(crate) fn new(labels: &[String], cancel: Option<CancellationToken>) -> Self {
        Self {
            lanes: labels.iter().map(|l| LaneState::new(l)).collect(),
            focused: None,
            cancel,
        }
    }

    pub(crate) fn lane_started(&mut self, index: usize) {
        if let Some(lane) = self.lanes.get_mut(index) {
            lane.status = LaneStatus::Running;
            lane.started = Some(Instant::now());
        }
    }

    /// Fold one lane's agent event in. `ui` resolves themed tool verbs.
    pub(crate) fn lane_event(&mut self, index: usize, event: &AgentEvent, ui: &Ui) {
        let Some(lane) = self.lanes.get_mut(index) else {
            return;
        };
        match event {
            AgentEvent::Token(t) => {
                // Activity is a one-line, whitespace-collapsed rolling readout
                // (cap ~120, so the collapse is trivial); the tail is the full
                // multi-line stream (cap ~4000), appended in place so a long
                // stream doesn't recopy the whole buffer on every token.
                lane.activity = roll_tail(&lane.activity, t, ACTIVITY_TAIL);
                harness_core::text::push_capped(&mut lane.tail, t, OUTPUT_TAIL);
            }
            AgentEvent::ToolStart { name, arguments } => {
                let verbs = ui.tool_verbs(name);
                let verb = verbs.first().map(String::as_str).unwrap_or("Working");
                let target = crate::live::tool_target(arguments).unwrap_or_default();
                let line = format!("{verb}… {target}");
                let line = line.trim_end();
                lane.activity = line.to_string();
                harness_core::text::push_capped(
                    &mut lane.tail,
                    &format!("\n◆ {line}\n"),
                    OUTPUT_TAIL,
                );
            }
            AgentEvent::Retrying {
                attempt,
                max_attempts,
                ..
            } => {
                lane.activity = format!("retrying after a hiccup ({attempt}/{max_attempts})…");
            }
            AgentEvent::Usage { tokens_used, .. } => lane.tokens = *tokens_used,
            _ => {}
        }
    }

    pub(crate) fn lane_completed(&mut self, index: usize, ok: bool, tokens: usize, summary: &str) {
        if let Some(lane) = self.lanes.get_mut(index) {
            lane.status = if ok {
                LaneStatus::Done
            } else {
                LaneStatus::Failed
            };
            lane.elapsed = lane.started.map(|s| s.elapsed());
            lane.tokens = tokens;
            lane.activity = summary.to_string();
        }
    }

    /// Focus lane `index` (out of range clears), or `None` for the overview.
    pub(crate) fn focus(&mut self, index: Option<usize>) {
        self.focused = index.filter(|i| *i < self.lanes.len());
    }

    /// Cycle focus: overview → lane 0 → 1 → … → overview.
    pub(crate) fn focus_next(&mut self) {
        self.focused = match self.focused {
            None => Some(0).filter(|_| !self.lanes.is_empty()),
            Some(i) if i + 1 < self.lanes.len() => Some(i + 1),
            Some(_) => None,
        };
    }

    /// Stop the fleet (the ctrl-c / stop-key action).
    pub(crate) fn stop(&self) {
        if let Some(cancel) = &self.cancel {
            cancel.cancel();
        }
    }

    /// Whether any lane is still running — i.e. whether the block has a
    /// spinner/clock that needs to keep animating. Once every lane has
    /// settled (done/failed) or is merely queued, the block is static and the
    /// live composer can stop repainting it each tick.
    pub(crate) fn has_running_lane(&self) -> bool {
        self.lanes.iter().any(|l| l.status == LaneStatus::Running)
    }
}

/// Apply one [`FleetEvent`] to the shared hub: advance the lane's state, and in
/// a plain (non-animating) terminal print the matching milestone line. This is
/// the single place a fleet event drives the CLI display — both the
/// `spawn_agents` sink and the review fan-out step route their events through
/// it, so the two surfaces can't drift on lane bookkeeping. `plain` is passed
/// in (rather than read from `ui`) so the caller decides once per fleet.
pub(crate) fn apply_fleet_event(hub: &FleetHub, ui: &Ui, plain: bool, event: &FleetEvent) {
    match event {
        FleetEvent::TaskStarted { index, label } => {
            if let Some(state) = hub.lock().as_mut() {
                state.lane_started(*index);
            }
            if plain {
                print_lane_started(ui, label);
            }
        }
        FleetEvent::Agent { index, event } => {
            if let Some(state) = hub.lock().as_mut() {
                state.lane_event(*index, event.as_ref(), ui);
            }
        }
        FleetEvent::TaskCompleted {
            index,
            label,
            ok,
            tokens_used,
            summary,
        } => {
            if let Some(state) = hub.lock().as_mut() {
                state.lane_completed(*index, *ok, *tokens_used, summary);
            }
            if plain {
                print_lane_completed(ui, label, *ok, *tokens_used, summary);
            }
        }
    }
}

/// The process-wide slot the `spawn_agents` sink publishes into, shared with
/// whichever display is active. `live` is set while the live composer owns the
/// terminal — the sink must then *not* start its own painter (the composer
/// paints the block in its pinned area instead).
#[derive(Default)]
pub(crate) struct FleetHub {
    state: StdMutex<Option<FleetState>>,
    live: AtomicBool,
}

impl FleetHub {
    /// The process-wide hub the `spawn_agents` sink and the live composer
    /// share. (The review pipeline uses its own local hub — its painter and
    /// its state have the same owner, so nothing global is needed there.)
    pub(crate) fn global() -> Arc<FleetHub> {
        static HUB: std::sync::OnceLock<Arc<FleetHub>> = std::sync::OnceLock::new();
        HUB.get_or_init(|| Arc::new(FleetHub::default())).clone()
    }

    pub(crate) fn install(&self, state: FleetState) {
        *self.state.lock().expect("fleet hub poisoned") = Some(state);
    }

    pub(crate) fn clear(&self) {
        *self.state.lock().expect("fleet hub poisoned") = None;
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, Option<FleetState>> {
        self.state.lock().expect("fleet hub poisoned")
    }

    /// Mark the live composer as owning the terminal for as long as the
    /// returned guard lives. While it's held, the `spawn_agents` sink paints
    /// through the composer's pinned block instead of starting its own
    /// painter; on drop — including any early return or unwind out of the
    /// composer loop — the flag clears, so a cooked-mode fleet can never be
    /// left un-painted by a missed reset.
    pub(crate) fn mark_live(self: &Arc<Self>) -> LiveGuard {
        self.live.store(true, Ordering::Relaxed);
        LiveGuard(self.clone())
    }

    pub(crate) fn is_live(&self) -> bool {
        self.live.load(Ordering::Relaxed)
    }
}

/// Clears [`FleetHub`]'s live flag on drop (see [`FleetHub::mark_live`]).
pub(crate) struct LiveGuard(Arc<FleetHub>);

impl Drop for LiveGuard {
    fn drop(&mut self) {
        self.0.live.store(false, Ordering::Relaxed);
    }
}

/// The colors + glyphs a painter needs, captured up front so painting threads
/// never borrow the `Ui` (mirrors the spinner's `SpinnerStyle`).
pub(crate) struct FleetStyle {
    glyphs: Vec<String>,
    accent_rgb: Rgb,
    text_rgb: Rgb,
    dim_rgb: Rgb,
    good_rgb: Rgb,
    bad_rgb: Rgb,
}

impl FleetStyle {
    /// `None` when color/animation is disabled (pipes, `NO_COLOR`, dumb terms).
    pub(crate) fn for_ui(ui: &Ui) -> Option<FleetStyle> {
        if !ui.animates() {
            return None;
        }
        let pal = &ui.theme().palette;
        let mut glyphs = ui.theme().voice.spinner_glyphs.clone();
        if glyphs.is_empty() {
            glyphs.push("✶".to_string());
        }
        Some(FleetStyle {
            glyphs,
            accent_rgb: pal.title.rgb(),
            text_rgb: pal.text.rgb(),
            dim_rgb: pal.muted.rgb(),
            good_rgb: pal.primary.rgb(),
            bad_rgb: pal.danger.rgb(),
        })
    }
}

// --- composing the block (shared by both displays) --------------------------

/// Compose the full fleet block: one line per lane, then (when a lane is
/// focused) a rule + its output tail, then the key hint. `width` bounds every
/// line; `frame` advances the spinner glyphs.
pub(crate) fn block_lines(
    state: &FleetState,
    style: &FleetStyle,
    width: usize,
    frame: usize,
    hint_keys: HintKeys,
) -> Vec<String> {
    let width = width.max(40);
    let label_width = state
        .lanes
        .iter()
        .map(|l| disp_width(&l.label))
        .max()
        .unwrap_or(0);
    let now = Instant::now();
    let mut out: Vec<String> = state
        .lanes
        .iter()
        .map(|lane| lane_line(lane, style, label_width, width, frame, now))
        .collect();

    if let Some(focused) = state.focused.and_then(|i| state.lanes.get(i)) {
        let title = format!("── watching {} ", focused.label);
        let fill = width.saturating_sub(title.chars().count() + 2);
        out.push(format!(
            "  {}",
            paint(&format!("{title}{}", "─".repeat(fill)), style.dim_rgb),
        ));
        for row in tail_rows(&focused.tail, width.saturating_sub(6), FOCUS_ROWS) {
            out.push(format!("    {}", paint(&row, style.text_rgb)));
        }
    }

    out.push(format!(
        "  {}",
        paint(&hint_line(state, hint_keys), style.dim_rgb)
    ));
    out
}

/// Which key vocabulary the hint advertises: plain digits (the cooked-mode
/// painter owns the keyboard) or alt-digits (the live composer shares it with
/// typing).
#[derive(Clone, Copy)]
pub(crate) enum HintKeys {
    Plain,
    Alt,
}

fn hint_line(state: &FleetState, keys: HintKeys) -> String {
    let n = state.lanes.len().min(9);
    let (digits, esc) = match keys {
        HintKeys::Plain => (format!("1-{n}"), "esc".to_string()),
        HintKeys::Alt => (format!("alt+1-{n}"), "alt+0".to_string()),
    };
    match (state.focused, keys) {
        (Some(_), _) => format!("{digits} switch lanes · {esc} overview · ctrl-c stop"),
        (None, HintKeys::Plain) => format!("{digits} watch a lane · ctrl-c stop"),
        (None, HintKeys::Alt) => format!("{digits} watch a lane"),
    }
}

/// Compose one painted lane line, fitted to `width` columns.
fn lane_line(
    lane: &LaneState,
    style: &FleetStyle,
    label_width: usize,
    width: usize,
    frame: usize,
    now: Instant,
) -> String {
    let (glyph, glyph_rgb) = match lane.status {
        LaneStatus::Queued => ("◌".to_string(), style.dim_rgb),
        LaneStatus::Running => (
            style.glyphs[frame % style.glyphs.len()].clone(),
            style.accent_rgb,
        ),
        LaneStatus::Done => ("✓".to_string(), style.good_rgb),
        LaneStatus::Failed => ("✗".to_string(), style.bad_rgb),
    };
    let meta = lane_meta(lane, now);
    let cells = lane_cells(lane, label_width, width, &meta);
    format!(
        "  {} {}  {}  {}",
        paint(&glyph, glyph_rgb),
        paint(&cells.label, style.text_rgb),
        paint(&cells.activity, style.dim_rgb),
        paint(&cells.meta, style.dim_rgb),
    )
}

/// A lane's right-hand readout: `12.3k tok · 8s` (whichever parts exist).
fn lane_meta(lane: &LaneState, now: Instant) -> String {
    let mut parts = Vec::new();
    if lane.tokens > 0 {
        parts.push(format!("{} tok", human_tokens(lane.tokens)));
    }
    if let Some(clock) = lane.clock(now) {
        parts.push(format!("{}s", clock.as_secs()));
    }
    parts.join(" · ")
}

/// The plain-text cells of a lane line, fitted so the painted line spans
/// exactly the terminal width.
struct LaneCells {
    label: String,
    activity: String,
    meta: String,
}

fn lane_cells(lane: &LaneState, label_width: usize, width: usize, meta: &str) -> LaneCells {
    // Fixed layout, in display columns: 2 indent + 1 glyph + 1 gap, label
    // field, 2 gap, activity, 2 gap, meta. Whatever is left belongs to activity.
    let fixed = 2 + 1 + 1 + label_width + 2 + 2 + disp_width(meta);
    let avail = width.saturating_sub(fixed + 1).max(8);
    LaneCells {
        label: pad(&lane.label, label_width),
        activity: fit(&lane.activity, avail),
        meta: meta.to_string(),
    }
}

/// The last `rows` display rows of a lane's output tail, wrapped to `width`
/// columns (splitting on display width so wide characters don't overflow).
fn tail_rows(tail: &str, width: usize, rows: usize) -> Vec<String> {
    let width = width.max(16);
    let mut wrapped: Vec<String> = Vec::new();
    for line in tail.lines() {
        if line.is_empty() {
            continue;
        }
        // Break the logical line into chunks of ≤ `width` columns.
        let mut chunk = String::new();
        let mut used = 0;
        for ch in line.chars() {
            let w = UnicodeWidthChar::width(ch).unwrap_or(0);
            if used + w > width && !chunk.is_empty() {
                wrapped.push(std::mem::take(&mut chunk));
                used = 0;
            }
            chunk.push(ch);
            used += w;
        }
        if !chunk.is_empty() {
            wrapped.push(chunk);
        }
    }
    if wrapped.len() > rows {
        wrapped.split_off(wrapped.len() - rows)
    } else {
        wrapped
    }
}

/// Append streamed text to a one-line rolling readout: whitespace collapsed,
/// only the freshest `cap` characters kept. (The tail buffer, by contrast, is
/// grown in place with [`harness_core::text::push_capped`].)
fn roll_tail(current: &str, addition: &str, cap: usize) -> String {
    let joined = format!("{current}{addition}");
    harness_core::text::tail_chars(&harness_core::text::collapse_ws(&joined), cap)
}

/// The terminal display width of `s` in columns — CJK ideographs and most
/// emoji are two columns, combining marks zero. All lane geometry measures in
/// columns, not characters, so a lane line's painted width matches what the
/// terminal actually advances the cursor by; measuring in `char`s would
/// under-count wide content, let the line overflow and wrap, and then the
/// painter's move-up-by-line-count would smear the block.
fn disp_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Truncate `s` to at most `width` display columns, keeping whole characters
/// (never splitting a wide char across the boundary).
fn truncate_to_width(s: &str, width: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for ch in s.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > width {
            break;
        }
        out.push(ch);
        used += w;
    }
    out
}

/// Truncate (with a trailing `…`) or right-pad `s` to exactly `width` display
/// columns.
fn fit(s: &str, width: usize) -> String {
    let flat = s.replace('\n', " ");
    if disp_width(&flat) > width {
        // Leave one column for the ellipsis marker.
        format!("{}…", truncate_to_width(&flat, width.saturating_sub(1)))
    } else {
        pad(&flat, width)
    }
}

/// Right-pad `s` with spaces to `width` display columns (a no-op when it is
/// already at least that wide).
fn pad(s: &str, width: usize) -> String {
    let mut out = s.to_string();
    for _ in disp_width(s)..width {
        out.push(' ');
    }
    out
}

// --- the cooked-mode painter -------------------------------------------------

/// Paints a [`FleetHub`]'s state in place from a background thread, owning the
/// keyboard (raw mode) so 1-9 focus a lane, tab cycles, esc returns to the
/// overview, and ctrl-c stops the fleet. Used where the CLI is in cooked mode
/// (`/code-review`, classic-terminal turns); the live composer paints the same
/// block itself via [`pinned_lines`].
pub(crate) struct BlockPainter {
    stop: Option<(Arc<AtomicBool>, thread::JoinHandle<()>)>,
}

impl BlockPainter {
    /// Start painting (no-op painter when animation is off — the state's owner
    /// prints milestones instead).
    pub(crate) fn start(ui: &Ui, hub: Arc<FleetHub>) -> Self {
        let Some(style) = FleetStyle::for_ui(ui) else {
            return Self { stop: None };
        };
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let handle = thread::spawn(move || run_painter(&stop_thread, &hub, &style));
        Self {
            stop: Some((stop, handle)),
        }
    }

    /// Stop painting, leaving the final block on screen.
    pub(crate) fn finish(mut self) {
        self.stop_now();
    }

    fn stop_now(&mut self) {
        if let Some((stop, handle)) = self.stop.take() {
            stop.store(true, Ordering::Relaxed);
            let _ = handle.join();
        }
    }
}

impl Drop for BlockPainter {
    /// A dropped painter (an abandoned turn) must still restore the terminal.
    fn drop(&mut self) {
        self.stop_now();
    }
}

/// Restores the terminal even if the painter thread panics mid-paint.
struct RawModeGuard;

impl RawModeGuard {
    fn enable() -> Option<RawModeGuard> {
        crossterm::terminal::enable_raw_mode().ok()?;
        Some(RawModeGuard)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

fn run_painter(stop: &AtomicBool, hub: &FleetHub, style: &FleetStyle) {
    let mut out = io::stdout();
    let _ = write!(out, "\x1b[?25l"); // hide cursor
    let _raw = RawModeGuard::enable();
    let mut drawn = 0usize;
    let mut frame = 0usize;
    // The last frame actually written, so we can skip a repaint when nothing
    // changed. A running lane's spinner glyph and elapsed clock vary by frame,
    // so it keeps animating; a block of settled (done/queued) lanes composes
    // identical lines and we go quiet instead of rewriting the terminal 10×/s.
    let mut last: Vec<String> = Vec::new();

    loop {
        let stopping = stop.load(Ordering::Relaxed);
        poll_keys(hub);
        // Compose under the lock, then release it before touching stdout — a
        // slow terminal flush must not stall the fleet's event ingestion,
        // which contends on this same lock per token.
        let width = crossterm::terminal::size()
            .map(|(w, _)| w as usize)
            .unwrap_or(100);
        let lines = {
            let state = hub.lock();
            state
                .as_ref()
                .map(|s| block_lines(s, style, width, frame, HintKeys::Plain))
        };
        if let Some(lines) = lines {
            if lines != last {
                repaint(&mut out, &lines, &mut drawn);
                last = lines;
            }
        }
        if stopping {
            break;
        }
        thread::sleep(TICK);
        frame += 1;
    }

    let _ = write!(out, "\x1b[?25h"); // show cursor; the block stays printed
    let _ = out.flush();
}

/// Drain pending keys: digits focus lanes, tab cycles, esc/0 the overview,
/// ctrl-c stops the fleet.
fn poll_keys(hub: &FleetHub) {
    use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
    while crossterm::event::poll(Duration::ZERO).unwrap_or(false) {
        let Ok(Event::Key(key)) = crossterm::event::read() else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        let mut state = hub.lock();
        let Some(state) = state.as_mut() else {
            continue;
        };
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => state.stop(),
            KeyCode::Char(c @ '1'..='9') => {
                state.focus(Some(c as usize - '1' as usize));
            }
            KeyCode::Char('0') | KeyCode::Esc => state.focus(None),
            KeyCode::Tab => state.focus_next(),
            _ => {}
        }
    }
}

/// Repaint the block in place: move to its top, redraw each line (clearing to
/// end-of-line), blank any rows the block shrank away from, and leave the
/// cursor just under the block.
fn repaint(out: &mut impl Write, lines: &[String], drawn: &mut usize) {
    if *drawn > 0 {
        let _ = write!(out, "\x1b[{}A\r", *drawn);
    }
    for line in lines {
        let _ = write!(out, "\x1b[2K{line}\r\n");
    }
    if lines.len() < *drawn {
        let extra = *drawn - lines.len();
        for _ in 0..extra {
            let _ = write!(out, "\x1b[2K\r\n");
        }
        let _ = write!(out, "\x1b[{extra}A\r");
    }
    *drawn = lines.len();
    let _ = out.flush();
}

/// Plain-terminal milestones (pipes, `NO_COLOR`): a lane's lifecycle as simple
/// scrollback lines. Shared by every fleet owner that can't animate — the
/// `spawn_agents` sink and the review display print identical lines.
pub(crate) fn print_lane_started(ui: &Ui, label: &str) {
    println!(
        "  {} {}",
        ui.green("◆"),
        ui.dim(&format!("{label} setting out…")),
    );
}

/// The matching completion line: `└─ label done — summary (12.3k tok)`.
pub(crate) fn print_lane_completed(ui: &Ui, label: &str, ok: bool, tokens: usize, summary: &str) {
    let outcome = if ok {
        ui.green(&format!("{label} done"))
    } else {
        ui.red(&format!("{label} failed"))
    };
    println!(
        "  {} {} {}",
        ui.brown("└─"),
        outcome,
        ui.dim(&format!(
            "— {} ({} tok)",
            crate::render::truncate(summary, 90),
            human_tokens(tokens)
        )),
    );
}

/// The fleet block composed for the live composer's pinned area (which paints
/// and drives keys itself — alt+digits — so this is just the lines).
pub(crate) fn pinned_lines(ui: &Ui, state: &FleetState, width: usize, frame: usize) -> Vec<String> {
    match FleetStyle::for_ui(ui) {
        Some(style) => block_lines(state, &style, width, frame, HintKeys::Alt),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(labels: &[&str]) -> FleetState {
        FleetState::new(
            &labels.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            None,
        )
    }

    fn style() -> FleetStyle {
        FleetStyle {
            glyphs: vec!["⠋".into(), "⠙".into()],
            accent_rgb: (1, 1, 1),
            text_rgb: (2, 2, 2),
            dim_rgb: (3, 3, 3),
            good_rgb: (4, 4, 4),
            bad_rgb: (5, 5, 5),
        }
    }

    /// Strip ANSI color escapes so tests can assert on visible text.
    fn plain(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for e in chars.by_ref() {
                    if e == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn events_drive_activity_tail_and_tokens() {
        let ui = Ui::with(false, std::sync::Arc::new(harness_theme::Theme::default()));
        let mut s = state(&["scan", "trace"]);
        s.lane_started(0);
        s.lane_event(0, &AgentEvent::Token("hello there".into()), &ui);
        s.lane_event(
            0,
            &AgentEvent::Usage {
                tokens_used: 1234,
                context_tokens: 0,
            },
            &ui,
        );
        assert_eq!(s.lanes[0].status, LaneStatus::Running);
        assert_eq!(s.lanes[0].activity, "hello there");
        assert!(s.lanes[0].tail.contains("hello there"));
        assert_eq!(s.lanes[0].tokens, 1234);

        s.lane_completed(0, true, 2000, "3 candidates");
        assert_eq!(s.lanes[0].status, LaneStatus::Done);
        assert_eq!(s.lanes[0].activity, "3 candidates");
        assert_eq!(s.lanes[0].tokens, 2000);
    }

    #[test]
    fn focus_clamps_cycles_and_clears() {
        let mut s = state(&["a", "b"]);
        s.focus(Some(1));
        assert_eq!(s.focused, Some(1));
        s.focus(Some(5)); // out of range clears
        assert_eq!(s.focused, None);
        s.focus_next();
        assert_eq!(s.focused, Some(0));
        s.focus_next();
        assert_eq!(s.focused, Some(1));
        s.focus_next();
        assert_eq!(s.focused, None);
    }

    #[test]
    fn block_shows_lanes_hint_and_focused_tail() {
        let ui = Ui::with(false, std::sync::Arc::new(harness_theme::Theme::default()));
        let mut s = state(&["scan", "trace"]);
        s.lane_started(0);
        s.lane_event(0, &AgentEvent::Token("digging into the parser".into()), &ui);

        // Overview: one line per lane + the hint.
        let lines = block_lines(&s, &style(), 80, 0, HintKeys::Plain);
        assert_eq!(lines.len(), 3);
        assert!(plain(&lines[0]).contains("scan"));
        assert!(plain(&lines[2]).contains("1-2 watch a lane"));

        // Focused: lanes + rule + tail rows + hint, alt vocabulary for live.
        s.focus(Some(0));
        let lines = block_lines(&s, &style(), 80, 0, HintKeys::Alt);
        let text = lines
            .iter()
            .map(|l| plain(l))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("── watching scan"));
        assert!(text.contains("digging into the parser"));
        assert!(text.contains("alt+1-2 switch lanes"));
    }

    #[test]
    fn tail_rows_wrap_and_keep_only_the_freshest() {
        let tail = "first line\nsecond line that is much longer than the width\nthird";
        let rows = tail_rows(tail, 16, 3);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows.last().unwrap(), "third");

        // The in-place tail buffer stays bounded near the cap as it grows.
        let mut huge = String::new();
        for _ in 0..OUTPUT_TAIL * 3 {
            harness_core::text::push_capped(&mut huge, "x", OUTPUT_TAIL);
        }
        let n = huge.chars().count();
        assert!((OUTPUT_TAIL..=OUTPUT_TAIL * 4).contains(&n));
    }

    #[test]
    fn rolling_readout_flattens_and_caps() {
        let tail = roll_tail("reading the", " parser\nmodule", 120);
        assert_eq!(tail, "reading the parser module");
        let long = roll_tail(&"x".repeat(200), "END", 120);
        assert_eq!(long.chars().count(), 120);
        assert!(long.ends_with("END"));
    }

    #[test]
    fn lane_cells_fill_the_width_exactly() {
        let mut l = LaneState::new("diff-scan");
        l.activity = "reading src/parser.rs and tracing calls".into();
        l.tokens = 12_300;
        l.elapsed = Some(Duration::from_secs(8));
        let meta = lane_meta(&l, Instant::now());
        assert_eq!(meta, "12.3k tok · 8s");

        let cells = lane_cells(&l, 12, 80, &meta);
        assert_eq!(cells.label, "diff-scan   ");
        let total = 4 + 12 + 2 + disp_width(&cells.activity) + 2 + disp_width(&meta);
        assert_eq!(total, 79);
    }

    #[test]
    fn wide_characters_are_measured_by_display_column_not_char_count() {
        // Two CJK ideographs are 4 display columns; fit must budget by columns
        // so the painted cell never overflows the terminal and wraps.
        assert_eq!(disp_width("你好"), 4);
        // Fit an all-wide string into an odd column budget: the result must not
        // exceed it (may be one under, since a wide char can't split a column).
        let fitted = fit("你好世界", 5);
        assert!(disp_width(&fitted) <= 5, "over budget: {fitted:?}");
        assert!(fitted.ends_with('…'));

        // A lane line with CJK activity spans no more than the terminal width.
        let mut l = LaneState::new("扫描");
        l.activity = "读取源代码并追踪调用点".into();
        l.tokens = 12_300;
        l.elapsed = Some(Duration::from_secs(8));
        let cells = lane_cells(&l, disp_width("扫描"), 80, &lane_meta(&l, Instant::now()));
        assert!(disp_width(&cells.activity) <= 80);
    }
}
