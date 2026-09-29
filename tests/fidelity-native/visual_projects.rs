//! `visual-projects` phase of the native flow (spec 011, AC-011-01/02/03):
//! 1. Project tree with collapsible groups, projects with branch, host badges, agent dots,
//!    active project surface-3 and 2px accent bar, menu "…".
//! 2. Connections footer with "Este computador · Local", SSH hosts with latency or offline,
//!    status dot, "+" button to open dialog.
//! 3. Connection dialog with 620px width, 12px radius, Local/SSH cards, fields,
//!    and ordered 4-line progress list.
//! 4. WCAG small text contrast >= 4.5:1.

use serde_json::Value;

pub use super::paste_flow::ParentLedger as Ledger;
pub use super::visual_frame::contrast;

pub const PHASE: &str = "visual-projects";
pub const STEPS: [&str; 3] = [
    "visual-projects-sidebar",
    "visual-projects-dialog",
    "visual-projects-close",
];
pub const CHECKS: [&str; 5] = [
    "project_tree_groups_and_projects",
    "active_project_surface3_and_accent_bar",
    "connections_footer_and_host_selection",
    "connection_dialog_layout_and_progress",
    "small_text_contrast_at_least_4_5",
];

const MIN_SMALL_TEXT_RATIO: f64 = 4.5;
const TOLERANCE: f64 = 2.0;

fn text(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

fn num(v: &Value) -> Option<f64> {
    v.as_f64()
}

fn field<'a>(v: &'a Value, key: &str, what: &str) -> Result<&'a Value, String> {
    match v.get(key) {
        Some(found) if !found.is_null() => Ok(found),
        _ => Err(format!("{PHASE}: {what} has no {key}")),
    }
}

pub fn checks(report: &Value, ledger: &Ledger) -> Result<Vec<(&'static str, bool)>, String> {
    if report["phase"] != PHASE {
        return Err(format!("{PHASE}: report of phase {}", report["phase"]));
    }
    if !report["error"].is_null() {
        return Err(format!("{PHASE}: page error {}", report["error"]));
    }
    let answer = |step: &str| {
        ledger
            .steps
            .get(step)
            .ok_or_else(|| format!("{PHASE}: parent never answered {step}"))
    };
    for step in STEPS {
        answer(step)?;
    }

    let sidebar = field(report, "sidebar", "report")?;
    let groups = sidebar["groups"].as_array().cloned().unwrap_or_default();
    let projects = sidebar["projects"].as_array().cloned().unwrap_or_default();
    let connections = sidebar["connections"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    // 1. AC-011-01: Groups and projects
    let groups_ok = !groups.is_empty()
        && groups.iter().all(|g| {
            !text(&g["name"]).is_empty()
                && !text(&g["color"]).is_empty()
                && g["count"].as_u64().is_some()
        });

    let projects_ok = !projects.is_empty()
        && projects.iter().all(|p| {
            !text(&p["name"]).is_empty() && !text(&p["branch"]).is_empty() && p["has_menu"] == true
        });

    // AC-011-01: Agent dots belong strictly to the project's workspace. The parent starts the
    // deterministic agent in the page's confirmed pane, so the active project (bound to that
    // pane's workspace) must show exactly the agents the engine lists for that workspace, while
    // projects without agents in their workspace show zero dots.
    let sidebar_answer = answer("visual-projects-sidebar")?;
    let agent_pane = text(&sidebar_answer["agent_pane"]);
    let agent_workspace = agent_pane.split(':').next().unwrap_or("");
    let engine_agents = sidebar_answer["agent_list_after_start"]["result"]["agents"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let in_workspace = engine_agents
        .iter()
        .filter(|a| {
            let ws = text(&a["workspace_id"]);
            ws == agent_workspace
                || (ws.is_empty() && text(&a["pane_id"]).split(':').next() == Some(agent_workspace))
        })
        .count();

    let active_p = projects.iter().find(|p| p["active"] == true);
    let active_dots = active_p
        .and_then(|p| p["agent_dots"].as_array())
        .cloned()
        .unwrap_or_default();
    let active_dots_ok = !agent_workspace.is_empty()
        && in_workspace > 0
        && active_dots.len() == in_workspace
        && active_dots.iter().all(|d| {
            let s = text(&d["status"]);
            let l = text(&d["label"]);
            !l.is_empty() && (s == "working" || s == "idle" || s == "blocked")
        });

    let other_projects_zero_dots = projects.iter().filter(|p| p["active"] != true).all(|p| {
        let dots = p["agent_dots"].as_array().cloned().unwrap_or_default();
        dots.is_empty()
    });

    let agent_dots_ok = active_dots_ok && other_projects_zero_dots;
    let tree_ok = groups_ok && projects_ok && agent_dots_ok;

    // 2. AC-011-01: Active project surface-3 and 2px accent bar
    let active_p = projects.iter().find(|p| p["active"] == true);
    let active_ok = active_p.is_some_and(|p| {
        let bg = text(&p["bg_color"]).to_ascii_lowercase();
        let is_surface3 = bg.contains("30, 34, 43") || bg.contains("1e222b");
        let accent_w = num(&p["accent_bar_width"]).unwrap_or(0.0);
        let accent_ok = (accent_w - 2.0).abs() <= 1.0;
        is_surface3 && accent_ok
    });

    // 3. AC-011-02: Connections footer
    let conn_ok = !connections.is_empty()
        && connections.first().is_some_and(|c| {
            text(&c["label"]).contains("Local") || text(&c["type_badge"]) == "Local"
        })
        && connections
            .iter()
            .all(|c| !text(&c["status_text"]).is_empty());

    // 4. AC-011-03: Connection dialog
    let dialog = field(report, "dialog", "report")?;
    let dialog_closed = report["dialog_closed"] == true;
    let width = num(&dialog["width"]).unwrap_or(0.0);
    let radius = num(&dialog["border_radius"]).unwrap_or(0.0);
    let cards = dialog["cards"].as_array().cloned().unwrap_or_default();
    let fields = dialog["fields"].as_array().cloned().unwrap_or_default();
    let progress = dialog["progress_items"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    let dialog_dims_ok = (width - 620.0).abs() <= TOLERANCE && (radius - 12.0).abs() <= TOLERANCE;

    let cards_ok = cards.len() == 2
        && cards.iter().any(|c| text(&c["kind"]) == "local")
        && cards.iter().any(|c| text(&c["kind"]) == "ssh")
        && cards.iter().all(|c| text(&c["role"]) == "radio");

    let fields_ok = fields.iter().any(|f| text(f).contains("Host"))
        && fields.iter().any(|f| text(f).contains("Porta"))
        && fields.iter().any(|f| text(f).contains("Autenticação"))
        && fields.iter().any(|f| text(f).contains("Nome de exibição"))
        && fields.iter().any(|f| text(f).contains("Sessão"));

    let progress_ok = progress.len() == 4
        && text(&progress[0]["text"]).contains("Host alcançável")
        && text(&progress[1]["text"]).contains("Autenticado como")
        && text(&progress[2]["text"]).contains("herdr")
        && text(&progress[3]["text"]).contains("Lendo workspaces");

    let dialog_ok = dialog_dims_ok && cards_ok && fields_ok && progress_ok && dialog_closed;

    // 5. Small text contrast
    let mut small_texts = sidebar["small_texts"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    small_texts.extend(
        dialog["small_texts"]
            .as_array()
            .cloned()
            .unwrap_or_default(),
    );

    let contrast_ok = !small_texts.is_empty()
        && small_texts
            .iter()
            .filter(|t| t["visibility"] == "visible")
            .all(|t| {
                contrast(
                    text(&t["color"]),
                    text(&t["background"]),
                    num(&t["opacity"]).unwrap_or(1.0),
                )
                .is_some_and(|r| r >= MIN_SMALL_TEXT_RATIO)
            });

    Ok(vec![
        ("project_tree_groups_and_projects", tree_ok),
        ("active_project_surface3_and_accent_bar", active_ok),
        ("connections_footer_and_host_selection", conn_ok),
        ("connection_dialog_layout_and_progress", dialog_ok),
        ("small_text_contrast_at_least_4_5", contrast_ok),
    ])
}
