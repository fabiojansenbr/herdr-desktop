//! Contracts of the `paste-selection` phase of the spec 007 native flow
//! (tests/fidelity-native/paste_flow.rs): parent steps over fake keyboard/PTY/clipboard/pane
//! seams and pure evaluators. No engine, display, GUI or clipboard tool runs here.
//! Each negative case names the wrong behavior it would catch.

#[allow(dead_code)]
#[path = "../../tests/fidelity-native/corpus.rs"]
mod corpus;
#[allow(dead_code)]
#[path = "../../tests/fidelity-native/display.rs"]
mod display;
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

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use paste_flow::{
    checks, cleanup_plan, parent_step, Clipboard, ClipboardEnv, Expectations, Keys, PaneFixture,
    ParentLedger, PtyCapture, CHECKS, COPY_KEYS, PHASE, SELECTION_END_COL, SELECTION_LINES,
    SELECTION_TEXT, STEPS,
};

const RUNTIME: &str = "/tmp/hd7L-4242";
const WAYLAND: &str = "/tmp/hd7L-4242/wayland-1";
const PANE: &str = "w1:p1";
const SENTINEL: &str = "hd007-clipboard-sentinel-n0nce7";

fn corpus() -> corpus::Corpus {
    corpus::parse(include_str!(
        "../../tests/fidelity-native/fixtures/corpus.json"
    ))
    .unwrap()
}

fn exp() -> Expectations {
    Expectations::new(&corpus(), RUNTIME, WAYLAND, "local", SENTINEL).unwrap()
}

/// Fake world: what the "app" does when the parent presses keys. Behaviors are closures so a
/// case can make the app paste twice, drop a line, copy the wrong text, ...
struct World {
    clipboard: Option<String>,
    pty: Vec<u8>,
    capture_alive: bool,
    shown: Option<(String, String)>,
    on_paste_keys: Box<dyn Fn(&mut World)>,
    on_copy_keys: Box<dyn Fn(&mut World)>,
    clipboard_display: String,
    keys: Vec<Vec<String>>,
}

impl World {
    fn good() -> Self {
        World {
            clipboard: None,
            pty: Vec::new(),
            capture_alive: true,
            shown: None,
            on_paste_keys: Box::new(|w| {
                let text = w.clipboard.clone().unwrap_or_default();
                w.pty.extend_from_slice(text.as_bytes());
            }),
            on_copy_keys: Box::new(|w| w.clipboard = Some(SELECTION_TEXT.to_owned())),
            clipboard_display: WAYLAND.to_owned(),
            keys: Vec::new(),
        }
    }
}

struct Fakes(RefCell<World>);

impl Keys for Fakes {
    fn press(&mut self, keys: &[String]) -> Result<(), String> {
        let mut w = self.0.borrow_mut();
        w.keys.push(keys.to_vec());
        let act = if keys.last().map(String::as_str) == Some("ctrl") && keys.contains(&"v".into()) {
            std::mem::replace(&mut w.on_paste_keys, Box::new(|_| {}))
        } else {
            std::mem::replace(&mut w.on_copy_keys, Box::new(|_| {}))
        };
        act(&mut w);
        Ok(())
    }
}

impl PtyCapture for Fakes {
    fn len(&self) -> usize {
        self.0.borrow().pty.len()
    }
    fn since(&self, offset: usize) -> String {
        corpus::hex(&self.0.borrow().pty[offset..])
    }
    fn alive_pids(&self) -> Vec<u32> {
        if self.0.borrow().capture_alive {
            vec![77]
        } else {
            vec![]
        }
    }
    fn stop(&mut self) -> Vec<u32> {
        let pids = self.alive_pids();
        self.0.borrow_mut().capture_alive = false;
        pids
    }
}

impl Clipboard for Fakes {
    fn display(&self) -> (String, String) {
        (
            RUNTIME.to_owned(),
            self.0.borrow().clipboard_display.clone(),
        )
    }
    fn set(&mut self, text: &str) -> Result<(), String> {
        self.0.borrow_mut().clipboard = Some(text.to_owned());
        Ok(())
    }
    fn read(&mut self) -> Result<Vec<u8>, String> {
        self.0
            .borrow()
            .clipboard
            .clone()
            .map(String::into_bytes)
            .ok_or_else(|| "No selection".to_owned())
    }
}

impl PaneFixture for Fakes {
    fn show(&mut self, pane: &str, text: &str) -> Result<(), String> {
        self.0.borrow_mut().shown = Some((pane.to_owned(), text.to_owned()));
        Ok(())
    }
}

fn identity() -> Value {
    json!({ "pane_id": PANE, "generation": "3", "boot_prefix": "b10c41aa", "endpoint": "local" })
}

fn ev_key(kind: &str, key: &str, trusted: bool) -> Value {
    json!({ "type": kind, "trusted": trusted, "key": key, "ctrl": true, "shift": true })
}

fn ev(kind: &str, trusted: bool) -> Value {
    ev_key(kind, if kind == "keydown" { "C" } else { "" }, trusted)
}

/// Correct run: parent steps answered through the fakes, page report of the phase.
fn run_with(world: World) -> (ParentLedger, Value) {
    let e = exp();
    let mut f = Fakes(RefCell::new(world));
    let mut ledger = ParentLedger::default();
    for step in STEPS {
        let answer = parent_step(step, &identity(), &e, &mut f).unwrap();
        ledger.record(step, answer).unwrap();
    }
    let page = json!({
        "phase": PHASE, "error": null,
        "identity_before": identity(), "identity_after": identity(),
        "paste_events": [],
        "paste_keydowns": [ev_key("keydown", "V", true)],
        "copy_keydowns": [ev_key("keydown", "C", true)],
        "drag": { "anchor": [0, 0], "cursor": [1, SELECTION_END_COL] },
        "status_before_copy": "Seleção: 2 linhas",
        "status_after_copy": "Seleção: 2 linhas Seleção copiada",
    });
    (ledger, page)
}

fn verdict(page: &Value, ledger: &ParentLedger) -> std::collections::BTreeMap<&'static str, bool> {
    checks(PHASE, page, ledger, &exp())
        .unwrap()
        .into_iter()
        .collect()
}

#[test]
fn checks_equal_the_plan_and_steps_are_valid_names() {
    let spec = plan::FLOW.iter().find(|p| p.name == PHASE).unwrap();
    assert_eq!(spec.checks, CHECKS);
    for step in STEPS {
        assert!(step.len() <= 64 && step.chars().all(|c| c.is_ascii_lowercase() || c == '-'));
    }
}

#[test]
fn literals_cover_multiline_unicode_and_double_width() {
    let e = exp();
    // From the existing corpus item, not a new invented paste.
    assert_eq!(e.paste_text, "linha 1\nlinha ação 2\n");
    assert_eq!(
        SELECTION_TEXT,
        format!("{}\n{}", SELECTION_LINES[0], SELECTION_LINES[1])
    );
    assert!(SELECTION_LINES[0].contains('你') && SELECTION_LINES[1].contains('表'));
    // End column counted by hand: "segunda " 8 + 表格 4 + " ação 2" 7 = 19 cells → last col 18.
    assert_eq!(SELECTION_END_COL, 18);
    assert!(
        !SELECTION_TEXT.contains(SELECTION_LINES[2]),
        "third line is outside the selection"
    );
    assert_eq!(
        COPY_KEYS.join(" "),
        "-M ctrl -M shift -k c -m shift -m ctrl"
    );
}

#[test]
fn correct_run_passes_both_checks_and_the_linked_phase_passes_alone_not_the_flow() {
    let (ledger, page) = run_with(World::good());
    let checks = checks(PHASE, &page, &ledger, &exp()).unwrap();
    assert!(checks.iter().all(|(_, ok)| *ok), "{checks:?}");
    let outcomes = [(PHASE.to_owned(), plan::PhaseOutcome::with(&checks))].into();
    let v = plan::evaluate(plan::FLOW, &outcomes);
    // Linked in live.rs (Prepared): the phase passes from these outcomes; the flow never does.
    assert!(
        v.passed == [PHASE] && v.failed.is_empty() && !v.is_pass(plan::FLOW),
        "{v:?}"
    );
}

#[test]
fn duplicated_paste_fails() {
    let mut w = World::good();
    w.on_paste_keys = Box::new(|w| {
        let t = w.clipboard.clone().unwrap();
        w.pty.extend_from_slice(t.as_bytes());
        w.pty.extend_from_slice(t.as_bytes());
    });
    let (ledger, page) = run_with(w);
    assert!(!verdict(&page, &ledger)["multiline_paste_once"]);
    let (ledger, mut page) = run_with(World::good());
    page["paste_keydowns"] = json!([ev_key("keydown", "V", true), ev_key("keydown", "V", true)]);
    assert!(
        !verdict(&page, &ledger)["multiline_paste_once"],
        "two paste keydowns"
    );
}

#[test]
fn lost_line_or_extra_bytes_fail() {
    let mut w = World::good();
    w.on_paste_keys = Box::new(|w| w.pty.extend_from_slice(b"linha 1\n"));
    let (ledger, page) = run_with(w);
    assert!(
        !verdict(&page, &ledger)["multiline_paste_once"],
        "lost line"
    );
    let mut w = World::good();
    w.on_paste_keys = Box::new(|w| {
        w.pty
            .extend_from_slice(b"\x1b[200~linha 1\nlinha a\xc3\xa7\xc3\xa3o 2\n\x1b[201~")
    });
    let (ledger, page) = run_with(w);
    assert!(
        !verdict(&page, &ledger)["multiline_paste_once"],
        "bracket bytes cat never enabled"
    );
    let mut w = World::good();
    w.on_paste_keys = Box::new(|w| {
        w.pty
            .extend_from_slice(b"linha 1\rlinha a\xc3\xa7\xc3\xa3o 2\r")
    });
    let (ledger, page) = run_with(w);
    assert!(
        !verdict(&page, &ledger)["multiline_paste_once"],
        "CR rewrite is not the literal"
    );
}

#[test]
fn shell_consuming_the_paste_or_untrusted_event_fails() {
    let mut w = World::good();
    w.on_paste_keys = Box::new(|w| {
        let t = w.clipboard.clone().unwrap();
        w.pty.extend_from_slice(t.as_bytes());
        w.capture_alive = false; // the raw cat died: the lines reached a shell
    });
    let (ledger, page) = run_with(w);
    assert!(!verdict(&page, &ledger)["multiline_paste_once"]);
    let (ledger, mut page) = run_with(World::good());
    page["paste_keydowns"] = json!([ev_key("keydown", "V", false)]);
    assert!(
        !verdict(&page, &ledger)["multiline_paste_once"],
        "untrusted paste keydown"
    );
}

#[test]
fn fabricated_dom_paste_event_fails() {
    let (ledger, mut page) = run_with(World::good());
    page["paste_events"] = json!([ev("paste", true)]);
    assert!(
        !verdict(&page, &ledger)["multiline_paste_once"],
        "fabricated DOM paste event is rejected"
    );
}

#[test]
fn absent_or_wrong_clipboard_fails() {
    let mut w = World::good();
    w.on_copy_keys = Box::new(|_| {}); // copy never reached the clipboard: sentinel stays
    let (ledger, page) = run_with(w);
    assert!(!verdict(&page, &ledger)["selection_copied_to_private_clipboard"]);
    let mut w = World::good();
    w.on_copy_keys = Box::new(|w| w.clipboard = None);
    let (ledger, page) = run_with(w);
    assert!(
        !verdict(&page, &ledger)["selection_copied_to_private_clipboard"],
        "cleared"
    );
    for wrong in [
        SELECTION_LINES[0].to_owned(),                 // truncated to one line
        format!("{SELECTION_TEXT}\n{SELECTION_TEXT}"), // doubled
        SELECTION_TEXT.replace('你', "?"),             // wide char lost
        format!("{SELECTION_TEXT}\n{}", SELECTION_LINES[2]), // over-selected
        format!("{SELECTION_TEXT}\n"),                 // extra newline
    ] {
        let mut w = World::good();
        w.on_copy_keys = Box::new(move |w| w.clipboard = Some(wrong.clone()));
        let (ledger, page) = run_with(w);
        assert!(!verdict(&page, &ledger)["selection_copied_to_private_clipboard"]);
    }
}

#[test]
fn page_claim_without_parent_observation_is_an_error() {
    let (mut ledger, page) = run_with(World::good());
    ledger.steps.remove("selection-copy");
    assert!(checks(PHASE, &page, &ledger, &exp()).is_err());
    let (ledger, mut page) = run_with(World::good());
    page["clipboard_text"] = json!(SELECTION_TEXT); // relayed by the page: ignored
    page.as_object_mut().unwrap().remove("status_after_copy");
    assert!(checks(PHASE, &page, &ledger, &exp()).is_err());
    let (ledger, mut page) = run_with(World::good());
    page["error"] = json!("timed out");
    assert!(checks(PHASE, &page, &ledger, &exp()).is_err());
    assert!(checks("native-keys", &page, &ledger, &exp()).is_err());
}

#[test]
fn missing_copy_confirmation_or_untrusted_chord_fails() {
    let key = "selection_copied_to_private_clipboard";
    let (ledger, mut page) = run_with(World::good());
    page["status_after_copy"] = json!("Seleção: 2 linhas Falha: clipboard_failed");
    assert!(!verdict(&page, &ledger)[key]);
    let (ledger, mut page) = run_with(World::good());
    page["copy_keydowns"] = json!([ev("keydown", false)]);
    assert!(
        !verdict(&page, &ledger)[key],
        "copy chord dispatched by the page"
    );
    let (ledger, mut page) = run_with(World::good());
    page["drag"]["cursor"] = json!([1, SELECTION_END_COL - 2]);
    assert!(
        !verdict(&page, &ledger)[key],
        "drag did not cover the known cells"
    );
}

#[test]
fn swapped_pane_host_or_boot_fails_both() {
    for (field, other) in [
        ("pane_id", "w1:p2"),
        ("boot_prefix", "55e4aabb"),
        ("endpoint", "ssh:6c1d"),
        ("generation", "4"),
    ] {
        let (ledger, mut page) = run_with(World::good());
        page["identity_after"][field] = json!(other);
        let v = verdict(&page, &ledger);
        assert!(
            !v["multiline_paste_once"] && !v["selection_copied_to_private_clipboard"],
            "{field}"
        );
    }
    // Parent measured another pane than the one the page confirmed.
    let e = exp();
    let mut f = Fakes(RefCell::new(World::good()));
    let mut ledger = ParentLedger::default();
    let mut other = identity();
    other["pane_id"] = json!("w1:p2");
    for step in STEPS {
        ledger
            .record(step, parent_step(step, &other, &e, &mut f).unwrap())
            .unwrap();
    }
    let (_, page) = run_with(World::good());
    let v = verdict(&page, &ledger);
    assert!(!v["multiline_paste_once"] && !v["selection_copied_to_private_clipboard"]);
    // The page must not be able to ask for a non-local host.
    let mut ssh = identity();
    ssh["endpoint"] = json!("ssh:6c1d");
    assert!(parent_step(
        "paste-native",
        &ssh,
        &e,
        &mut Fakes(RefCell::new(World::good()))
    )
    .is_err());
}

#[test]
fn observation_on_another_display_fails() {
    let mut w = World::good();
    w.clipboard_display = "/run/user/1000/wayland-1".into();
    let (ledger, page) = run_with(w);
    let v = verdict(&page, &ledger);
    assert!(!v["multiline_paste_once"] && !v["selection_copied_to_private_clipboard"]);
}

#[test]
fn ledger_refuses_replay_and_parent_refuses_unknown_step() {
    let (mut ledger, _) = run_with(World::good());
    assert!(ledger.record("paste-native", json!({})).is_err());
    let e = exp();
    let mut f = Fakes(RefCell::new(World::good()));
    assert!(parent_step("native-keys", &identity(), &e, &mut f).is_err());
    assert!(parent_step("paste-native", &json!({ "pane_id": "" }), &e, &mut f).is_err());
    assert!(
        f.0.borrow().keys.is_empty(),
        "no key pressed for a refused step"
    );
}

fn launch(runtime: &str, wayland: &str) -> display::Launch {
    display::Launch {
        program: PathBuf::from("/usr/bin/wtype"),
        args: vec!["-s".into(), "250".into()],
        env: vec![
            ("PATH".into(), "/usr/bin:/bin".into()),
            ("XDG_RUNTIME_DIR".into(), runtime.into()),
            ("WAYLAND_DISPLAY".into(), wayland.into()),
        ],
    }
}

#[test]
fn clipboard_env_is_explicitly_private_and_never_the_users() {
    let bus = format!("unix:path={RUNTIME}/bus");
    let env = ClipboardEnv::new(&launch(RUNTIME, WAYLAND), &bus, 1000).unwrap();
    let copy = env.copy_foreground();
    assert_eq!(copy.program, Path::new("/usr/bin/wl-copy"));
    assert_eq!(copy.args[0], "--foreground");
    let paste = env.paste();
    assert_eq!(paste.program, Path::new("/usr/bin/wl-paste"));
    assert!(paste.args.contains(&"--no-newline".to_owned()));
    for l in [&copy, &paste] {
        assert_eq!(l.var("XDG_RUNTIME_DIR"), Some(RUNTIME));
        assert_eq!(l.var("WAYLAND_DISPLAY"), Some(WAYLAND));
        assert_eq!(l.var("DBUS_SESSION_BUS_ADDRESS"), Some(bus.as_str()));
        assert!(l.var("DISPLAY").is_none());
    }
    let user = "/run/user/1000";
    assert!(ClipboardEnv::new(&launch(user, "/run/user/1000/wayland-1"), &bus, 1000).is_err());
    assert!(ClipboardEnv::new(&launch(RUNTIME, "/run/user/1000/wayland-1"), &bus, 1000).is_err());
    assert!(ClipboardEnv::new(&launch(RUNTIME, "wayland-1"), &bus, 1000).is_err());
    assert!(ClipboardEnv::new(
        &launch(RUNTIME, WAYLAND),
        "unix:path=/run/user/1000/bus",
        1000
    )
    .is_err());
    let mut no_runtime = launch(RUNTIME, WAYLAND);
    no_runtime.env.retain(|(k, _)| k != "XDG_RUNTIME_DIR");
    assert!(ClipboardEnv::new(&no_runtime, &bus, 1000).is_err());
}

#[test]
fn cleanup_signals_only_tracked_processes_with_the_same_starttime() {
    let tracked = [(10, 500), (11, 600), (12, 700)];
    // 10 still ours; 11 exited; 12 reused by another process.
    let plan = cleanup_plan(&tracked, |pid| match pid {
        10 => Some(500),
        12 => Some(9999),
        _ => None,
    });
    assert_eq!(plan, vec![10]);
}

#[test]
fn clipboard_env_accepts_the_observed_private_bus_address_with_guid_only() {
    // Native r1 (pointer-live): dbus-daemon printed `unix:path=<runtime>/bus,guid=<hex>`; the exact
    // compare refused the private bus. Would catch: refusing that address, or accepting another
    // socket/path/transport that merely starts like the private one.
    let observed = format!("unix:path={RUNTIME}/bus,guid=b38dbbee0e2b1d0faf0c158c6aaba0e5");
    let env = ClipboardEnv::new(&launch(RUNTIME, WAYLAND), &observed, 1000).unwrap();
    assert_eq!(
        env.paste().var("DBUS_SESSION_BUS_ADDRESS"),
        Some(observed.as_str())
    );
    for bad in [
        format!("unix:path={RUNTIME}/bus2,guid=b38d"),
        format!("unix:path={RUNTIME}/bus,guid="),
        format!("unix:path={RUNTIME}/bus,guid=zz"),
        format!("unix:path={RUNTIME}/bus;unix:path=/run/user/1000/bus"),
        format!("unix:path={RUNTIME}/bus,guid=b38d,path=/run/user/1000/bus"),
        "unix:path=/run/user/1000/bus,guid=b38d".to_owned(),
    ] {
        assert!(
            ClipboardEnv::new(&launch(RUNTIME, WAYLAND), &bad, 1000).is_err(),
            "{bad}"
        );
    }
}
