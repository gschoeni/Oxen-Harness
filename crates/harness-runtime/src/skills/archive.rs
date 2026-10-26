//! Share skills as files: export a skill to a `.zip`, import one from a
//! `.zip`, a `.skill`, or a lone `SKILL.md`.
//!
//! The archive layout is the one claude.ai's "Upload skill" takes: a single
//! top-level folder named after the skill, holding `SKILL.md` and its
//! supporting files (`release-notes.zip` → `release-notes/SKILL.md`). Codex
//! and Claude Code keep skills as that same folder on disk, so an export
//! unzips straight into `~/.codex/skills/` or `.claude/skills/`. Imports also
//! take a `SKILL.md` at the archive root (how `.skill` packages are often
//! built) and the occasional `Skill.md`.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use harness_tools::skill::{parse_skill_md, SkillScope};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use super::{check_name, io_context, scope_dir, vacant_skill_dir};
use crate::RuntimeError;

/// Imports larger than this are refused (a skill is instructions and a few
/// helpers, and a zip can claim any size).
const MAX_IMPORT_BYTES: u64 = 50 * 1024 * 1024;
/// Imports with more files than this are refused.
const MAX_IMPORT_FILES: usize = 2_000;

/// Write the skill `name` (its whole folder) to `dest` as a zip.
pub fn export_skill(
    workspace_root: &Path,
    scope: SkillScope,
    name: &str,
    dest: &Path,
) -> Result<(), RuntimeError> {
    check_name(name)?;
    let dir = scope_dir(workspace_root, scope)?.join(name);
    if !dir.join("SKILL.md").is_file() {
        return Err(RuntimeError::Invalid(format!(
            "No skill named `{name}` to export."
        )));
    }
    let io_context = |e: std::io::Error| {
        RuntimeError::Io(std::io::Error::new(
            e.kind(),
            format!("exporting skill `{name}` to {}: {e}", dest.display()),
        ))
    };
    let zip_context = |e: zip::result::ZipError| {
        RuntimeError::Invalid(format!(
            "exporting skill `{name}` to {}: {e}",
            dest.display()
        ))
    };

    let file = File::create(dest).map_err(io_context)?;
    let mut zip = ZipWriter::new(file);
    add_dir(&mut zip, &dir, name).map_err(|e| match e {
        ArchiveError::Io(e) => io_context(e),
        ArchiveError::Zip(e) => zip_context(e),
    })?;
    zip.finish().map_err(zip_context)?;
    Ok(())
}

enum ArchiveError {
    Io(std::io::Error),
    Zip(zip::result::ZipError),
}

impl From<std::io::Error> for ArchiveError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<zip::result::ZipError> for ArchiveError {
    fn from(e: zip::result::ZipError) -> Self {
        Self::Zip(e)
    }
}

/// Add `dir` to the archive under `prefix/`. Symlinks are skipped, so an
/// export never reaches outside the skill's folder.
fn add_dir(zip: &mut ZipWriter<File>, dir: &Path, prefix: &str) -> Result<(), ArchiveError> {
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    zip.add_directory(format!("{prefix}/"), options)?;
    let mut entries = std::fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let file_name = entry.file_name().to_string_lossy().to_string();
        if file_name == ".DS_Store" {
            continue;
        }
        let path = format!("{prefix}/{file_name}");
        let kind = entry.file_type()?;
        if kind.is_dir() {
            add_dir(zip, &entry.path(), &path)?;
        } else if kind.is_file() {
            zip.start_file(path, options.unix_permissions(mode(&entry.metadata()?)))?;
            std::io::copy(&mut File::open(entry.path())?, zip)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn mode(meta: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o777
}

#[cfg(not(unix))]
fn mode(_meta: &std::fs::Metadata) -> u32 {
    0o644
}

/// Import a skill from `src` into `scope`, returning its name. `src` is a
/// zip (any extension, `.skill` included) or a markdown `SKILL.md`. Refuses
/// to overwrite an existing skill.
pub fn import_skill(
    workspace_root: &Path,
    scope: SkillScope,
    src: &Path,
) -> Result<String, RuntimeError> {
    let is_markdown = src
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
    if is_markdown {
        import_markdown(workspace_root, scope, src)
    } else {
        import_zip(workspace_root, scope, src)
    }
}

fn read_error(src: &Path, e: std::io::Error) -> RuntimeError {
    RuntimeError::Io(std::io::Error::new(
        e.kind(),
        format!("reading {}: {e}", src.display()),
    ))
}

/// The name a skill gets when its frontmatter has none: the folder it came
/// from (`my-skill/SKILL.md`), else the file's own name (`my-skill.zip`).
fn fallback_name(src: &Path, folder: Option<&str>) -> String {
    let stem = src
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    match folder {
        Some(folder) => folder.to_string(),
        None if stem.eq_ignore_ascii_case("skill") => src
            .parent()
            .and_then(Path::file_name)
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default(),
        None => stem,
    }
}

/// Read `contents` as a SKILL.md and decide the skill's name.
fn skill_name(contents: &str, fallback: &str, scope: SkillScope) -> Result<String, RuntimeError> {
    parse_skill_md(contents, Path::new(fallback), scope)
        .map(|skill| skill.name)
        .map_err(|e| {
            RuntimeError::Invalid(format!("That isn't a skill this harness can read: {e}."))
        })
}

fn import_markdown(
    workspace_root: &Path,
    scope: SkillScope,
    src: &Path,
) -> Result<String, RuntimeError> {
    let size = std::fs::metadata(src)
        .map_err(|e| read_error(src, e))?
        .len();
    if size > MAX_IMPORT_BYTES {
        return Err(RuntimeError::Invalid(format!(
            "{} is {} MB; a skill can be at most {} MB.",
            src.display(),
            size / (1024 * 1024),
            MAX_IMPORT_BYTES / (1024 * 1024)
        )));
    }
    let contents = std::fs::read_to_string(src).map_err(|e| read_error(src, e))?;
    let name = skill_name(&contents, &fallback_name(src, None), scope)?;
    let dest = vacant_skill_dir(workspace_root, scope, &name)?;
    std::fs::create_dir_all(&dest).map_err(io_context("creating", &dest))?;
    let md = dest.join("SKILL.md");
    if let Err(e) = std::fs::write(&md, contents) {
        // An empty folder would be invisible yet block every later import
        // of this name.
        if let Err(cleanup) = std::fs::remove_dir_all(&dest) {
            return Err(RuntimeError::Invalid(format!(
                "writing {}: {e}; and could not remove the half-made {}: {cleanup}",
                md.display(),
                dest.display()
            )));
        }
        return Err(io_context("writing", &md)(e));
    }
    Ok(name)
}

/// Archive entries that are packaging noise, not part of the skill.
fn is_noise(path: &Path) -> bool {
    path.components().any(|c| match c {
        Component::Normal(part) => part == "__MACOSX" || part == ".DS_Store",
        _ => false,
    })
}

fn is_skill_md(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|n| n.eq_ignore_ascii_case("SKILL.md"))
}

fn import_zip(
    workspace_root: &Path,
    scope: SkillScope,
    src: &Path,
) -> Result<String, RuntimeError> {
    let not_a_skill = |detail: String| {
        RuntimeError::Invalid(format!(
            "{} isn't a skill archive: {detail}",
            src.file_name().unwrap_or_default().to_string_lossy()
        ))
    };
    let file = File::open(src).map_err(|e| read_error(src, e))?;
    let mut archive = ZipArchive::new(file).map_err(|e| not_a_skill(e.to_string()))?;
    if archive.len() > MAX_IMPORT_FILES {
        return Err(not_a_skill(format!(
            "it holds over {MAX_IMPORT_FILES} files"
        )));
    }

    // The skill's root is the folder holding the shallowest SKILL.md.
    let mut paths = Vec::with_capacity(archive.len());
    for i in 0..archive.len() {
        let entry = archive
            .by_index(i)
            .map_err(|e| not_a_skill(e.to_string()))?;
        // `enclosed_name` is None for absolute or `..` paths: never extracted.
        paths.push(entry.enclosed_name().filter(|p| !is_noise(p)));
    }
    let manifests: Vec<&PathBuf> = paths.iter().flatten().filter(|p| is_skill_md(p)).collect();
    let depth = manifests.iter().map(|p| p.components().count()).min();
    let Some(depth) = depth else {
        return Err(not_a_skill("no SKILL.md inside".into()));
    };
    let roots: Vec<PathBuf> = manifests
        .iter()
        .filter(|p| p.components().count() == depth)
        .map(|p| p.parent().map(Path::to_path_buf).unwrap_or_default())
        .collect();
    let [root] = roots.as_slice() else {
        return Err(not_a_skill(
            "it holds several skills — import them one at a time".into(),
        ));
    };

    // Unpack into a hidden staging folder beside the skills, then move it
    // into place in one step, so a failed import leaves nothing half-written.
    let skills_dir = scope_dir(workspace_root, scope)?;
    std::fs::create_dir_all(&skills_dir).map_err(io_context("creating", &skills_dir))?;
    let staging = tempfile::Builder::new()
        .prefix(".import-")
        .tempdir_in(&skills_dir)
        .map_err(io_context("staging the import in", &skills_dir))?;
    let mut budget = MAX_IMPORT_BYTES;
    for (i, path) in paths.iter().enumerate() {
        let Some(rel) = path.as_ref().and_then(|p| p.strip_prefix(root).ok()) else {
            continue;
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let mut entry = archive
            .by_index(i)
            .map_err(|e| not_a_skill(e.to_string()))?;
        if entry.is_symlink() {
            continue;
        }
        let rel = if rel.components().count() == 1 && is_skill_md(rel) {
            PathBuf::from("SKILL.md")
        } else {
            rel.to_path_buf()
        };
        let out = staging.path().join(&rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(io_context("creating", &out))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(io_context("creating", parent))?;
        }
        let mut file = File::create(&out).map_err(io_context("writing", &out))?;
        // Read one byte past the budget to tell "exactly fits" from "too big".
        let written = std::io::copy(&mut (&mut entry).take(budget + 1), &mut file)
            .map_err(|e| not_a_skill(format!("{}: {e}", rel.display())))?;
        if written > budget {
            return Err(not_a_skill(format!(
                "it unpacks to more than {} MB",
                MAX_IMPORT_BYTES / (1024 * 1024)
            )));
        }
        budget -= written;
        file.flush().map_err(io_context("writing", &out))?;
        set_mode(&out, entry.unix_mode())?;
    }

    let contents =
        std::fs::read_to_string(staging.path().join("SKILL.md")).map_err(|e| read_error(src, e))?;
    let folder = root.file_name().map(|n| n.to_string_lossy().to_string());
    let name = skill_name(&contents, &fallback_name(src, folder.as_deref()), scope)?;
    let dest = vacant_skill_dir(workspace_root, scope, &name)?;
    // On success the staging folder is gone and the TempDir's cleanup finds
    // nothing; on any error above, dropping it removes the partial unpack.
    std::fs::rename(staging.path(), &dest).map_err(io_context("installing", &dest))?;
    Ok(name)
}

/// Keep scripts runnable: an entry stored executable stays executable.
#[cfg(unix)]
fn set_mode(path: &Path, mode: Option<u32>) -> Result<(), RuntimeError> {
    use std::os::unix::fs::PermissionsExt;
    if mode.is_some_and(|m| m & 0o111 != 0) {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .map_err(io_context("setting permissions on", path))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: Option<u32>) -> Result<(), RuntimeError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{project_skills_dir, save_skill};
    use harness_tools::skill::load_skills_dir;

    /// Build a zip at `path` from (name, contents) pairs; a trailing `/`
    /// makes a directory entry.
    fn zip_of(path: &Path, files: &[(&str, &str)]) {
        let mut zip = ZipWriter::new(File::create(path).unwrap());
        for (name, contents) in files {
            if name.ends_with('/') {
                zip.add_directory(*name, SimpleFileOptions::default())
                    .unwrap();
            } else {
                zip.start_file(*name, SimpleFileOptions::default()).unwrap();
                zip.write_all(contents.as_bytes()).unwrap();
            }
        }
        zip.finish().unwrap();
    }

    fn project_skills(root: &Path) -> Vec<harness_tools::skill::Skill> {
        load_skills_dir(&project_skills_dir(root), SkillScope::Project)
    }

    const MD: &str = "---\nname: pdf\ndescription: Fill PDF forms.\nlicense: MIT\n---\n\nRun `scripts/fill.py`.\n";

    #[test]
    fn export_then_import_round_trips_with_supporting_files() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
        save_skill(&a, SkillScope::Project, "notes", "Writes notes.", "Steps.").unwrap();
        let skill_dir = project_skills_dir(&a).join("notes");
        std::fs::create_dir_all(skill_dir.join("templates")).unwrap();
        std::fs::write(skill_dir.join("templates/t.md"), "template").unwrap();

        let zip = tmp.path().join("notes.zip");
        export_skill(&a, SkillScope::Project, "notes", &zip).unwrap();

        // The claude.ai layout: one top-level folder named after the skill.
        let mut archive = ZipArchive::new(File::open(&zip).unwrap()).unwrap();
        let names: Vec<String> = archive
            .file_names()
            .map(|n| n.unwrap().to_string())
            .collect();
        assert!(names.contains(&"notes/SKILL.md".to_string()), "{names:?}");
        assert!(
            names.contains(&"notes/templates/t.md".to_string()),
            "{names:?}"
        );
        assert!(archive.by_name("notes/SKILL.md").is_ok());

        let name = import_skill(&b, SkillScope::Project, &zip).unwrap();
        assert_eq!(name, "notes");
        let imported = project_skills(&b);
        assert_eq!(imported[0].description, "Writes notes.");
        let t = project_skills_dir(&b).join("notes/templates/t.md");
        assert_eq!(std::fs::read_to_string(t).unwrap(), "template");
    }

    #[test]
    fn imports_a_folder_zip_like_claude_ai_takes() {
        let tmp = tempfile::tempdir().unwrap();
        let zip = tmp.path().join("download.zip");
        zip_of(
            &zip,
            &[
                ("pdf/", ""),
                ("pdf/SKILL.md", MD),
                ("pdf/scripts/fill.py", "print()"),
                ("__MACOSX/pdf/._SKILL.md", "junk"),
            ],
        );
        assert_eq!(
            import_skill(tmp.path(), SkillScope::Project, &zip).unwrap(),
            "pdf"
        );
        let dir = project_skills_dir(tmp.path()).join("pdf");
        assert!(dir.join("scripts/fill.py").is_file());
        assert!(!dir.join("__MACOSX").exists());
        // Extra frontmatter is kept verbatim.
        assert_eq!(std::fs::read_to_string(dir.join("SKILL.md")).unwrap(), MD);
    }

    #[test]
    fn imports_a_dot_skill_package_with_skill_md_at_the_root() {
        let tmp = tempfile::tempdir().unwrap();
        let pkg = tmp.path().join("my-pdf.skill");
        let no_name = "---\ndescription: Fill PDF forms.\n---\nSteps.";
        zip_of(&pkg, &[("Skill.md", no_name), ("reference.md", "ref")]);
        // No frontmatter name: the package's file name names it.
        assert_eq!(
            import_skill(tmp.path(), SkillScope::Project, &pkg).unwrap(),
            "my-pdf"
        );
        let dir = project_skills_dir(tmp.path()).join("my-pdf");
        assert!(dir.join("SKILL.md").is_file());
        assert!(dir.join("reference.md").is_file());
    }

    #[test]
    fn imports_a_lone_skill_md() {
        let tmp = tempfile::tempdir().unwrap();
        let md = tmp.path().join("SKILL.md");
        std::fs::write(&md, MD).unwrap();
        assert_eq!(
            import_skill(tmp.path(), SkillScope::Project, &md).unwrap(),
            "pdf"
        );
        assert_eq!(project_skills(tmp.path())[0].name, "pdf");
    }

    #[test]
    fn refuses_to_overwrite_an_existing_skill() {
        let tmp = tempfile::tempdir().unwrap();
        save_skill(tmp.path(), SkillScope::Project, "pdf", "Mine.", "Mine.").unwrap();
        let zip = tmp.path().join("pdf.zip");
        zip_of(&zip, &[("pdf/SKILL.md", MD)]);
        let err = import_skill(tmp.path(), SkillScope::Project, &zip).unwrap_err();
        assert!(err.to_string().contains("already exists"), "{err}");
        assert_eq!(project_skills(tmp.path())[0].description, "Mine.");
        // The staging folder is cleaned up.
        let leftovers: Vec<_> = std::fs::read_dir(project_skills_dir(tmp.path()))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with('.'))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn rejects_archives_that_are_not_one_skill() {
        let tmp = tempfile::tempdir().unwrap();
        let none = tmp.path().join("none.zip");
        zip_of(&none, &[("readme.txt", "hi")]);
        let err = import_skill(tmp.path(), SkillScope::Project, &none).unwrap_err();
        assert!(err.to_string().contains("no SKILL.md"), "{err}");

        let two = tmp.path().join("two.zip");
        zip_of(&two, &[("a/SKILL.md", MD), ("b/SKILL.md", MD)]);
        let err = import_skill(tmp.path(), SkillScope::Project, &two).unwrap_err();
        assert!(err.to_string().contains("several skills"), "{err}");

        let garbage = tmp.path().join("garbage.zip");
        std::fs::write(&garbage, "not a zip").unwrap();
        let err = import_skill(tmp.path(), SkillScope::Project, &garbage).unwrap_err();
        assert!(err.to_string().contains("isn't a skill archive"), "{err}");
        assert!(project_skills(tmp.path()).is_empty());
    }

    #[test]
    fn never_writes_outside_the_skill_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let zip = tmp.path().join("evil.zip");
        zip_of(
            &zip,
            &[("pdf/SKILL.md", MD), ("pdf/../../escaped.txt", "x")],
        );
        import_skill(tmp.path(), SkillScope::Project, &zip).unwrap();
        assert!(!tmp.path().join("escaped.txt").exists());
        assert!(!project_skills_dir(tmp.path()).join("escaped.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn scripts_stay_executable_through_a_round_trip() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
        save_skill(&a, SkillScope::Project, "s", "D.", "I.").unwrap();
        let script = project_skills_dir(&a).join("s/run.sh");
        std::fs::write(&script, "#!/bin/sh").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let zip = tmp.path().join("s.zip");
        export_skill(&a, SkillScope::Project, "s", &zip).unwrap();
        import_skill(&b, SkillScope::Project, &zip).unwrap();
        let mode = std::fs::metadata(project_skills_dir(&b).join("s/run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111);
    }
}
