//! Carving a workspace into territories — the landmarks a study run visits.
//!
//! A territory is a directory worth knowing as a unit: a crate, a feature
//! folder, a package. The split is mechanical so it works for any project:
//! walk the source tree, and keep splitting a directory into its children
//! while it is big and has children of its own, until the pieces are small
//! enough to quiz on. Root-level Markdown becomes the "docs" territory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One region of the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Territory {
    /// The directory relative to the workspace root, `/`-separated. The
    /// docs territory is `docs`.
    pub id: String,
    /// The display name: the last path segment, disambiguated with its
    /// parent when two territories share one.
    pub name: String,
    /// Source files under it, relative to the workspace root.
    pub files: Vec<String>,
}

/// The docs territory's id.
pub const DOCS: &str = "docs";

/// Directories never worth quizzing on (build output, dependencies, VCS).
const IGNORED_DIRS: &[&str] = &[
    "target",
    "node_modules",
    "dist",
    "build",
    "out",
    "coverage",
    "vendor",
    "generations",
    "__pycache__",
    "venv",
    ".venv",
];

const SOURCE_EXTS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "mjs", "py", "go", "java", "kt", "swift", "c", "h", "cpp",
    "hpp", "cc", "rb", "php", "sh", "css", "scss", "md", "toml", "yaml", "yml", "sql", "proto",
];

/// Files above this many bytes are skipped (generated bundles, fixtures).
const MAX_FILE_BYTES: u64 = 200 * 1024;

/// The most territories a project splits into, so a monorepo still reads
/// as a few dozen regions.
const MAX_TERRITORIES: usize = 48;

/// A region with more source files than this is split into its child
/// directories when it has at least two.
const SPLIT_ABOVE: usize = 40;

/// A split directory's own files become a territory only when there are at
/// least this many; fewer (a lone config file) aren't worth a landmark.
const MIN_OWN_FILES: usize = 3;

/// Discover the territories of `root`. Never fails: an unreadable or empty
/// workspace is simply no territories.
pub fn discover(root: &Path) -> Vec<Territory> {
    let Some(tree) = scan_dir(root, "") else {
        return Vec::new();
    };
    let mut out = build(&tree);
    disambiguate(&mut out);
    out
}

/// The territory a file belongs to: the longest territory id that prefixes
/// its path, or the docs territory for root-level files.
pub fn territory_for<'a>(territories: &'a [Territory], path: &str) -> Option<&'a Territory> {
    let path = path.trim_start_matches("./");
    let mut best: Option<&Territory> = None;
    for t in territories {
        if t.id == DOCS {
            continue;
        }
        let prefix = format!("{}/", t.id);
        if path.starts_with(&prefix) && best.is_none_or(|b| b.id.len() < t.id.len()) {
            best = Some(t);
        }
    }
    best.or_else(|| {
        if !path.contains('/') {
            territories.iter().find(|t| t.id == DOCS)
        } else {
            None
        }
    })
}

/// A scanned directory: its own source files and its scanned children.
#[derive(Debug, Default)]
struct Dir {
    rel: String,
    files: Vec<String>,
    children: Vec<Dir>,
    total: usize,
}

fn is_source(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name.ends_with(".lock") || name == "package-lock.json" || name.ends_with(".min.js") {
        return false;
    }
    SOURCE_EXTS.contains(&ext.to_ascii_lowercase().as_str())
}

fn is_ignored_dir(name: &str) -> bool {
    name.starts_with('.') || IGNORED_DIRS.contains(&name)
}

fn scan_dir(dir: &Path, rel: &str) -> Option<Dir> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut node = Dir {
        rel: rel.to_string(),
        ..Default::default()
    };
    let mut names: Vec<(String, PathBuf, bool)> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            let is_dir = e.file_type().ok()?.is_dir();
            Some((name, e.path(), is_dir))
        })
        .collect();
    names.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, path, is_dir) in names {
        if is_dir {
            if is_ignored_dir(&name) {
                continue;
            }
            let child_rel = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            if let Some(child) = scan_dir(&path, &child_rel) {
                if child.total > 0 {
                    node.total += child.total;
                    node.children.push(child);
                }
            }
        } else if is_source(&path) {
            let small = std::fs::metadata(&path)
                .map(|m| m.len() <= MAX_FILE_BYTES)
                .unwrap_or(false);
            if small {
                let file_rel = if rel.is_empty() {
                    name
                } else {
                    format!("{rel}/{name}")
                };
                node.files.push(file_rel);
                node.total += 1;
            }
        }
    }
    Some(node)
}

/// A region under consideration: a whole directory, or just the files
/// directly inside one whose children were split off.
struct Part<'a> {
    dir: &'a Dir,
    own_only: bool,
    /// Splitting it would exceed the cap; leave it whole.
    frozen: bool,
}

impl Part<'_> {
    fn size(&self) -> usize {
        if self.own_only {
            self.dir.files.len()
        } else {
            self.dir.total
        }
    }

    fn splittable(&self) -> bool {
        !self.own_only
            && !self.frozen
            && self.dir.total > SPLIT_ABOVE
            && self.dir.children.len() >= 2
    }
}

/// Best-first split: start from the top-level directories and keep
/// splitting the largest region into its children while the total stays
/// under the cap. The result is balanced — the biggest crates and feature
/// folders become their own landmarks before anything small is divided.
fn build(tree: &Dir) -> Vec<Territory> {
    // Root-level Markdown is the docs (README, AGENTS.md, ARCHITECTURE.md…).
    let docs: Vec<String> = tree
        .files
        .iter()
        .filter(|f| f.to_ascii_lowercase().ends_with(".md"))
        .cloned()
        .collect();
    let reserved = usize::from(!docs.is_empty());
    let mut parts: Vec<Part<'_>> = tree
        .children
        .iter()
        .map(|dir| Part {
            dir,
            own_only: false,
            frozen: false,
        })
        .collect();
    while let Some(index) = parts
        .iter()
        .enumerate()
        .filter(|(_, p)| p.splittable())
        .max_by_key(|(_, p)| p.size())
        .map(|(i, _)| i)
    {
        let dir = parts[index].dir;
        let keeps_own = dir.files.len() >= MIN_OWN_FILES;
        let after = parts.len() - 1 + dir.children.len() + usize::from(keeps_own) + reserved;
        if after > MAX_TERRITORIES {
            parts[index].frozen = true;
            continue;
        }
        let mut replacement: Vec<Part<'_>> = Vec::new();
        if keeps_own {
            replacement.push(Part {
                dir,
                own_only: true,
                frozen: false,
            });
        }
        replacement.extend(dir.children.iter().map(|child| Part {
            dir: child,
            own_only: false,
            frozen: false,
        }));
        parts.splice(index..=index, replacement);
    }

    let mut out = Vec::new();
    if !docs.is_empty() {
        out.push(Territory {
            id: DOCS.into(),
            name: DOCS.into(),
            files: docs,
        });
    }
    out.extend(parts.iter().map(|p| Territory {
        id: p.dir.rel.clone(),
        name: last_segment(&p.dir.rel),
        files: if p.own_only {
            p.dir.files.clone()
        } else {
            all_files(p.dir)
        },
    }));
    out
}

fn all_files(dir: &Dir) -> Vec<String> {
    let mut files = dir.files.clone();
    for child in &dir.children {
        files.extend(all_files(child));
    }
    files
}

fn last_segment(rel: &str) -> String {
    rel.rsplit('/').next().unwrap_or(rel).to_string()
}

/// Two territories with one name (`src` under two packages) get their parent
/// segment prepended so the strip reads unambiguously.
fn disambiguate(territories: &mut [Territory]) {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for t in territories.iter() {
        *counts.entry(t.name.clone()).or_default() += 1;
    }
    for t in territories.iter_mut() {
        if counts.get(&t.name).copied().unwrap_or(0) > 1 {
            let segments: Vec<&str> = t.id.rsplit('/').take(2).collect();
            if segments.len() == 2 {
                t.name = format!("{}/{}", segments[1], segments[0]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn small_projects_are_one_territory_per_top_level_dir_plus_docs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(root, "README.md", "# hi");
        touch(root, "src/main.rs", "fn main() {}");
        touch(root, "src/lib.rs", "pub fn x() {}");
        touch(root, "target/debug/junk.rs", "nope");
        touch(root, "node_modules/x/index.js", "nope");
        touch(root, "Cargo.lock", "");
        let t = discover(root);
        let ids: Vec<&str> = t.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["docs", "src"]);
        assert_eq!(t[1].files, vec!["src/lib.rs", "src/main.rs"]);
    }

    #[test]
    fn big_directories_split_into_their_children() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for krate in ["alpha", "beta", "gamma"] {
            for i in 0..30 {
                touch(root, &format!("crates/{krate}/src/f{i}.rs"), "fn f() {}");
            }
        }
        touch(root, "scripts/run.sh", "echo");
        let t = discover(root);
        let ids: Vec<&str> = t.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["crates/alpha", "crates/beta", "crates/gamma", "scripts"]
        );
        assert_eq!(t[0].name, "alpha");
    }

    #[test]
    fn a_file_maps_to_its_deepest_territory_and_root_files_to_docs() {
        let territories = vec![
            Territory {
                id: DOCS.into(),
                name: DOCS.into(),
                files: vec![],
            },
            Territory {
                id: "app".into(),
                name: "app".into(),
                files: vec![],
            },
            Territory {
                id: "app/src/features".into(),
                name: "features".into(),
                files: vec![],
            },
        ];
        assert_eq!(
            territory_for(&territories, "app/src/features/chat/x.tsx").map(|t| t.id.as_str()),
            Some("app/src/features")
        );
        assert_eq!(
            territory_for(&territories, "app/src/main.tsx").map(|t| t.id.as_str()),
            Some("app")
        );
        assert_eq!(
            territory_for(&territories, "README.md").map(|t| t.id.as_str()),
            Some(DOCS)
        );
        assert!(territory_for(&territories, "other/x.rs").is_none());
    }

    #[test]
    fn duplicate_names_get_their_parent() {
        let mut t = vec![
            Territory {
                id: "a/src".into(),
                name: "src".into(),
                files: vec![],
            },
            Territory {
                id: "b/src".into(),
                name: "src".into(),
                files: vec![],
            },
        ];
        disambiguate(&mut t);
        assert_eq!(t[0].name, "a/src");
        assert_eq!(t[1].name, "b/src");
    }
}
