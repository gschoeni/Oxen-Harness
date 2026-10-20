//! Skill discovery, preferences, and authoring, shared by the CLI and desktop.
//!
//! Skills (see `harness_tools::skill`) are `SKILL.md` files that teach the
//! agent reusable workflows. They live in two places: globally in
//! `~/.oxen-harness/skills/` and per-project in `<workspace>/.oxen-harness/skills/`
//! (a project skill shadows a global one with the same name, so repos can
//! specialize). This module discovers both sets, applies the user's
//! enable/disable preferences (`~/.oxen-harness/skills.json`), builds the
//! ready-to-register `skill` tool, and lets UIs create/edit/delete skills.
//!
//! Like tool preferences, changes apply when an agent is built — new and
//! resumed chats, not the live one.

use std::path::{Path, PathBuf};

use harness_config::paths;
use harness_tools::skill::{is_valid_skill_name, load_skills_dir, Skill, SkillScope, SkillTool};
use serde::{Deserialize, Serialize};

use crate::RuntimeError;

/// Schema version for `skills.json`.
pub const SCHEMA_VERSION: u32 = 1;

/// Persisted skill preferences. Empty by default — every discovered skill
/// starts enabled.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SkillPrefs {
    /// Skill names the user has turned off.
    #[serde(default)]
    pub disabled: Vec<String>,
}

impl SkillPrefs {
    pub fn is_enabled(&self, name: &str) -> bool {
        !self.disabled.iter().any(|n| n == name)
    }

    /// Turn a skill on or off, returning whether anything changed.
    pub fn set_enabled(&mut self, name: &str, enabled: bool) -> bool {
        let present = self.disabled.iter().any(|n| n == name);
        match (enabled, present) {
            (true, true) => {
                self.disabled.retain(|n| n != name);
                true
            }
            (false, false) => {
                self.disabled.push(name.to_string());
                true
            }
            _ => false,
        }
    }
}

/// Read the saved skill preferences (defaults to "everything enabled").
pub fn load() -> SkillPrefs {
    crate::config::load_or_default(paths::skills_file())
}

/// Atomically persist the skill preferences and snapshot the config repo.
pub fn save(prefs: &SkillPrefs) -> Result<(), RuntimeError> {
    crate::config::write_and_snapshot(
        &paths::skills_file()?,
        SCHEMA_VERSION,
        prefs,
        "Update skill preferences",
    )
}

/// The project-scoped skills directory for a workspace.
pub fn project_skills_dir(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".oxen-harness").join("skills")
}

/// Discover every skill visible from `workspace_root`: global skills plus the
/// project's own, with a project skill shadowing a global one of the same name.
pub fn discover(workspace_root: &Path) -> Vec<Skill> {
    let mut skills = match paths::skills_dir() {
        Ok(dir) => load_skills_dir(&dir, SkillScope::Global),
        Err(_) => Vec::new(),
    };
    for project in load_skills_dir(&project_skills_dir(workspace_root), SkillScope::Project) {
        skills.retain(|s| s.name != project.name);
        skills.push(project);
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// Build the `skill` tool for a workspace: discovered skills minus disabled
/// ones and drafts. Returns `None` when nothing is enabled, so hosts don't advertise an
/// empty tool.
pub fn enabled_tool(workspace_root: &Path) -> Option<SkillTool> {
    let prefs = load();
    let skills: Vec<Skill> = discover(workspace_root)
        .into_iter()
        .filter(|s| prefs.is_enabled(&s.name) && !s.is_draft())
        .collect();
    if skills.is_empty() {
        None
    } else {
        Some(SkillTool::new(skills))
    }
}

/// One skill as shown on the Skills settings page.
#[derive(Debug, Clone, Serialize)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub instructions: String,
    pub scope: SkillScope,
    /// The skill's directory (so UIs can point users at supporting files).
    pub dir: String,
    pub enabled: bool,
    /// Still being written (no description or instructions yet), so it is
    /// not offered to the model.
    pub draft: bool,
}

/// Enumerate every discovered skill with its enabled state, for the settings
/// page. Disabled skills still appear (toggled off) rather than vanishing.
pub fn list(workspace_root: &Path, prefs: &SkillPrefs) -> Vec<SkillInfo> {
    discover(workspace_root)
        .into_iter()
        .map(|s| SkillInfo {
            enabled: prefs.is_enabled(&s.name),
            draft: s.is_draft(),
            name: s.name,
            description: s.description,
            instructions: s.instructions,
            scope: s.scope,
            dir: s.dir.display().to_string(),
        })
        .collect()
}

/// The directory a skill of `scope` lives in (global config dir or the
/// project's `.oxen-harness/skills/`).
fn scope_dir(workspace_root: &Path, scope: SkillScope) -> Result<PathBuf, RuntimeError> {
    match scope {
        SkillScope::Global => Ok(paths::skills_dir()?),
        SkillScope::Project => Ok(project_skills_dir(workspace_root)),
    }
}

fn check_name(name: &str) -> Result<(), RuntimeError> {
    if is_valid_skill_name(name) {
        Ok(())
    } else {
        Err(RuntimeError::Invalid(
            "Use a name like `release-notes` — lowercase letters, digits, and hyphens.".into(),
        ))
    }
}

fn scope_label(scope: SkillScope) -> &'static str {
    match scope {
        SkillScope::Global => "every project",
        SkillScope::Project => "this project",
    }
}

/// Where a new skill named `name` would go, refusing to clobber one already
/// there.
fn vacant_skill_dir(
    workspace_root: &Path,
    scope: SkillScope,
    name: &str,
) -> Result<PathBuf, RuntimeError> {
    check_name(name)?;
    let dir = scope_dir(workspace_root, scope)?.join(name);
    if dir.exists() {
        return Err(RuntimeError::Invalid(format!(
            "A skill named `{name}` already exists in {}.",
            scope_label(scope)
        )));
    }
    Ok(dir)
}

/// Start a new skill: an empty draft `SKILL.md` that later saves fill in.
/// Unlike [`save_skill`], refuses to overwrite an existing skill.
pub fn create_skill(
    workspace_root: &Path,
    scope: SkillScope,
    name: &str,
) -> Result<(), RuntimeError> {
    let name = name.trim();
    vacant_skill_dir(workspace_root, scope, name)?;
    save_skill(workspace_root, scope, name, "", "")
}

/// Create or update a skill: writes `<dir>/<name>/SKILL.md` with the standard
/// frontmatter. An empty description or instructions saves a draft, so an
/// editor can persist every keystroke. Returns messages UIs can show inline.
pub fn save_skill(
    workspace_root: &Path,
    scope: SkillScope,
    name: &str,
    description: &str,
    instructions: &str,
) -> Result<(), RuntimeError> {
    let name = name.trim();
    check_name(name)?;
    // The description is one frontmatter line; a stray newline would end it.
    let description = description.split_whitespace().collect::<Vec<_>>().join(" ");
    let instructions = instructions.trim();

    let dir = scope_dir(workspace_root, scope)?.join(name);
    std::fs::create_dir_all(&dir).map_err(RuntimeError::Io)?;
    let md = format!("---\nname: {name}\ndescription: {description}\n---\n\n{instructions}\n");
    std::fs::write(dir.join("SKILL.md"), md).map_err(RuntimeError::Io)?;
    Ok(())
}

/// Rename a skill and/or move it between scopes. The whole directory moves,
/// so supporting files beside `SKILL.md` come along, and a disabled skill
/// stays disabled under its new name. Refuses to overwrite an existing skill.
pub fn rename_skill(
    workspace_root: &Path,
    from: (SkillScope, &str),
    to: (SkillScope, &str),
) -> Result<(), RuntimeError> {
    let (from_scope, from_name) = from;
    let (to_scope, to_name) = (to.0, to.1.trim());
    if (from_scope, from_name) == (to_scope, to_name) {
        return Ok(());
    }
    check_name(from_name)?;
    let src = scope_dir(workspace_root, from_scope)?.join(from_name);
    let contents = std::fs::read_to_string(src.join("SKILL.md")).map_err(|e| {
        RuntimeError::Io(std::io::Error::new(
            e.kind(),
            format!("reading skill `{from_name}` at {}: {e}", src.display()),
        ))
    })?;
    let skill = harness_tools::skill::parse_skill_md(&contents, &src, from_scope)
        .map_err(|e| RuntimeError::Invalid(format!("skill `{from_name}`: {e}")))?;
    let dest = vacant_skill_dir(workspace_root, to_scope, to_name)?;

    move_dir(&src, &dest)?;
    // The frontmatter carries the name too; keep it in step with the folder.
    save_skill(
        workspace_root,
        to_scope,
        to_name,
        &skill.description,
        &skill.instructions,
    )?;

    if from_name != to_name {
        let mut prefs = load();
        if !prefs.is_enabled(from_name) {
            prefs.set_enabled(from_name, true);
            prefs.set_enabled(to_name, false);
            save(&prefs)?;
        }
    }
    Ok(())
}

/// Move a directory, falling back to copy-then-delete when `rename` can't
/// cross filesystems (a global skill moving into a repo on another volume).
fn move_dir(src: &Path, dest: &Path) -> Result<(), RuntimeError> {
    let context = |e: std::io::Error| {
        RuntimeError::Io(std::io::Error::new(
            e.kind(),
            format!("moving {} to {}: {e}", src.display(), dest.display()),
        ))
    };
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(context)?;
    }
    match std::fs::rename(src, dest) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => {
            copy_dir(src, dest).map_err(context)?;
            std::fs::remove_dir_all(src).map_err(context)
        }
        Err(e) => Err(context(e)),
    }
}

fn copy_dir(src: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let target = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Delete a skill's directory (its `SKILL.md` and any supporting files) and
/// drop it from the disabled list.
pub fn delete_skill(
    workspace_root: &Path,
    scope: SkillScope,
    name: &str,
) -> Result<(), RuntimeError> {
    if !is_valid_skill_name(name) {
        return Err(RuntimeError::Invalid(format!("unknown skill `{name}`")));
    }
    let dir = scope_dir(workspace_root, scope)?.join(name);
    if dir.is_dir() {
        std::fs::remove_dir_all(&dir).map_err(RuntimeError::Io)?;
    }
    let mut prefs = load();
    if prefs.set_enabled(name, true) {
        save(&prefs)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_tools::TypedTool;

    #[test]
    fn prefs_toggle_round_trips() {
        let mut prefs = SkillPrefs::default();
        assert!(prefs.is_enabled("release-notes"));
        assert!(prefs.set_enabled("release-notes", false));
        assert!(!prefs.is_enabled("release-notes"));
        assert!(!prefs.set_enabled("release-notes", false));
        assert!(prefs.set_enabled("release-notes", true));
    }

    #[test]
    fn save_skill_writes_loadable_skill_md() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        save_skill(
            root,
            SkillScope::Project,
            "release-notes",
            "Writes notes.",
            "Do the steps.",
        )
        .unwrap();

        let skills = load_skills_dir(&project_skills_dir(root), SkillScope::Project);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "release-notes");
        assert_eq!(skills[0].description, "Writes notes.");
        assert_eq!(skills[0].instructions, "Do the steps.");
    }

    #[test]
    fn a_project_skill_shadows_a_global_one() {
        crate::with_temp_home(|| {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            save_skill(
                root,
                SkillScope::Global,
                "release-notes",
                "Global take.",
                "g",
            )
            .unwrap();
            save_skill(
                root,
                SkillScope::Global,
                "only-global",
                "Stays visible.",
                "g",
            )
            .unwrap();
            save_skill(
                root,
                SkillScope::Project,
                "release-notes",
                "Project take.",
                "p",
            )
            .unwrap();

            let skills = discover(root);
            assert_eq!(skills.len(), 2);
            let notes = skills.iter().find(|s| s.name == "release-notes").unwrap();
            assert_eq!(notes.description, "Project take.");
            assert_eq!(notes.scope, SkillScope::Project);
        });
    }

    #[test]
    fn save_skill_validates_the_name_and_keeps_drafts() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert!(save_skill(root, SkillScope::Project, "Bad Name", "d", "i").is_err());

        // Half-written skills save (autosave persists every keystroke) and
        // come back as drafts.
        save_skill(root, SkillScope::Project, "wip", " ", "Step one.").unwrap();
        let skills = load_skills_dir(&project_skills_dir(root), SkillScope::Project);
        assert_eq!(skills[0].instructions, "Step one.");
        assert!(skills[0].is_draft());
    }

    #[test]
    fn save_skill_keeps_the_description_on_one_line() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        save_skill(root, SkillScope::Project, "s", "Two\nlines.", "Body.").unwrap();
        let skills = load_skills_dir(&project_skills_dir(root), SkillScope::Project);
        assert_eq!(skills[0].description, "Two lines.");
        assert_eq!(skills[0].instructions, "Body.");
    }

    #[test]
    fn create_skill_starts_a_draft_and_never_clobbers() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        create_skill(root, SkillScope::Project, "deploy").unwrap();
        let skills = load_skills_dir(&project_skills_dir(root), SkillScope::Project);
        assert_eq!(skills[0].name, "deploy");
        assert!(skills[0].is_draft());

        save_skill(root, SkillScope::Project, "deploy", "Deploys.", "Run it.").unwrap();
        let err = create_skill(root, SkillScope::Project, "deploy").unwrap_err();
        assert!(err.to_string().contains("already exists"), "{err}");
        let skills = load_skills_dir(&project_skills_dir(root), SkillScope::Project);
        assert_eq!(skills[0].instructions, "Run it.");

        assert!(create_skill(root, SkillScope::Project, "Bad Name").is_err());
    }

    #[test]
    fn rename_skill_moves_the_folder_with_its_files_and_toggle() {
        crate::with_temp_home(|| {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            save_skill(root, SkillScope::Global, "old", "Does it.", "Steps.").unwrap();
            let old_dir = paths::skills_dir().unwrap().join("old");
            std::fs::write(old_dir.join("template.md"), "t").unwrap();
            let mut prefs = load();
            prefs.set_enabled("old", false);
            save(&prefs).unwrap();

            rename_skill(
                root,
                (SkillScope::Global, "old"),
                (SkillScope::Project, "new"),
            )
            .unwrap();

            assert!(!old_dir.exists());
            let new_dir = project_skills_dir(root).join("new");
            assert!(new_dir.join("template.md").is_file());
            let skills = load_skills_dir(&project_skills_dir(root), SkillScope::Project);
            assert_eq!(skills[0].name, "new");
            assert_eq!(skills[0].description, "Does it.");
            assert_eq!(skills[0].instructions, "Steps.");
            let prefs = load();
            assert!(!prefs.is_enabled("new"));
            assert!(prefs.is_enabled("old"));
        });
    }

    #[test]
    fn rename_skill_refuses_to_overwrite_another_skill() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        save_skill(root, SkillScope::Project, "a", "A.", "a").unwrap();
        save_skill(root, SkillScope::Project, "b", "B.", "b").unwrap();
        let err =
            rename_skill(root, (SkillScope::Project, "a"), (SkillScope::Project, "b")).unwrap_err();
        assert!(err.to_string().contains("already exists"), "{err}");
        assert!(project_skills_dir(root).join("a").is_dir());
        // Renaming to itself is a no-op.
        rename_skill(root, (SkillScope::Project, "a"), (SkillScope::Project, "a")).unwrap();
    }

    #[test]
    fn disabled_and_draft_skills_are_kept_out_of_the_tool_but_listed_for_the_ui() {
        crate::with_temp_home(|| {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            save_skill(root, SkillScope::Project, "on", "Enabled skill.", "i").unwrap();
            save_skill(root, SkillScope::Project, "off", "Disabled skill.", "i").unwrap();
            let mut prefs = load();
            prefs.set_enabled("off", false);
            save(&prefs).unwrap();

            let tool = enabled_tool(root).expect("one skill is still enabled");
            assert!(tool.description().contains("on:"));
            assert!(!tool.description().contains("off:"));

            save_skill(root, SkillScope::Project, "wip", "", "").unwrap();
            let infos = list(root, &load());
            assert_eq!(infos.len(), 3);
            assert!(infos.iter().any(|s| s.name == "off" && !s.enabled));
            assert!(infos.iter().any(|s| s.name == "wip" && s.draft));
            assert!(!tool.description().contains("wip"));
        });
    }

    #[test]
    fn delete_skill_removes_the_directory() {
        crate::with_temp_home(|| {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            save_skill(root, SkillScope::Project, "gone", "d", "i").unwrap();
            assert!(project_skills_dir(root).join("gone").is_dir());
            delete_skill(root, SkillScope::Project, "gone").unwrap();
            assert!(!project_skills_dir(root).join("gone").exists());
            // Deleting a non-existent skill is a no-op, not an error.
            delete_skill(root, SkillScope::Project, "gone").unwrap();
        });
    }
}
