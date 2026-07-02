//! `skill` — load a skill's instructions into the conversation on demand.
//!
//! A **skill** is a reusable set of instructions that teaches the agent how to
//! do something — a workflow, a house style, a domain procedure — written as a
//! `SKILL.md` file in its own directory (the same shape as Claude Code skills):
//!
//! ```markdown
//! ---
//! name: release-notes
//! description: Writes release notes from the git log in our house style.
//! ---
//!
//! # Writing release notes
//! 1. Run `git log --oneline` since the last tag…
//! ```
//!
//! Skills follow the *progressive disclosure* pattern: only each skill's name
//! and one-line description are advertised to the model (inside this tool's
//! description), costing a few tokens per skill. When the model decides a skill
//! applies, it calls `skill` with the name and receives the full `SKILL.md`
//! body as the tool result — the instructions enter context only when needed.
//! A skill's directory can hold supporting files (templates, scripts, examples)
//! the instructions reference; the tool result tells the model where they live.
//!
//! Discovery, persistence, and enable/disable preferences are host concerns
//! (see `harness-runtime`'s `skills` module); this module owns the data model,
//! `SKILL.md` parsing, and the model-facing tool.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{ToolError, TypedTool};

/// The tool name the model calls to load a skill.
pub const SKILL_TOOL: &str = "skill";

/// Where a skill was discovered — shown in UIs so users know what edits affect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillScope {
    /// `~/.oxen-harness/skills/` — available in every project.
    Global,
    /// `<workspace>/.oxen-harness/skills/` — travels with the repository.
    Project,
}

/// One loaded skill: its identity, trigger description, and full instructions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    /// Stable identifier the model passes to the `skill` tool (the directory
    /// name, e.g. `release-notes`).
    pub name: String,
    /// One-line "when to use this" trigger, advertised to the model.
    pub description: String,
    /// The full `SKILL.md` body — the instructions loaded on invocation.
    pub instructions: String,
    /// The skill's directory, for supporting files the instructions reference.
    pub dir: PathBuf,
    /// Where the skill was discovered.
    pub scope: SkillScope,
}

/// A valid skill name: lowercase letters, digits, and hyphens (Claude Code's
/// convention), e.g. `release-notes`.
pub fn is_valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
        && !name.ends_with('-')
}

/// Parse a `SKILL.md` file: YAML-style `---` frontmatter with `name:` and
/// `description:`, followed by the markdown instructions. Returns a
/// human-readable error for malformed files so UIs can surface it.
pub fn parse_skill_md(contents: &str, dir: &Path, scope: SkillScope) -> Result<Skill, String> {
    let dir_name = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    let rest = contents
        .strip_prefix("---")
        .ok_or("SKILL.md must start with `---` frontmatter")?;
    let (frontmatter, body) = rest
        .split_once("\n---")
        .ok_or("frontmatter is missing its closing `---`")?;

    let mut name = None;
    let mut description = None;
    for line in frontmatter.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        match key.trim() {
            "name" => name = Some(value.trim().to_string()),
            "description" => description = Some(value.trim().to_string()),
            _ => {} // tolerate extra keys (version, license, …)
        }
    }

    // The directory is the identity; a frontmatter `name` must agree so a
    // renamed folder can't silently advertise a stale name.
    let name = name
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| dir_name.clone());
    if !is_valid_skill_name(&name) {
        return Err(format!(
            "invalid skill name {name:?}: use lowercase letters, digits, and hyphens"
        ));
    }
    let description = description.filter(|d| !d.is_empty()).ok_or(
        "frontmatter needs a `description:` — it's how the model decides when to use the skill",
    )?;
    let instructions = body.trim_start_matches('-').trim().to_string();
    if instructions.is_empty() {
        return Err("SKILL.md has no instructions after the frontmatter".into());
    }

    Ok(Skill {
        name,
        description,
        instructions,
        dir: dir.to_path_buf(),
        scope,
    })
}

/// Load every well-formed skill under `dir` (one subdirectory per skill, each
/// holding a `SKILL.md`). Malformed skills are skipped, not fatal — one broken
/// file shouldn't take down the set.
pub fn load_skills_dir(dir: &Path, scope: SkillScope) -> Vec<Skill> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new(); // no directory yet — no skills
    };
    let mut skills: Vec<Skill> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let md = e.path().join("SKILL.md");
            let contents = std::fs::read_to_string(&md).ok()?;
            parse_skill_md(&contents, &e.path(), scope).ok()
        })
        .collect();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// Arguments to `skill`.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct SkillArgs {
    /// The name of the skill to load, exactly as listed.
    pub name: String,
}

/// The model-facing tool: advertises the available skills (name + description)
/// and returns a skill's full instructions when invoked.
pub struct SkillTool {
    skills: Vec<Skill>,
    /// Built once at construction — `description()` returns a borrowed str.
    description: String,
}

impl SkillTool {
    pub fn new(skills: Vec<Skill>) -> Self {
        let mut description = String::from(
            "Load a skill — reusable instructions for a specific kind of task. \
             When a request matches a skill's description, call this FIRST with \
             that skill's name and follow the returned instructions. Available \
             skills:",
        );
        for s in &skills {
            description.push_str(&format!("\n- {}: {}", s.name, s.description));
        }
        Self {
            skills,
            description,
        }
    }

    /// Whether any skills were loaded — hosts skip registering an empty tool.
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }
}

#[async_trait]
impl TypedTool for SkillTool {
    const NAME: &'static str = SKILL_TOOL;
    type Args = SkillArgs;

    fn description(&self) -> &str {
        &self.description
    }

    async fn run(&self, args: SkillArgs) -> Result<String, ToolError> {
        let Some(skill) = self.skills.iter().find(|s| s.name == args.name) else {
            let known = self
                .skills
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(ToolError::InvalidArguments(format!(
                "unknown skill `{}`; available: {known}",
                args.name
            )));
        };
        Ok(format!(
            "Loaded skill \"{}\". Its supporting files (if any) live in {} — read \
             them with run_shell if the instructions reference them.\n\n\
             Follow these instructions for the current task:\n\n{}",
            skill.name,
            skill.dir.display(),
            skill.instructions
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "---\nname: release-notes\ndescription: Writes release notes in our style.\n---\n\n# Steps\n1. Look at the log.\n";

    fn dir() -> PathBuf {
        PathBuf::from("/tmp/skills/release-notes")
    }

    #[test]
    fn parses_frontmatter_and_body() {
        let skill = parse_skill_md(GOOD, &dir(), SkillScope::Global).unwrap();
        assert_eq!(skill.name, "release-notes");
        assert_eq!(skill.description, "Writes release notes in our style.");
        assert!(skill.instructions.starts_with("# Steps"));
        assert_eq!(skill.scope, SkillScope::Global);
    }

    #[test]
    fn name_defaults_to_directory() {
        let md = "---\ndescription: Does a thing.\n---\nBody.";
        let skill = parse_skill_md(md, &dir(), SkillScope::Project).unwrap();
        assert_eq!(skill.name, "release-notes");
    }

    #[test]
    fn rejects_missing_description_frontmatter_or_body() {
        assert!(parse_skill_md("---\nname: x\n---\nBody.", &dir(), SkillScope::Global).is_err());
        assert!(parse_skill_md("no frontmatter", &dir(), SkillScope::Global).is_err());
        assert!(
            parse_skill_md("---\ndescription: d\n---\n   ", &dir(), SkillScope::Global).is_err()
        );
    }

    #[test]
    fn rejects_bad_names() {
        let md = "---\nname: Bad Name\ndescription: d\n---\nBody.";
        assert!(parse_skill_md(md, &dir(), SkillScope::Global).is_err());
        assert!(is_valid_skill_name("release-notes"));
        assert!(!is_valid_skill_name("-leading"));
        assert!(!is_valid_skill_name(""));
    }

    #[test]
    fn loads_skills_from_directories_and_skips_broken_ones() {
        let tmp = tempfile::tempdir().unwrap();
        let good = tmp.path().join("good-skill");
        std::fs::create_dir_all(&good).unwrap();
        std::fs::write(
            good.join("SKILL.md"),
            "---\ndescription: A good skill.\n---\nDo it well.",
        )
        .unwrap();
        let broken = tmp.path().join("broken");
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(broken.join("SKILL.md"), "no frontmatter at all").unwrap();

        let skills = load_skills_dir(tmp.path(), SkillScope::Global);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "good-skill");

        // A missing directory is simply empty.
        assert!(load_skills_dir(&tmp.path().join("nope"), SkillScope::Global).is_empty());
    }

    #[tokio::test]
    async fn tool_advertises_and_loads_skills() {
        let skill = parse_skill_md(GOOD, &dir(), SkillScope::Global).unwrap();
        let tool = SkillTool::new(vec![skill]);
        assert!(!tool.is_empty());
        assert!(tool
            .description()
            .contains("release-notes: Writes release notes"));

        let out = tool
            .invoke(serde_json::json!({"name": "release-notes"}))
            .await
            .unwrap();
        assert!(out.contains("# Steps"), "out: {out}");
        assert!(out.contains("/tmp/skills/release-notes"), "out: {out}");

        let err = tool
            .invoke(serde_json::json!({"name": "nope"}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
    }
}
