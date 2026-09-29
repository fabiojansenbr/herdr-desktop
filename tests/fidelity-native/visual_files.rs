//! `visual-files` phase of the native flow (spec 015, AC-015-01/02/03): literal expectations, the
//! parent's pure helpers (expected dock label, dictated buffer, remote contents) and the evaluator.
//!
//! The page (`src/features/fidelity/visual-files.ts`) reports raw DOM observations of the real
//! composed window: the review header/breadcrumb/tabs, the side-by-side diff of the Local file it
//! edited with the parent's dictated buffer, the dock label and its 190 px, the remote banner and
//! the two-snapshot remote diff. The parent dictates the buffer, writes both remote contents, reads
//! the engine (agent, panes), measures the PTY bytes of the corpus and presses the marker echo.
//! Values every check compares against come from this file (design literals) or from the parent
//! ledger (engine, PTY), never from the page's report.

use serde_json::Value;

pub use super::paste_flow::ParentLedger as Ledger;

pub const PHASE: &str = "visual-files";
pub const STEPS: [&str; 9] = [
    "visual-files-viewport",
    "visual-files-local",
    "visual-files-engine",
    "visual-files-keys",
    "visual-files-frames",
    "visual-files-remote-before",
    "visual-files-remote-change",
    "visual-files-evidence",
    "visual-files-restore",
];
pub const CHECKS: [&str; 10] = [
    "review_title_breadcrumb_and_tabs",
    "editor_lazy_before_open_in_review",
    "side_by_side_headers_prefixes_and_sets",
    "diff_line_numbers_match_their_sides",
    "diff_error_and_working_at_15_percent",
    "small_text_contrast_at_least_4_5",
    "remote_banner_read_only_and_snapshot_diff",
    "dock_height_label_and_close_returns_height",
    "dock_surface_keeps_input",
    "dock_surface_frames_painted",
];
pub const TARGET: (f64, f64) = (1440.0, 900.0);
/// Output mode the flow's display is restored to after the phase (`visual_frame::RESTORE_MODE`).
pub const RESTORE_MODE: (u32, u32) = (1280, 720);
pub const DOCK_HEIGHT: f64 = 190.0;
pub const MIN_SMALL_TEXT_RATIO: f64 = 4.5;
pub const REVIEW_TITLE: &str = "Revisar alterações";
pub const HEADING_BASE: &str = "Snapshot anterior";
pub const HEADING_CURRENT: &str = "Versão atual";
pub const OPEN_FILE_LABEL: &str = "+ Abrir arquivo";
/// Labels fixed by the parent's flow params (`live::run`), asserted against the page.
pub const LOCAL_PROJECT_LABEL: &str = "Fidelidade Local";
pub const SSH_PROJECT_LABEL: &str = "Fidelidade SSH";
pub const FIXTURE_FILE: &str = "fidelity_fixture.rs";
pub const REMOTE_FILE: &str = "notas.txt";
/// Local fixture content written by `fidelity_native.rs`; the page must have read exactly this.
pub const ORIGINAL: &str =
    "// spec 007 lazy-editor positive control\nfn main() {\n    println!(\"fidelidade\");\n}\n";
/// Buffer the parent dictates; its diff against `ORIGINAL` has two removed and three added lines.
pub const BUFFER: &str = "// spec 015 review\nfn main() {\n    println!(\"revisao 015\");\n    println!(\"linha nova\");\n}\n";
/// Remote snapshot the parent writes before the page's first read.
pub const REMOTE_A: &str = "linha um remota\nlinha dois\nlinha tres\n";
/// Remote snapshot the parent writes before the page's reload; one line changed, one added.
pub const REMOTE_B: &str = "linha um remota\nlinha dois ALTERADA\nlinha tres\nlinha quatro\n";
pub const BANNER_SUFFIX: &str = " · SSH · Somente leitura · Comparação entre snapshots";
/// `--accent-soft` of `src/app.css` (design guide): accent at 31/255 alpha.
pub const ACCENT_SOFT: (u8, u8, u8, f64) = (143, 168, 255, 31.0 / 255.0);
/// Removed/added backgrounds: `--error`/`--working` at 15 % over the diff surface.
pub const ERROR_15: (u8, u8, u8, f64) = (242, 119, 122, 0.15);
pub const WORKING_15: (u8, u8, u8, f64) = (91, 214, 138, 0.15);
const TOLERANCE: f64 = 1.0;

/// Lines of a text as the diff and the editor see them (trailing newline ends the last line).
pub fn lines(text: &str) -> Vec<String> {
    let mut out: Vec<String> = text.split('\n').map(str::to_owned).collect();
    if out.len() > 1 && out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out
}

/// Lines of `text` that are not in `other` (the diff's removed/added sets for these fixtures).
pub fn only_in(text: &str, other: &str) -> Vec<String> {
    let other_lines = lines(other);
    let mut out: Vec<String> = lines(text)
        .into_iter()
        .filter(|line| !other_lines.contains(line))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// `TERMINAL · <agente> / <projeto>` the dock must show: the focused agent of the engine's list,
/// else the first by pane id; the label falls back from name to kind to pane id as the DTO does.
pub fn expected_dock_label(agent_list: &Value, project: &str) -> Result<String, String> {
    let (_, label) = expected_dock(agent_list, project)?;
    Ok(label)
}

/// Agent label the dock must name and the whole dock label; both from the engine's list only.
pub fn expected_dock(agent_list: &Value, project: &str) -> Result<(String, String), String> {
    let agents = agent_list["result"]["agents"]
        .as_array()
        .ok_or_else(|| format!("agent list without result.agents: {agent_list}"))?;
    if agents.is_empty() {
        return Err(format!(
            "{PHASE}: the Local session lists no agent: {agent_list}"
        ));
    }
    let focused = agents.iter().find(|a| a["focused"] == true);
    let pick = focused.or_else(|| {
        agents
            .iter()
            .min_by_key(|a| a["pane_id"].as_str().unwrap_or(""))
    });
    let agent = pick.ok_or("no agent to name")?;
    let text = |k: &str| agent[k].as_str().unwrap_or("").trim();
    let label = if !text("name").is_empty() {
        text("name")
    } else if !text("agent").is_empty() {
        text("agent")
    } else {
        text("pane_id")
    };
    Ok((label.to_owned(), format!("TERMINAL · {label} / {project}")))
}

fn num(v: &Value) -> Option<f64> {
    v.as_f64()
}

fn text(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

fn close(v: &Value, want: f64) -> bool {
    num(v).is_some_and(|got| (got - want).abs() <= TOLERANCE)
}

/// `rgba()`/`rgb()` or `#rrggbb[aa]` as computed styles serialize it.
fn parse_color(css: &str) -> Option<(f64, f64, f64, f64)> {
    let value = css.trim().to_ascii_lowercase();
    if let Some(hex) = value.strip_prefix('#') {
        let byte = |i: usize| {
            u8::from_str_radix(hex.get(i..i + 2)?, 16)
                .ok()
                .map(f64::from)
        };
        return match hex.len() {
            6 | 8 => Some((
                byte(0)?,
                byte(2)?,
                byte(4)?,
                if hex.len() == 8 {
                    byte(6)? / 255.0
                } else {
                    1.0
                },
            )),
            _ => None,
        };
    }
    let inner = value
        .strip_prefix("rgba(")
        .or_else(|| value.strip_prefix("rgb("))?
        .strip_suffix(')')?;
    let parts: Vec<f64> = inner
        .split(|c: char| c == ',' || c == '/' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .map(|p| match p.strip_suffix('%') {
            Some(pct) => pct.parse::<f64>().map(|v| v / 100.0),
            None => p.parse::<f64>(),
        })
        .collect::<Result<_, _>>()
        .ok()?;
    match parts[..] {
        [r, g, b] => Some((r, g, b, 1.0)),
        [r, g, b, a] => Some((r, g, b, a)),
        _ => None,
    }
}

/// Color equals `want` within a small channel/alpha tolerance (computed styles round).
fn same_color(css: &str, want: (u8, u8, u8, f64)) -> bool {
    parse_color(css).is_some_and(|(r, g, b, a)| {
        (r - f64::from(want.0)).abs() <= 1.0
            && (g - f64::from(want.1)).abs() <= 1.0
            && (b - f64::from(want.2)).abs() <= 1.0
            && (a - want.3).abs() <= 0.01
    })
}

fn field<'a>(v: &'a Value, key: &str, what: &str) -> Result<&'a Value, String> {
    match v.get(key) {
        Some(found) if !found.is_null() => Ok(found),
        _ => Err(format!("{PHASE}: {what} has no {key}")),
    }
}

fn array<'a>(v: &'a Value, key: &str, what: &str) -> Result<&'a Vec<Value>, String> {
    field(v, key, what)?
        .as_array()
        .ok_or_else(|| format!("{PHASE}: {what}.{key} is not an array"))
}

/// Cell texts of one side, sorted (the diff's removed/added sets).
fn cell_texts(diff: &Value, side: &str, op: &str) -> Result<Vec<String>, String> {
    let cells = array(diff, "cells", "diff")?;
    let mut out: Vec<String> = cells
        .iter()
        .filter(|c| text(&c["side"]) == side && text(&c["op"]) == op)
        .map(|c| text(&c["text"]).to_owned())
        .collect();
    out.sort();
    Ok(out)
}

/// Each cell's number must be the 1-based line of its own side's text.
fn numbers_match(side: &str, source: &str, cells: &[Value]) -> bool {
    let expected = lines(source);
    cells
        .iter()
        .filter(|c| {
            text(&c["side"]) == side
                && text(&c["op"]) != if side == "base" { "added" } else { "removed" }
        })
        .all(|c| {
            let line = c["line"].as_u64().unwrap_or(0) as usize;
            line >= 1 && line <= expected.len() && text(&c["text"]) == expected[line - 1]
        })
}

/// Cells of one row share the top; rows are laid out in order.
fn rows_aligned(cells: &[Value]) -> bool {
    for cell in cells {
        let row = cell["row"].as_u64().unwrap_or(0);
        if row == 0 {
            return false;
        }
        let mut same_row = cells.iter().filter(|c| c["row"] == cell["row"]);
        let top = num(&cell["top"]).unwrap_or(-1.0);
        if !same_row.all(|c| (num(&c["top"]).unwrap_or(-1.0) - top).abs() <= TOLERANCE) {
            return false;
        }
    }
    true
}

/// Root folder name used as the breadcrumb's project segment.
fn root_name(path: &str) -> String {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_owned()
}

fn arrays_equal(got: &[String], want: &[String]) -> bool {
    got == want
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
    let local = answer("visual-files-local")?;
    let engine = answer("visual-files-engine")?;
    let keys = answer("visual-files-keys")?;
    let frames = answer("visual-files-frames")?;
    let remote = answer("visual-files-remote-change")?;
    let mount = field(report, "mount", "report")?;
    let review = field(report, "review", "report")?;
    let remote_review = field(report, "remote", "report")?;

    // AC-015-01 — title, breadcrumb, tabs and the `+ Abrir arquivo` action.
    let breadcrumb = format!("{} / {FIXTURE_FILE}", root_name(text(&local["root"])));
    let entries: Vec<&str> = array(review, "ordered", "review")?
        .iter()
        .map(text)
        .collect();
    let want_entries = [
        FIXTURE_FILE.to_owned(),
        format!("{FIXTURE_FILE} · diff"),
        OPEN_FILE_LABEL.to_owned(),
    ];
    let tabs_ok = text(&review["title"]) == REVIEW_TITLE
        && text(&review["breadcrumb"]) == breadcrumb
        && entries == want_entries.iter().map(String::as_str).collect::<Vec<_>>()
        && text(&report["open_focus"]) == FIXTURE_FILE
        && text(&mount["open_file_label"]) == OPEN_FILE_LABEL
        && text(&mount["empty_text"]).contains("Abra um arquivo")
        && array(mount, "tabs", "mount")?.is_empty();

    // AC-015-01 — the review screen never loads the editor before a file is opened: no editor DOM
    // at the mount (empty state) nor in the diff view, and no editor module the editor load had not
    // already fetched; the same probe of spec 005, kept here as a set delta plus the DOM control.
    let map = super::chunks::ChunkMap::parse(text(&report["chunk_map"]))?;
    let probe = |name: &str| super::chunks::Snapshot::from_json(&report["probes"][name]);
    let editor_set = |snapshot: &super::chunks::Snapshot| -> Result<Vec<String>, String> {
        Ok(map
            .modules(&snapshot.scripts)?
            .into_iter()
            .filter(|m| super::chunks::is_editor_module(m))
            .collect())
    };
    let (mount_probe, opened_probe, diff_probe) =
        (probe("mount")?, probe("opened")?, probe("diff")?);
    let lazy_ok = !mount_probe.editor_dom
        && !diff_probe.editor_dom
        && opened_probe.editor_dom
        && review["editor_dom"] == false
        && editor_set(&mount_probe)? == editor_set(&diff_probe)?
        && text(&local["original"]) == ORIGINAL
        && text(&report["buffer_applied"]) == BUFFER;

    // AC-015-01 — side-by-side diff: headers, prefixes and the removed/added sets the parent knows.
    let diff = field(review, "diff", "review")?;
    let headings = field(diff, "headings", "diff")?;
    let cells = array(diff, "cells", "diff")?;
    let removed_want = only_in(ORIGINAL, BUFFER);
    let added_want = only_in(BUFFER, ORIGINAL);
    let marks_ok = cells.iter().all(|cell| {
        let mark = text(&cell["mark"]);
        match text(&cell["op"]) {
            "removed" => mark == "-",
            "added" => mark == "+",
            _ => mark.trim().is_empty(),
        }
    });
    let sets_ok = headings["base"] == HEADING_BASE
        && headings["current"] == HEADING_CURRENT
        && text(&diff["base"]) == "original"
        && arrays_equal(&cell_texts(diff, "base", "removed")?, &removed_want)
        && arrays_equal(&cell_texts(diff, "current", "added")?, &added_want)
        && arrays_equal(
            &cell_texts(diff, "base", "same")?,
            &cell_texts(diff, "current", "same")?,
        )
        && !removed_want.is_empty()
        && !added_want.is_empty()
        && marks_ok;

    // AC-015-01 — numbering: each cell's number is its own side's line and rows share the top.
    let numbers_ok = numbers_match("base", ORIGINAL, cells)
        && numbers_match("current", BUFFER, cells)
        && rows_aligned(cells);

    // AC-015-01 — removed lines on `error` at 15 %, added on `working` at 15 %.
    let colors_ok = cells
        .iter()
        .filter(|c| text(&c["op"]) == "removed")
        .all(|c| same_color(text(&c["background"]), ERROR_15))
        && cells
            .iter()
            .filter(|c| text(&c["op"]) == "added")
            .all(|c| same_color(text(&c["background"]), WORKING_15));

    // Premise 7 — every visible small text of the review and of the dock label reaches 4,5:1.
    let mut texts: Vec<Value> = array(review, "small_texts", "review")?.clone();
    texts.extend(
        array(remote_review, "small_texts", "remote")?
            .iter()
            .cloned(),
    );
    let small: Vec<&Value> = texts
        .iter()
        .filter(|t| t["visibility"] == "visible")
        .filter(|t| {
            let size = num(&t["font_size"]).unwrap_or(0.0);
            let weight = num(&t["font_weight"]).unwrap_or(400.0);
            !(size >= 24.0 || (size >= 18.66 && weight >= 700.0))
        })
        .collect();
    // The page's texts are the rendered ones (CSS may uppercase the headings); labels are input
    // by the design in their source case, so the search is case-insensitive.
    let has = |scope: &str, needle: &str| {
        needle.chars().next().is_some_and(|_| !needle.is_empty())
            && small.iter().any(|t| {
                t["scope"] == scope
                    && text(&t["text"])
                        .to_lowercase()
                        .contains(&needle.to_lowercase())
            })
    };
    let contrast_ok = has("review", REVIEW_TITLE)
        && has("review", HEADING_BASE)
        && has("dock", "TERMINAL")
        && small.iter().all(|t| {
            super::visual_frame::contrast(
                text(&t["color"]),
                text(&t["background"]),
                num(&t["opacity"]).unwrap_or(0.0),
            )
            .is_some_and(|ratio| ratio >= MIN_SMALL_TEXT_RATIO)
        });

    // AC-015-02 — remote banner in accent-soft, read-only editor and the two-snapshot diff.
    let banner = field(remote_review, "banner", "remote")?;
    let label = text(&remote["banner_label"]);
    let content_a = text(&remote["content_a"]);
    let content_b = text(&remote["content_b"]);
    let read_only = field(remote_review, "read_only", "remote")?;
    let remote_diff = field(remote_review, "diff", "remote")?;
    let remote_cells = array(remote_diff, "cells", "remote diff")?;
    let remote_headings = field(remote_diff, "headings", "remote diff")?;
    let sources: Vec<&str> = text(&remote_diff["sources"])
        .split('→')
        .map(str::trim)
        .collect();
    let remote_ok = !label.is_empty()
        && text(&banner["text"]) == format!("{label}{BANNER_SUFFIX}")
        && same_color(text(&banner["background"]), ACCENT_SOFT)
        && read_only["contenteditable"] == "false"
        && read_only["typed_ignored"] == true
        && read_only["save_buttons"] == 0
        && remote_headings["base"] == HEADING_BASE
        && remote_headings["current"] == HEADING_CURRENT
        && sources.len() == 2
        && sources.iter().all(|s| s.starts_with("leitura "))
        && arrays_equal(
            &cell_texts(remote_diff, "base", "removed")?,
            &only_in(content_a, content_b),
        )
        && arrays_equal(
            &cell_texts(remote_diff, "current", "added")?,
            &only_in(content_b, content_a),
        )
        && numbers_match("base", content_a, remote_cells)
        && numbers_match("current", content_b, remote_cells)
        && rows_aligned(remote_cells)
        && text(&remote_review["breadcrumb"])
            == format!("{} / {REMOTE_FILE}", root_name(text(&remote["root"])))
        && !only_in(content_a, content_b).is_empty()
        && !only_in(content_b, content_a).is_empty();

    // AC-015-03 — dock: 190 px, `TERMINAL · <agente> / <projeto>`, closing returns the height.
    let dock = field(review, "dock", "review")?;
    let dock_rect = field(dock, "rect", "dock")?;
    let closed = field(report, "closed", "report")?;
    let expected_label = text(&engine["expected_label"]);
    let dock_ok = close(&dock_rect["height"], DOCK_HEIGHT)
        && text(&dock["label"]) == expected_label
        && expected_label
            == format!(
                "TERMINAL · {} / {LOCAL_PROJECT_LABEL}",
                text(&engine["agent"])
            )
        && closed["present"] == false
        && close(&closed["dock_height"], DOCK_HEIGHT)
        && (num(&closed["diff_after"]).unwrap_or(0.0)
            - num(&closed["diff_before"]).unwrap_or(0.0)
            - num(&closed["dock_band"]).unwrap_or(0.0))
        .abs()
            <= 2.0
        && num(&closed["dock_band"]).unwrap_or(0.0) >= DOCK_HEIGHT;

    // AC-015-03 — the surface keeps receiving input: the corpus bytes at the PTY once, with the
    // dock focused, exactly as the native-keys phase measures them.
    let items = array(keys, "items", "keys")?;
    let input_ok = report["dock"]["focused"] == true
        && !items.is_empty()
        && items.iter().all(|item| {
            let expected = text(&item["expected_hex"]);
            !expected.is_empty() && text(&item["observed_hex"]) == expected
        })
        && keys["capture_pids_stopped"]
            .as_array()
            .is_some_and(|p| !p.is_empty());

    // AC-015-03 — and frames: the marker echo reached the pane and the dock painted new rows.
    let frame_counts = field(&report["dock"], "frames", "frames")?;
    let before = field(frame_counts, "before", "frames")?;
    let after = field(frame_counts, "after", "frames")?;
    let frames_ok = text(&frames["pane_read"]).contains(text(&frames["marker"]))
        && !text(&frames["marker"]).is_empty()
        && num(&after["painted_rows"]).unwrap_or(0.0)
            > num(&before["painted_rows"]).unwrap_or(-1.0)
        && num(&after["paint_calls"]).unwrap_or(0.0) > num(&before["paint_calls"]).unwrap_or(-1.0);

    Ok(vec![
        (CHECKS[0], tabs_ok),
        (CHECKS[1], lazy_ok),
        (CHECKS[2], sets_ok),
        (CHECKS[3], numbers_ok),
        (CHECKS[4], colors_ok),
        (CHECKS[5], contrast_ok),
        (CHECKS[6], remote_ok),
        (CHECKS[7], dock_ok),
        (CHECKS[8], input_ok),
        (CHECKS[9], frames_ok),
    ])
}
