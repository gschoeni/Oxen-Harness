//! Convention context files — the instructions a repository already carries.
//!
//! Most repositories that are worked on with an agent grow an `AGENTS.md` (or
//! `CLAUDE.md`, or a `.cursor/rules/` directory) stating how *this* project
//! wants to be worked on: the check commands, the layering rules, the things
//! that look wrong but aren't. Until now the harness ignored all of it — the
//! model started every session knowing only its own policy prompt and the
//! working directory, so it re-derived (or violated) house rules every time.
//!
//! This module discovers those files and turns them into a prompt section:
//!
//! - **Always-apply** files (no `globs:` frontmatter) are rendered into the
//!   system prompt at construction, which puts them inside the stable cache
//!   prefix — paid for once per session, not once per turn.
//! - **Path-scoped** files (a `globs:` list, or a nested `AGENTS.md` that
//!   governs its own subtree) are *not* in the prompt. They're listed
//!   one-line-each, and [`ContextFiles::rules_for`] hands the body to the fs
//!   tools so it can ride along on the first read/edit of a matching file.
//!
//! [`crate::project`] is the neighbouring concept and stays separate: that is
//! the *user's* durable project metadata, authored in the desktop Projects UI
//! and stored in `.oxen-harness/project.json`. This module reads what the
//! repository already committed for whichever agent the author happened to use.

use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};

/// The most of one context file that reaches the prompt. Past this the body is
/// clipped with a marker — the model can always `read_file` the rest.
pub const MAX_FILE_CHARS: usize = 16_000;

/// The most that *all* always-apply files together may contribute. Past this,
/// the lowest-priority files degrade to a one-line "this file exists" mention.
pub const MAX_TOTAL_CHARS: usize = 32_000;

/// How deep below the workspace root a nested `AGENTS.md` is still discovered.
const MAX_NESTED_DEPTH: usize = 3;

/// Directories never descended into when looking for nested context files.
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    "vendor",
    ".git",
    ".venv",
    "venv",
    "__pycache__",
];

/// Where a context file came from. Ordering is significance: a file closer to
/// the work wins the budget and is rendered last, so its instructions read as
/// the most specific word on the subject.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    /// `~/.oxen-harness/AGENTS.md` — the user's standing instructions.
    Global,
    /// A file at the workspace root.
    Repo,
    /// A file in a subdirectory, governing that subtree.
    Nested,
}

impl Scope {
    /// The label shown next to the file in the prompt and in `/context`.
    pub fn label(self) -> &'static str {
        match self {
            Scope::Global => "user",
            Scope::Repo => "repo",
            Scope::Nested => "directory",
        }
    }
}

/// One discovered context file.
#[derive(Debug, Clone)]
pub struct ContextFile {
    /// Absolute path on disk.
    pub path: PathBuf,
    /// How it's named in the prompt: workspace-relative, or `~/…` for global.
    pub display: String,
    pub scope: Scope,
    /// The instructions, clipped to [`MAX_FILE_CHARS`].
    pub body: String,
    /// Path globs this file applies to. Empty means always-apply.
    pub globs: Vec<String>,
    /// A one-line summary (frontmatter `description:`), for the scoped listing.
    pub description: String,
    /// Whether [`Self::body`] was clipped.
    pub truncated: bool,
    /// Compiled form of [`Self::globs`], built once at discovery.
    matcher: Option<GlobSet>,
}

impl ContextFile {
    /// Always-apply files carry no globs — they're prompt-resident.
    pub fn is_always_apply(&self) -> bool {
        self.globs.is_empty()
    }

    /// Does this file's glob set cover `rel` (a workspace-relative path)?
    fn matches(&self, rel: &Path) -> bool {
        self.matcher.as_ref().is_some_and(|set| set.is_match(rel))
    }
}

/// Every context file discovered for one workspace.
#[derive(Debug, Clone, Default)]
pub struct ContextFiles {
    pub files: Vec<ContextFile>,
}

impl ContextFiles {
    /// The files whose bodies belong in the system prompt.
    pub fn always_apply(&self) -> impl Iterator<Item = &ContextFile> {
        self.files.iter().filter(|f| f.is_always_apply())
    }

    /// The files that only apply to paths matching their globs.
    pub fn scoped(&self) -> impl Iterator<Item = &ContextFile> {
        self.files.iter().filter(|f| !f.is_always_apply())
    }

    /// The scoped files governing `rel`, most general first. Handed to the fs
    /// tools so touching a matching file surfaces its rules once.
    pub fn rules_for(&self, rel: &Path) -> Vec<&ContextFile> {
        self.scoped().filter(|f| f.matches(rel)).collect()
    }

    /// The scoped files in the form the fs tools take, for
    /// [`harness_tools::FileState::set_rules`]. Matching happens down there so
    /// a rule fires on the path the model actually touched.
    pub fn path_rules(&self) -> Vec<harness_tools::PathRule> {
        self.scoped()
            .map(|f| harness_tools::PathRule::new(&f.display, &f.body, &f.globs))
            .collect()
    }

    /// The prompt section: always-apply bodies (budgeted) plus a one-line
    /// mention of every scoped file. Empty when nothing was discovered, so the
    /// prompt is unchanged for a repository that carries no conventions.
    pub fn prompt_section(&self) -> String {
        if self.files.is_empty() {
            return String::new();
        }

        // Budget by significance (nearest scope first), render by generality
        // (global first) so the most specific instructions are read last.
        let mut budgeted: Vec<&ContextFile> = self.always_apply().collect();
        budgeted.sort_by_key(|f| std::cmp::Reverse(f.scope));
        let mut spent = 0usize;
        let mut included: Vec<&ContextFile> = Vec::new();
        let mut mentioned: Vec<&ContextFile> = Vec::new();
        for file in budgeted {
            // Chars, not bytes: the per-file clip is in chars, so measuring
            // the total in bytes would demote a CJK file that fits.
            let cost = file.body.chars().count();
            if spent + cost <= MAX_TOTAL_CHARS {
                spent += cost;
                included.push(file);
            } else {
                mentioned.push(file);
            }
        }
        included.sort_by(|a, b| a.scope.cmp(&b.scope).then(a.display.cmp(&b.display)));

        let mut section = String::from(
            "\n\nProject conventions:\n\
             The files below are this project's own instructions, committed to the \
             repository. Treat them as standing user instruction here — follow them \
             as you would a direct request, and say so if the user asks for \
             something that contradicts them.",
        );
        for file in &included {
            section.push_str(&format!(
                "\n\n--- {} ({}) ---\n{}",
                file.display,
                file.scope.label(),
                file.body.trim_end()
            ));
            if file.truncated {
                section.push_str(&format!(
                    "\n… [clipped — `read_file {}` for the full text]",
                    file.display
                ));
            }
        }
        if !mentioned.is_empty() {
            section
                .push_str("\n\nAlso present but too large to include — read them if relevant:\n");
            for file in mentioned {
                section.push_str(&format!("- {}\n", file.display));
            }
        }
        let scoped: Vec<&ContextFile> = self.scoped().collect();
        if !scoped.is_empty() {
            section.push_str(
                "\n\nPath-scoped conventions (their rules are surfaced when you \
                 read or edit a matching file):\n",
            );
            for file in scoped {
                let summary = if file.description.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", file.description)
                };
                section.push_str(&format!(
                    "- {} (applies to {}){}\n",
                    file.display,
                    file.globs.join(", "),
                    summary
                ));
            }
        }
        section
    }
}

/// Discover every context file that applies to a workspace: the user's global
/// `AGENTS.md`, the repository's own root files, nested `AGENTS.md` files, and
/// the rule files other agents leave behind (Cursor, Cline, Copilot).
///
/// Unreadable or empty files are skipped silently — a broken convention file
/// must never stop a session from starting.
pub fn discover(root: &Path) -> ContextFiles {
    let mut files = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();

    let push = |files: &mut Vec<ContextFile>, seen: &mut Vec<PathBuf>, file: ContextFile| {
        let key = file.path.canonicalize().unwrap_or(file.path.clone());
        if seen.contains(&key) {
            return;
        }
        seen.push(key);
        files.push(file);
    };

    // 1. The user's own instructions, applied in every project.
    if let Some(base) = harness_config::paths::base_dir_unchecked() {
        for name in ["AGENTS.md", "CLAUDE.md"] {
            let path = base.join(name);
            if let Some(file) = load(
                &path,
                Scope::Global,
                format!("~/.oxen-harness/{name}"),
                None,
            ) {
                push(&mut files, &mut seen, file);
            }
        }
    }

    // 2. The repository's own root conventions.
    for name in [
        "AGENTS.md",
        "CLAUDE.md",
        ".oxen-harness/AGENTS.md",
        ".github/copilot-instructions.md",
    ] {
        let path = root.join(name);
        if let Some(file) = load(&path, Scope::Repo, name.to_string(), None) {
            push(&mut files, &mut seen, file);
        }
    }

    // 3. Cline rules: either a single file or a directory of them.
    let clinerules = root.join(".clinerules");
    if clinerules.is_file() {
        if let Some(file) = load(&clinerules, Scope::Repo, ".clinerules".into(), None) {
            push(&mut files, &mut seen, file);
        }
    } else if clinerules.is_dir() {
        for entry in read_dir_sorted(&clinerules) {
            let display = format!(".clinerules/{}", file_name(&entry));
            if let Some(file) = load(&entry, Scope::Repo, display, None) {
                push(&mut files, &mut seen, file);
            }
        }
    }

    // 4. Cursor rules — frontmatter carries `globs:` and `alwaysApply:`.
    let cursor = root.join(".cursor/rules");
    if cursor.is_dir() {
        for entry in read_dir_sorted(&cursor) {
            let display = format!(".cursor/rules/{}", file_name(&entry));
            if let Some(file) = load(&entry, Scope::Repo, display, None) {
                push(&mut files, &mut seen, file);
            }
        }
    }

    // 5. Nested `AGENTS.md`/`CLAUDE.md`, each governing its own subtree.
    for (path, rel_dir) in nested_context_files(root) {
        let display = rel_dir
            .join(file_name(&path))
            .to_string_lossy()
            .replace('\\', "/");
        // Escaped: a directory named `[slug]` or `{a,b}` is a literal path
        // here, but glob syntax to the matcher — unescaped it would match the
        // wrong files, or fail to compile and silently govern nothing.
        let subtree = format!(
            "{}/**",
            globset::escape(&rel_dir.to_string_lossy().replace('\\', "/"))
        );
        if let Some(file) = load(&path, Scope::Nested, display, Some(vec![subtree])) {
            push(&mut files, &mut seen, file);
        }
    }

    ContextFiles { files }
}

/// Read one candidate file into a [`ContextFile`], parsing optional
/// frontmatter. `forced_globs` overrides whatever the frontmatter says — it's
/// how a nested `AGENTS.md` is pinned to its own subtree.
fn load(
    path: &Path,
    scope: Scope,
    display: String,
    forced_globs: Option<Vec<String>>,
) -> Option<ContextFile> {
    let raw = std::fs::read_to_string(path).ok()?;
    let (front, body) = split_frontmatter(&raw);
    let body = body.trim();
    if body.is_empty() {
        return None;
    }

    let mut globs = forced_globs.unwrap_or_else(|| front.globs.clone());
    // Cursor's `alwaysApply: true` means "prompt-resident regardless of globs".
    if front.always_apply {
        globs.clear();
    }
    globs.retain(|g| !g.trim().is_empty());

    let truncated = body.chars().count() > MAX_FILE_CHARS;
    let clipped: String = if truncated {
        body.chars().take(MAX_FILE_CHARS).collect()
    } else {
        body.to_string()
    };

    let matcher = if globs.is_empty() {
        None
    } else {
        compile_globs(&globs)
    };

    Some(ContextFile {
        path: path.to_path_buf(),
        display,
        scope,
        body: clipped,
        globs,
        description: front.description,
        truncated,
        matcher,
    })
}

/// Build a matcher for a rule's globs. A bare `*.ts` is also matched against
/// the file name at any depth (that's what Cursor users mean by it), so the
/// set gets a `**/` variant alongside the literal pattern.
fn compile_globs(globs: &[String]) -> Option<GlobSet> {
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

/// Frontmatter fields we understand. Everything else is tolerated and ignored.
#[derive(Debug, Default)]
struct Frontmatter {
    globs: Vec<String>,
    description: String,
    always_apply: bool,
}

/// Split optional `---` frontmatter off the head of a markdown file. Files
/// without frontmatter (the common `AGENTS.md`) come back unchanged.
fn split_frontmatter(raw: &str) -> (Frontmatter, &str) {
    let Some(rest) = raw.strip_prefix("---") else {
        return (Frontmatter::default(), raw);
    };
    let Some((front, body)) = rest.split_once("\n---") else {
        return (Frontmatter::default(), raw);
    };

    let mut parsed = Frontmatter::default();
    for line in front.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "globs" => parsed.globs = parse_glob_list(value),
            "description" => parsed.description = value.to_string(),
            "alwaysApply" | "always_apply" => parsed.always_apply = value == "true",
            _ => {}
        }
    }
    (parsed, body.trim_start_matches('-'))
}

/// Accept the three shapes seen in the wild: `["a", "b"]`, `a, b`, and `a`.
fn parse_glob_list(value: &str) -> Vec<String> {
    value
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(|part| part.trim().trim_matches(['"', '\'']).to_string())
        .filter(|part| !part.is_empty())
        .collect()
}

/// Walk the workspace for `AGENTS.md`/`CLAUDE.md` below the root, bounded in
/// depth and skipping the directories no project keeps conventions in.
fn nested_context_files(root: &Path) -> Vec<(PathBuf, PathBuf)> {
    let mut found = Vec::new();
    let mut stack = vec![(root.to_path_buf(), PathBuf::new(), 0usize)];
    while let Some((dir, rel, depth)) = stack.pop() {
        for entry in read_dir_sorted(&dir) {
            let name = file_name(&entry);
            if entry.is_dir() {
                if depth >= MAX_NESTED_DEPTH || name.starts_with('.') || SKIP_DIRS.contains(&&*name)
                {
                    continue;
                }
                stack.push((entry.clone(), rel.join(&name), depth + 1));
            } else if depth > 0 && (name == "AGENTS.md" || name == "CLAUDE.md") {
                found.push((entry.clone(), rel.clone()));
            }
        }
    }
    found.sort();
    found
}

/// Directory entries in a stable order (readdir order is filesystem-dependent,
/// and the prompt must be byte-identical across runs for the cache prefix).
fn read_dir_sorted(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    paths
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    /// Discover against an isolated `~/.oxen-harness`. Without this, a
    /// developer's own global `AGENTS.md` would leak into every assertion here
    /// (and pass locally while failing in CI, or the reverse).
    fn discover_isolated(root: &Path) -> ContextFiles {
        crate::with_temp_home(|| discover(root))
    }

    fn write(root: &Path, rel: &str, contents: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn a_repo_without_conventions_contributes_nothing() {
        let tmp = workspace();
        let found = discover_isolated(tmp.path());
        assert!(found.files.is_empty());
        assert!(found.prompt_section().is_empty());
    }

    #[test]
    fn root_agents_md_becomes_a_prompt_section() {
        let tmp = workspace();
        write(
            tmp.path(),
            "AGENTS.md",
            "Run `cargo nextest run` before done.",
        );

        let section = discover_isolated(tmp.path()).prompt_section();

        assert!(section.contains("Project conventions"));
        assert!(section.contains("AGENTS.md (repo)"));
        assert!(section.contains("cargo nextest run"));
    }

    #[test]
    fn nested_agents_md_is_scoped_to_its_subtree_not_the_prompt() {
        let tmp = workspace();
        write(tmp.path(), "AGENTS.md", "Root rules.");
        write(tmp.path(), "app/AGENTS.md", "Frontend rules.");

        let found = discover_isolated(tmp.path());
        let section = found.prompt_section();

        assert!(section.contains("Root rules."));
        // The nested file's body stays out of the prompt...
        assert!(!section.contains("Frontend rules."));
        // ...but it's advertised, and it fires for a path inside its subtree.
        assert!(section.contains("app/AGENTS.md (applies to app/**)"));
        let hits = found.rules_for(Path::new("app/src/main.tsx"));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].body, "Frontend rules.");
        assert!(found.rules_for(Path::new("crates/lib.rs")).is_empty());
    }

    #[test]
    fn cursor_rules_carry_globs_and_always_apply() {
        let tmp = workspace();
        write(
            tmp.path(),
            ".cursor/rules/api.mdc",
            "---\ndescription: REST conventions\nglobs: [\"src/api/**\"]\n---\nUse camelCase.",
        );
        write(
            tmp.path(),
            ".cursor/rules/global.mdc",
            "---\ndescription: House style\nglobs: *.ts\nalwaysApply: true\n---\nNo default exports.",
        );

        let found = discover_isolated(tmp.path());
        let section = found.prompt_section();

        // alwaysApply wins over the glob list and lands in the prompt.
        assert!(section.contains("No default exports."));
        // The scoped rule is listed, not inlined.
        assert!(!section.contains("Use camelCase."));
        assert!(section.contains("REST conventions"));
        let hits = found.rules_for(Path::new("src/api/users.ts"));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].body, "Use camelCase.");
    }

    #[test]
    fn a_bare_extension_glob_matches_at_any_depth() {
        let tmp = workspace();
        write(
            tmp.path(),
            ".cursor/rules/ts.mdc",
            "---\nglobs: *.ts\n---\nStrict mode only.",
        );

        let found = discover_isolated(tmp.path());

        assert_eq!(found.rules_for(Path::new("deep/nested/file.ts")).len(), 1);
        assert!(found.rules_for(Path::new("deep/file.rs")).is_empty());
    }

    #[test]
    fn an_oversized_file_is_clipped_with_a_pointer_to_the_rest() {
        let tmp = workspace();
        write(tmp.path(), "AGENTS.md", &"x".repeat(MAX_FILE_CHARS + 500));

        let found = discover_isolated(tmp.path());
        let file = found.always_apply().next().unwrap();

        assert!(file.truncated);
        assert_eq!(file.body.chars().count(), MAX_FILE_CHARS);
        assert!(found.prompt_section().contains("clipped"));
    }

    #[test]
    fn the_total_budget_degrades_the_least_specific_files_to_a_mention() {
        let tmp = workspace();
        // Two root files, each within the per-file cap but together over the
        // total. CLAUDE.md sorts second, so AGENTS.md keeps its body.
        let big = "y".repeat(MAX_FILE_CHARS);
        write(tmp.path(), "AGENTS.md", &big);
        write(tmp.path(), "CLAUDE.md", &big);
        write(tmp.path(), ".oxen-harness/AGENTS.md", &big);

        let section = discover_isolated(tmp.path()).prompt_section();

        assert!(section.contains("too large to include"));
        assert!(section.len() < MAX_TOTAL_CHARS + 4_000);
    }

    #[test]
    fn an_empty_convention_file_is_ignored() {
        let tmp = workspace();
        write(
            tmp.path(),
            "AGENTS.md",
            "---\ndescription: nothing\n---\n\n",
        );
        assert!(discover_isolated(tmp.path()).files.is_empty());
    }

    #[test]
    fn the_users_global_instructions_apply_in_every_project() {
        let tmp = workspace();
        write(tmp.path(), "AGENTS.md", "Repo rules.");

        let section = crate::with_temp_home(|| {
            let home = harness_config::paths::base_dir().unwrap();
            std::fs::write(home.join("AGENTS.md"), "Always answer in metric units.").unwrap();
            discover(tmp.path()).prompt_section()
        });

        assert!(section.contains("Always answer in metric units."));
        // General to specific: the user's standing instructions are read first,
        // the repository's own conventions last.
        let user_at = section.find("metric units").unwrap();
        let repo_at = section.find("Repo rules.").unwrap();
        assert!(
            user_at < repo_at,
            "global section must precede repo section"
        );
    }

    #[test]
    fn a_directory_whose_name_looks_like_a_glob_still_governs_its_subtree() {
        let tmp = workspace();
        // Next.js route groups are the common case: `[slug]` is a literal
        // directory, and a character class to a glob matcher.
        write(tmp.path(), "app/[slug]/AGENTS.md", "Route rules.");

        let found = discover_isolated(tmp.path());

        assert_eq!(
            found.rules_for(Path::new("app/[slug]/page.tsx")).len(),
            1,
            "the nested file should govern its own directory"
        );
    }

    #[test]
    fn the_budget_counts_characters_the_way_the_clip_does() {
        let tmp = workspace();
        // 12k CJK characters: well inside the per-file clip, ~36KB of bytes.
        write(tmp.path(), "AGENTS.md", &"文".repeat(12_000));

        let section = discover_isolated(tmp.path()).prompt_section();

        assert!(section.contains('文'), "the file should be in the prompt");
        assert!(!section.contains("too large to include"), "{section}");
    }

    #[test]
    fn discovery_is_byte_stable_across_runs() {
        // The section rides in the cached prompt prefix; a reordering between
        // two constructions would silently invalidate the cache every session.
        let tmp = workspace();
        write(tmp.path(), "AGENTS.md", "Root.");
        write(tmp.path(), "app/AGENTS.md", "App.");
        write(tmp.path(), "docs/AGENTS.md", "Docs.");
        write(
            tmp.path(),
            ".cursor/rules/a.mdc",
            "---\nglobs: a/**\n---\nA.",
        );
        write(
            tmp.path(),
            ".cursor/rules/b.mdc",
            "---\nglobs: b/**\n---\nB.",
        );

        let first = discover_isolated(tmp.path()).prompt_section();
        let second = discover_isolated(tmp.path()).prompt_section();

        assert_eq!(first, second);
    }
}
