//! Driving one turn (or a queue of them) through the agent, plus the shared
//! turn-status reporting both terminals use.
//!
//! [`run_turn_and_drain`] is the single entry point the REPL and `/queue run`
//! go through: on a live TTY it hands off to the sticky composer
//! ([`crate::live`]), otherwise it drives the classic [`TurnRenderer`] path
//! and drains stacked messages after. The report helpers ([`retry_notice`],
//! [`turn_failure_lines`], [`context_usage_lines`]) live here so the classic
//! prompt and the live composer explain a turn the same way.

use std::cell::RefCell;
use std::rc::Rc;

use anyhow::Result;
use harness_agent::Agent;

use crate::queue::MessageQueue;
use crate::render::{truncate, TurnRenderer};
use crate::theme::Ui;
use crate::{attach, brave, commands, live, pricing};

/// What to drive through the agent for one turn: a fresh prompt (the normal
/// case), or a continuation of the transcript's dangling last turn — `/retry`
/// after a failure, with no user message re-appended.
#[derive(Debug, Clone)]
pub(crate) enum TurnRequest {
    Prompt(String),
    Continue,
}

/// Whether the transcript stops mid-turn — it ends on a user message (the
/// reply never arrived: a provider error, no internet, a crash) or on a tool
/// result the model never got to react to. Such a session can be continued in
/// place with `/retry` (`Agent::continue_turn`).
pub(crate) fn ends_mid_turn(messages: &[harness_llm::ChatMessage]) -> bool {
    messages
        .last()
        .is_some_and(|m| m.role == "user" || m.role == "tool")
}

/// The one-line notice for a transient model-call failure being retried with
/// backoff — shared by the classic renderer and the live composer.
pub(crate) fn retry_notice(
    attempt: u32,
    max_attempts: u32,
    delay_ms: u64,
    error: &str,
    switching_to: Option<&str>,
) -> String {
    // A model switch is the more useful headline: the attempt counter is
    // starting over on a different endpoint, so reporting it as "attempt 5 of
    // 4" would read as a bug.
    if let Some(model) = switching_to {
        return format!("{error} — {max_attempts} attempts spent, continuing on {model}");
    }
    let secs = (delay_ms as f64 / 1000.0).ceil() as u64;
    format!(
        "{error} — retrying in {secs}s (attempt {} of {max_attempts})",
        attempt + 1
    )
}

/// Whether a failed turn is worth pre-filling `/retry` for: the transcript
/// stops mid-turn (so there's a dangling turn to re-drive) and the failure
/// isn't an auth one (where `/auth` is the fix, not a retry). Both terminals
/// use this to seed the next prompt with `/retry` so a bare ⏎ tries again.
pub(crate) fn seed_retry(
    messages: &[harness_llm::ChatMessage],
    err: &harness_agent::AgentError,
) -> bool {
    ends_mid_turn(messages) && !commands::auth::is_auth_error(&err.to_string())
}

/// The failure report for a turn that died even after retries: what happened,
/// then how to pick the conversation back up — now (`/retry`, `/model`) or
/// later (`--continue` / `--resume`). Auth failures get the `/auth` hint
/// instead of the generic recovery lines; `retry_seeded` says the prompt was
/// pre-filled with `/retry`, so the hint points at ⏎ instead. Shared by the
/// classic prompt and the live composer so both terminals explain the same
/// way out.
pub(crate) fn turn_failure_lines(
    agent: &Agent,
    ui: &Ui,
    err: &harness_agent::AgentError,
    retry_seeded: bool,
) -> Vec<String> {
    let mut lines = vec![format!("  {}", ui.red(&ui.death()))];
    // Retries exhausted is the "endpoint is down" case: break the report into
    // what failed, the last error, and where — a headline the eye can't skip,
    // instead of one long dim line.
    if let harness_agent::AgentError::RetriesExhausted {
        attempts,
        model,
        endpoint,
        source,
    } = err
    {
        lines.push(format!(
            "  {}",
            ui.red(&format!(
                "The model endpoint failed {attempts} times in a row — the turn did not finish."
            )),
        ));
        lines.push(format!("  {}", ui.cream(&format!("Last error: {source}"))));
        lines.push(format!("  {}", ui.dim(&format!("({model} at {endpoint})"))));
    } else {
        lines.push(format!(
            "  {}",
            ui.dim(&format!("The trail guide says: {err}"))
        ));
    }
    if let Some(hint) = commands::auth::auth_hint(ui, &err.to_string()) {
        lines.push(hint);
        return lines;
    }
    lines.push(format!(
        "  {}",
        ui.dim("Nothing is lost — every step is saved in the trail journal.")
    ));
    lines.push(format!(
        "  {} {}",
        ui.dim("·"),
        ui.dim(if retry_seeded {
            "/retry is ready below — press ⏎ to try again · /model <name> to switch oxen first"
        } else {
            "/retry to try the turn again · /model <name> to switch oxen first"
        }),
    ));
    lines.push(format!(
        "  {} {}",
        ui.dim("·"),
        ui.dim(&format!(
            "later: oxen-harness --continue (or --resume {}), then /retry",
            agent.session_id()
        )),
    ));
    lines.push(format!(
        "  {} {}",
        ui.dim("·"),
        ui.dim("full error details: ~/.oxen-harness/errors.jsonl"),
    ));
    lines
}

/// Whether the sticky-bottom live composer should drive this turn: only for an
/// interactive, animating TTY. Pipes, tests, `NO_COLOR`, and `TERM=dumb` fall
/// back to the classic blocking prompt with unchanged behavior.
pub(crate) fn live_enabled(ui: &Ui) -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal() && ui.animates()
}

/// Run one turn (a prompt or a `/retry` continuation), then auto-drain any
/// stacked messages in order. On a live TTY this hands off to the sticky
/// composer (which lets the user keep stacking while turns run); otherwise it
/// runs the classic prompt and drains after.
/// Returns `Ok(true)` when the session should end (Ctrl-D). A Ctrl-C only
/// cancels the running turn and its drain — the caller returns to the prompt.
pub(crate) async fn run_turn_and_drain(
    agent: &mut Agent,
    request: TurnRequest,
    ui: &Ui,
    queue: &mut MessageQueue,
    carryover: &mut String,
) -> Result<bool> {
    if live_enabled(ui) {
        // The live composer hands back any half-typed next message so the idle
        // prompt can keep it instead of wiping it when the turn ends.
        let (exit, draft) = live::run_prompt(agent, request, ui, queue).await?;
        *carryover = draft;
        return Ok(exit);
    }
    if matches!(
        run_prompt(agent, &request, ui, carryover).await?,
        PromptOutcome::Interrupted
    ) {
        return Ok(false);
    }
    while !queue.is_empty() {
        let next = queue.pop_front().expect("queue is non-empty");
        println!(
            "  {} {}",
            ui.brown("▶ rolling the wagon:"),
            ui.cream(&truncate(&next, 80)),
        );
        if matches!(
            run_prompt(agent, &TurnRequest::Prompt(next), ui, carryover).await?,
            PromptOutcome::Interrupted
        ) {
            return Ok(false);
        }
    }
    Ok(false)
}

/// How one classic-prompt turn ended.
enum PromptOutcome {
    /// The turn ran to completion (success or a reported failure).
    Ran,
    /// Ctrl-C cancelled the stream — stop draining, but keep the session.
    Interrupted,
}

/// Run one turn, racing it against Ctrl-C. A Ctrl-C cancels the in-flight
/// turn and returns [`PromptOutcome::Interrupted`] so the caller stops
/// draining the queue — the session itself continues at the prompt.
///
/// `carryover` seeds the next idle prompt: a retryable failure fills it with
/// `/retry` so a bare ⏎ re-drives the dangling turn; a finished turn clears
/// any stale seed (in the classic prompt this string is ours alone — there is
/// no mid-turn composer that could hold the user's typing).
async fn run_prompt(
    agent: &mut Agent,
    request: &TurnRequest,
    ui: &Ui,
    carryover: &mut String,
) -> Result<PromptOutcome> {
    let (text, attachments) = match request {
        TurnRequest::Prompt(prompt) => {
            let (text, attachments, warnings) = attach::extract_attachments(prompt);
            for w in &warnings {
                println!("  {} {}", ui.red("⚠"), ui.dim(w));
            }
            if !attachments.is_empty() {
                let names: Vec<&str> = attachments.iter().map(|a| a.filename.as_str()).collect();
                println!(
                    "  {} {}",
                    ui.green("📎 attached:"),
                    ui.cream(&names.join(", "))
                );
            }
            (text, attachments)
        }
        // A retry re-drives the transcript as-is; there is no new message.
        TurnRequest::Continue => (String::new(), Vec::new()),
    };

    let renderer = Rc::new(RefCell::new(TurnRenderer::new(ui.clone())));
    renderer.borrow_mut().begin_thinking();

    let cb = renderer.clone();
    let mut on_event = move |event: &harness_agent::AgentEvent| cb.borrow_mut().on_event(event);
    let is_continue = matches!(request, TurnRequest::Continue);
    let result = tokio::select! {
        // Ctrl-C cancels the in-flight turn; the session continues.
        _ = tokio::signal::ctrl_c() => {
            renderer.borrow_mut().finish();
            println!();
            for line in crate::interrupt::interrupted_lines(ui, ends_mid_turn(agent.messages())) {
                println!("{line}");
            }
            return Ok(PromptOutcome::Interrupted);
        }
        result = async {
            if is_continue {
                agent.continue_turn(&mut on_event).await
            } else {
                agent.run_turn_with_attachments(text, attachments, &mut on_event).await
            }
        } => result,
    };
    renderer.borrow_mut().finish();
    let needs_brave_key = renderer.borrow().needs_brave_key();

    match result {
        Ok(_) => {
            carryover.clear();
            // Learn this model's rate (once) so the trailer can show the
            // session's running cost. Cheap when already cached.
            pricing::warm_for(agent.model()).await;
            print_context_usage(agent, ui);
            // Offer to set up web search if the model tried it without a key.
            if needs_brave_key {
                brave::prompt_after_failed_search(ui);
            }
            Ok(PromptOutcome::Ran)
        }
        Err(e) => {
            println!();
            let seeded = seed_retry(agent.messages(), &e);
            for line in turn_failure_lines(agent, ui, &e, seeded) {
                println!("{line}");
            }
            if seeded {
                *carryover = "/retry".to_string();
            }
            Ok(PromptOutcome::Ran)
        }
    }
}

/// A subtle trailer showing how full the model's context window is, set apart
/// from the turn's output by a blank line.
fn print_context_usage(agent: &Agent, ui: &Ui) {
    println!();
    for line in context_usage_lines(agent, ui) {
        println!("{line}");
    }
}

/// The context-usage trailer as a (themed, indented) line — the current model
/// alongside how full its context window is. Shared by the classic prompt and
/// the live composer (which pins it just above the input divider).
///
/// Two stacked lines: the first is the live context-window fill (what's in the
/// model's head *right now*, which shrinks on compaction); the second is the
/// session's cumulative spend — total tokens with an input/output breakdown so
/// it's auditable — and the running price. The two are deliberately distinct:
/// context fill ≠ total tokens used.
pub(crate) fn context_usage_lines(agent: &Agent, ui: &Ui) -> Vec<String> {
    remember_permission_mode(agent);
    context_usage_lines_from(
        ui,
        agent.model(),
        agent.context_tokens(),
        agent.context_window(),
        agent.prompt_tokens_used(),
        agent.completion_tokens_used(),
    )
}

/// The two trailer lines built from raw figures rather than an [`Agent`], so the
/// live composer can rebuild them from a mid-turn `Usage` event (which carries
/// the same numbers) and keep the meters climbing in real time — not just jump
/// at turn boundaries.
///
/// `used` is the current context fill; `prompt_tokens`/`completion_tokens` are
/// the session's cumulative input/output totals (their sum is the total tokens
/// used, and what the price is computed from). The workspace's git branch and
/// the permission mode are read from the process (see [`git_branch`] and
/// [`permission_mode`]) rather than passed in, so this signature stays the one
/// a `Usage` event can satisfy.
pub(crate) fn context_usage_lines_from(
    ui: &Ui,
    model: &str,
    used: usize,
    window: usize,
    prompt_tokens: usize,
    completion_tokens: usize,
) -> Vec<String> {
    meter_lines(
        ui,
        &Meters {
            model,
            used,
            window,
            prompt_tokens,
            completion_tokens,
            branch: git_branch(),
            mode: permission_mode(),
        },
        meter_budget(),
    )
}

/// Everything the two meter lines report about the session right now.
struct Meters<'a> {
    model: &'a str,
    used: usize,
    window: usize,
    prompt_tokens: usize,
    completion_tokens: usize,
    /// The workspace's git branch, when it is a git work tree.
    branch: Option<String>,
    /// The permission mode in force (`relaxed` / `cautious` / `bypass`).
    mode: Option<&'static str>,
}

/// One `· `-joined piece of a meter line, with the width it costs and how
/// eagerly it gives way on a narrow terminal (higher `expendable` goes first).
struct Seg {
    plain: String,
    styled: String,
    expendable: u8,
}

impl Seg {
    fn new(plain: impl Into<String>, styled: impl Into<String>, expendable: u8) -> Self {
        Seg {
            plain: plain.into(),
            styled: styled.into(),
            expendable,
        }
    }
}

/// Render `segs` into one indented line, dropping the most expendable pieces
/// until the result fits `budget` columns. `budget` is `None` when the terminal
/// width is unknown (piped output, tests) — then nothing is dropped.
fn fit(mut segs: Vec<Seg>, budget: Option<usize>) -> String {
    if let Some(budget) = budget {
        // Two-space indent, plus a `· ` joiner ahead of every segment but the
        // first — the same arithmetic the render below uses.
        let width = |segs: &[Seg]| -> usize {
            2 + segs
                .iter()
                .map(|s| crate::width::str_width(&s.plain))
                .sum::<usize>()
                + 3 * segs.len().saturating_sub(1)
        };
        while width(&segs) > budget {
            // The price gives way first, then the mode, branch, and model —
            // the context fill itself is never dropped.
            let Some(worst) = segs
                .iter()
                .enumerate()
                .filter(|(_, s)| s.expendable > 0)
                .max_by_key(|(_, s)| s.expendable)
                .map(|(i, _)| i)
            else {
                break;
            };
            segs.remove(worst);
        }
    }
    let joined: Vec<&str> = segs.iter().map(|s| s.styled.as_str()).collect();
    format!("  {}", joined.join(" · "))
}

/// The two meter lines for a set of figures, fitted to `budget` columns.
fn meter_lines(ui: &Ui, m: &Meters, budget: Option<usize>) -> Vec<String> {
    // Line 1 — the context window fill, with the branch, permission mode, the
    // model, and its per-token rate.
    let pct = (m.used * 100)
        .checked_div(m.window)
        .map_or(0, |p| p.min(100));
    let fill = format!(
        "🧭 context {} / {} tokens",
        human_tokens(m.used),
        human_tokens(m.window),
    );
    let pct_text = format!("({pct}%)");
    let mut line1 = vec![Seg::new(
        format!("{fill} {pct_text}"),
        format!("{} {}", ui.dim(&fill), paint_pct(ui, pct, &pct_text)),
        0,
    )];
    if let Some(branch) = &m.branch {
        let text = format!("⎇ {branch}");
        line1.push(Seg::new(text.clone(), ui.dim(&text), 2));
    }
    if let Some(mode) = m.mode {
        line1.push(Seg::new(mode, ui.dim(mode), 3));
    }
    line1.push(Seg::new(m.model, ui.accent(m.model), 1));
    if let Some(rate) = crate::pricing::session_rate(m.model)
        .as_ref()
        .and_then(crate::pricing::format_rate)
    {
        line1.push(Seg::new(rate.clone(), ui.dim(&rate), 4));
    }

    // Line 2 — the session's cumulative spend: total tokens = input + output,
    // spelled out so the figure is auditable, plus the running dollar cost.
    let total = m.prompt_tokens + m.completion_tokens;
    let totals = format!(
        "📊 {} tokens used · {} in · {} out",
        human_tokens(total),
        human_tokens(m.prompt_tokens),
        human_tokens(m.completion_tokens),
    );
    let mut line2 = vec![Seg::new(totals.clone(), ui.dim(&totals), 0)];
    if let Some(cost) = crate::pricing::session_cost(m.model, m.prompt_tokens, m.completion_tokens)
        // Only surface a price once it rounds to something visible, so a session
        // with a few cheap tokens doesn't read as "$0.00".
        .filter(|&c| c > 0.0)
        .map(crate::theme::format_usd)
    {
        line2.push(Seg::new(cost.clone(), ui.accent(&cost), 4));
    }

    vec![fit(line1, budget), fit(line2, budget)]
}

/// The context percentage, colored by how much room is left: normal below 60%,
/// a warning through 85%, red above it — so a context about to compact is
/// visible from across the room.
fn paint_pct(ui: &Ui, pct: usize, text: &str) -> String {
    match pct {
        0..=59 => ui.dim(text),
        60..=85 => ui.brown(text),
        _ => ui.red(text),
    }
}

/// Columns available to a meter line, or `None` when the width is unknown
/// (output isn't a terminal) — in which case nothing is dropped, since a
/// guessed width would silently hide real figures.
///
/// A running turn prefixes the first line with its spinner + timer
/// ([`TURN_INDICATOR_CELLS`]), so that much is reserved up front rather than
/// letting the indicator push the line past the right edge.
fn meter_budget() -> Option<usize> {
    if !std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        return None;
    }
    crossterm::terminal::size()
        .ok()
        .map(|(cols, _)| (cols as usize).saturating_sub(TURN_INDICATOR_CELLS))
        .filter(|w| *w > 0)
}

/// Cells reserved on the meter line for the running turn's `⠋ 123s ` prefix.
pub(crate) const TURN_INDICATOR_CELLS: usize = 10;

/// How long a resolved git branch is trusted before re-reading `.git/HEAD`.
const BRANCH_TTL: std::time::Duration = std::time::Duration::from_secs(2);

/// The branch shown on the meter line, cached for [`BRANCH_TTL`] so a meter
/// rebuilt on every `Usage` event doesn't stat the repo each time.
fn git_branch() -> Option<String> {
    static CACHE: std::sync::Mutex<Option<(std::time::Instant, Option<String>)>> =
        std::sync::Mutex::new(None);
    let mut cache = CACHE.lock().expect("branch cache poisoned");
    if let Some((at, branch)) = cache.as_ref() {
        if at.elapsed() < BRANCH_TTL {
            return branch.clone();
        }
    }
    let branch = std::env::current_dir().ok().and_then(|d| read_branch(&d));
    *cache = Some((std::time::Instant::now(), branch.clone()));
    branch
}

/// Read the checked-out branch by walking up from `start` for a `.git`, then
/// parsing its `HEAD` — a couple of file reads, no `git` subprocess, because
/// this runs on the meter's refresh path. A detached HEAD reports its short
/// sha; anything unreadable reports `None`.
fn read_branch(start: &std::path::Path) -> Option<String> {
    let git = start
        .ancestors()
        .map(|d| d.join(".git"))
        .find(|p| p.exists())?;
    // A worktree/submodule `.git` is a file pointing at the real git dir.
    let git = if git.is_file() {
        let text = std::fs::read_to_string(&git).ok()?;
        let target = text.trim().strip_prefix("gitdir:")?.trim();
        let target = std::path::Path::new(target);
        if target.is_absolute() {
            target.to_path_buf()
        } else {
            git.parent()?.join(target)
        }
    } else {
        git
    };
    let head = std::fs::read_to_string(git.join("HEAD")).ok()?;
    let head = head.trim();
    Some(match head.strip_prefix("ref: ") {
        Some(r) => r.rsplit('/').next().unwrap_or(r).to_string(),
        None => head.chars().take(7).collect(),
    })
}

/// The permission mode last seen on the agent, for the meter line.
///
/// The mode lives on the agent's gate, but the meter is also rebuilt from
/// mid-turn `Usage` events that carry only token figures — so
/// [`context_usage_lines`] records it here as it passes, and the figures-only
/// path reads it back. The mode can only change between turns (`/permissions`),
/// so the remembered value is never stale on screen.
fn remember_permission_mode(agent: &Agent) {
    // Plan mode outranks the mode on the meter, as it does at the gate.
    let mode = if agent.permission_gate().is_some_and(|g| g.plan_mode()) {
        "plan"
    } else {
        agent
            .permission_gate()
            .map(|g| g.mode())
            .unwrap_or_else(|| {
                harness_permissions::policy::load_global()
                    .mode
                    .unwrap_or_default()
            })
            .label()
    };
    *MODE.lock().expect("permission mode poisoned") = Some(mode);
}

fn permission_mode() -> Option<&'static str> {
    *MODE.lock().expect("permission mode poisoned")
}

static MODE: std::sync::Mutex<Option<&'static str>> = std::sync::Mutex::new(None);

/// Human-friendly token count: `980`, `12.3k`, `1.2M`.
// Token counts render identically everywhere; the shared formatter lives in
// harness-core and is re-exported here for the CLI's meters and lanes.
pub(crate) use harness_core::fmt::human_tokens;

#[cfg(test)]
mod tests {
    use super::{
        context_usage_lines_from, ends_mid_turn, meter_lines, read_branch, retry_notice,
        seed_retry, Meters,
    };
    use crate::theme::Ui;
    use harness_agent::AgentError;
    use harness_llm::{ChatMessage, LlmError};

    fn plain_ui() -> Ui {
        Ui::with(false, std::sync::Arc::new(harness_theme::Theme::default()))
    }

    #[test]
    fn trailer_shows_context_fill_and_auditable_totals_that_climb() {
        use harness_local::source::ModelPricing;
        // Prime the cache so the trailer can price this model.
        crate::pricing::seed_for_test(
            "live-meter-model",
            Some(ModelPricing {
                input_cost_per_token: 0.000_003,
                output_cost_per_token: 0.000_015,
            }),
        );
        let ui = plain_ui();

        // A mid-turn snapshot: 50k of a 200k window; 40k in + 10k out so far.
        let lines =
            context_usage_lines_from(&ui, "live-meter-model", 50_000, 200_000, 40_000, 10_000);
        assert_eq!(lines.len(), 2, "two stacked lines: {lines:?}");
        let (ctx, usage) = (&lines[0], &lines[1]);

        // Line 1 — the context-window fill, model, and per-token rate.
        assert!(ctx.contains("50.0k / 200.0k"), "context: {ctx}");
        assert!(ctx.contains("(25%)"), "percent: {ctx}");
        assert!(ctx.contains("live-meter-model"), "model: {ctx}");
        assert!(ctx.contains("$3/M in · $15/M out"), "rate: {ctx}");

        // Line 2 — auditable totals: total = in + out (50k = 40k + 10k), + cost.
        assert!(usage.contains("50.0k tokens used"), "total: {usage}");
        assert!(usage.contains("40.0k in"), "input: {usage}");
        assert!(usage.contains("10.0k out"), "output: {usage}");
        // 40k * 3e-6 + 10k * 15e-6 = 0.12 + 0.15 = 0.27
        assert!(usage.contains("$0.27"), "cost: {usage}");
        // The context fill and the total are distinct figures, not conflated.
        assert!(
            !ctx.contains("tokens used"),
            "fill leaked into line 1: {ctx}"
        );

        // A later snapshot in the same turn climbs — the whole point of wiring
        // Usage events through: more context, more tokens, higher cost.
        let later =
            context_usage_lines_from(&ui, "live-meter-model", 120_000, 200_000, 100_000, 40_000);
        assert!(later[0].contains("120.0k / 200.0k"), "context: {later:?}");
        assert!(later[0].contains("(60%)"), "percent: {later:?}");
        assert!(later[1].contains("140.0k tokens used"), "total: {later:?}");
        assert!(later[1].contains("100.0k in"), "input: {later:?}");
        assert!(later[1].contains("40.0k out"), "output: {later:?}");
        // 100k * 3e-6 + 40k * 15e-6 = 0.30 + 0.60 = 0.90
        assert!(later[1].contains("$0.90"), "cost: {later:?}");
    }

    fn meters<'a>(used: usize, model: &'a str) -> Meters<'a> {
        Meters {
            model,
            used,
            window: 200_000,
            prompt_tokens: 40_000,
            completion_tokens: 10_000,
            branch: Some("wagon-trail".to_string()),
            mode: Some("cautious"),
        }
    }

    #[test]
    fn the_meter_reports_branch_and_permission_mode() {
        let ui = plain_ui();
        let lines = meter_lines(&ui, &meters(50_000, "meter-facts-model"), None);
        // Where you are and how much rope the agent has, on the same glance as
        // how full the context is.
        assert!(lines[0].contains("⎇ wagon-trail"), "branch: {lines:?}");
        assert!(lines[0].contains("cautious"), "mode: {lines:?}");
        assert!(lines[0].contains("(25%)"), "fill: {lines:?}");

        // Neither is invented when there's nothing to report.
        let bare = Meters {
            branch: None,
            mode: None,
            ..meters(50_000, "meter-facts-model")
        };
        let lines = meter_lines(&ui, &bare, None);
        assert!(!lines[0].contains("⎇"), "phantom branch: {lines:?}");
        assert!(!lines[0].contains("cautious"), "phantom mode: {lines:?}");
    }

    #[test]
    fn the_context_percentage_is_colored_by_how_full_it_is() {
        let ui = Ui::with(true, std::sync::Arc::new(harness_theme::Theme::default()));
        // The percentage carries its own color; the same three thresholds a
        // reader learns once — comfortable, filling up, about to compact.
        let color_of = |used: usize| -> String {
            let line = meter_lines(&ui, &meters(used, "meter-color-model"), None).remove(0);
            let at = line.find("(").expect("a percentage");
            let start = line[..at].rfind("\x1b[").expect("a color before it");
            line[start..at].to_string()
        };
        let normal = color_of(40_000); // 20%
        let warn = color_of(140_000); // 70%
        let danger = color_of(190_000); // 95%
        assert_ne!(normal, warn, "a filling context must change color");
        assert_ne!(warn, danger, "a nearly-full context must change again");
        // …and the danger color is the palette's own alarm color.
        assert!(
            danger.contains(ui.red("x").trim_end_matches("x\x1b[0m")),
            "danger must be the red slot: {danger}"
        );
    }

    #[test]
    fn a_narrow_terminal_drops_the_price_before_anything_else() {
        use harness_local::source::ModelPricing;
        crate::pricing::seed_for_test(
            "meter-width-model",
            Some(ModelPricing {
                input_cost_per_token: 0.000_003,
                output_cost_per_token: 0.000_015,
            }),
        );
        let ui = plain_ui();
        let m = meters(50_000, "meter-width-model");
        let width = |lines: &[String]| {
            lines
                .iter()
                .map(|l| crate::width::str_width(l))
                .max()
                .unwrap()
        };

        // Wide enough for everything.
        let full = meter_lines(&ui, &m, Some(200));
        assert!(full[0].contains("$3/M in"), "rate: {full:?}");
        assert!(full[1].contains("$0.27"), "cost: {full:?}");

        // Squeezed: both lines stay inside the width, and what gives way first
        // is the price — the rate off line 1, the running cost off line 2.
        let tight = meter_lines(&ui, &m, Some(60));
        assert!(width(&tight) <= 60, "over width: {tight:?}");
        assert!(
            !tight[0].contains("$3/M in"),
            "rate must go first: {tight:?}"
        );
        assert!(
            tight[0].contains("meter-width-model"),
            "the model outlives the price: {tight:?}"
        );
        let tighter = meter_lines(&ui, &m, Some(45));
        assert!(width(&tighter) <= 45, "over width: {tighter:?}");
        assert!(
            !tighter[1].contains("$0.27"),
            "price must go first: {tighter:?}"
        );
        // The context fill itself survives every squeeze.
        assert!(tighter[0].contains("🧭 context"), "fill lost: {tighter:?}");
        assert!(
            tighter[1].contains("tokens used"),
            "totals lost: {tighter:?}"
        );

        // Brutally narrow: only the un-droppable core is left.
        let squeezed = meter_lines(&ui, &m, Some(20));
        assert!(squeezed[0].contains("(25%)"), "fill lost: {squeezed:?}");
        assert!(
            !squeezed[0].contains("meter-width-model"),
            "model should give way: {squeezed:?}"
        );
    }

    #[test]
    fn read_branch_reads_head_without_shelling_out_to_git() {
        let dir = tempfile::tempdir().unwrap();
        let git = dir.path().join(".git");
        std::fs::create_dir_all(&git).unwrap();

        // The ordinary case: HEAD points at a branch ref.
        std::fs::write(git.join("HEAD"), "ref: refs/heads/feature/wagon\n").unwrap();
        assert_eq!(read_branch(dir.path()).as_deref(), Some("wagon"));

        // From a subdirectory, the walk up still finds the repo.
        let deep = dir.path().join("crates/harness-cli");
        std::fs::create_dir_all(&deep).unwrap();
        assert_eq!(read_branch(&deep).as_deref(), Some("wagon"));

        // A detached HEAD (mid-rebase, a checked-out tag) labels itself with
        // the short sha rather than lying about a branch.
        std::fs::write(
            git.join("HEAD"),
            "9f1c0de6b3a2f4c5d6e7f8091a2b3c4d5e6f7081\n",
        )
        .unwrap();
        assert_eq!(read_branch(dir.path()).as_deref(), Some("9f1c0de"));

        // A worktree/submodule `.git` file points at the real git dir.
        let wt = tempfile::tempdir().unwrap();
        std::fs::write(
            wt.path().join(".git"),
            format!("gitdir: {}\n", git.display()),
        )
        .unwrap();
        assert_eq!(read_branch(wt.path()).as_deref(), Some("9f1c0de"));

        // Not a repo at all: nothing to show, and no panic.
        let bare = tempfile::tempdir().unwrap();
        assert_eq!(read_branch(bare.path()), None);
    }

    #[test]
    fn ends_mid_turn_flags_dangling_user_and_tool_messages() {
        // Ends on a user message: the reply never arrived → retryable.
        let dangling_user = vec![ChatMessage::system("s"), ChatMessage::user("hi")];
        assert!(ends_mid_turn(&dangling_user));

        // Ends on a tool result the model never reacted to → retryable.
        let dangling_tool = vec![
            ChatMessage::user("hi"),
            ChatMessage::tool_result("t1", "output".to_string()),
        ];
        assert!(ends_mid_turn(&dangling_tool));

        // A settled conversation (assistant spoke last) has nothing to retry.
        let settled = vec![ChatMessage::user("hi"), ChatMessage::assistant("hello")];
        assert!(!ends_mid_turn(&settled));
        assert!(!ends_mid_turn(&[]));
    }

    #[test]
    fn seed_retry_only_for_dangling_retryable_failures() {
        let dangling = vec![ChatMessage::system("s"), ChatMessage::user("hi")];
        let settled = vec![ChatMessage::user("hi"), ChatMessage::assistant("done")];
        let exhausted = AgentError::RetriesExhausted {
            attempts: 4,
            model: "claude-opus-4-8".into(),
            endpoint: "https://hub.oxen.ai/api/ai".into(),
            source: LlmError::Api {
                status: 502,
                message: "The model provider returned an error.".into(),
            },
        };
        let auth = AgentError::Llm(LlmError::Api {
            status: 401,
            message: "Invalid API key".into(),
        });

        // A dangling turn that died on a provider error → pre-fill /retry.
        assert!(seed_retry(&dangling, &exhausted));
        // A settled conversation has nothing to re-drive.
        assert!(!seed_retry(&settled, &exhausted));
        // Auth failures need /auth first, not a retry.
        assert!(!seed_retry(&dangling, &auth));
    }

    #[test]
    fn retry_notice_reports_the_upcoming_attempt_and_wait() {
        let notice = retry_notice(1, 4, 2000, "Oxen API error (502): provider error", None);
        assert!(notice.contains("Oxen API error (502)"));
        assert!(notice.contains("retrying in 2s"));
        assert!(notice.contains("attempt 2 of 4"));
    }

    #[test]
    fn retry_notice_reports_a_model_switch_instead_of_a_countdown() {
        // The attempt counter restarts on the new model, so showing "attempt 5
        // of 4" would read as a bug rather than a fallback.
        let notice = retry_notice(4, 4, 0, "Oxen API error (503)", Some("claude-sonnet-5"));
        assert!(notice.contains("continuing on claude-sonnet-5"), "{notice}");
        assert!(!notice.contains("attempt 5"), "{notice}");
        assert!(!notice.contains("retrying in"), "{notice}");
    }
}
