//! The `/model` command: show, switch, or add a model — plus the shared
//! model-row catalog the interactive picker and the composer's completion
//! list both render from, so the two surfaces can't drift.
//!
//! `/model roles` is the same catalog pointed at the *other* models a session
//! uses: the summary model compaction runs on, the smol model fleet and review
//! lanes ride, and the fallback chain a failing provider hands off to. They
//! live in `~/.oxen-harness/limits.json` (see [`harness_runtime::limits`]) and
//! were hand-edited JSON until now.

use anyhow::Result;
use harness_agent::Agent;
use harness_local::source::ModelPricing;

use crate::picker::{self, Choice};
use crate::theme::Ui;

/// One model the user can pick: a cloud-catalog entry or an installed local
/// model, described the same way everywhere it's shown.
pub(crate) struct ModelRow {
    /// The id to switch to (what `/model <id>` takes).
    pub(crate) id: String,
    /// Human-friendly display name (may be empty for bare cloud ids).
    pub(crate) title: String,
    /// Where it runs: `cloud` or `local · 8B · Q4`.
    pub(crate) source: String,
    /// Whether this is an installed local (llama.cpp) model.
    pub(crate) local: bool,
}

impl ModelRow {
    /// The `title · source` description, skipping an empty title so a bare id
    /// never renders a dangling separator.
    pub(crate) fn describe(&self) -> String {
        if self.title.is_empty() {
            self.source.clone()
        } else {
            format!("{} · {}", self.title, self.source)
        }
    }
}

/// Every model the user can pick — the cloud catalog plus installed local
/// models — sorted by id and deduplicated. The single source of the rows
/// behind the interactive `/model` picker and the live composer's
/// `/model <arg>` completion.
pub(crate) fn model_rows() -> Vec<ModelRow> {
    let mut rows: Vec<ModelRow> = harness_runtime::models::catalog()
        .into_iter()
        .map(|m| ModelRow {
            title: m.name,
            source: "cloud".to_string(),
            local: false,
            id: m.id,
        })
        .collect();
    if let Ok(store) = harness_local::ModelStore::open() {
        rows.extend(store.installed().into_iter().map(|m| {
            let meta = [m.params.as_str(), m.quant.as_str()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
            ModelRow {
                title: m.display,
                source: if meta.is_empty() {
                    "local".to_string()
                } else {
                    format!("local · {meta}")
                },
                local: true,
                id: m.id,
            }
        }));
    }
    rows.sort_by_key(|r| r.id.to_lowercase());
    rows.dedup_by(|a, b| a.id == b.id);
    rows
}

/// A short price tag for a model, shown in the picker so the cost is visible
/// at a glance: `free` for local (llama.cpp) models — they run on your own
/// hardware — and a per-million-token rate like `$3/M in · $15/M out` for a
/// cloud model the endpoint catalog prices. A cloud model the catalog doesn't
/// list yields `None` (no tag) rather than implying it's free.
fn price_tag(
    row: &ModelRow,
    pricing: Option<&std::collections::HashMap<String, ModelPricing>>,
) -> Option<String> {
    if row.local {
        return Some("free".to_string());
    }
    let rate = pricing?.get(&row.id)?;
    crate::pricing::format_rate(rate)
}

/// Fetch the active endpoint's pricing catalog for the picker. Best-effort: a
/// failed request just means no price tags, and local models are free
/// regardless.
async fn pricing_catalog() -> Option<std::collections::HashMap<String, ModelPricing>> {
    let connection = harness_runtime::connection::load();
    let base_url = harness_runtime::connection::effective_base_url(&connection);
    let token = harness_runtime::connection::effective_api_key(&base_url);
    harness_local::source::oxen_model_pricing_catalog_at(
        &base_url,
        (!token.trim().is_empty()).then_some(token.as_str()),
    )
    .await
    .ok()
}

/// `/model [id]` — switch models. With no argument, opens the interactive
/// picker over the cloud catalog + installed local models (current one
/// marked, each with its price — free for local, per-million-token rates for
/// cloud); its "type my own answer" row takes a brand-new id. An id we've
/// never seen is saved as a custom catalog entry so it shows up in the picker
/// from then on — here and in the desktop. The choice is persisted as the
/// default for future sessions.
pub(crate) async fn handle_repl(rest: Option<String>, agent: &mut Agent, ui: &Ui) -> Result<()> {
    // `/model roles …` routes here rather than to a command of its own: the
    // registry already sends every `/model <anything>` this way, and roles are
    // picked from the same catalog as the session model.
    if let Some(cmd) = rest.as_deref().and_then(parse_roles) {
        return handle_roles(cmd, agent, ui);
    }
    // The current-model readout, with its cached per-million rate when known
    // (warmed at startup/turn boundaries) — price stays visible even when the
    // user just asks what they're riding.
    let yoked = |model: &str| {
        let rate = crate::pricing::session_rate(model)
            .and_then(|r| crate::pricing::format_rate(&r))
            .map(|r| format!(" · {r}"))
            .unwrap_or_default();
        println!(
            "  {} {}{}",
            ui.brown("oxen yoked:"),
            ui.cream(model),
            ui.dim(&rate)
        );
    };
    let rows = model_rows();
    let chosen = match rest {
        Some(id) => id,
        None => {
            let current = agent.model().to_string();
            let mark = |id: &str| if id == current { " ← current" } else { "" };
            // One catalog request feeds every row's price tag.
            let pricing = pricing_catalog().await;
            let options: Vec<Choice> = rows
                .iter()
                .map(|r| {
                    let price = price_tag(r, pricing.as_ref())
                        .map(|p| format!(" · {p}"))
                        .unwrap_or_default();
                    Choice::new(
                        r.id.clone(),
                        format!("{}{price}{}", r.describe(), mark(&r.id)),
                    )
                })
                .collect();
            let question = format!("Yoked to `{current}` — trade for another?");
            match picker::select(ui, "Model", &question, &options, false)? {
                Some(sel) => sel.into_iter().next().unwrap_or_default(),
                // Cancelled, or no interactive terminal (piped input) — just
                // report the current model like `/model` always did there.
                None => {
                    yoked(agent.model());
                    return Ok(());
                }
            }
        }
    };

    let id = chosen.trim();
    if id.is_empty() {
        yoked(agent.model());
        return Ok(());
    }

    // A local model can't be swapped into a live cloud session (it needs its
    // own llama-server, started at launch). Persist it as the active local
    // model — the same switch the desktop dropdown makes — and say how to
    // ride it, instead of pointing the cloud client at a GGUF id.
    if rows.iter().any(|r| r.local && r.id == id) {
        match harness_runtime::models::set_active_local(id) {
            Ok(()) => {
                println!(
                    "  {} {}",
                    ui.brown("🐂 local oxen picked:"),
                    ui.cream(&format!("{id} — saved as your local model")),
                );
                println!(
                    "  {}",
                    ui.dim(&format!(
                        "restart to ride it: oxen-harness (or oxen-harness --local {id})"
                    )),
                );
            }
            Err(e) => println!("  {} {e}", ui.dim("couldn't save the local selection:")),
        }
        return Ok(());
    }

    agent.set_model(id);
    // The new model's catalog-reported limits, when cached; `None` falls back
    // to the name-derived window and the configured reply reserve.
    agent.set_context_window(harness_local::limits::context_window(id));
    agent.set_max_output_tokens(harness_local::limits::max_output_tokens(id));
    // Follow the swap through to the fleet spawner so a later spawn_agents
    // fleet runs on the new model, not the one captured at startup.
    crate::endpoint::update_fleet_endpoint(None, Some(id));
    // An id we've never seen (not in the cloud catalog, not an installed local
    // model) is saved as a custom catalog entry so it shows up in the picker
    // from now on — here and in the desktop. Only a *cataloged* id is
    // persisted as the default: if the save fails, the live session still
    // switches, but the config never points at a model the catalog can't show.
    let known = rows.iter().any(|r| r.id == id);
    let cataloged = known
        || match harness_runtime::models::add(id, "") {
            Ok(_) => {
                println!(
                    "  {} {}",
                    ui.dim("new model saved to the catalog:"),
                    ui.cream(id)
                );
                true
            }
            Err(e) => {
                println!("  {} {e}", ui.dim("couldn't save to the catalog:"));
                false
            }
        };
    // Persist the choice (clearing any local selection) so it's the default
    // next launch — here and in the desktop dropdown.
    if cataloged {
        let _ = harness_runtime::models::set_selected(id);
    }
    // Price transparency on every switch: warm the shared rate cache for the
    // new model (also feeds the context trailer + completion picker) and show
    // what it costs right in the confirmation.
    crate::pricing::warm_for(id).await;
    let rate = crate::pricing::session_rate(id)
        .and_then(|r| crate::pricing::format_rate(&r))
        .map(|r| format!(" · {r}"))
        .unwrap_or_default();
    println!(
        "  {} {}{}",
        ui.brown("fresh oxen yoked:"),
        ui.accent(id),
        ui.dim(&rate)
    );
    Ok(())
}

// ===========================================================================
// `/model roles` — the models a session routes work to (limits.json)
// ===========================================================================

/// The row label that clears a role back to the session model.
const SESSION_ROW: &str = "session model (default)";

/// Words that clear a role rather than name a model.
const CLEAR_WORDS: &[&str] = &["clear", "none", "default", "off"];

/// A routed model role, as persisted in `limits.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Summary,
    Smol,
    Fallback,
}

impl Role {
    fn parse(word: &str) -> Option<Self> {
        match word.to_ascii_lowercase().as_str() {
            "summary" | "summaries" => Some(Self::Summary),
            "smol" | "small" => Some(Self::Smol),
            "fallback" | "fallbacks" => Some(Self::Fallback),
            _ => None,
        }
    }

    /// The role's name in the table and in confirmations.
    fn label(self) -> &'static str {
        match self {
            Self::Summary => "summary",
            Self::Smol => "smol",
            Self::Fallback => "fallbacks",
        }
    }

    /// What the role is for, one line, shown beside its assignment.
    fn blurb(self) -> &'static str {
        match self {
            Self::Summary => "compaction summaries",
            Self::Smol => "fleet lanes and review passes",
            Self::Fallback => "tried in order when the session model keeps failing",
        }
    }

    /// How a confirmation names the role's assignment.
    fn assignment(self) -> &'static str {
        match self {
            Self::Summary => "summary model",
            Self::Smol => "smol model",
            Self::Fallback => "fallback chain",
        }
    }

    /// The question the picker asks for this role.
    fn question(self) -> &'static str {
        match self {
            Self::Summary => "Which model should write compaction summaries?",
            Self::Smol => "Which model should run fleet lanes and review passes?",
            Self::Fallback => {
                "Pick the fallback models, in the order they should be tried \
                 (space checks, enter confirms)."
            }
        }
    }
}

/// The parsed form of `/model roles …`.
#[derive(Debug, PartialEq, Eq)]
enum RolesCmd {
    /// `/model roles` — print the current assignments.
    Show,
    /// `/model roles <role>` — choose from the picker.
    Pick(Role),
    /// `/model roles <role> <model-id>…` — assign directly. `fallback` takes
    /// the whole list, in the order given; the single-model roles take one.
    Set(Role, Vec<String>),
    /// `/model roles <role> clear` — drop the assignment.
    Clear(Role),
    /// `/model roles <word>` — not a role we know.
    Unknown(String),
}

/// Parse `/model`'s argument as a roles command, or `None` when it's an
/// ordinary `/model <id>` switch.
fn parse_roles(rest: &str) -> Option<RolesCmd> {
    let mut parts = rest.split_whitespace();
    let head = parts.next()?;
    if !head.eq_ignore_ascii_case("roles") && !head.eq_ignore_ascii_case("role") {
        return None;
    }
    let Some(word) = parts.next() else {
        return Some(RolesCmd::Show);
    };
    let Some(role) = Role::parse(word) else {
        return Some(RolesCmd::Unknown(word.to_string()));
    };
    let ids: Vec<String> = parts.map(str::to_string).collect();
    Some(match ids.first() {
        None => RolesCmd::Pick(role),
        Some(v) if CLEAR_WORDS.contains(&v.to_ascii_lowercase().as_str()) => RolesCmd::Clear(role),
        _ => RolesCmd::Set(role, ids),
    })
}

/// What a typed model id resolved to against the candidate list.
#[derive(Debug, PartialEq, Eq)]
enum Resolved {
    /// Exactly one candidate matched.
    Id(String),
    /// A prefix that several candidates share.
    Ambiguous(Vec<String>),
    /// Nothing in the catalog matches.
    Unknown,
}

/// Resolve a typed model id against the pickable candidates: an exact id
/// (case-insensitive) wins outright, otherwise a *unique* prefix does, so
/// `/model roles smol qwen3` lands without typing the quantization suffix.
fn resolve_model_id(ids: &[&str], input: &str) -> Resolved {
    let needle = input.trim().to_lowercase();
    if needle.is_empty() {
        return Resolved::Unknown;
    }
    if let Some(hit) = ids.iter().find(|id| id.to_lowercase() == needle) {
        return Resolved::Id((*hit).to_string());
    }
    let hits: Vec<String> = ids
        .iter()
        .filter(|id| id.to_lowercase().starts_with(&needle))
        .map(|id| (*id).to_string())
        .collect();
    match hits.len() {
        0 => Resolved::Unknown,
        1 => Resolved::Id(hits.into_iter().next().unwrap_or_default()),
        _ => Resolved::Ambiguous(hits),
    }
}

/// `/model roles [summary|smol|fallback [model-id|clear]]` — show or change
/// the models a session routes work to. Roles are persisted, not live: like
/// the desktop's settings pages, they apply to new and resumed chats.
fn handle_roles(cmd: RolesCmd, agent: &Agent, ui: &Ui) -> Result<()> {
    match cmd {
        RolesCmd::Show => {
            print_roles(agent, ui);
            Ok(())
        }
        RolesCmd::Unknown(word) => {
            println!(
                "  {} {}",
                ui.red("✗"),
                ui.dim(&format!(
                    "unknown role `{word}` — expected summary, smol, or fallback"
                )),
            );
            Ok(())
        }
        RolesCmd::Clear(role) => assign(role, Vec::new(), ui),
        RolesCmd::Set(role, wanted) => {
            if role != Role::Fallback && wanted.len() > 1 {
                println!(
                    "  {} {}",
                    ui.red("✗"),
                    ui.dim(&format!(
                        "the {} role takes one model — only `fallback` is a list",
                        role.label()
                    )),
                );
                return Ok(());
            }
            let rows = model_rows();
            let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
            let mut chosen = Vec::with_capacity(wanted.len());
            for want in &wanted {
                match resolve_model_id(&ids, want) {
                    Resolved::Id(id) => chosen.push(id),
                    // Half-applying a chain would be worse than not applying
                    // it: say what went wrong and leave the config alone.
                    Resolved::Ambiguous(hits) => {
                        println!(
                            "  {} {}",
                            ui.red("✗"),
                            ui.dim(&format!(
                                "`{want}` matches {} — be more specific",
                                hits.join(", ")
                            )),
                        );
                        return Ok(());
                    }
                    Resolved::Unknown => {
                        println!(
                            "  {} {}",
                            ui.red("✗"),
                            ui.dim(&format!(
                                "no model matches `{want}` — `/model {want}` adds it to the \
                                 catalog first"
                            )),
                        );
                        return Ok(());
                    }
                }
            }
            assign(role, chosen, ui)
        }
        RolesCmd::Pick(role) => pick_role(role, ui),
    }
}

/// The current assignments as a small aligned table, with the spend ceiling
/// (the other thing `limits.json` holds) so one command reads the whole file.
fn print_roles(agent: &Agent, ui: &Ui) {
    let limits = harness_runtime::limits::load();
    let dash = "—".to_string();
    let fallbacks = if limits.fallback_models.is_empty() {
        dash.clone()
    } else {
        limits.fallback_models.join(" → ")
    };
    let ceiling = limits
        .max_session_tokens
        .map(|t| format!("{} tokens", crate::turn::human_tokens(t)))
        .unwrap_or_else(|| dash.clone());
    let rows = [
        ("session", agent.model().to_string(), "this chat's model"),
        (
            Role::Summary.label(),
            limits.summary_model.clone().unwrap_or_else(|| dash.clone()),
            Role::Summary.blurb(),
        ),
        (
            Role::Smol.label(),
            limits.smol_model.clone().unwrap_or_else(|| dash.clone()),
            Role::Smol.blurb(),
        ),
        (Role::Fallback.label(), fallbacks, Role::Fallback.blurb()),
        ("spend cap", ceiling, "per-session token ceiling"),
    ];
    let value_width = rows
        .iter()
        .map(|(_, value, _)| crate::width::str_width(value))
        .max()
        .unwrap_or(0);
    println!("  {}", ui.brown("🐂 model roles"));
    for (name, value, blurb) in rows {
        // Pad the raw text, then color: a colored string carries escape bytes
        // that a `{:<10}` fill would count as visible columns.
        let pad = " ".repeat(value_width.saturating_sub(crate::width::str_width(&value)));
        println!(
            "    {} {}{pad}  {}",
            ui.dim(&format!("{name:<10}")),
            ui.cream(&value),
            ui.dim(blurb),
        );
    }
    println!(
        "  {}",
        ui.dim(
            "/model roles summary|smol|fallback [model-id] to change \
             · fallback clear to empty it · applies to new and resumed chats"
        )
    );
}

/// Open the picker for a role over the same candidates as `/model` — plus, for
/// the single-model roles, a row that clears the override. `fallback` is
/// multi-select and keeps the order the rows were checked in.
fn pick_role(role: Role, ui: &Ui) -> Result<()> {
    let rows = model_rows();
    if rows.is_empty() {
        println!(
            "  {}",
            ui.dim("no models in the catalog yet — `/model <id>` adds one, then set a role")
        );
        return Ok(());
    }
    let limits = harness_runtime::limits::load();
    let assigned: Vec<String> = match role {
        Role::Summary => limits.summary_model.clone().into_iter().collect(),
        Role::Smol => limits.smol_model.clone().into_iter().collect(),
        Role::Fallback => limits.fallback_models.clone(),
    };
    let mark = |id: &str| match assigned.iter().position(|a| a == id) {
        Some(_) if role != Role::Fallback => " ← current".to_string(),
        Some(i) => format!(" ← #{}", i + 1),
        None => String::new(),
    };
    let mut options: Vec<Choice> = Vec::new();
    if role != Role::Fallback {
        let mark = if assigned.is_empty() {
            " ← current"
        } else {
            ""
        };
        options.push(Choice::new(
            SESSION_ROW,
            format!("run this work on the session model{mark}"),
        ));
    }
    options.extend(
        rows.iter()
            .map(|r| Choice::new(r.id.clone(), format!("{}{}", r.describe(), mark(&r.id)))),
    );
    let picked = picker::select(
        ui,
        "Model roles",
        role.question(),
        &options,
        role == Role::Fallback,
    )?;
    let Some(picked) = picked else {
        // Cancelled, or no interactive terminal (piped input) — show what's
        // set rather than changing anything.
        println!("  {}", ui.dim("nothing changed."));
        return Ok(());
    };
    let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
    let mut chosen: Vec<String> = Vec::new();
    for label in picked {
        if label == SESSION_ROW {
            continue;
        }
        match resolve_model_id(&ids, &label) {
            Resolved::Id(id) => chosen.push(id),
            // A typed answer that names nothing we know: refuse the whole
            // change rather than silently routing to a model that isn't there.
            _ => {
                println!(
                    "  {} {}",
                    ui.red("✗"),
                    ui.dim(&format!("no model matches `{label}` — nothing changed")),
                );
                return Ok(());
            }
        }
    }
    assign(role, chosen, ui)
}

/// Persist a role's new assignment and say what it means. An empty `chosen`
/// clears the role.
fn assign(role: Role, chosen: Vec<String>, ui: &Ui) -> Result<()> {
    let mut limits = harness_runtime::limits::load();
    match role {
        Role::Summary => limits.summary_model = chosen.first().cloned(),
        Role::Smol => limits.smol_model = chosen.first().cloned(),
        Role::Fallback => limits.fallback_models = chosen.clone(),
    }
    if let Err(e) = harness_runtime::limits::save(&limits) {
        println!("  {} {e}", ui.dim("couldn't save the role:"));
        return Ok(());
    }
    let value = match (chosen.is_empty(), role) {
        (false, _) => chosen.join(" → "),
        (true, Role::Fallback) => "none — a failing model ends the turn".to_string(),
        (true, _) => "the session model".to_string(),
    };
    println!(
        "  {} {}",
        ui.brown(&format!("🐂 {}:", role.assignment())),
        ui.accent(&value),
    );
    println!(
        "  {}",
        ui.dim(&format!(
            "{} · applies to new and resumed chats",
            role.blurb()
        ))
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn row(id: &str, local: bool) -> ModelRow {
        ModelRow {
            id: id.to_string(),
            title: id.to_string(),
            source: if local {
                "local".into()
            } else {
                "cloud".into()
            },
            local,
        }
    }

    #[test]
    fn local_models_are_free() {
        // Local models run on your own hardware — always free, no catalog lookup.
        let tag = price_tag(&row("qwen3-8b", true), None);
        assert_eq!(tag.as_deref(), Some("free"));
    }

    #[test]
    fn cloud_model_shows_its_catalog_rate() {
        let mut catalog = HashMap::new();
        catalog.insert(
            "muse-spark-1".to_string(),
            ModelPricing {
                input_cost_per_token: 0.000_003,
                output_cost_per_token: 0.000_015,
            },
        );
        let tag = price_tag(&row("muse-spark-1", false), Some(&catalog));
        assert_eq!(tag.as_deref(), Some("$3/M in · $15/M out"));
    }

    #[test]
    fn roles_argument_parses_into_show_pick_set_and_clear() {
        // Anything that isn't the `roles` keyword stays an ordinary switch.
        assert_eq!(parse_roles("claude-opus-4-8"), None);
        assert_eq!(parse_roles(""), None);

        assert_eq!(parse_roles("roles"), Some(RolesCmd::Show));
        assert_eq!(parse_roles("  ROLES  "), Some(RolesCmd::Show));
        assert_eq!(
            parse_roles("roles summary"),
            Some(RolesCmd::Pick(Role::Summary))
        );
        assert_eq!(parse_roles("role smol"), Some(RolesCmd::Pick(Role::Smol)));
        assert_eq!(
            parse_roles("roles fallbacks"),
            Some(RolesCmd::Pick(Role::Fallback))
        );
        assert_eq!(
            parse_roles("roles summary gemini-2-5-flash"),
            Some(RolesCmd::Set(
                Role::Summary,
                vec!["gemini-2-5-flash".into()]
            ))
        );
        // A fallback chain can be given in one line, in order.
        assert_eq!(
            parse_roles("roles fallback claude-sonnet-5 gemini-2-5-flash"),
            Some(RolesCmd::Set(
                Role::Fallback,
                vec!["claude-sonnet-5".into(), "gemini-2-5-flash".into()]
            ))
        );
        assert_eq!(
            parse_roles("roles fallback clear"),
            Some(RolesCmd::Clear(Role::Fallback))
        );
        // `none`/`default` clear a single-model role the same way.
        assert_eq!(
            parse_roles("roles smol none"),
            Some(RolesCmd::Clear(Role::Smol))
        );
        assert_eq!(
            parse_roles("roles wagon"),
            Some(RolesCmd::Unknown("wagon".into()))
        );
    }

    #[test]
    fn model_ids_resolve_by_exact_match_or_unique_prefix() {
        let ids = ["claude-opus-4-8", "claude-sonnet-5", "qwen3-8b-q4-k-m"];
        assert_eq!(
            resolve_model_id(&ids, "claude-sonnet-5"),
            Resolved::Id("claude-sonnet-5".into())
        );
        // Case-insensitive, and whitespace-tolerant.
        assert_eq!(
            resolve_model_id(&ids, "  CLAUDE-SONNET-5 "),
            Resolved::Id("claude-sonnet-5".into())
        );
        // A unique prefix is enough.
        assert_eq!(
            resolve_model_id(&ids, "qwen3"),
            Resolved::Id("qwen3-8b-q4-k-m".into())
        );
        // A shared prefix names both rather than guessing.
        assert_eq!(
            resolve_model_id(&ids, "claude-"),
            Resolved::Ambiguous(vec!["claude-opus-4-8".into(), "claude-sonnet-5".into()])
        );
        assert_eq!(resolve_model_id(&ids, "gpt"), Resolved::Unknown);
        assert_eq!(resolve_model_id(&ids, "   "), Resolved::Unknown);
    }

    #[test]
    fn an_exact_id_beats_a_longer_id_it_prefixes() {
        // `claude-opus-4-8` is also a prefix of the dated alias — the exact
        // match must still win instead of reading as ambiguous.
        let ids = ["claude-opus-4-8", "claude-opus-4-8-20260101"];
        assert_eq!(
            resolve_model_id(&ids, "claude-opus-4-8"),
            Resolved::Id("claude-opus-4-8".into())
        );
    }

    #[test]
    fn cloud_model_absent_from_catalog_has_no_tag() {
        // Unlisted (or no catalog at all) → no tag, rather than implying free.
        let catalog = HashMap::new();
        assert!(price_tag(&row("mystery-model", false), Some(&catalog)).is_none());
        assert!(price_tag(&row("mystery-model", false), None).is_none());
    }
}
