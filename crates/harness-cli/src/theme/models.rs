//! The local-model catalog table and the download progress bar.

use super::Ui;

/// One row in the `models list` table (a catalog or downloaded model + its
/// local status).
pub struct ModelRow {
    pub id: String,
    pub params: String,
    /// Pre-formatted size (actual when installed, else the estimate).
    pub size: String,
    pub installed: bool,
    pub note: String,
}

/// Notes wrap at this width even on a wide terminal: a short measure reads
/// faster than a line stretched across the screen.
const NOTE_MAX_WIDTH: usize = 88;

/// Render the local-model catalog: installed models first, then the ones
/// available to pull, each with its note wrapped beneath it. `width` is the
/// terminal width in cells.
pub fn models_table(
    ui: &Ui,
    rows: &[ModelRow],
    total_disk: &str,
    dir: &str,
    width: usize,
) -> String {
    let col = |f: fn(&ModelRow) -> usize| rows.iter().map(f).max().unwrap_or(0);
    let widths = (
        col(|r| r.id.len()),
        col(|r| r.params.len()),
        col(|r| r.size.len()),
    );
    let note_width = width.min(NOTE_MAX_WIDTH).saturating_sub(4);

    let (on_disk, available): (Vec<&ModelRow>, Vec<&ModelRow>) =
        rows.iter().partition(|r| r.installed);

    let mut out = String::new();
    for (heading, group) in [("On disk", &on_disk), ("Available", &available)] {
        if group.is_empty() {
            continue;
        }
        out.push_str(&format!(
            "\n  {}  {}\n",
            ui.title(heading),
            ui.dim(&format!("({})", group.len()))
        ));
        for r in group {
            out.push('\n');
            out.push_str(&model_entry(ui, r, widths, note_width));
        }
    }

    out.push('\n');
    out.push_str(&format!(
        "  {} {}   {} {}\n",
        ui.brown(&ui.theme().voice.label_disk_used),
        ui.cream(total_disk),
        ui.brown(&ui.theme().voice.label_models_dir),
        ui.dim(dir)
    ));
    out.push_str(&format!(
        "  {}\n",
        ui.dim(
            "Pull one with  oxen-harness models pull <Model>   ·   run it with  --local <Model>"
        ),
    ));
    out
}

/// One model: a marker, id, params, and size on the first line, then the
/// note wrapped and indented under the id.
fn model_entry(
    ui: &Ui,
    r: &ModelRow,
    (id_w, par_w, size_w): (usize, usize, usize),
    note_width: usize,
) -> String {
    let (marker, id) = if r.installed {
        (ui.green("●"), ui.green(&format!("{:<id_w$}", r.id)))
    } else {
        (ui.dim("○"), ui.cream(&format!("{:<id_w$}", r.id)))
    };
    let mut out = format!(
        "  {marker} {id}  {}  {}\n",
        ui.brown(&format!("{:<par_w$}", r.params)),
        ui.cream(&format!("{:>size_w$}", r.size)),
    );
    for line in crate::picker::wrap(&r.note, note_width) {
        out.push_str(&format!("    {}\n", ui.dim(&line)));
    }
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

    fn row(id: &str, installed: bool, note: &str) -> ModelRow {
        ModelRow {
            id: id.to_string(),
            params: "8B".to_string(),
            size: "5.0 GB".to_string(),
            installed,
            note: note.to_string(),
        }
    }

    #[test]
    fn models_table_groups_installed_first_and_shows_disk_usage() {
        let ui = Ui::plain();
        let rows = [
            row("qwen3-32b", false, "heaviest"),
            row("qwen3-8b", true, "all-rounder"),
        ];
        let table = models_table(&ui, &rows, "5.0 GB", "/home/me/.oxen-harness/models", 80);
        let on_disk = table.find("On disk").expect("on-disk heading");
        let available = table.find("Available").expect("available heading");
        let installed = table.find("● qwen3-8b").expect("installed row");
        let missing = table.find("○ qwen3-32b").expect("available row");
        assert!(on_disk < installed && installed < available && available < missing);
        assert!(table.contains("5.0 GB"));
        assert!(!table.contains("\x1b["));
    }

    #[test]
    fn models_table_wraps_long_notes_under_the_model() {
        let ui = Ui::plain();
        let note = "word ".repeat(40);
        let table = models_table(&ui, &[row("m", false, &note)], "0 B", "/m", 60);
        let note_lines: Vec<&str> = table.lines().filter(|l| l.contains("word")).collect();
        assert!(note_lines.len() > 1, "long note should wrap");
        assert!(note_lines
            .iter()
            .all(|l| l.starts_with("    ") && l.len() <= 60));
    }

    #[test]
    fn models_table_skips_an_empty_group() {
        let ui = Ui::plain();
        let table = models_table(&ui, &[row("m", false, "n")], "0 B", "/m", 80);
        assert!(!table.contains("On disk"));
        assert!(table.contains("Available"));
    }
}
