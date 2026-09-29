//! `visual-center` phase of the native flow (spec 013, AC-013-01/02/03): parent steps, literals and
//! evaluator of the center region measured on the real composed window.
//!
//! The page (`src/features/fidelity/visual-center.ts`) clicks the header/tab/frame actions and
//! reports raw observations (boxes, texts, computed colours, probe counters). Everything the checks
//! compare against comes from this file or from the engine snapshots the parent took (`pane list`,
//! `tab list`, `agent list`), never from the page's own opinion.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

pub use super::paste_flow::ParentLedger as Ledger;

pub const PHASE: &str = "visual-center";
pub const STEPS: [&str; 9] = [
    "center-engine-before",
    "center-actions",
    "center-observe-start",
    "center-apply-grow",
    "center-observe-grow",
    "center-apply-scale2",
    "center-observe-scale2",
    "center-apply-restore",
    "center-observe-restore",
];
pub const CHECKS: [&str; 7] = [
    "project_header_shows_group_branch_and_path",
    "header_actions_confirmed_by_the_engine",
    "workspace_tabs_match_the_session",
    "pane_frames_match_inner_rect_at_four_stages",
    "frame_identity_state_path_and_edges",
    "frame_click_focuses_and_zoom_reaches_the_engine",
    "hidden_panes_force_no_repaint",
];
/// Stages measured, in order (the same four as `resize-dpi`).
pub const STAGES: [&str; 4] = ["start", "grow", "scale2", "restore"];
/// Geometry tolerance of a frame against `inner_rect × cell`.
pub const TOLERANCE: f64 = 1.0;
/// The five engine states, each with its own text; "Desconhecido" is never "Concluído".
pub const STATE_LABELS: [&str; 5] = [
    "Trabalhando",
    "Aguardando você",
    "Ocioso",
    "Concluído",
    "Desconhecido",
];
/// Computed colours of the frame edges (design tokens `--accent` and `--border`).
pub const ACCENT_RGB: &str = "rgb(143, 168, 255)";
pub const ATTENTION_RGB: &str = "rgb(244, 180, 84)";
pub const BORDER_RGB: &str = "rgb(36, 40, 51)";
pub const EDGE_WIDTH: &str = "1px";
/// Branch of the Local fixture repository, shown in the header as `⎇ <branch>`.
pub use super::remote::LOCAL_GIT_BRANCH as FIXTURE_BRANCH;
/// Tabs and panes the header/tab actions must add in the engine (Shell, "+" and Dividir).
pub const NEW_TABS: usize = 2;
pub const NEW_PANES: usize = 3;
/// Quiet window with the terminal hidden (`HIDDEN_MS` of the page).
pub const HIDDEN_MS: u64 = 2000;

/// `herdr pane zoom` words that restore a zoomed pane. The engine's CLI takes the mode as a flag
/// (`--toggle`, `--on`, `--off`; `src/cli/pane.rs::parse_pane_zoom_args`), never as `--mode VALUE`,
/// which it refuses with "unknown option" (spec 013, gate r6).
pub fn zoom_off_command(pane: &str) -> Vec<String> {
    ["pane", "zoom", pane, "--off"]
        .iter()
        .map(|word| (*word).to_owned())
        .collect()
}

fn panes_of(list: &Value) -> Vec<&Value> {
    list["result"]["panes"]
        .as_array()
        .map(|p| p.iter().collect())
        .unwrap_or_default()
}

fn tabs_of(list: &Value) -> Vec<&Value> {
    list["result"]["tabs"]
        .as_array()
        .map(|t| t.iter().collect())
        .unwrap_or_default()
}

fn agents_of(list: &Value) -> Vec<&Value> {
    list["result"]["agents"]
        .as_array()
        .map(|a| a.iter().collect())
        .unwrap_or_default()
}

/// pane_id → cwd, as the engine reported it.
pub fn pane_cwds(list: &Value) -> BTreeMap<String, String> {
    panes_of(list)
        .iter()
        .filter_map(|p| {
            Some((
                p["pane_id"].as_str()?.to_owned(),
                p["cwd"].as_str()?.to_owned(),
            ))
        })
        .collect()
}

/// pane_id → tab_id.
fn pane_tabs(list: &Value) -> BTreeMap<String, String> {
    panes_of(list)
        .iter()
        .filter_map(|p| {
            Some((
                p["pane_id"].as_str()?.to_owned(),
                p["tab_id"].as_str()?.to_owned(),
            ))
        })
        .collect()
}

/// cwd of the focused pane of the engine's `pane list` (the workspace whose branch is shown).
pub fn focused_cwd(list: &Value) -> Option<String> {
    panes_of(list)
        .iter()
        .find(|p| p["focused"] == Value::Bool(true))
        .and_then(|p| p["cwd"].as_str().map(str::to_owned))
}

fn focused_pane(list: &Value) -> Option<String> {
    panes_of(list)
        .iter()
        .find(|p| p["focused"] == Value::Bool(true))
        .and_then(|p| p["pane_id"].as_str().map(str::to_owned))
}

fn focused_tab(list: &Value) -> Option<String> {
    tabs_of(list)
        .iter()
        .find(|t| t["focused"] == Value::Bool(true))
        .and_then(|t| t["tab_id"].as_str().map(str::to_owned))
}

fn num(value: &Value) -> Option<f64> {
    value.as_f64()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= TOLERANCE
}

fn step<'a>(ledger: &'a Ledger, name: &str) -> Result<&'a Value, String> {
    ledger
        .steps
        .get(name)
        .ok_or_else(|| format!("{PHASE}: the parent never answered {name}"))
}

/// Header: `<grupo> › <projeto>`, `⎇ <branch>` and the project path the window opened.
fn header_check(report: &Value, before: &Value) -> bool {
    let header = &report["before"]["header"];
    let crumbs: Vec<&str> = header["crumbs"]
        .as_array()
        .map(|c| c.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let branch = header["branch"].as_str().unwrap_or_default();
    let path = header["path"].as_str().unwrap_or_default();
    let catalog = &before["project_catalog"];
    let projects = catalog["projects"].as_array().cloned().unwrap_or_default();
    // The breadcrumb names a project of the window's own catalog, with its root and its collection.
    let project = projects.iter().find(|p| {
        p["label"].as_str() == crumbs.last().copied() && p["root"].as_str() == Some(path)
    });
    let collection_ok = project.is_some_and(|project| {
        catalog["collections"]
            .as_array()
            .map(|collections| {
                collections.iter().any(|c| {
                    c["name"].as_str() == crumbs.first().copied()
                        && c["project_ids"]
                            .as_array()
                            .is_some_and(|ids| ids.iter().any(|id| id == &project["id"]))
                })
            })
            .unwrap_or(false)
    });
    let expected_branch = before["branch"].as_str().unwrap_or_default();
    crumbs.len() == 2
        && crumbs.iter().all(|c| !c.is_empty())
        && expected_branch == FIXTURE_BRANCH
        && branch == format!("\u{2387} {expected_branch}")
        && !path.is_empty()
        && collection_ok
        && header["empty"] == Value::Bool(false)
        && header["actions"] == json!(["split", "newTab", "newAgent"])
}

/// Dividir, Shell and "+" confirmed by the engine, and exactly one agent started.
fn actions_check(report: &Value, before: &Value, after: &Value) -> bool {
    let (tabs_before, tabs_after) = (
        tabs_of(&before["engine_tab_list"]).len(),
        tabs_of(&after["engine_tab_list"]).len(),
    );
    let (panes_before, panes_after) = (
        panes_of(&before["engine_pane_list"]).len(),
        panes_of(&after["engine_pane_list"]).len(),
    );
    let agents_before: BTreeSet<&str> = agents_of(&before["engine_agent_list"])
        .iter()
        .filter_map(|a| a["pane_id"].as_str())
        .collect();
    let agents_after: BTreeSet<&str> = agents_of(&after["engine_agent_list"])
        .iter()
        .filter_map(|a| a["pane_id"].as_str())
        .collect();
    let started: Vec<&&str> = agents_after.difference(&agents_before).collect();
    let actions = &report["actions"];
    let clicked_once = ["shell_click", "plus_click", "split_click", "agent_click"]
        .iter()
        .all(|k| actions[*k].as_str().is_some_and(|s| !s.is_empty()));
    let agent_pane = actions["agent_pane"].as_str().unwrap_or_default();
    tabs_after == tabs_before + NEW_TABS
        && panes_after == panes_before + NEW_PANES
        && started.len() == 1
        && started.first().is_some_and(|p| **p == agent_pane)
        && clicked_once
}

/// The tab bar lists exactly the engine's tabs, with its pane counts, dots and confirmed focus.
fn tabs_check(report: &Value, after: &Value) -> bool {
    let engine_tabs: Vec<&Value> = tabs_of(&after["engine_tab_list"]);
    let counts: BTreeMap<String, usize> = pane_tabs(&after["engine_pane_list"]).into_values().fold(
        BTreeMap::new(),
        |mut acc, tab| {
            *acc.entry(tab).or_insert(0) += 1;
            acc
        },
    );
    let waiting: BTreeSet<String> = agents_of(&after["engine_agent_list"])
        .iter()
        .filter(|a| a["agent_status"] == "blocked")
        .filter_map(|a| a["tab_id"].as_str().map(str::to_owned))
        .collect();
    let working: BTreeSet<String> = agents_of(&after["engine_agent_list"])
        .iter()
        .filter(|a| a["agent_status"] == "working")
        .filter_map(|a| a["tab_id"].as_str().map(str::to_owned))
        .collect();
    let seen = report["seen"]["tabs"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let engine_ids: Vec<&str> = engine_tabs
        .iter()
        .filter_map(|t| t["tab_id"].as_str())
        .collect();
    let seen_ids: Vec<&str> = seen.iter().filter_map(|t| t["tab_id"].as_str()).collect();
    if engine_ids.is_empty() || seen_ids != engine_ids {
        return false;
    }
    let counts_ok = seen.iter().all(|tab| {
        let id = tab["tab_id"].as_str().unwrap_or_default();
        tab["panes"].as_str().and_then(|p| p.parse::<usize>().ok()) == counts.get(id).copied()
    });
    let dots_ok = seen.iter().all(|tab| {
        let id = tab["tab_id"].as_str().unwrap_or_default().to_owned();
        let expected = if waiting.contains(&id) {
            Some("attention")
        } else if working.contains(&id) {
            Some("working")
        } else {
            None
        };
        tab["dot"].as_str() == expected
    });
    let active: Vec<&str> = seen
        .iter()
        .filter(|t| t["active"] == Value::Bool(true))
        .filter_map(|t| t["tab_id"].as_str())
        .collect();
    counts_ok
        && dots_ok
        && active.len() == 1
        && focused_tab(&after["engine_tab_list"]).as_deref() == Some(active[0])
}

/// A frame per pane at every stage, each box equal to `inner_rect × cell` (±1 px), and the toolbar
/// never over a content cell (the band is the row above the pane).
fn geometry_check(report: &Value) -> bool {
    let stages = report["stages"].as_array().cloned().unwrap_or_default();
    if stages.len() != STAGES.len() {
        return false;
    }
    let names: Vec<&str> = stages.iter().filter_map(|s| s["stage"].as_str()).collect();
    if names != STAGES.to_vec() {
        return false;
    }
    let dpr_of = |name: &str| {
        stages
            .iter()
            .find(|s| s["stage"] == name)
            .and_then(|s| num(&s["dpr"]))
    };
    // The DPI stage really happened (the scale change is part of the four stages).
    if dpr_of("scale2") != Some(2.0)
        || dpr_of("start") != Some(1.0)
        || dpr_of("restore") != Some(1.0)
    {
        return false;
    }
    stages.iter().all(|stage| {
        let panes = stage["panes"].as_array().cloned().unwrap_or_default();
        let frames = stage["frames"].as_array().cloned().unwrap_or_default();
        let canvas = &stage["canvas"];
        let surface = &stage["surface"];
        let (Some(left), Some(top), Some(width), Some(height)) = (
            num(&canvas["left"]),
            num(&canvas["top"]),
            num(&canvas["width"]),
            num(&canvas["height"]),
        ) else {
            return false;
        };
        let (Some(cols), Some(rows)) = (num(&surface["cols"]), num(&surface["rows"])) else {
            return false;
        };
        if panes.len() < 2 || frames.len() != panes.len() || cols <= 0.0 || rows <= 0.0 {
            return false;
        }
        let (cell_w, cell_h) = (width / cols, height / rows);
        let boxes_ok = panes.iter().all(|pane| {
            let id = pane["pane_id"].as_str().unwrap_or_default();
            let inner = &pane["inner_rect"];
            let Some(frame) = frames.iter().find(|f| f["pane_id"] == id) else {
                return false;
            };
            let rect = &frame["rect"];
            let (Some(x), Some(y), Some(w), Some(h)) = (
                num(&inner["x"]),
                num(&inner["y"]),
                num(&inner["width"]),
                num(&inner["height"]),
            ) else {
                return false;
            };
            close(num(&rect["left"]).unwrap_or(f64::NAN), left + x * cell_w)
                && close(num(&rect["top"]).unwrap_or(f64::NAN), top + y * cell_h)
                && close(num(&rect["width"]).unwrap_or(f64::NAN), w * cell_w)
                && close(num(&rect["height"]).unwrap_or(f64::NAN), h * cell_h)
        });
        // Header band (013) sits above the box; a frameless overlay (019) has no band or a 1 px
        // divider. Geometry of the pane itself is always inner_rect × cell.
        let bands_ok = frames.iter().all(|frame| {
            if frame["band"].is_null() {
                return true;
            }
            let (Some(band_top), Some(band_height), Some(band_width), Some(rect_top)) = (
                num(&frame["band"]["top"]),
                num(&frame["band"]["height"]),
                num(&frame["band"]["width"]),
                num(&frame["rect"]["top"]),
            ) else {
                return false;
            };
            if band_width <= 1.0 + TOLERANCE || band_height <= 1.0 + TOLERANCE {
                return true;
            }
            close(band_top + band_height, rect_top) && close(band_height, cell_h)
        });
        // The terminal's own action toolbar, when shown, lives in a band as well (010 debt).
        let toolbar_ok = stage["toolbar"].is_null() || {
            let bar_bottom = num(&stage["toolbar"]["top"]).unwrap_or(f64::NAN)
                + num(&stage["toolbar"]["height"]).unwrap_or(f64::NAN);
            frames.iter().all(|frame| {
                let frame_top = num(&frame["rect"]["top"]).unwrap_or(f64::NAN);
                let frame_bottom = frame_top + num(&frame["rect"]["height"]).unwrap_or(0.0);
                bar_bottom <= frame_top + TOLERANCE
                    || num(&stage["toolbar"]["top"]).unwrap_or(f64::NAN) >= frame_bottom - TOLERANCE
            })
        };
        boxes_ok && bands_ok && toolbar_ok
    })
}

/// Name, state (text + colour), path of each frame and the two distinct edges.
fn content_check(report: &Value, after: &Value) -> bool {
    let cwds = pane_cwds(&after["engine_pane_list"]);
    let agents: BTreeMap<&str, &str> = agents_of(&after["engine_agent_list"])
        .iter()
        .filter_map(|a| Some((a["pane_id"].as_str()?, a["agent"].as_str()?)))
        .collect();
    let frames = report["seen"]["frames"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if frames.len() < 2 {
        return false;
    }
    let content_ok = frames.iter().all(|frame| {
        let id = frame["pane_id"].as_str().unwrap_or_default();
        let name = frame["name"].as_str().unwrap_or_default();
        let state = frame["state"].as_str().unwrap_or_default();
        let path = frame["path"].as_str();
        let named = match agents.get(id) {
            Some(agent) => name == *agent,
            None => name == "shell",
        };
        named
            && STATE_LABELS.contains(&state)
            && path == cwds.get(id).map(String::as_str)
            && path.is_some()
            && frame["edge_width"].as_str() == Some(EDGE_WIDTH)
            && frame["cache"] == Value::Bool(false)
    });
    let focused: Vec<&Value> = frames
        .iter()
        .filter(|f| f["focused"] == Value::Bool(true))
        .collect();
    let others: Vec<&Value> = frames
        .iter()
        .filter(|f| f["focused"] != Value::Bool(true))
        .collect();
    let focus_edge_ok = focused.len() == 1
        && (focused[0]["edge_color"].as_str() == Some(ACCENT_RGB)
            || (focused[0]["edge"] == "attention"
                && focused[0]["edge_color"].as_str() == Some(ATTENTION_RGB)));
    let other_edge_ok = others.iter().all(|f| match f["edge"].as_str() {
        Some("attention") => f["edge_color"].as_str() == Some(ATTENTION_RGB),
        _ => f["edge_color"].as_str() == Some(BORDER_RGB),
    });
    // The engine's agent is named in its own frame (a real agent was started by the header).
    let agent_named = !agents.is_empty()
        && frames.iter().any(|f| {
            agents.get(f["pane_id"].as_str().unwrap_or_default())
                == Some(&f["name"].as_str().unwrap_or_default())
        });
    content_ok && focus_edge_ok && other_edge_ok && agent_named
}

/// Clicking a frame focuses that pane in the engine; the expand button zooms it there. The zoom is
/// proven by the engine itself: restoring it afterwards only changes something when it was zoomed.
fn focus_zoom_check(report: &Value, after: &Value) -> bool {
    let target = report["actions"]["focus_target"]
        .as_str()
        .unwrap_or_default();
    // `herdr pane zoom` answers `result.zoom` (checked against the reference binary, gate r7).
    let zoom_off = &after["zoom_off"]["result"]["zoom"];
    !target.is_empty()
        && focused_pane(&after["engine_pane_list"]).as_deref() == Some(target)
        && report["actions"]["focused_after_click"].as_str() == Some(target)
        && report["actions"]["zoom_click"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
        && zoom_off["zoom_changed"] == Value::Bool(true)
        && zoom_off["zoomed"] == Value::Bool(false)
        && zoom_off["pane_id"].as_str() == Some(target)
}

/// A hidden terminal repaints nothing and shows no frame (the RAF probe stays where it was).
fn hidden_check(report: &Value) -> bool {
    let hidden = &report["hidden"];
    let same = |key: &str| {
        hidden["before"][key] == hidden["after"][key] && !hidden["before"][key].is_null()
    };
    hidden["ms"].as_u64() == Some(HIDDEN_MS)
        && hidden["frames"].as_u64() == Some(0)
        && same("raf_requests")
        && same("raf_callbacks")
        && same("paint_calls")
        && same("painted_rows")
}

/// The checks of the phase, in the order of [`CHECKS`].
pub fn checks(report: &Value, ledger: &Ledger) -> Result<Vec<(&'static str, bool)>, String> {
    if let Some(error) = report["error"].as_str() {
        return Err(format!("{PHASE}: the window reported an error: {error}"));
    }
    for name in STEPS {
        let answer = step(ledger, name)?;
        if let Some(error) = answer["error"].as_str() {
            return Err(format!("{PHASE}: {name}: {error}"));
        }
    }
    let before = step(ledger, "center-engine-before")?;
    let after = step(ledger, "center-actions")?;
    Ok(vec![
        (CHECKS[0], header_check(report, before)),
        (CHECKS[1], actions_check(report, before, after)),
        (CHECKS[2], tabs_check(report, after)),
        (CHECKS[3], geometry_check(report)),
        (CHECKS[4], content_check(report, after)),
        (CHECKS[5], focus_zoom_check(report, after)),
        (CHECKS[6], hidden_check(report)),
    ])
}
