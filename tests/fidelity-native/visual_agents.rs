//! `visual-agents` phase of the native flow (spec 014, AC-014-01/02/03): literal expectations,
//! the parent's pure helpers (engine expectations, key words) and the evaluator.
//!
//! The page (`src/features/fidelity/visual-agents.ts`) reports raw DOM observations of the
//! designed agents panel. The parent drives the engine from outside: it puts a nonce on the last
//! screen line of the fixture agent's pane, reports the agent waiting for the user, presses the
//! native Enter on the focused card and reads the engine (`agent list`, `tab list`, `agent read
//! --source detection`, `pane get`). Every value a check compares against comes from this file or
//! from the parent ledger, never from the page's own report.

use serde_json::{json, Value};

pub use super::paste_flow::ParentLedger as Ledger;

pub const PHASE: &str = "visual-agents";
pub const STEPS: [&str; 4] = [
    "visual-agents-arm",
    "visual-agents-block",
    "visual-agents-focus",
    "visual-agents-collapse",
];
pub const CHECKS: [&str; 7] = [
    "counters_match_engine_states",
    "attention_card_within_500ms",
    "attention_card_shows_engine_pane_project_tab_and_line",
    "enter_focuses_the_pane_once_without_keys",
    "running_rows_ordered_by_observed_activity",
    "collapse_gives_the_center_256_px",
    "panel_small_text_contrast_at_least_4_5",
];

/// Width of the agents column in the guide (design/GUIA-IMPLEMENTACAO.md), which the center
/// gains when the panel is collapsed.
pub const AGENTS_WIDTH: f64 = 256.0;
/// Deadline of AC-014-02 between the engine state change and the card on screen.
pub const ATTENTION_DEADLINE_MS: f64 = 500.0;
/// Counter labels of the design, in order.
pub const COUNTER_LABELS: [&str; 3] = ["ativos", "esperando", "ociosos"];
/// State texts of the engine states (`src/agents/status.ts`); the card and the row show text,
/// never colour alone, and unknown is never the text of done.
pub const STATE_LABELS: [(&str, &str); 5] = [
    ("working", "Trabalhando"),
    ("blocked", "Aguardando você"),
    ("idle", "Ocioso"),
    ("done", "Concluído"),
    ("unknown", "Desconhecido"),
];
/// Text this phase leaves pending on the agent's screen line, so the line the card shows can
/// only have come from the engine's detection snapshot of that pane.
pub const NONCE: &str = "hd014 aprovar migracao 0042?";
/// Source this phase reports the state with: the same one the fixture agent itself uses, so the
/// engine keeps a single reporting authority for the pane.
pub const REPORT_SOURCE: &str = "hd007-fake";
pub const REPORT_AGENT: &str = "pi";
/// Relative time of a transition this window has just observed.
pub const FRESH_TIME: &str = "agora";
pub const MIN_SMALL_TEXT_RATIO: f64 = 4.5;
/// How long the parent waits for the engine to answer the nonce on the detection snapshot.
pub const SCREEN_WAIT_MS: u64 = 15_000;
/// How long the parent waits for the engine to confirm the focused pane.
pub const FOCUS_WAIT_MS: u64 = 10_000;
const TOLERANCE: f64 = 1.0;

/// `wtype` words of the only key this phase presses (Enter on the focused attention card).
pub fn wtype_args(token: &str) -> Result<Vec<String>, String> {
    let words: &[&str] = match token {
        "Return" => &["-k", "Return"],
        _ => return Err(format!("key {token:?} is not allowed in {PHASE}")),
    };
    Ok(words.iter().map(|w| (*w).to_owned()).collect())
}

fn num(v: &Value) -> Option<f64> {
    v.as_f64()
}

fn text(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

fn agents_of(agent_list: &Value) -> Result<&Vec<Value>, String> {
    agent_list["result"]["agents"]
        .as_array()
        .ok_or_else(|| format!("{PHASE}: agent list without result.agents: {agent_list}"))
}

/// The agent the engine lists in `pane`, as the engine published it.
pub fn engine_agent<'a>(agent_list: &'a Value, pane: &str) -> Result<&'a Value, String> {
    agents_of(agent_list)?
        .iter()
        .find(|a| a["pane_id"] == pane)
        .ok_or_else(|| format!("{PHASE}: the engine lists no agent in {pane}"))
}

/// Pane of the session where this phase drives the agent: the pane of an agent the engine
/// already lists, other than the one whose surface the window confirmed.
pub fn agent_pane(agent_list: &Value, confirmed: &str) -> Result<String, String> {
    agents_of(agent_list)?
        .iter()
        .filter_map(|a| a["pane_id"].as_str())
        .find(|p| *p != confirmed)
        .map(str::to_owned)
        .ok_or_else(|| format!("{PHASE}: no agent pane other than {confirmed}"))
}

/// Last non-empty line of a snapshot, with trailing blanks removed and nothing interpreted
/// (the same rule the backend applies to the detection read).
pub fn last_snapshot_line(read: &Value) -> Option<String> {
    read["result"]["read"]["text"]
        .as_str()?
        .lines()
        .rev()
        .map(str::trim_end)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
}

/// Counters of AC-014-01 computed from the engine's agent list: working, waiting (blocked),
/// idle + done, and anything else as unknown (never counted as done).
pub fn engine_counters(agent_list: &Value) -> Result<Value, String> {
    let (mut active, mut waiting, mut idle, mut unknown) = (0, 0, 0, 0);
    for agent in agents_of(agent_list)? {
        match agent["agent_status"].as_str() {
            Some("working") => active += 1,
            Some("blocked") => waiting += 1,
            Some("idle") | Some("done") => idle += 1,
            _ => unknown += 1,
        }
    }
    Ok(json!({ "active": active, "waiting": waiting, "idle": idle, "unknown": unknown }))
}

/// Metadata token the window tags a project's workspace with (src-tauri/src/project_store.rs).
pub const PROJECT_TOKEN: &str = "herdr_desktop_project";

/// `projeto › tab` expected for one engine agent: the project of the window's catalog whose UUID
/// the engine reports on the agent's workspace (the workspace id when the workspace carries no
/// project) and the engine label of the agent's tab (its id when the engine published no label).
pub fn engine_path(
    agent: &Value,
    tab_list: &Value,
    workspace_list: &Value,
    catalog: &Value,
) -> Result<String, String> {
    let workspace = agent["workspace_id"]
        .as_str()
        .ok_or_else(|| format!("{PHASE}: engine agent without workspace_id: {agent}"))?;
    let tab_id = agent["tab_id"]
        .as_str()
        .ok_or_else(|| format!("{PHASE}: engine agent without tab_id: {agent}"))?;
    let tagged = workspace_list["result"]["workspaces"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|w| w["workspace_id"] == workspace)
        .and_then(|w| w.pointer(&format!("/tokens/{PROJECT_TOKEN}")))
        .and_then(Value::as_str);
    let project = tagged
        .and_then(|id| {
            catalog["projects"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|p| p["id"] == id)
                .and_then(|p| p["label"].as_str())
        })
        .unwrap_or(workspace);
    let tab = tab_list["result"]["tabs"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|t| t["tab_id"] == tab_id)
        .and_then(|t| t["label"].as_str())
        .filter(|label| !label.trim().is_empty())
        .unwrap_or(tab_id);
    Ok(format!("{project} › {tab}"))
}

/// What the panel must show for one engine agent: name, path, state text and activity summary
/// (the engine's stripped terminal title; the state text when the engine published no title).
pub fn engine_row(
    agent: &Value,
    tab_list: &Value,
    workspace_list: &Value,
    catalog: &Value,
) -> Result<Value, String> {
    let pane = agent["pane_id"]
        .as_str()
        .ok_or_else(|| format!("{PHASE}: engine agent without pane_id: {agent}"))?;
    let status = agent["agent_status"].as_str().unwrap_or("");
    let label = STATE_LABELS
        .iter()
        .find(|(state, _)| *state == status)
        .map(|(_, label)| *label)
        .unwrap_or(STATE_LABELS[4].1);
    let title = agent["terminal_title_stripped"]
        .as_str()
        .map(str::trim)
        .filter(|t| !t.is_empty());
    Ok(json!({
        "pane": pane,
        "name": agent["name"].as_str().unwrap_or(pane),
        "path": engine_path(agent, tab_list, workspace_list, catalog)?,
        "status": if STATE_LABELS.iter().any(|(s, _)| *s == status) { status } else { "unknown" },
        "label": label,
        "summary": title.unwrap_or(label),
    }))
}

// ------------------------------------------------------------------------------ evaluator

fn field<'a>(v: &'a Value, key: &str, what: &str) -> Result<&'a Value, String> {
    match v.get(key) {
        Some(found) if !found.is_null() => Ok(found),
        _ => Err(format!("{PHASE}: {what} has no {key}")),
    }
}

fn close(v: &Value, want: f64) -> bool {
    num(v).is_some_and(|got| (got - want).abs() <= TOLERANCE)
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
    let arm = answer("visual-agents-arm")?;
    let block = answer("visual-agents-block")?;
    let focus = answer("visual-agents-focus")?;
    let pane = text(field(arm, "agent_pane", "visual-agents-arm")?);
    if report["agent_pane"] != pane {
        return Err(format!(
            "{PHASE}: page observed pane {} instead of {pane}",
            report["agent_pane"]
        ));
    }
    let agent_list = field(block, "agent_list", "visual-agents-block")?;
    let tab_list = field(block, "tab_list", "visual-agents-block")?;
    let workspace_list = field(block, "workspace_list", "visual-agents-block")?;
    let catalog = field(arm, "project_catalog", "visual-agents-arm")?;
    let engine = engine_agent(agent_list, pane)?;

    // AC-014-01 — the counters are the engine's states.
    let want_counters = engine_counters(agent_list)?;
    let shown = field(report, "counters", "report")?;
    let counters_ok = ["active", "waiting", "idle", "unknown"]
        .iter()
        .all(|key| shown[*key] == want_counters[*key])
        && want_counters["waiting"].as_u64().unwrap_or(0) > 0
        && engine["agent_status"] == "blocked"
        && report["counter_labels"]
            .as_array()
            .is_some_and(|labels| labels.iter().map(text).eq(COUNTER_LABELS));

    // AC-014-02 — the card appears within the deadline of the engine's state change.
    let reported_at = num(field(block, "reported_at_ms", "visual-agents-block")?)
        .ok_or_else(|| format!("{PHASE}: reported_at_ms is not a number"))?;
    let seen_at = num(field(report, "card_seen_at_ms", "report")?)
        .ok_or_else(|| format!("{PHASE}: card_seen_at_ms is not a number"))?;
    let latency = seen_at - reported_at;
    let deadline_ok = latency > 0.0 && latency <= ATTENTION_DEADLINE_MS;

    // AC-014-02 — the card carries the engine's data, including the line of the detection
    // snapshot the parent read itself, which contains the nonce it left on that screen.
    let want_row = engine_row(engine, tab_list, workspace_list, catalog)?;
    let want_line = last_snapshot_line(field(block, "detection", "visual-agents-block")?)
        .ok_or_else(|| format!("{PHASE}: the engine detection snapshot has no line"))?;
    let cards = report["attention"]["cards"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let card = cards.iter().find(|c| c["pane"] == pane);
    let card_ok = card.is_some_and(|c| {
        c["name"] == want_row["name"]
            && c["path"] == want_row["path"]
            && c["status"] == "blocked"
            && c["label"] == want_row["label"]
            && c["last_line"] == json!(want_line)
            && c["time"] == FRESH_TIME
            && c["disabled"] == false
    }) && want_line.contains(NONCE)
        && cards.len() == want_counters["waiting"].as_u64().unwrap_or(0) as usize;

    // AC-014-02 — the trusted Enter focused that pane in the engine once and sent no key to the
    // agent (the pending screen line is untouched).
    let keys = report["enter"]["keys"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let focus_ok = keys.len() == 1
        && keys[0]["trusted"] == true
        && keys[0]["key"] == "Enter"
        && keys[0]["card"] == pane
        && report["enter"]["activations"] == 1
        && focus["focused_pane"] == pane
        && focus["detection_after"] == json!(want_line)
        && focus["agent_status_after"] == "blocked";

    // AC-014-03 — every engine agent is a row, newest observed transition first.
    let rows = report["running"].as_array().cloned().unwrap_or_default();
    let mut want_rows = Vec::new();
    for agent in agents_of(agent_list)? {
        want_rows.push(engine_row(agent, tab_list, workspace_list, catalog)?);
    }
    let same_rows = rows.len() == want_rows.len()
        && want_rows.iter().all(|want| {
            rows.iter().any(|row| {
                row["pane"] == want["pane"]
                    && row["name"] == want["name"]
                    && row["path"] == want["path"]
                    && row["status"] == want["status"]
                    && row["label"] == want["label"]
                    && row["summary"] == want["summary"]
            })
        });
    let rows_ok =
        same_rows && !rows.is_empty() && rows[0]["pane"] == pane && rows[0]["time"] == FRESH_TIME;

    // AC-014-01 — collapsing the panel hands its 256 px to the center.
    let layout = field(report, "layout", "report")?;
    let grew = num(&layout["center_width_after"]).unwrap_or(0.0)
        - num(&layout["center_width_before"]).unwrap_or(0.0);
    let layout_ok = close(&layout["agents_width_before"], AGENTS_WIDTH)
        && (grew - AGENTS_WIDTH).abs() <= TOLERANCE
        && layout["agents_present_after"] == false
        && !text(&layout["collapse_label"]).trim().is_empty();

    // Contrast of the panel's small texts, recomputed here.
    let texts = report["small_texts"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let small: Vec<&Value> = texts
        .iter()
        .filter(|t| t["visibility"] == "visible")
        .filter(|t| {
            let size = num(&t["font_size"]).unwrap_or(0.0);
            let weight = num(&t["font_weight"]).unwrap_or(400.0);
            !(size >= 24.0 || (size >= 18.66 && weight >= 700.0))
        })
        .collect();
    let has = |needle: &str| small.iter().any(|t| text(&t["text"]).contains(needle));
    let contrast_ok = small.len() >= COUNTER_LABELS.len()
        && COUNTER_LABELS.iter().all(|label| has(label))
        && has(text(&want_row["label"]))
        && has(&want_line.chars().take(40).collect::<String>())
        && small.iter().all(|t| {
            super::visual_frame::contrast(
                text(&t["color"]),
                text(&t["background"]),
                num(&t["opacity"]).unwrap_or(0.0),
            )
            .is_some_and(|ratio| ratio >= MIN_SMALL_TEXT_RATIO)
        });

    Ok(vec![
        (CHECKS[0], counters_ok),
        (CHECKS[1], deadline_ok),
        (CHECKS[2], card_ok),
        (CHECKS[3], focus_ok),
        (CHECKS[4], rows_ok),
        (CHECKS[5], layout_ok),
        (CHECKS[6], contrast_ok),
    ])
}
