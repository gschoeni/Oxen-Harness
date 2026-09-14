//! `/gallery` — the project's generated images and videos, newest first:
//! open one in its viewer, or stage it as an `[Image #N]` chip so the next
//! prompt can hand it to `generate_image` / `generate_video` as a reference.

use anyhow::Result;
use harness_media::{MediaItem, MediaKind, MediaStatus};

use crate::picker::{self, Choice};
use crate::theme::Ui;

/// How many generations the picker shows.
const LIMIT: usize = 40;

pub(crate) fn handle_repl(rest: Option<String>, ui: &Ui) -> Result<()> {
    let root = std::env::current_dir()?;
    let library = crate::media::library(&root);
    let items: Vec<MediaItem> = library
        .items()
        .into_iter()
        .filter(|i| i.status == MediaStatus::Succeeded && i.path.is_some())
        .take(LIMIT)
        .collect();
    if items.is_empty() {
        let in_flight = library.in_flight(None).len();
        println!(
            "  {}",
            ui.dim(&if in_flight > 0 {
                format!("{in_flight} generation(s) in flight — nothing finished yet.")
            } else {
                format!(
                    "No generations yet. Ask for one (\"make me an image of…\"); they land in {}/.",
                    library.dir_rel()
                )
            })
        );
        return Ok(());
    }

    let mut words = rest.as_deref().unwrap_or("").split_whitespace();
    let (action, item) = match (words.next(), words.next()) {
        (Some(action @ ("open" | "use" | "desktop")), Some(n)) => {
            let Some(item) = n
                .parse::<usize>()
                .ok()
                .and_then(|n| items.get(n.checked_sub(1)?))
            else {
                println!(
                    "  {}",
                    ui.dim(&format!(
                        "no generation #{n}; /gallery lists 1–{}",
                        items.len()
                    ))
                );
                return Ok(());
            };
            (action.to_string(), item.clone())
        }
        _ => {
            let options: Vec<Choice> = items
                .iter()
                .enumerate()
                .map(|(i, item)| {
                    Choice::new(
                        // The picker numbers rows itself; the label is the file.
                        item.file_name()
                            .map(str::to_string)
                            .unwrap_or_else(|| format!("#{} {}", i + 1, item.id)),
                        format!(
                            "{} · {} · {}",
                            item.model,
                            when(item.created_at),
                            short(&item.prompt)
                        ),
                    )
                })
                .collect();
            let Some(sel) = picker::select(
                ui,
                "Gallery",
                &format!(
                    "{} generation(s) in {}/ — pick one",
                    items.len(),
                    library.dir_rel()
                ),
                &options,
                false,
            )?
            else {
                return Ok(());
            };
            let Some(label) = sel.into_iter().next() else {
                return Ok(());
            };
            // Match the chosen label back to its item (two files can't share
            // a name within one listing: names carry the time and index).
            let Some(item) = items
                .iter()
                .find(|item| item.file_name().is_some_and(|n| n == label))
                .or_else(|| {
                    label
                        .strip_prefix('#')
                        .and_then(|rest| rest.split(' ').next())
                        .and_then(|n| n.parse::<usize>().ok())
                        .and_then(|n| items.get(n.checked_sub(1)?))
                })
            else {
                return Ok(());
            };
            let what = picker::select(
                ui,
                "Gallery",
                item.file_name().unwrap_or(&item.id),
                &[
                    Choice::new("open", "show it in your image/video viewer"),
                    Choice::new("use", "stage it as a reference chip for your next prompt"),
                    Choice::new("desktop", "browse the whole gallery in the desktop app"),
                ],
                false,
            )?;
            match what.and_then(|w| w.into_iter().next()) {
                Some(action) => (action, item.clone()),
                None => return Ok(()),
            }
        }
    };

    let rel = item.path.clone().unwrap_or_default();
    let abs = library.abs(&rel);
    match action.as_str() {
        "desktop" => {
            if let Err(e) =
                crate::commands::ui::run_ui(Some(root.clone()), Some("gallery".into()), ui)
            {
                println!(
                    "  {}",
                    ui.dim(&format!("could not open the desktop app: {e}"))
                );
            }
        }
        "use" => match crate::media::refs().stage(&abs) {
            Some(label) => {
                // Mirror the chip into the composer registry too, so the label
                // resolves to an attachment at submit as well as in the tools.
                let chip = crate::media::stage_path(&abs);
                println!(
                    "  {} {} {}",
                    ui.green("📎"),
                    ui.accent(&chip),
                    ui.dim(&format!("→ {rel}  (mention it in your next prompt; the tools resolve it as {label})"))
                );
            }
            None => println!(
                "  {}",
                ui.dim("that file isn't a stageable image/video/audio")
            ),
        },
        _ => {
            crate::canvas::open_in_browser(&abs);
            println!(
                "  {} {}",
                ui.green(if item.kind == MediaKind::Video {
                    "🎬"
                } else {
                    "🖼"
                }),
                ui.dim(&format!("opened {rel}"))
            );
        }
    }
    Ok(())
}

fn short(text: &str) -> String {
    let t: String = text.chars().take(70).collect();
    if t.len() < text.len() {
        format!("{t}…")
    } else {
        t
    }
}

/// `today 14:02`, `2026-09-10`.
fn when(created_at: i64) -> String {
    let (date, hhmm) = harness_media::library::date_parts(created_at);
    let (today, _) = harness_media::library::date_parts(harness_media::library::now_unix());
    if date == today {
        format!("today {}:{}", &hhmm[..2], &hhmm[2..])
    } else {
        date
    }
}
