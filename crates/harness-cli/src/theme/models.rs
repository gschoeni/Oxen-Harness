//! The local-model catalog table and the download progress bar.

use super::{flourish, Ui};

/// One row in the `models list` table (a catalog model + its local status).
pub struct ModelRow<'a> {
    pub id: &'a str,
    pub params: &'a str,
    /// Pre-formatted size (actual when installed, else the estimate).
    pub size: String,
    pub installed: bool,
    pub note: &'a str,
}

/// Render the local-model catalog as an aligned, themed table.
pub fn models_table(ui: &Ui, rows: &[ModelRow], total_disk: &str, dir: &str) -> String {
    let id_w = rows
        .iter()
        .map(|r| r.id.len())
        .chain(std::iter::once("Model".len()))
        .max()
        .unwrap_or(5);
    let par_w = rows
        .iter()
        .map(|r| r.params.len())
        .chain(std::iter::once("Params".len()))
        .max()
        .unwrap_or(6);
    let size_w = rows
        .iter()
        .map(|r| r.size.len())
        .chain(std::iter::once("Size".len()))
        .max()
        .unwrap_or(4);

    let mut out = String::from("\n");
    out.push_str(&format!(
        "  {}  {}  {}   {}\n",
        ui.title(&format!("{:<id_w$}", "Model")),
        ui.title(&format!("{:<par_w$}", "Params")),
        ui.title(&format!("{:>size_w$}", "Size")),
        ui.title("Status"),
    ));
    out.push_str(&flourish(ui));
    for r in rows {
        let status = if r.installed {
            ui.green("● on disk")
        } else {
            ui.dim("○ not yet")
        };
        out.push_str(&format!(
            "  {}  {}  {}   {}\n",
            ui.cream(&format!("{:<id_w$}", r.id)),
            ui.brown(&format!("{:<par_w$}", r.params)),
            ui.cream(&format!("{:>size_w$}", r.size)),
            status,
        ));
        out.push_str(&format!(
            "  {}\n",
            ui.dim(&format!("{:id_w$}   {}", "", r.note))
        ));
    }
    out.push('\n');
    out.push_str(&format!(
        "  {} {}\n",
        ui.brown(&ui.theme().voice.label_disk_used),
        ui.cream(total_disk),
    ));
    out.push_str(&format!(
        "  {} {}\n",
        ui.brown(&ui.theme().voice.label_models_dir),
        ui.dim(dir)
    ));
    out.push('\n');
    out.push_str(&format!(
        "  {}\n",
        ui.dim(
            "Pull one with  oxen-harness models pull <Model>   ·   ride it with  --local <Model>"
        ),
    ));
    out
}

/// A single-line, in-place download progress bar with theme flavor.
///
/// `fraction` is `None` when the total size is unknown (the bar shows `?%`).
/// Print it with a leading `\r`; finish with a newline once complete.
pub fn progress_bar(ui: &Ui, fraction: Option<f64>, detail: &str) -> String {
    const WIDTH: usize = 24;
    let frac = fraction.unwrap_or(0.0).clamp(0.0, 1.0);
    let filled = (frac * WIDTH as f64).round() as usize;
    let bar: String = (0..WIDTH)
        .map(|i| if i < filled { '▰' } else { '▱' })
        .collect();
    let pct = match fraction {
        Some(f) => format!("{:>3.0}%", (f * 100.0).clamp(0.0, 100.0)),
        None => "  ?%".to_string(),
    };
    format!(
        "  {} {}  {}  {}",
        ui.brown(&ui.theme().voice.progress_icon),
        ui.green(&bar),
        ui.accent(&pct),
        ui.dim(detail),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_bar_tracks_fraction_and_handles_unknown() {
        let ui = Ui::plain();
        let half = progress_bar(&ui, Some(0.5), "2.5 GB / 5.0 GB");
        assert!(half.contains("50%"));
        assert!(half.contains("2.5 GB / 5.0 GB"));
        assert!(half.contains('▰') && half.contains('▱'));
        let unknown = progress_bar(&ui, None, "downloading");
        assert!(unknown.contains("?%"));
    }

    #[test]
    fn models_table_lists_rows_and_disk_usage() {
        let ui = Ui::plain();
        let rows = [
            ModelRow {
                id: "qwen3-8b",
                params: "8B",
                size: "5.0 GB".to_string(),
                installed: true,
                note: "all-rounder",
            },
            ModelRow {
                id: "qwen3-32b",
                params: "32B",
                size: "20 GB".to_string(),
                installed: false,
                note: "heaviest",
            },
        ];
        let table = models_table(&ui, &rows, "5.0 GB", "/home/me/.oxen-harness/models");
        assert!(table.contains("qwen3-8b"));
        assert!(table.contains("● on disk"));
        assert!(table.contains("○ not yet"));
        assert!(table.contains("5.0 GB"));
        assert!(!table.contains("\x1b["));
    }
}
