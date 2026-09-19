//! Built-in view descriptions and resource resolution; independent of rendering.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ViewDefinition {
    pub id: String,
    pub title: String,
    pub description: String,
    pub file_patterns: Vec<String>,
    pub requires_file: bool,
    #[serde(default)]
    pub priority: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_schema: Option<serde_json::Value>,
}

pub fn builtins() -> Vec<ViewDefinition> {
    [
        ("workflow", "Workflow", "Oxen nodes for prompts, images, video and upscaling. Edit the .graph.json file to drive the graph.", vec!["*.graph.json"], true),
        ("editor", "File", "Source, markdown, images, video and diffs.", vec!["*"], true),
        ("gallery", "Gallery", "Generated media saved in the project.", vec![], false),
        ("preview", "Preview", "The conversation's running website.", vec![], false),
        ("canvas", "Canvas", "Documents created with the canvas tool.", vec!["*.canvas.json"], false),
        ("browser", "Browser", "Links opened in this conversation.", vec![], false),
    ].into_iter().map(|(id,title,description,patterns,requires_file)| ViewDefinition {
        id:id.into(), title:title.into(), description:description.into(),
        file_patterns:patterns.into_iter().map(str::to_string).collect(), requires_file,
        priority:match id {"workflow"=>100,"canvas"=>90,"editor"=>-100,_=>0}, document_schema:None,
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
            priority: 50,
            document_schema: None,
        });
    }
    Ok(views)
}

pub fn resolve_view(path: &str) -> Result<String, String> {
    resolve_from(path, &available()?)
}

pub fn resolve_from(path: &str, views: &[ViewDefinition]) -> Result<String, String> {
    let mut candidates: Vec<_> = views.iter().collect();
    candidates.sort_by_key(|view| std::cmp::Reverse(view.priority));
    for view in candidates {
        for pattern in &view.file_patterns {
            let glob = globset::GlobBuilder::new(pattern)
                .case_insensitive(true)
                .build()
                .map_err(|e| e.to_string())?
                .compile_matcher();
            if glob.is_match(path)
                || (!pattern.contains('/')
                    && std::path::Path::new(path)
                        .file_name()
                        .is_some_and(|p| glob.is_match(p)))
            {
                return Ok(view.id.clone());
            }
        }
    }
    Ok("editor".into())
}

pub fn validate_pattern(pattern: &str) -> Result<(), String> {
    globset::Glob::new(pattern)
        .map(|_| ())
        .map_err(|e| e.to_string())
}
