//! Durable, repository-local project metadata and reference context.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::RuntimeError;

pub const PROJECT_FILE: &str = ".oxen-harness/project.json";
pub const CONTEXT_DIR: &str = ".oxen-harness/context";
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectConfig {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub instructions: String,
    #[serde(default)]
    pub context: Vec<ProjectContext>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectContext {
    pub path: String,
    pub name: String,
    pub kind: ContextKind,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextKind {
    Text,
    Pdf,
    Image,
}

pub fn config_path(root: &Path) -> PathBuf {
    root.join(PROJECT_FILE)
}

pub fn load(root: &Path) -> ProjectConfig {
    let (_, mut config) = harness_config::read_versioned::<ProjectConfig>(&config_path(root));
    if config.name.trim().is_empty() {
        config.name = folder_name(root);
    }
    config
}

pub fn save(root: &Path, config: &ProjectConfig) -> Result<ProjectConfig, RuntimeError> {
    if !root.is_dir() {
        return Err(RuntimeError::Invalid(format!(
            "project folder does not exist: {}",
            root.display()
        )));
    }
    let mut saved = config.clone();
    saved.name = saved.name.trim().to_string();
    saved.description = saved.description.trim().to_string();
    saved.instructions = saved.instructions.trim().to_string();
    if saved.name.is_empty() {
        saved.name = folder_name(root);
    }
    harness_config::write_versioned(&config_path(root), SCHEMA_VERSION, &saved)?;
    Ok(saved)
}

pub fn add_context(root: &Path, sources: &[PathBuf]) -> Result<ProjectConfig, RuntimeError> {
    let mut config = load(root);
    for source in sources {
        if !source.is_file() {
            return Err(RuntimeError::Invalid(format!(
                "context file does not exist: {}",
                source.display()
            )));
        }
        let name = source
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| {
                RuntimeError::Invalid(format!(
                    "context path has no file name: {}",
                    source.display()
                ))
            })?;
        let kind = context_kind(source)?;
        let (hash, size_bytes) = hash_file(source)?;
        let stored_name = format!("{}-{}", &hash[..12], safe_filename(&name));
        let relative = Path::new(CONTEXT_DIR).join(stored_name);
        let destination = root.join(&relative);
        if !destination.exists() {
            copy_file(source, &destination)?;
        }
        let path = relative.to_string_lossy().replace('\\', "/");
        if !config.context.iter().any(|entry| entry.path == path) {
            config.context.push(ProjectContext {
                path,
                name,
                kind,
                size_bytes,
            });
        }
    }
    save(root, &config)
}

pub fn remove_context(root: &Path, relative_path: &str) -> Result<ProjectConfig, RuntimeError> {
    let mut config = load(root);
    let Some(index) = config
        .context
        .iter()
        .position(|entry| entry.path == relative_path)
    else {
        return Err(RuntimeError::Invalid(format!(
            "project context does not contain {relative_path}"
        )));
    };
    let entry = config.context.remove(index);
    let path = Path::new(&entry.path);
    if !path.starts_with(CONTEXT_DIR) {
        return Err(RuntimeError::Invalid("invalid project context path".into()));
    }
    match std::fs::remove_file(root.join(path)) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    save(root, &config)
}

pub fn prompt_section(root: &Path) -> String {
    if !config_path(root).is_file() {
        return String::new();
    }
    let config = load(root);
    let mut section = format!("\n\nProject: {}", config.name);
    if !config.description.is_empty() {
        section.push_str("\nGoal:\n");
        section.push_str(&config.description);
    }
    if !config.instructions.is_empty() {
        section.push_str("\nInstructions:\n");
        section.push_str(&config.instructions);
    }
    if !config.context.is_empty() {
        section.push_str(
            "\nReference context:\nThese files are durable project context. Use `read_file` to read relevant text references before responding; PDF and image references are attached to the first prompt in a new chat.\n",
        );
        for entry in &config.context {
            section.push_str(&format!(
                "- {} ({:?}): {}\n",
                entry.name, entry.kind, entry.path
            ));
        }
    }
    section
}

/// Absolute PDF/image context paths to attach to the first user prompt in a
/// new chat. Text references stay on demand through `read_file`.
pub fn binary_context_paths(root: &Path) -> Vec<PathBuf> {
    load(root)
        .context
        .into_iter()
        .filter(|entry| matches!(entry.kind, ContextKind::Pdf | ContextKind::Image))
        .filter_map(|entry| confined_context_path(root, &entry.path))
        .collect()
}

fn confined_context_path(root: &Path, relative: &str) -> Option<PathBuf> {
    let relative = Path::new(relative);
    if relative.is_absolute() || !relative.starts_with(CONTEXT_DIR) {
        return None;
    }
    if relative
        .components()
        .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return None;
    }
    let root = root.canonicalize().ok()?;
    let path = root.join(relative).canonicalize().ok()?;
    (path.is_file() && path.starts_with(root)).then_some(path)
}

fn folder_name(root: &Path) -> String {
    root.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| root.display().to_string())
}

fn context_kind(path: &Path) -> Result<ContextKind, RuntimeError> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "pdf" => Ok(ContextKind::Pdf),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tiff" | "heic" => Ok(ContextKind::Image),
        "txt" | "md" | "markdown" | "rst" | "csv" | "tsv" | "json" | "jsonl" | "toml" | "yaml"
        | "yml" | "xml" | "html" | "htm" | "css" | "js" | "jsx" | "ts" | "tsx" | "py" | "rs"
        | "go" | "java" | "kt" | "swift" | "c" | "h" | "cpp" | "hpp" | "rb" | "php" | "sh"
        | "sql" | "log" => Ok(ContextKind::Text),
        _ => Err(RuntimeError::Invalid(format!(
            "unsupported project context type: {}",
            path.display()
        ))),
    }
}

fn hash_file(path: &Path) -> Result<(String, u64), RuntimeError> {
    let mut source = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = source.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size = size.saturating_add(read as u64);
    }
    Ok((format!("{:x}", hasher.finalize()), size))
}

fn copy_file(source: &Path, destination: &Path) -> Result<(), RuntimeError> {
    let Some(parent) = destination.parent() else {
        return Err(RuntimeError::Invalid(
            "context destination has no parent".into(),
        ));
    };
    std::fs::create_dir_all(parent)?;
    let temporary = destination.with_extension("context.tmp");
    let result = (|| -> Result<(), std::io::Error> {
        let mut input = File::open(source)?;
        let mut output = File::create(&temporary)?;
        std::io::copy(&mut input, &mut output)?;
        output.flush()?;
        std::fs::rename(&temporary, destination)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.map_err(Into::into)
}

fn safe_filename(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect();
    let safe = safe.trim_matches(['.', '-']).to_string();
    if safe.is_empty() {
        "context".into()
    } else {
        safe
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_without_metadata_is_still_a_named_project() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("existing-app");
        std::fs::create_dir(&root).unwrap();

        let project = load(&root);

        assert_eq!(project.name, "existing-app");
        assert!(project.description.is_empty());
        assert!(project.instructions.is_empty());
        assert!(project.context.is_empty());
    }

    #[test]
    fn metadata_round_trips_in_the_repository() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("demo");
        std::fs::create_dir(&root).unwrap();
        let config = ProjectConfig {
            name: "Demo App".into(),
            description: "Build a calm writing space.".into(),
            instructions: "Prefer accessible, keyboard-first interactions.".into(),
            context: vec![],
        };

        let saved = save(&root, &config).unwrap();

        assert_eq!(saved, config);
        assert_eq!(load(&root), config);
        let text = std::fs::read_to_string(config_path(&root)).unwrap();
        assert!(text.contains(r#""schema_version": 1"#));
    }

    #[test]
    fn context_is_copied_content_addressed_and_deduplicated() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("demo");
        std::fs::create_dir(&root).unwrap();
        save(
            &root,
            &ProjectConfig {
                name: "Demo".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let source = tmp.path().join("Product brief.md");
        std::fs::write(&source, "# Product brief\nMake it warm.").unwrap();

        let first = add_context(&root, std::slice::from_ref(&source)).unwrap();
        let second = add_context(&root, &[source]).unwrap();

        assert_eq!(first.context.len(), 1);
        assert_eq!(second.context.len(), 1);
        let entry = &second.context[0];
        assert_eq!(entry.name, "Product brief.md");
        assert_eq!(entry.kind, ContextKind::Text);
        assert!(entry.path.starts_with(".oxen-harness/context/"));
        assert_eq!(
            std::fs::read_to_string(root.join(&entry.path)).unwrap(),
            "# Product brief\nMake it warm."
        );
    }

    #[test]
    fn removing_context_updates_the_manifest_and_deletes_the_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("demo");
        std::fs::create_dir(&root).unwrap();
        let source = tmp.path().join("brief.pdf");
        std::fs::write(&source, b"%PDF-demo").unwrap();
        let config = add_context(&root, &[source]).unwrap();
        let path = config.context[0].path.clone();

        let updated = remove_context(&root, &path).unwrap();

        assert!(updated.context.is_empty());
        assert!(!root.join(path).exists());
    }

    #[test]
    fn prompt_section_carries_goal_instructions_and_context_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("demo");
        std::fs::create_dir(&root).unwrap();
        let source = tmp.path().join("brief.txt");
        std::fs::write(&source, "reference").unwrap();
        save(
            &root,
            &ProjectConfig {
                name: "Demo App".into(),
                description: "Ship the onboarding flow.".into(),
                instructions: "Use plain language and preserve accessibility.".into(),
                context: vec![],
            },
        )
        .unwrap();
        add_context(&root, &[source]).unwrap();

        let section = prompt_section(&root);

        assert!(section.contains("Project: Demo App"));
        assert!(section.contains("Ship the onboarding flow."));
        assert!(section.contains("Use plain language"));
        assert!(section.contains(".oxen-harness/context/"));
        assert!(section.contains("read_file"));
    }

    #[test]
    fn a_repository_manifest_cannot_attach_files_outside_its_context_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("demo");
        std::fs::create_dir(&root).unwrap();
        let secret = tmp.path().join("secret.pdf");
        std::fs::write(&secret, b"secret").unwrap();
        save(
            &root,
            &ProjectConfig {
                name: "Untrusted clone".into(),
                context: vec![ProjectContext {
                    path: ".oxen-harness/context/../../secret.pdf".into(),
                    name: "secret.pdf".into(),
                    kind: ContextKind::Pdf,
                    size_bytes: 6,
                }],
                ..Default::default()
            },
        )
        .unwrap();

        assert!(binary_context_paths(&root).is_empty());
    }
}
