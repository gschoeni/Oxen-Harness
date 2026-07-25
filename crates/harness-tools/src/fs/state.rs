//! Session file state: what the model has actually seen on disk, who is
//! writing to a path right now, and the path-scoped conventions to surface the
//! first time a matching file is touched.
//!
//! Three problems, one piece of shared state because they're all keyed by path:
//!
//! 1. **Editing blind.** `edit_file` used to read, replace, and write with no
//!    check that the model had ever seen the file. An edit assembled from
//!    memory (or from a stale earlier turn) would silently rewrite code the
//!    model never looked at.
//! 2. **Editing stale.** Between a read and the edit that follows it, the file
//!    can change — the user saves in their editor, a `run_shell` formatter
//!    rewrites it, another fleet lane edits the same file. The edit still
//!    "succeeds" and quietly reverts whatever happened in between.
//! 3. **Racing.** `spawn_agents` hands every lane the same registry, rooted at
//!    the same workspace, so N lanes doing read-modify-write on one file
//!    interleave with last-write-wins.
//!
//! So: reads record a whole-file fingerprint, writes verify it first, and
//! every mutation holds a per-path lock for its whole read-modify-write.

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::Hasher;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use globset::{Glob, GlobSet, GlobSetBuilder};

/// How many paths keep a fingerprint. Past this the oldest is dropped and its
/// next edit asks for a re-read — cheap, and far better than unbounded growth
/// in a long session.
const MAX_TRACKED_PATHS: usize = 64;

/// The most of a path-scoped convention that rides along on a tool result.
/// The full text is always a `read_file` away.
const MAX_RULE_CHARS: usize = 4_000;

/// What the model was last shown of a file.
#[derive(Debug, Clone)]
struct Snapshot {
    fingerprint: u64,
    lines: usize,
    /// 1-based inclusive line ranges actually displayed. A whole-file read is
    /// one range; a windowed or outlined read leaves gaps, and an edit aimed
    /// into a gap is an edit written blind.
    seen: Vec<(usize, usize)>,
}

/// The result of checking a file against what the model last read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Freshness {
    /// On-disk content still matches what the model was shown.
    Fresh,
    /// No read of this path is on record.
    NeverRead,
    /// The file changed since the read.
    Changed { seen_lines: usize, now_lines: usize },
}

/// A convention scoped to a set of path globs — a nested `AGENTS.md`, or a
/// Cursor rule with a `globs:` list. Surfaced on the first tool result that
/// touches a matching file rather than living in the prompt, so a rule for a
/// corner of the repo costs nothing in the sessions that never go there.
#[derive(Debug, Clone)]
pub struct PathRule {
    /// How the source file is named to the model (workspace-relative).
    pub source: String,
    /// The instructions to surface.
    pub body: String,
    matcher: Option<GlobSet>,
}

impl PathRule {
    pub fn new(source: impl Into<String>, body: impl Into<String>, globs: &[String]) -> Self {
        Self {
            source: source.into(),
            body: body.into(),
            matcher: compile(globs),
        }
    }

    fn matches(&self, rel: &Path) -> bool {
        self.matcher.as_ref().is_some_and(|set| set.is_match(rel))
    }
}

/// A bare `*.ts` is understood the way its author meant it — any depth — so
/// each pattern without a separator also gets a `**/` form.
fn compile(globs: &[String]) -> Option<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    let mut any = false;
    for pattern in globs {
        let pattern = pattern.trim();
        if let Ok(glob) = Glob::new(pattern) {
            builder.add(glob);
            any = true;
        }
        if !pattern.contains('/') {
            if let Ok(glob) = Glob::new(&format!("**/{pattern}")) {
                builder.add(glob);
                any = true;
            }
        }
    }
    any.then(|| builder.build().ok()).flatten()
}

/// Per-session, per-workspace file state shared by the fs tools. Cloneable
/// through an `Arc`: fleet lanes share their parent's, which is exactly what
/// makes the per-path lock protect them from each other.
pub struct FileState {
    /// Whether an unread file may be edited. On for the shipped registry; the
    /// bare `ToolTool::new` constructors leave it off so a tool built outside
    /// a session behaves as it always did.
    require_read: bool,
    snapshots: Mutex<VecDeque<(PathBuf, Snapshot)>>,
    locks: Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
    rules: Mutex<Vec<PathRule>>,
    surfaced: Mutex<HashSet<String>>,
}

impl std::fmt::Debug for FileState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileState")
            .field("require_read", &self.require_read)
            .finish_non_exhaustive()
    }
}

impl FileState {
    /// State that gates edits on a prior read — what a real session wants.
    pub fn gated() -> Arc<Self> {
        Arc::new(Self::with_require_read(true))
    }

    /// Ungated state: fingerprints are still recorded (so stale-write
    /// detection works once a read has happened) but an unread file may be
    /// edited. This is what a standalone tool built with `new()` gets.
    pub fn ungated() -> Arc<Self> {
        Arc::new(Self::with_require_read(false))
    }

    fn with_require_read(require_read: bool) -> Self {
        Self {
            require_read,
            snapshots: Mutex::new(VecDeque::new()),
            locks: Mutex::new(HashMap::new()),
            rules: Mutex::new(Vec::new()),
            surfaced: Mutex::new(HashSet::new()),
        }
    }

    pub fn requires_read(&self) -> bool {
        self.require_read
    }

    /// Install the path-scoped conventions discovered for this workspace.
    /// Hosts call this after building the registry, since discovery lives a
    /// layer up (in `harness-runtime`, which depends on this crate).
    pub fn set_rules(&self, rules: Vec<PathRule>) {
        *self.rules.lock().expect("rules lock") = rules;
    }

    /// Record what the model was just shown. Called after a successful read,
    /// and after a write/edit so the next edit in the same turn doesn't read
    /// as stale against content the model itself just produced.
    pub fn record(&self, path: &Path, contents: &str) {
        let lines = contents.lines().count();
        self.record_parts(
            path,
            fingerprint(contents.as_bytes()),
            lines,
            vec![(1, lines.max(1))],
        );
    }

    /// Record a fingerprint computed elsewhere — `read_file` streams the file
    /// to count its lines anyway, so it hashes as it goes rather than reading
    /// the whole thing a second time. `seen` is the part of the file that
    /// actually reached the model.
    pub fn record_parts(
        &self,
        path: &Path,
        fingerprint: u64,
        lines: usize,
        seen: Vec<(usize, usize)>,
    ) {
        let snapshot = Snapshot {
            fingerprint,
            lines,
            seen,
        };
        let key = canonical(path);
        let mut snapshots = self.snapshots.lock().expect("snapshot lock");
        snapshots.retain(|(p, _)| p != &key);
        snapshots.push_back((key, snapshot));
        while snapshots.len() > MAX_TRACKED_PATHS {
            snapshots.pop_front();
        }
    }

    /// The line ranges recorded for `path`, if any.
    pub fn seen_ranges(&self, path: &Path) -> Vec<(usize, usize)> {
        let key = canonical(path);
        let snapshots = self.snapshots.lock().expect("snapshot lock");
        snapshots
            .iter()
            .find(|(p, _)| p == &key)
            .map(|(_, snapshot)| snapshot.seen.clone())
            .unwrap_or_default()
    }

    /// Is `current` still what the model was shown for `path`?
    pub fn verify(&self, path: &Path, current: &str) -> Freshness {
        let key = canonical(path);
        let snapshots = self.snapshots.lock().expect("snapshot lock");
        let Some((_, snapshot)) = snapshots.iter().find(|(p, _)| p == &key) else {
            return Freshness::NeverRead;
        };
        if snapshot.fingerprint == fingerprint(current.as_bytes()) {
            Freshness::Fresh
        } else {
            Freshness::Changed {
                seen_lines: snapshot.lines,
                now_lines: current.lines().count(),
            }
        }
    }

    /// Was every line in `first..=last` actually displayed to the model? An
    /// outlined read hides function bodies; an edit aimed inside one is
    /// written against code nobody looked at, so it's refused with the range
    /// to re-read. Returns the covering ranges when unseen, for the message.
    pub fn unseen_around(&self, path: &Path, first: usize, last: usize) -> Option<(usize, usize)> {
        let key = canonical(path);
        let snapshots = self.snapshots.lock().expect("snapshot lock");
        let (_, snapshot) = snapshots.iter().find(|(p, _)| p == &key)?;
        // No recorded window means the read displayed nothing (an offset past
        // the end of the file), so every line is unseen.
        if snapshot.seen.is_empty() {
            return Some((first, last));
        }
        // Shown ranges are disjoint and never adjacent (a gap between two of
        // them is exactly what was elided), so a span is fully seen only if
        // one single range contains all of it.
        let contained = snapshot.seen.iter().any(|&(a, b)| first >= a && last <= b);
        (!contained).then_some((first, last))
    }

    /// Decide whether a mutation of `path` may proceed, given what's on disk
    /// right now. The messages are the whole point: a model that gets
    /// "not found" retries blindly, while a model told *what* to do next does
    /// it. Same reasoning as [`super::edit_diagnostics`].
    pub fn guard(&self, display: &str, path: &Path, current: &str) -> Result<(), crate::ToolError> {
        match self.verify(path, current) {
            Freshness::Fresh => Ok(()),
            Freshness::NeverRead if !self.require_read => Ok(()),
            Freshness::NeverRead => Err(crate::ToolError::InvalidArguments(format!(
                "`read_file` {display} before editing it — an edit written from memory \
                 silently reverts whatever you haven't seen. Read it, then re-apply \
                 your change against the real content."
            ))),
            Freshness::Changed {
                seen_lines,
                now_lines,
            } => Err(crate::ToolError::InvalidArguments(format!(
                "{display} changed on disk since you read it ({seen_lines} lines then, \
                 {now_lines} now) — the user, a formatter, or another agent has edited \
                 it. Re-read it and re-apply your change on top of the new content; \
                 writing now would revert theirs."
            ))),
        }
    }

    /// Hold this path for the duration of a read-modify-write. Different paths
    /// never block each other; the same path serializes, so two fleet lanes
    /// editing one file queue instead of clobbering.
    pub async fn lock(&self, path: &Path) -> tokio::sync::OwnedMutexGuard<()> {
        let key = canonical(path);
        let mutex = {
            let mut locks = self.locks.lock().expect("path lock map");
            // Reclaim locks nobody holds before the map can grow without
            // bound in a long session.
            if locks.len() > MAX_TRACKED_PATHS {
                locks.retain(|_, m| Arc::strong_count(m) > 1);
            }
            locks.entry(key).or_default().clone()
        };
        mutex.lock_owned().await
    }

    /// The conventions governing `rel` that haven't been surfaced yet this
    /// session, rendered for appending to a tool result. Empty once each rule
    /// has been shown — repeating it every read would be noise the model
    /// learns to skip.
    pub fn take_rules_for(&self, rel: &Path) -> String {
        let rules = self.rules.lock().expect("rules lock");
        let mut surfaced = self.surfaced.lock().expect("surfaced lock");
        let mut out = String::new();
        for rule in rules.iter().filter(|r| r.matches(rel)) {
            if !surfaced.insert(rule.source.clone()) {
                continue;
            }
            let body = harness_core::text::truncate_with_marker(
                &rule.body,
                MAX_RULE_CHARS,
                "\n… [clipped — read the source file for the rest]",
            );
            out.push_str(&format!(
                "\n\n<project-convention source=\"{}\">\n{}\n</project-convention>",
                rule.source,
                body.trim_end()
            ));
        }
        out
    }
}

/// One applied replacement, in the coordinates the edit was matched in:
/// the original lines it covered, and how many lines replaced them.
#[derive(Debug, Clone, Copy)]
pub struct AppliedEdit {
    pub first_line: usize,
    pub last_line: usize,
    pub new_lines: usize,
}

/// Move recorded seen-ranges from a file's pre-edit coordinates to its
/// post-edit ones.
///
/// Without this, an edit had to either drop what the model had seen (forcing a
/// re-read before every follow-up edit) or claim the whole file as seen (which
/// would silently hand back the licence that windowed and outlined reads exist
/// to withhold). Ranges before an edit keep their numbers, ranges after shift
/// by the accumulated delta, and a range that spanned an edit stretches to
/// cover the replacement — the model wrote that text, so it has seen it.
pub fn remap_seen(seen: &[(usize, usize)], edits: &[AppliedEdit]) -> Vec<(usize, usize)> {
    let mut sorted: Vec<AppliedEdit> = edits.to_vec();
    sorted.sort_by_key(|e| e.first_line);

    // Lines added (or removed) by every edit that ends before `line`.
    let delta_before = |line: usize| -> isize {
        sorted
            .iter()
            .filter(|e| e.last_line < line)
            .map(|e| e.new_lines as isize - (e.last_line - e.first_line + 1) as isize)
            .sum()
    };
    // …and by every edit that has started by `line`, for the far end of a
    // range the edit landed inside.
    let delta_through = |line: usize| -> isize {
        sorted
            .iter()
            .filter(|e| e.first_line <= line)
            .map(|e| e.new_lines as isize - (e.last_line - e.first_line + 1) as isize)
            .sum()
    };

    let shift = |line: usize, delta: isize| -> usize { (line as isize + delta).max(1) as usize };

    let mut out: Vec<(usize, usize)> = seen
        .iter()
        .map(|&(start, end)| {
            (
                shift(start, delta_before(start)),
                shift(end, delta_through(end)),
            )
        })
        .filter(|(start, end)| start <= end)
        .collect();

    // The replacement text is seen too — the model wrote it. Without this, an
    // edit that spans the gap between two seen ranges leaves its own new lines
    // marked unseen, and a follow-up edit there is refused.
    out.extend(sorted.iter().map(|edit| {
        let start = shift(edit.first_line, delta_before(edit.first_line));
        (start, start + edit.new_lines.saturating_sub(1))
    }));

    // Overlaps are possible once ranges stretch; merging keeps the invariant
    // `unseen_around` relies on (disjoint, sorted, non-adjacent).
    out.sort();
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(out.len());
    for (start, end) in out {
        match merged.last_mut() {
            Some((_, prev_end)) if start <= *prev_end + 1 => *prev_end = (*prev_end).max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

/// Content fingerprint. Change detection only — never persisted, never
/// compared across processes, so a fast hash beats a cryptographic one.
/// Hashing raw bytes (rather than `str`) keeps the whole-file value the same
/// whether it's computed in one shot or streamed chunk by chunk.
pub(crate) fn fingerprint(bytes: &[u8]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hasher.write(bytes);
    hasher.finish()
}

/// The streaming counterpart of [`fingerprint`], for a reader that sees the
/// file in chunks. `finish()` must equal `fingerprint(whole_file)`.
#[derive(Default)]
pub(crate) struct StreamingFingerprint {
    hasher: std::collections::hash_map::DefaultHasher,
}

impl StreamingFingerprint {
    pub(crate) fn update(&mut self, bytes: &[u8]) {
        self.hasher.write(bytes);
    }

    pub(crate) fn finish(&self) -> u64 {
        self.hasher.finish()
    }
}

/// Key paths by their resolved form so `./src/a.rs` and `src/a.rs` are one
/// entry. A path that can't be canonicalized (not created yet) keys as-is.
fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unread_file_is_reported_as_never_read() {
        let state = FileState::gated();
        assert_eq!(
            state.verify(Path::new("/nope/a.rs"), "content"),
            Freshness::NeverRead
        );
    }

    #[test]
    fn a_recorded_file_is_fresh_until_its_content_moves() {
        let state = FileState::gated();
        let path = Path::new("/tmp/oxen-state-test.rs");
        state.record(path, "one\ntwo\n");
        assert_eq!(state.verify(path, "one\ntwo\n"), Freshness::Fresh);
        assert_eq!(
            state.verify(path, "one\ntwo\nthree\n"),
            Freshness::Changed {
                seen_lines: 2,
                now_lines: 3
            }
        );
    }

    #[test]
    fn recording_a_path_twice_keeps_one_entry_and_the_latest_content() {
        let state = FileState::gated();
        let path = Path::new("/tmp/oxen-state-twice.rs");
        state.record(path, "first");
        state.record(path, "second");
        assert_eq!(state.verify(path, "second"), Freshness::Fresh);
        assert_eq!(state.snapshots.lock().unwrap().len(), 1);
    }

    #[test]
    fn tracking_is_bounded_and_evicts_the_oldest_path() {
        let state = FileState::gated();
        for i in 0..MAX_TRACKED_PATHS + 5 {
            state.record(&PathBuf::from(format!("/tmp/oxen-evict-{i}.rs")), "x");
        }
        assert_eq!(state.snapshots.lock().unwrap().len(), MAX_TRACKED_PATHS);
        // The first path aged out; its next edit asks for a re-read.
        assert_eq!(
            state.verify(Path::new("/tmp/oxen-evict-0.rs"), "x"),
            Freshness::NeverRead
        );
        assert_eq!(
            state.verify(
                &PathBuf::from(format!("/tmp/oxen-evict-{}.rs", MAX_TRACKED_PATHS + 4)),
                "x"
            ),
            Freshness::Fresh
        );
    }

    #[tokio::test]
    async fn different_paths_do_not_block_each_other() {
        let state = FileState::gated();
        let _held = state.lock(Path::new("/tmp/oxen-lock-a")).await;
        // Would hang if the lock were global rather than per path.
        let _other = state.lock(Path::new("/tmp/oxen-lock-b")).await;
    }

    #[tokio::test]
    async fn the_same_path_serializes() {
        let state = FileState::gated();
        let held = state.lock(Path::new("/tmp/oxen-lock-same")).await;
        let waiter = {
            let state = state.clone();
            tokio::spawn(async move {
                let _guard = state.lock(Path::new("/tmp/oxen-lock-same")).await;
                "second"
            })
        };
        // The waiter cannot finish while the first guard is alive.
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());
        drop(held);
        assert_eq!(waiter.await.unwrap(), "second");
    }

    #[test]
    fn seen_ranges_move_with_the_edits_applied_to_them() {
        // The model read lines 1-10 and 30-40 of a 40-line file, then replaced
        // line 5 (one line) with three.
        let seen = [(1, 10), (30, 40)];
        let edits = [AppliedEdit {
            first_line: 5,
            last_line: 5,
            new_lines: 3,
        }];

        let moved = remap_seen(&seen, &edits);

        // The first range stretches over the replacement the model wrote…
        assert_eq!(moved[0], (1, 12));
        // …and the later range shifts by the two lines gained above it.
        assert_eq!(moved[1], (32, 42));
    }

    #[test]
    fn an_edit_below_a_seen_range_leaves_it_alone() {
        let moved = remap_seen(
            &[(1, 10)],
            &[AppliedEdit {
                first_line: 30,
                last_line: 32,
                new_lines: 1,
            }],
        );
        // The earlier range keeps its numbers, and the one line the edit wrote
        // is seen as well — it came from the model.
        assert_eq!(moved, vec![(1, 10), (30, 30)]);
    }

    #[test]
    fn ranges_that_collide_after_moving_are_merged() {
        // Deleting the gap between two seen ranges brings them together; they
        // must come back as one, since `unseen_around` assumes disjoint,
        // non-adjacent ranges.
        let moved = remap_seen(
            &[(1, 10), (20, 30)],
            &[AppliedEdit {
                first_line: 11,
                last_line: 19,
                new_lines: 1,
            }],
        );
        assert_eq!(moved, vec![(1, 22)]);
    }

    #[test]
    fn a_scoped_rule_surfaces_once_for_a_matching_path() {
        let state = FileState::gated();
        state.set_rules(vec![PathRule::new(
            "app/AGENTS.md",
            "Frontend rules.",
            &["app/**".to_string()],
        )]);

        let first = state.take_rules_for(Path::new("app/src/main.tsx"));
        assert!(first.contains("Frontend rules."));
        assert!(first.contains("source=\"app/AGENTS.md\""));
        // Repeating it on every read would be noise.
        assert!(state
            .take_rules_for(Path::new("app/src/other.tsx"))
            .is_empty());
        assert!(state.take_rules_for(Path::new("crates/lib.rs")).is_empty());
    }

    #[test]
    fn an_oversized_rule_is_clipped() {
        let state = FileState::gated();
        state.set_rules(vec![PathRule::new(
            "big.md",
            "z".repeat(MAX_RULE_CHARS * 2),
            &["**/*.rs".to_string()],
        )]);
        let surfaced = state.take_rules_for(Path::new("src/lib.rs"));
        assert!(surfaced.chars().count() < MAX_RULE_CHARS + 300);
    }
}
