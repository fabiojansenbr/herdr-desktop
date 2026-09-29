//! Contracts of the `mouse-scroll-links` phase of the spec 007 native flow
//! (tests/fidelity-native/mouse_flow.rs): parent steps over a fake pointer/app/engine/recorder
//! world and pure evaluators. No engine, display, GUI, pointer, opener or browser runs here.
//! Each negative case names the wrong behavior it would catch.

#[allow(dead_code)]
#[path = "../../tests/fidelity-native/corpus.rs"]
mod corpus;
#[allow(dead_code)]
#[path = "../../tests/fidelity-native/display.rs"]
mod display;
#[allow(dead_code, unused_imports)]
#[path = "../../tests/fidelity-native/mouse_flow.rs"]
mod mouse_flow;
#[allow(dead_code, unused_imports)]
#[path = "../../tests/fidelity-native/paste_flow.rs"]
mod paste_flow;
#[allow(dead_code)]
#[path = "../../tests/fidelity-native/plan.rs"]
mod plan;
#[cfg(target_os = "linux")]
#[allow(dead_code)]
#[path = "../../tests/fidelity-native/supervisor.rs"]
mod supervisor;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use mouse_flow::{
    checks, first_numbered, link_fixture, mouse_app_command, parent_step, pointer_ledger_entry,
    scroll_fixture, EngineView, Expectations, LinkHandler, LinkRecorder, MouseApp, Point, Pointer,
    PointerEnv, CHECKS, CLICK_CELL, LINK_BASE, LINK_CELL, MOUSE_REPORTS, PHASE, SCROLL_CELL,
    SCROLL_LINES, STEPS, WHEEL_CELL,
};
use paste_flow::{PaneFixture, ParentLedger, PtyCapture};

const RUNTIME: &str = "/tmp/hd7L-4242";
const SOCKET: &str = "/tmp/hd7L-4242/sway-ipc.1000.77.sock";
const HOME: &str = "/tmp/hd7-home-4242";
const NONCE: &str = "n0nce7ab";
const ROWS: u64 = 24;

fn handler() -> LinkHandler {
    LinkHandler::new(Path::new(HOME), Path::new("/home/someone"), 1000).unwrap()
}

fn exp() -> Expectations {
    Expectations::new("local", RUNTIME, SOCKET, &handler(), NONCE).unwrap()
}

/// Wrong behaviors the fake app/engine/opener can show.
#[derive(Default, Clone)]
struct Quirks {
    duplicate_press: bool,
    no_release: bool,
    col_shift: u16,
    wheel_reports: Option<u8>,
    scroll_ignored: bool,
    offset_without_viewport: bool,
    link_opens: usize,
    link_uri: Option<String>,
    stale_entry: bool,
    socket: Option<String>,
}

/// Fake world: 10×20 px cells at output origin; the shell prompt adds one line below fixtures.
struct World {
    q: Quirks,
    app: bool,
    pty: Vec<u8>,
    shown: Option<String>,
    offset: u64,
    entries: Vec<String>,
    actions: Vec<String>,
    seat_cmds: Vec<Value>,
}

impl World {
    fn new(q: Quirks) -> Self {
        let entries = if q.stale_entry {
            vec![format!("{LINK_BASE}old")]
        } else {
            Vec::new()
        };
        Self {
            q,
            app: false,
            pty: Vec::new(),
            shown: None,
            offset: 0,
            entries,
            actions: Vec::new(),
            seat_cmds: Vec::new(),
        }
    }

    fn cell(&self, at: Point) -> (u16, u16) {
        (
            (at.y / 20.0) as u16,
            (at.x / 10.0) as u16 + self.q.col_shift,
        )
    }

    fn lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .shown
            .clone()
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect();
        lines.push("$ ".into());
        lines
    }

    fn max(&self) -> u64 {
        (self.lines().len() as u64).saturating_sub(ROWS)
    }
}

impl Pointer for World {
    fn socket(&self) -> String {
        self.q.socket.clone().unwrap_or_else(|| SOCKET.into())
    }

    fn click(&mut self, at: Point, ctrl: bool) -> Result<(), String> {
        self.actions.push(format!("click ctrl={ctrl}"));
        self.seat_cmds.push(ok_motion(at.x, at.y));
        self.seat_cmds.push(ok_button("pressed"));
        self.seat_cmds.push(ok_button("released"));
        let (row, col) = self.cell(at);
        if self.app {
            let press = format!("\x1b[<0;{};{}M", col + 1, row + 1);
            self.pty.extend(press.as_bytes());
            if self.q.duplicate_press {
                self.pty.extend(press.as_bytes());
            }
            if !self.q.no_release {
                self.pty
                    .extend(format!("\x1b[<0;{};{}m", col + 1, row + 1).as_bytes());
            }
        } else if ctrl
            && row == 0
            && self
                .shown
                .as_deref()
                .is_some_and(|s| s.contains("\x1b]8;;"))
        {
            let uri = self
                .q
                .link_uri
                .clone()
                .unwrap_or_else(|| format!("{LINK_BASE}{NONCE}"));
            for _ in 0..self.q.link_opens {
                self.entries.push(uri.clone());
            }
        }
        Ok(())
    }

    fn wheel_up(&mut self, at: Point, notches: u8) -> Result<(), String> {
        self.actions.push(format!("wheel {notches}"));
        self.seat_cmds.push(ok_motion(at.x, at.y));
        for _ in 0..notches {
            self.seat_cmds.push(ok_axis());
        }
        let (row, col) = self.cell(at);
        if self.app {
            for _ in 0..self.q.wheel_reports.unwrap_or(notches) {
                self.pty
                    .extend(format!("\x1b[<64;{};{}M", col + 1, row + 1).as_bytes());
            }
        } else if !self.q.scroll_ignored {
            self.offset = (self.offset + 3 * u64::from(notches)).min(self.max());
        }
        Ok(())
    }

    fn seat_commands(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.seat_cmds)
    }
}

impl PtyCapture for World {
    fn len(&self) -> usize {
        self.pty.len()
    }
    fn since(&self, offset: usize) -> String {
        corpus::hex(&self.pty[offset..])
    }
    fn alive_pids(&self) -> Vec<u32> {
        if self.app {
            vec![4242]
        } else {
            Vec::new()
        }
    }
    fn stop(&mut self) -> Vec<u32> {
        let pids = self.alive_pids();
        self.app = false;
        pids
    }
}

impl MouseApp for World {
    fn start_mouse_app(&mut self, pane: &str) -> Result<(), String> {
        self.actions.push(format!("app {pane}"));
        self.app = true;
        Ok(())
    }
}

impl EngineView for World {
    fn scroll(&mut self, _pane: &str) -> Result<Value, String> {
        Ok(
            json!({ "offset_from_bottom": self.offset, "max_offset_from_bottom": self.max(), "viewport_rows": ROWS }),
        )
    }
    fn visible(&mut self, _pane: &str) -> Result<String, String> {
        let lines = self.lines();
        let shown_offset = if self.q.offset_without_viewport {
            0
        } else {
            self.offset
        };
        let top = (self.max() - shown_offset) as usize;
        Ok(lines[top..(top + ROWS as usize).min(lines.len())].join("\n"))
    }
}

impl PaneFixture for World {
    fn show(&mut self, pane: &str, text: &str) -> Result<(), String> {
        self.actions.push(format!("show {pane}"));
        self.shown = Some(text.to_owned());
        self.offset = 0;
        Ok(())
    }
}

impl LinkRecorder for World {
    fn log_path(&self) -> String {
        handler().log.to_string_lossy().into_owned()
    }
    fn entries(&self) -> Result<Vec<String>, String> {
        Ok(self.entries.clone())
    }
}

fn ok_motion(x: f64, y: f64) -> Value {
    let (x, y) = (x.round() as u32, y.round() as u32);
    pointer_ledger_entry(
        "motion_absolute",
        vec![
            "motion_absolute".into(),
            x.to_string(),
            y.to_string(),
            "1280".into(),
            "720".into(),
        ],
        json!({ "x": x, "y": y, "x_extent": 1280, "y_extent": 720 }),
        &Ok(()),
    )
}

fn ok_button(state: &str) -> Value {
    pointer_ledger_entry(
        "button",
        vec!["button".into(), "272".into(), state.into()],
        json!({ "button": 272, "state": state }),
        &Ok(()),
    )
}

fn ok_axis() -> Value {
    pointer_ledger_entry(
        "axis",
        vec!["axis".into(), "vertical".into(), "-15".into(), "-1".into()],
        json!({ "axis": "vertical", "value": -15.0, "discrete": -1 }),
        &Ok(()),
    )
}

fn identity() -> Value {
    json!({ "pane_id": "w1:p1", "generation": "3", "boot_prefix": "b00t1234", "endpoint": "local" })
}

fn point(cell: (u16, u16)) -> Value {
    json!({ "x": f64::from(cell.1) * 10.0 + 5.0, "y": f64::from(cell.0) * 20.0 + 5.0 })
}

fn detail(step: &str) -> Value {
    let mut d = identity();
    d["points"] = match step {
        "alt-screen-mouse" => json!({ "click": point(CLICK_CELL), "wheel": point(WHEEL_CELL) }),
        "scrollback-wheel" => json!({ "scroll": point(SCROLL_CELL) }),
        _ => json!({ "link": point(LINK_CELL) }),
    };
    d
}

fn ev(kind: &str, button: i64, ctrl: bool) -> Value {
    json!({ "type": kind, "trusted": true, "button": button, "ctrl": ctrl, "deltaY": if kind == "wheel" { -53.0 } else { 0.0 } })
}

fn page() -> Value {
    let cell = |c: (u16, u16)| json!([c.0, c.1]);
    json!({
        "identity_before": identity(),
        "identity_after": identity(),
        "cells": { "click": cell(CLICK_CELL), "wheel": cell(WHEEL_CELL), "scroll": cell(SCROLL_CELL), "link": cell(LINK_CELL) },
        "mouse_events": [ev("pointerdown", 0, false), ev("pointerup", 0, false), ev("wheel", -1, false)],
        "scroll_events": [ev("wheel", -1, false), ev("wheel", -1, false), ev("wheel", -1, false)],
        "link_events": [ev("pointerdown", 0, true), ev("pointerup", 0, true)],
        "link_uri": format!("{LINK_BASE}{NONCE}"),
    })
}

fn run(q: Quirks) -> (ParentLedger, World) {
    let mut world = World::new(q);
    let mut ledger = ParentLedger::default();
    for step in STEPS {
        let answer = parent_step(step, &detail(step), &exp(), &mut world).unwrap();
        ledger.record(step, answer).unwrap();
    }
    (ledger, world)
}

fn verdict(page: &Value, ledger: &ParentLedger) -> BTreeMap<&'static str, bool> {
    checks(PHASE, page, ledger, &exp())
        .unwrap()
        .into_iter()
        .collect()
}

fn only_failing(v: &BTreeMap<&'static str, bool>) -> Vec<&'static str> {
    v.iter().filter(|(_, ok)| !**ok).map(|(k, _)| *k).collect()
}

#[test]
fn checks_equal_the_plan_and_steps_are_valid_names() {
    let spec = plan::FLOW.iter().find(|p| p.name == PHASE).unwrap();
    assert_eq!(spec.checks, CHECKS);
    for s in STEPS {
        assert!(
            s.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'),
            "{s}"
        );
    }
}

#[test]
fn literals_are_fixed_and_distinguish_wrong_cells_or_encodings() {
    // Independent of the fake: SGR 1006 of CLICK_CELL/WHEEL_CELL, 1-based col;row.
    let (cr, cc) = CLICK_CELL;
    let (wr, wc) = WHEEL_CELL;
    let press = format!("\x1b[<0;{};{}M", cc + 1, cr + 1);
    assert_eq!(
        MOUSE_REPORTS,
        format!(
            "{press}\x1b[<0;{};{}m\x1b[<64;{};{}M",
            cc + 1,
            cr + 1,
            wc + 1,
            wr + 1
        )
    );
    assert_ne!(
        CLICK_CELL, WHEEL_CELL,
        "one cell would hide a swapped report"
    );
    let fixture = scroll_fixture();
    let lines: Vec<&str> = fixture.lines().collect();
    assert_eq!(lines.len() as u32, SCROLL_LINES);
    assert_eq!((lines[0], lines[199]), ("hd007-sb-001", "hd007-sb-200"));
    assert!(
        u64::from(SCROLL_LINES) > 3 * ROWS,
        "fixture must leave the viewport"
    );
    let e = exp();
    assert_eq!(
        e.link_uri,
        format!("https://hd007-link.invalid/open?nonce={NONCE}")
    );
    assert_eq!(
        link_fixture(&e.link_uri, NONCE),
        format!(
            "\x1b]8;;{}\x1b\\hd007-link-{NONCE}\x1b]8;;\x1b\\\n",
            e.link_uri
        )
    );
    assert_eq!(first_numbered("$ ls\nhd007-sb-042\nhd007-sb-043"), Some(42));
    assert_eq!(first_numbered("hd007-sb-42x\nnothing"), None);
    let cmd = mouse_app_command(Path::new("/tmp/hd7L-4242/mouse.raw")).unwrap();
    assert!(cmd.starts_with("printf '\\033[?1049h\\033[?1000h\\033[?1006h'; stty raw -echo; cat > '/tmp/hd7L-4242/mouse.raw'"), "{cmd}");
    assert!(
        cmd.ends_with("printf '\\033[?1006l\\033[?1000l\\033[?1049l'"),
        "{cmd}"
    );
    assert!(mouse_app_command(Path::new("rel/mouse.raw")).is_err());
    assert!(mouse_app_command(Path::new("/tmp/it's")).is_err());
}

#[test]
fn correct_run_passes_all_checks_and_the_linked_phase_is_prepared() {
    let (ledger, world) = run(Quirks {
        link_opens: 1,
        ..Quirks::default()
    });
    let v = verdict(&page(), &ledger);
    assert_eq!(v.keys().copied().collect::<Vec<_>>(), {
        let mut c = CHECKS.to_vec();
        c.sort();
        c
    });
    assert!(v.values().all(|ok| *ok), "{v:?}");
    assert_eq!(
        world.actions,
        [
            "app w1:p1",
            "click ctrl=false",
            "wheel 1",
            "show w1:p1",
            "wheel 3",
            "show w1:p1",
            "click ctrl=true"
        ]
    );
    let spec = plan::FLOW.iter().find(|p| p.name == PHASE).unwrap();
    // Linked in live.rs: Prepared, so a real run can pass it; the flow still needs every phase.
    assert!(matches!(spec.readiness, plan::Readiness::Prepared));
    let outcomes = [(
        PHASE.to_owned(),
        plan::PhaseOutcome::with(&v.clone().into_iter().collect::<Vec<_>>()),
    )]
    .into();
    let flow = plan::evaluate(plan::FLOW, &outcomes);
    assert!(
        flow.passed == [PHASE] && flow.failed.is_empty() && !flow.is_pass(plan::FLOW),
        "{flow:?}"
    );
}

#[test]
fn parent_step_records_seat_commands_with_exit_and_stdout_for_every_pointer_step() {
    let (ledger, _) = run(Quirks::default());
    for step in STEPS {
        let entry = &ledger.steps[step];
        let cmds = entry["seat_commands"]
            .as_array()
            .unwrap_or_else(|| panic!("{step} has no seat_commands array"));
        assert!(!cmds.is_empty(), "{step} has empty seat_commands");
        for cmd in cmds {
            assert!(
                cmd.get("argv").and_then(|v| v.as_array()).is_some(),
                "{step}: seat command missing argv array"
            );
            assert!(
                cmd.get("exit").is_some() || cmd.get("status").is_some(),
                "{step}: seat command missing exit status"
            );
            assert!(
                cmd.get("stdout").and_then(|v| v.as_str()).is_some(),
                "{step}: seat command missing stdout string"
            );
        }
    }
}

#[test]
fn every_pointer_step_records_virtual_pointer_actions_with_arguments_and_result() {
    // Wrong behavior: a step injecting with another method, losing the arguments of the action,
    // or reporting a failed injection as success. The sequences are the literal contract of the
    // private pointer: motion, left button press/release, one vertical axis step per wheel notch.
    let (ledger, _) = run(Quirks {
        link_opens: 1,
        ..Quirks::default()
    });
    let actions = |step: &str| -> Vec<String> {
        ledger.steps[step]["seat_commands"]
            .as_array()
            .unwrap_or_else(|| panic!("{step} has no seat_commands"))
            .iter()
            .map(|c| c["action"].as_str().unwrap_or_default().to_owned())
            .collect()
    };
    assert_eq!(
        actions("alt-screen-mouse"),
        [
            "motion_absolute",
            "button",
            "button",
            "motion_absolute",
            "axis"
        ]
    );
    assert_eq!(
        actions("scrollback-wheel"),
        ["motion_absolute", "axis", "axis", "axis"]
    );
    assert_eq!(
        actions("link-ctrl-click"),
        ["motion_absolute", "button", "button"]
    );
    for step in STEPS {
        let cmds = ledger.steps[step]["seat_commands"].as_array().unwrap();
        assert!(!cmds.is_empty(), "{step}");
        for cmd in cmds {
            assert_eq!(cmd["method"], "virtual_pointer", "{step}: {cmd}");
            assert_eq!(cmd["result"], "ok", "{step}: {cmd}");
            assert_eq!(cmd["exit"], 0, "{step}: {cmd}");
            assert!(cmd["args"].is_object(), "{step}: {cmd}");
            assert!(cmd["argv"].as_array().is_some(), "{step}: {cmd}");
        }
    }
    let click = &ledger.steps["alt-screen-mouse"]["seat_commands"][0];
    assert_eq!(click["args"]["x"].as_f64(), point(CLICK_CELL)["x"].as_f64());
    assert_eq!(click["args"]["y"].as_f64(), point(CLICK_CELL)["y"].as_f64());
    assert_eq!(click["args"]["x_extent"], 1280);
    assert_eq!(click["args"]["y_extent"], 720);
    let press = &ledger.steps["alt-screen-mouse"]["seat_commands"][1];
    assert_eq!(press["args"]["button"], 272);
    assert_eq!(press["args"]["state"], "pressed");
    let notch = &ledger.steps["scrollback-wheel"]["seat_commands"][1];
    assert_eq!(notch["args"]["axis"], "vertical");
    assert_eq!(notch["args"]["value"], -15.0);
    assert_eq!(notch["args"]["discrete"], -1);
    let link_press = &ledger.steps["link-ctrl-click"]["seat_commands"][1];
    assert_eq!(link_press["action"], "button");
    assert!(
        ledger.steps["link-ctrl-click"]["seat_commands"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["action"] != "axis"),
        "a click has no wheel axis"
    );
}

#[test]
fn duplicated_missing_or_misplaced_mouse_reports_fail_only_the_mouse_check() {
    let base = Quirks {
        link_opens: 1,
        ..Quirks::default()
    };
    for (name, q) in [
        (
            "duplicated press",
            Quirks {
                duplicate_press: true,
                ..base.clone()
            },
        ),
        (
            "missing release",
            Quirks {
                no_release: true,
                ..base.clone()
            },
        ),
        (
            "wrong column",
            Quirks {
                col_shift: 1,
                ..base.clone()
            },
        ),
        (
            "wheel reported twice",
            Quirks {
                wheel_reports: Some(2),
                ..base.clone()
            },
        ),
        (
            "wheel not reported",
            Quirks {
                wheel_reports: Some(0),
                ..base.clone()
            },
        ),
    ] {
        let (ledger, _) = run(q);
        assert_eq!(
            only_failing(&verdict(&page(), &ledger)),
            ["alt_screen_mouse_reports_once"],
            "{name}"
        );
    }
}

#[test]
fn untrusted_extra_or_modified_pointer_events_fail() {
    let (ledger, _) = run(Quirks {
        link_opens: 1,
        ..Quirks::default()
    });
    let mut synthetic = page();
    synthetic["mouse_events"][0]["trusted"] = json!(false);
    let mut extra = page();
    extra["mouse_events"]
        .as_array_mut()
        .unwrap()
        .push(ev("pointerdown", 0, false));
    let mut shift_click = page();
    shift_click["link_events"][0]["ctrl"] = json!(false);
    let mut scrolled_down = page();
    scrolled_down["scroll_events"][1]["deltaY"] = json!(53.0);
    let mut no_wheel = page();
    no_wheel["scroll_events"] = json!([]);
    for (name, p, check) in [
        (
            "synthetic pointerdown",
            synthetic,
            "alt_screen_mouse_reports_once",
        ),
        ("extra pointerdown", extra, "alt_screen_mouse_reports_once"),
        (
            "link click without ctrl",
            shift_click,
            "link_open_effect_recorded_once",
        ),
        ("wheel down", scrolled_down, "scrollback_navigates"),
        ("no native wheel", no_wheel, "scrollback_navigates"),
    ] {
        assert_eq!(only_failing(&verdict(&p, &ledger)), [check], "{name}");
    }
}

#[test]
fn unchanged_offset_or_request_without_viewport_move_fails_scrollback() {
    let base = Quirks {
        link_opens: 1,
        ..Quirks::default()
    };
    for (name, q) in [
        (
            "offset unchanged",
            Quirks {
                scroll_ignored: true,
                ..base.clone()
            },
        ),
        (
            "offset moved but viewport did not",
            Quirks {
                offset_without_viewport: true,
                ..base.clone()
            },
        ),
    ] {
        let (ledger, _) = run(q);
        assert_eq!(
            only_failing(&verdict(&page(), &ledger)),
            ["scrollback_navigates"],
            "{name}"
        );
    }
    let (mut ledger, _) = run(base);
    let answer = ledger.steps.get_mut("scrollback-wheel").unwrap();
    answer["scroll_before"]["offset_from_bottom"] = json!(2);
    answer["scroll_after"]["offset_from_bottom"] = json!(11);
    assert_eq!(
        only_failing(&verdict(&page(), &ledger)),
        ["scrollback_navigates"],
        "did not start at the bottom"
    );
}

#[test]
fn zero_duplicated_stale_or_wrong_link_effect_fails() {
    let base = Quirks {
        link_opens: 1,
        ..Quirks::default()
    };
    for (name, q) in [
        (
            "never opened",
            Quirks {
                link_opens: 0,
                ..base.clone()
            },
        ),
        (
            "opened twice",
            Quirks {
                link_opens: 2,
                ..base.clone()
            },
        ),
        (
            "other uri",
            Quirks {
                link_uri: Some(format!("{LINK_BASE}other1")),
                ..base.clone()
            },
        ),
        (
            "recorder already had an entry",
            Quirks {
                stale_entry: true,
                ..base.clone()
            },
        ),
    ] {
        let (ledger, _) = run(q);
        assert_eq!(
            only_failing(&verdict(&page(), &ledger)),
            ["link_open_effect_recorded_once"],
            "{name}"
        );
    }
    let (ledger, _) = run(base);
    let mut wrong_toolbar = page();
    wrong_toolbar["link_uri"] = json!(format!("{LINK_BASE}other1"));
    assert_eq!(
        only_failing(&verdict(&wrong_toolbar, &ledger)),
        ["link_open_effect_recorded_once"]
    );
}

#[test]
fn swapped_pane_host_boot_socket_or_cells_fail_every_check() {
    let (ledger, _) = run(Quirks {
        link_opens: 1,
        ..Quirks::default()
    });
    let all_false = |p: &Value, l: &ParentLedger| verdict(p, l).values().all(|ok| !*ok);
    for (key, value) in [
        ("pane_id", "w1:p2"),
        ("boot_prefix", "b00t9999"),
        ("generation", "4"),
    ] {
        let mut p = page();
        p["identity_after"][key] = json!(value);
        assert!(
            all_false(&p, &ledger),
            "page {key} changed during the phase"
        );
        let mut l = ledger.clone();
        for answer in l.steps.values_mut() {
            answer["identity"][key] = json!(value);
        }
        assert!(all_false(&page(), &l), "parent acted on another {key}");
    }
    let mut host = page();
    host["identity_before"]["endpoint"] = json!("ssh:box");
    host["identity_after"]["endpoint"] = json!("ssh:box");
    assert!(all_false(&host, &ledger), "another host");
    let mut cells = page();
    cells["cells"]["click"] = json!([2, 6]);
    assert!(all_false(&cells, &ledger), "page aimed at other cells");
    let (other_display, _) = run(Quirks {
        link_opens: 1,
        socket: Some("/tmp/hd7L-9/sway-ipc.1000.5.sock".into()),
        ..Quirks::default()
    });
    assert!(
        all_false(&page(), &other_display),
        "pointer on another compositor"
    );
}

#[test]
fn page_claim_without_parent_observation_or_page_error_is_an_error() {
    let (ledger, _) = run(Quirks {
        link_opens: 1,
        ..Quirks::default()
    });
    for missing in STEPS {
        let mut l = ledger.clone();
        l.steps.remove(missing);
        assert!(checks(PHASE, &page(), &l, &exp()).is_err(), "{missing}");
    }
    for key in [
        "mouse_events",
        "scroll_events",
        "link_events",
        "cells",
        "identity_after",
    ] {
        let mut p = page();
        p.as_object_mut().unwrap().remove(key);
        assert!(checks(PHASE, &p, &ledger, &exp()).is_err(), "{key}");
    }
    let mut failed = page();
    failed["error"] = json!("terminal canvas missing");
    assert!(checks(PHASE, &failed, &ledger, &exp()).is_err());
    assert!(checks("paste-selection", &page(), &ledger, &exp()).is_err());
}

#[test]
fn parent_refuses_before_any_action() {
    let e = exp();
    let mut world = World::new(Quirks::default());
    assert!(parent_step(
        "scroll-anything",
        &detail("alt-screen-mouse"),
        &e,
        &mut world
    )
    .is_err());
    let mut foreign = detail("alt-screen-mouse");
    foreign["endpoint"] = json!("ssh:box");
    assert!(parent_step("alt-screen-mouse", &foreign, &e, &mut world).is_err());
    let mut no_boot = detail("scrollback-wheel");
    no_boot["boot_prefix"] = json!("");
    assert!(parent_step("scrollback-wheel", &no_boot, &e, &mut world).is_err());
    for bad in [
        json!({ "x": 0.0, "y": 10.0 }),
        json!({ "x": 1280.0, "y": 10.0 }),
        json!({ "x": 5.0 }),
        json!(null),
    ] {
        let mut d = detail("link-ctrl-click");
        d["points"]["link"] = bad.clone();
        assert!(
            parent_step("link-ctrl-click", &d, &e, &mut world).is_err(),
            "{bad}"
        );
    }
    assert!(world.actions.is_empty(), "{:?}", world.actions);
    assert!(Point::output(&json!({ "x": 1.0, "y": 719.5 })).is_ok());
    let mut ledger = ParentLedger::default();
    ledger.record("alt-screen-mouse", json!({})).unwrap();
    assert!(
        ledger.record("alt-screen-mouse", json!({})).is_err(),
        "replayed observation"
    );
}

#[test]
fn expectations_refuse_non_private_compositor_or_weak_nonce() {
    let h = handler();
    assert!(Expectations::new("", RUNTIME, SOCKET, &h, NONCE).is_err());
    assert!(Expectations::new(
        "local",
        "/run/user/1000",
        "/run/user/1000/sway-ipc.1000.7.sock",
        &h,
        NONCE
    )
    .is_err());
    assert!(Expectations::new(
        "local",
        RUNTIME,
        "/tmp/hd7L-9/sway-ipc.1000.7.sock",
        &h,
        NONCE
    )
    .is_err());
    for nonce in ["", "abc", "UPPER123", "a b c d e f", "ab/../cd"] {
        assert!(
            Expectations::new("local", RUNTIME, SOCKET, &h, nonce).is_err(),
            "{nonce}"
        );
    }
    assert_eq!(
        exp().recorder_log,
        format!("{HOME}/state/hd007-link-recorder.log")
    );
}

fn wtype_launch(runtime: &str) -> display::Launch {
    display::Launch {
        program: PathBuf::from("/usr/bin/wtype"),
        args: vec!["-s".into(), "250".into()],
        env: vec![
            ("PATH".into(), "/usr/bin:/bin".into()),
            ("XDG_RUNTIME_DIR".into(), runtime.into()),
            ("WAYLAND_DISPLAY".into(), format!("{runtime}/wayland-1")),
            (
                "SWAYSOCK".into(),
                "/run/user/1000/sway-ipc.1000.1.sock".into(),
            ),
            ("DISPLAY".into(), ":0".into()),
        ],
    }
}

#[test]
fn pointer_env_targets_only_the_private_compositor() {
    let prefix = Path::new("/p/native-input/prefix");
    let env = PointerEnv::new(&wtype_launch(RUNTIME), prefix, 1000, 77).unwrap();
    assert_eq!(env.socket(), SOCKET);
    let set = env.cursor_set(Point { x: 55.4, y: 44.6 });
    assert_eq!(set.program, prefix.join("usr/bin/swaymsg"));
    assert_eq!(
        set.args,
        ["-s", SOCKET, "seat", "seat0", "cursor", "set", "55", "45"]
    );
    assert_eq!(env.button(true, 1).args[5..], ["press", "button1"]);
    assert_eq!(env.button(false, 1).args[5..], ["release", "button1"]);
    assert_eq!(env.button(true, 4).args[5..], ["press", "button4"]);
    let ctrl = env.ctrl_hold(1500);
    assert_eq!(ctrl.program, Path::new("/usr/bin/wtype"));
    assert_eq!(ctrl.args, ["-M", "ctrl", "-s", "1500", "-m", "ctrl"]);
    for l in [&set, &ctrl] {
        assert_eq!(l.var("XDG_RUNTIME_DIR"), Some(RUNTIME));
        assert!(
            l.var("SWAYSOCK").is_none() && l.var("DISPLAY").is_none(),
            "user session leaked"
        );
    }
    assert_eq!(
        set.var("LD_LIBRARY_PATH"),
        Some("/p/native-input/prefix/usr/lib")
    );
    assert!(PointerEnv::new(&wtype_launch("/run/user/1000"), prefix, 1000, 77).is_err());
    let mut no_runtime = wtype_launch(RUNTIME);
    no_runtime.env.retain(|(k, _)| k != "XDG_RUNTIME_DIR");
    assert!(PointerEnv::new(&no_runtime, prefix, 1000, 77).is_err());
}

#[test]
fn link_handler_is_private_records_only_and_parses_each_run() {
    let h = handler();
    let files = h.files();
    let paths: Vec<_> = files.iter().map(|(p, _, _)| p.clone()).collect();
    assert_eq!(
        paths,
        [
            PathBuf::from(format!("{HOME}/bin/hd007-link-recorder")),
            PathBuf::from(format!(
                "{HOME}/data/applications/hd007-link-recorder.desktop"
            )),
            PathBuf::from(format!("{HOME}/config/mimeapps.list")),
        ]
    );
    let (script, desktop, mime) = (&files[0].1, &files[1].1, &files[2].1);
    assert_eq!(files[0].2, 0o700);
    assert!(script.starts_with("#!/bin/sh\n") && script.ends_with("exit 0\n"));
    assert!(script.contains(&format!(">> '{HOME}/state/hd007-link-recorder.log'")));
    for forbidden in [
        "xdg-open", "BROWSER", "curl", "wget", "firefox", "chrom", "gio",
    ] {
        assert!(
            !script.contains(forbidden),
            "recorder must not open anything: {forbidden}"
        );
    }
    assert!(desktop.contains(&format!("\nExec={HOME}/bin/hd007-link-recorder %u\n")));
    assert!(mime.contains("x-scheme-handler/https=hd007-link-recorder.desktop"));
    assert!(mime.contains("x-scheme-handler/http=hd007-link-recorder.desktop"));
    assert_eq!(
        h.window_env(),
        [
            ("XDG_CURRENT_DESKTOP".to_owned(), "X-Generic".to_owned()),
            (
                "BROWSER".to_owned(),
                format!("{HOME}/bin/hd007-link-recorder")
            ),
        ]
    );
    for bad in [
        "/",
        "rel/home",
        "/run/user/1000/h",
        "/home/someone",
        "/home",
        "/tmp/with space",
    ] {
        assert!(
            LinkHandler::new(Path::new(bad), Path::new("/home/someone"), 1000).is_err(),
            "{bad}"
        );
    }
    let uri = format!("{LINK_BASE}{NONCE}");
    assert_eq!(
        LinkHandler::parse_log(&format!("91\t1\t{uri}\n")),
        [uri.as_str()]
    );
    assert_eq!(
        LinkHandler::parse_log(&format!("91\t1\t{uri}\n92\t1\t{uri}\n")).len(),
        2,
        "two runs stay two"
    );
    assert_eq!(
        LinkHandler::parse_log(&format!("93\t2\t{uri}\n")),
        [format!("malformed:93\t2\t{uri}")]
    );
    assert!(LinkHandler::parse_log("").is_empty());
}

#[test]
fn virtual_pointer_ledger_entry_carries_method_arguments_and_result() {
    // Wrong behavior: the ledger hides how the pointer was injected (no `method`), loses the
    // arguments of the action, or reports a failed injection as success.
    use mouse_flow::pointer_ledger_entry;
    let ok: Result<(), String> = Ok(());
    let motion = pointer_ledger_entry(
        "motion_absolute",
        vec![
            "motion_absolute".into(),
            "55".into(),
            "45".into(),
            "1280".into(),
            "720".into(),
        ],
        json!({ "x": 55, "y": 45, "x_extent": 1280, "y_extent": 720 }),
        &ok,
    );
    assert_eq!(motion["method"], "virtual_pointer");
    assert_eq!(motion["action"], "motion_absolute");
    assert_eq!(motion["args"]["x"], 55);
    assert_eq!(motion["args"]["y"], 45);
    assert_eq!(motion["args"]["x_extent"], 1280);
    assert_eq!(motion["args"]["y_extent"], 720);
    assert_eq!(
        motion["argv"],
        json!(["motion_absolute", "55", "45", "1280", "720"])
    );
    assert_eq!(motion["result"], "ok");
    assert_eq!(motion["exit"], 0);
    assert_eq!(motion["stdout"], "");
    let failed = pointer_ledger_entry(
        "axis",
        vec!["axis".into(), "vertical".into(), "-15".into(), "-1".into()],
        json!({ "axis": "vertical", "value": -15.0, "discrete": -1 }),
        &Err("connection lost".into()),
    );
    assert_eq!(failed["method"], "virtual_pointer");
    assert_eq!(failed["result"], "error: connection lost");
    assert_eq!(failed["exit"], -1);
    assert_ne!(
        failed["result"], "ok",
        "a failed action never reports success"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn private_pointer_without_the_private_wayland_display_is_an_error_without_metric() {
    // Wrong behavior: connecting to the user's display (or to nothing) and still producing a
    // pointer metric. The absent private socket must fail before any action exists.
    use std::os::unix::fs::PermissionsExt;

    use mouse_flow::PrivatePointer;
    let runtime = format!("/tmp/hd7L-{}-absent", std::process::id());
    let _ = std::fs::remove_dir_all(&runtime);
    std::fs::create_dir_all(&runtime).unwrap();
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).unwrap();
    let env = PointerEnv::new(
        &wtype_launch(&runtime),
        Path::new("/p/native-input/prefix"),
        1000,
        77,
    )
    .unwrap();
    assert!(
        env.wayland_socket().is_err(),
        "an absent socket cannot resolve"
    );
    let err = match PrivatePointer::new(env) {
        Ok(_) => panic!("an absent private display must not yield a pointer"),
        Err(e) => e,
    };
    assert!(err.contains(&format!("{runtime}/wayland-1")), "{err}");
    assert!(err.contains("absent"), "{err}");
    let _ = std::fs::remove_dir_all(&runtime);
}

#[cfg(target_os = "linux")]
#[test]
fn a_point_outside_the_private_output_is_refused_without_any_pointer_action() {
    // Wrong behavior: the virtual pointer is asked to inject at a coordinate the headless output
    // does not have (e.g. a stale/foreign mapping) and still reports a metric.
    use mouse_flow::PrivatePointer;
    let mut pointer = PrivatePointer::with_commands(Vec::new());
    for at in [
        Point {
            x: 1280.0,
            y: 100.0,
        },
        Point { x: 5.0, y: 720.0 },
        Point { x: 0.5, y: 10.0 },
        Point { x: 10.0, y: 0.0 },
    ] {
        assert!(pointer.click(at, false).is_err(), "{at:?}");
    }
    assert!(pointer.wheel_up(Point { x: 5.0, y: 0.0 }, 3).is_err());
    assert!(
        pointer.seat_commands().is_empty(),
        "a refused point must not produce a metric"
    );
    // A valid point still needs the private display device; that failure is recorded as an error
    // result, never as success.
    assert!(pointer.click(Point { x: 5.0, y: 5.0 }, false).is_err());
    let commands = pointer.seat_commands();
    assert_eq!(commands.len(), 1, "{commands:?}");
    assert_eq!(commands[0]["method"], "virtual_pointer");
    assert_eq!(commands[0]["action"], "motion_absolute");
    assert_eq!(commands[0]["exit"], -1);
    assert!(
        commands[0]["result"]
            .as_str()
            .is_some_and(|r| r.starts_with("error:")),
        "{commands:?}"
    );
}

#[test]
fn pointer_env_refuses_a_wayland_display_outside_the_private_runtime() {
    // Wrong behavior: falling back to the user's session socket (WAYLAND_DISPLAY under /run/user),
    // a relative display name, or a missing display.
    let prefix = Path::new("/p/native-input/prefix");
    for display in [Some("/run/user/1000/wayland-0"), Some("wayland-1"), None] {
        let mut launch = wtype_launch(RUNTIME);
        launch.env.retain(|(k, _)| k != "WAYLAND_DISPLAY");
        if let Some(d) = display {
            launch.env.push(("WAYLAND_DISPLAY".into(), d.into()));
        }
        assert!(
            PointerEnv::new(&launch, prefix, 1000, 77).is_err(),
            "{display:?}"
        );
    }
}

#[test]
fn get_tree_reads_only_the_private_compositor() {
    // Wrong behavior: reading geometry of the user's sway (SWAYSOCK/DISPLAY inherited, no `-s`),
    // or a mutating/unrelated IPC message instead of the raw tree.
    let prefix = Path::new("/p/native-input/prefix");
    let env = PointerEnv::new(&wtype_launch(RUNTIME), prefix, 1000, 77).unwrap();
    let tree = env.get_tree();
    assert_eq!(tree.program, prefix.join("usr/bin/swaymsg"));
    assert_eq!(tree.args, ["-s", SOCKET, "-t", "get_tree", "-r"]);
    assert_eq!(tree.var("XDG_RUNTIME_DIR"), Some(RUNTIME));
    assert!(tree.var("SWAYSOCK").is_none() && tree.var("DISPLAY").is_none());
    assert_eq!(
        tree.var("LD_LIBRARY_PATH"),
        Some("/p/native-input/prefix/usr/lib")
    );
}
