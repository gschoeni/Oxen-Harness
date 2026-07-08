//! Theme management for the CLI: the `oxen-harness theme` subcommand and the
//! in-REPL `/theme` command (select, create-by-vibe, import, export).

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use harness_agent::Agent;
use harness_theme::{Store, Theme};

use crate::picker::{self, Choice};
use crate::theme::{self, Ui};

/// `oxen-harness theme <action>` — non-interactive theme management.
#[derive(Debug, clap::Subcommand)]
pub enum ThemeAction {
    /// List available themes (built-in + installed).
    List,
    /// Switch the active theme (persists for future sessions).
    Use { name: String },
    /// Export a theme to a TOML file for sharing.
    Export { name: String, path: PathBuf },
    /// Import a theme from a TOML or JSON file.
    Import { path: PathBuf },
    /// Print the directory where themes are stored.
    Path,
    /// Remove an installed theme (built-ins always remain).
    Remove { name: String },
}

/// Run a top-level `theme` subcommand and exit.
pub async fn run_theme(action: ThemeAction, ui: &Ui) -> Result<()> {
    let store = Store::open().context("opening theme store")?;
    match action {
        ThemeAction::List => print_list(ui, &store),
        ThemeAction::Use { name } => {
            let theme = store
                .set_active(&name)
                .with_context(|| format!("no theme `{name}`"))?;
            println!(
                "  {} {}",
                ui.green("✓ active theme:"),
                ui.cream(&theme.meta.name)
            );
        }
        ThemeAction::Export { name, path } => {
            let dest = store
                .export(&name, &path)
                .with_context(|| format!("exporting `{name}`"))?;
            println!(
                "  {} {}",
                ui.green("✓ exported to"),
                ui.cream(&dest.display().to_string())
            );
        }
        ThemeAction::Import { path } => {
            let theme = store
                .import(&path)
                .with_context(|| format!("importing {}", path.display()))?;
            println!(
                "  {} {}  ({})",
                ui.green("✓ imported"),
                ui.cream(&theme.meta.name),
                ui.dim("activate with: oxen-harness theme use ..."),
            );
        }
        ThemeAction::Path => println!("{}", store.themes_dir().display()),
        ThemeAction::Remove { name } => {
            store
                .remove(&name)
                .with_context(|| format!("removing `{name}`"))?;
            println!("  {} {}", ui.brown("removed theme:"), ui.cream(&name));
        }
    }
    Ok(())
}

/// Handle an in-REPL `/theme ...` command. May hot-swap `ui` to a new theme.
pub async fn handle_repl(args: Vec<String>, agent: &Agent, ui: &mut Ui) -> Result<()> {
    let store = Store::open().context("opening theme store")?;
    let sub = args.first().map(String::as_str).unwrap_or("");
    let rest = args.get(1..).unwrap_or(&[]);

    match sub {
        // `/theme` with no args opens the interactive selector.
        "" | "select" => select_interactive(&store, ui)?,
        "list" => print_list(ui, &store),
        "use" => {
            let name = rest.join(" ");
            if name.is_empty() {
                println!("  {}", ui.dim("usage: /theme use <name>"));
            } else {
                activate(&store, ui, &name)?;
            }
        }
        "new" | "create" | "vibe" => {
            let description = rest.join(" ");
            create_by_vibe(&store, agent, ui, &description).await?;
        }
        "export" => {
            let path = rest.join(" ");
            if path.is_empty() {
                println!("  {}", ui.dim("usage: /theme export <path>"));
            } else {
                let active = ui.theme().meta.name.clone();
                let dest = store.export(&active, &path)?;
                println!(
                    "  {} {}",
                    ui.green("✓ exported active theme to"),
                    ui.cream(&dest.display().to_string())
                );
            }
        }
        "import" => {
            let path = rest.join(" ");
            if path.is_empty() {
                println!("  {}", ui.dim("usage: /theme import <path>"));
            } else {
                let theme = store.import(PathBuf::from(&path))?;
                let name = theme.meta.name.clone();
                println!("  {} {}", ui.green("✓ imported"), ui.cream(&name));
                activate(&store, ui, &name)?;
            }
        }
        _ => {
            println!(
            "  {}",
            ui.dim("usage: /theme [list | use <name> | new [vibe] | import <path> | export <path>]")
        )
        }
    }
    Ok(())
}

/// Render the list of themes with the active one marked.
fn print_list(ui: &Ui, store: &Store) {
    println!();
    println!("  {}", ui.title("Available themes"));
    for s in store.list() {
        let marker = if s.active {
            ui.green("●")
        } else {
            ui.dim("○")
        };
        let tag = if s.builtin {
            ui.dim("built-in")
        } else {
            ui.brown("custom")
        };
        println!(
            "  {} {}  {}  {}",
            marker,
            ui.cream(&format!("{:<16}", s.name)),
            tag,
            ui.dim(&s.description),
        );
    }
    println!();
    println!(
        "  {}",
        ui.dim("Switch with  /theme use <name>   ·   create one with  /theme new")
    );
}

/// Switch to a named theme and hot-swap the live UI.
fn activate(store: &Store, ui: &mut Ui, name: &str) -> Result<()> {
    let theme = store
        .set_active(name)
        .with_context(|| format!("no theme `{name}`"))?;
    *ui = ui.with_theme(Arc::new(theme.clone()));
    println!(
        "  {} {}",
        ui.green("✓ wearing theme:"),
        ui.cream(&theme.meta.name)
    );
    preview(ui);
    Ok(())
}

/// Interactive theme selection via the picker.
fn select_interactive(store: &Store, ui: &mut Ui) -> Result<()> {
    let summaries = store.list();
    let options: Vec<Choice> = summaries
        .iter()
        .map(|s| {
            let tag = if s.builtin { "built-in" } else { "custom" };
            Choice::new(s.name.clone(), format!("{tag} · {}", s.description))
        })
        .collect();
    let ui_owned = ui.clone();
    let chosen = picker::select(&ui_owned, "Theme", "Choose a theme", &options, false)
        .context("theme picker failed")?;
    if let Some(labels) = chosen {
        if let Some(name) = labels.first() {
            activate(store, ui, name)?;
        }
    }
    Ok(())
}

/// Print a small sample so the user can see the active theme's colors + voice.
fn preview(ui: &Ui) {
    println!();
    println!("  {}", theme::prompt(ui).trim_end());
    println!(
        "    {}  {}  {}  {}",
        ui.title("title"),
        ui.accent("accent"),
        ui.cream("text"),
        ui.dim("muted"),
    );
    println!("    {}", ui.red(&ui.death()));
    println!();
}

/// Vibe-code a brand new theme: a short interview + LLM generation.
async fn create_by_vibe(
    store: &Store,
    agent: &Agent,
    ui: &mut Ui,
    description: &str,
) -> Result<()> {
    let answers = match interview(ui, description)? {
        Some(a) => a,
        None => {
            println!("  {}", ui.dim("(theme creation cancelled)"));
            return Ok(());
        }
    };

    println!(
        "  {} {}",
        ui.brown("🎨 Designing your theme with"),
        ui.accent(agent.model()),
    );
    let spinner = theme::Spinner::start(
        ui,
        vec!["Mixing the colors".into(), "Choosing the words".into()],
    );
    let raw = agent
        .complete(&Theme::generation_system_prompt(), &answers)
        .await;
    spinner.stop();
    let raw = raw.context("model failed to generate a theme")?;

    let theme = Theme::from_model_output(&raw).context("the model did not return a valid theme")?;
    store.save(&theme).context("saving the new theme")?;
    let name = theme.meta.name.clone();
    store.set_active(&name)?;
    *ui = ui.with_theme(Arc::new(theme));

    println!(
        "  {} {}",
        ui.green("✓ created + activated:"),
        ui.cream(&name)
    );
    preview(ui);
    println!(
        "  {}",
        ui.dim(&format!(
            "Saved to {}. Share it with  /theme export <path>",
            store.themes_dir().display()
        ))
    );
    Ok(())
}

/// Ask a few quick questions to shape the vibe, returning a combined brief.
/// Runs the blocking picker off-thread. `None` if cancelled.
fn interview(ui: &Ui, description: &str) -> Result<Option<String>> {
    let ui = ui.clone();
    let description = description.to_string();
    let brief = tokio::task::block_in_place(move || run_interview(&ui, &description))?;
    Ok(brief)
}

fn run_interview(ui: &Ui, description: &str) -> Result<Option<String>> {
    let mood = picker::select(
        ui,
        "Mood",
        "What overall mood do you want?",
        &[
            Choice::new("Cozy & warm", "soft, earthy, inviting"),
            Choice::new("Sleek & dark", "modern, low-key, focused"),
            Choice::new("Loud & vibrant", "bold neon energy"),
            Choice::new("Minimal & calm", "quiet, restrained, clean"),
        ],
        false,
    )?;
    let Some(mood) = mood else { return Ok(None) };

    let colors = picker::select(
        ui,
        "Colors",
        "Pick a color inspiration",
        &[
            Choice::new("Sunset oranges", "warm reds, ambers, golds"),
            Choice::new("Ocean blues", "teal, navy, cyan"),
            Choice::new("Forest greens", "moss, pine, sage"),
            Choice::new("Neon magenta + cyan", "retro synthwave"),
            Choice::new("Monochrome", "grayscale with one accent"),
        ],
        false,
    )?;
    let Some(colors) = colors else {
        return Ok(None);
    };

    let voice = picker::select(
        ui,
        "Voice",
        "What personality should the words have?",
        &[
            Choice::new("Playful & punny", "jokes, theme gags, fun phrases"),
            Choice::new("Professional & terse", "clean, no-nonsense"),
            Choice::new("Epic & dramatic", "grand, cinematic language"),
            Choice::new("Chill & friendly", "casual and warm"),
        ],
        false,
    )?;
    let Some(voice) = voice else { return Ok(None) };

    let desc = if description.trim().is_empty() {
        "(no extra description)".to_string()
    } else {
        description.to_string()
    };
    Ok(Some(format!(
        "Create a complete terminal theme.\n\
         User's description: {desc}\n\
         Mood: {}\n\
         Color inspiration: {}\n\
         Voice/personality: {}\n\n\
         Give it a short, evocative name. Make the palette cohesive and readable \
         on a dark terminal, and write all the voice phrases (thinking, tool_verbs, \
         deaths, subtitle, labels, help_items, banner_art) to match the mood and \
         personality. Output the theme now.",
        mood.join(", "),
        colors.join(", "),
        voice.join(", "),
    )))
}
