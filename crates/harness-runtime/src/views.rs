//! Built-in view descriptions and resource resolution; independent of rendering.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ViewDefinition {
    pub id: String,
    pub title: String,
    pub description: String,
    pub file_patterns: Vec<String>,
    pub requires_file: bool,
}

pub fn builtins() -> Vec<ViewDefinition> {
    [
        ("workflow", "Workflow", "Oxen nodes for prompts, images, video and upscaling. Edit the .graph.json file to drive the graph.", vec!["*.graph.json"], true),
        ("editor", "File", "Source, markdown, images, video and diffs.", vec!["*"], true),
        ("gallery", "Gallery", "Generated media saved in the project.", vec![], false),
        ("preview", "Preview", "The conversation's running website.", vec![], false),
        ("canvas", "Canvas", "Documents created with the canvas tool.", vec![], false),
        ("browser", "Browser", "Links opened in this conversation.", vec![], false),
    ].into_iter().map(|(id,title,description,patterns,requires_file)| ViewDefinition {
        id:id.into(), title:title.into(), description:description.into(),
        file_patterns:patterns.into_iter().map(str::to_string).collect(), requires_file,
    }).collect()
}

pub fn resolve(path: &str) -> &'static str {
    if path.to_ascii_lowercase().ends_with(".graph.json") {
        "workflow"
    } else {
        "editor"
    }
}

/// Installed descriptors are host-discovered; package scripts never run here.
pub fn available() -> Result<Vec<ViewDefinition>, String> {
    let mut views = builtins();
    for package in crate::view_packages::installed()? {
        let manifest = package.manifest;
        views.push(ViewDefinition {
            id: format!("package:{}", manifest.id),
            title: manifest.title,
            description: manifest.description,
            file_patterns: manifest.file_patterns,
            requires_file: false,
        });
    }
    Ok(views)
}

pub fn resolve_view(path: &str) -> Result<String, String> {
    if path.ends_with(".canvas.json") {
        return Ok("canvas".into());
    }
    if resolve(path) == "workflow" {
        return Ok("workflow".into());
    }
    for view in available()?
        .into_iter()
        .filter(|v| v.id.starts_with("package:"))
    {
        for pattern in &view.file_patterns {
            let glob = globset::Glob::new(pattern)
                .map_err(|e| e.to_string())?
                .compile_matcher();
            if glob.is_match(path)
                || (!pattern.contains('/')
                    && std::path::Path::new(path)
                        .file_name()
                        .is_some_and(|p| glob.is_match(p)))
            {
                return Ok(view.id);
            }
        }
    }
    Ok("editor".into())
}
