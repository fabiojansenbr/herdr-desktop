//! Spec 007 — the single native E2E of the composed window (`e2e_fidelity_flow`).
//!
//! Seam: the real product (`herdr_desktop::{configure, install, handler}`, the `tauri.conf.json`
//! window and `src/App.svelte` through `src/features/fidelity/preview.svelte`) driven by
//! `src/features/fidelity/e2e.ts`, with native input on a private headless display. Support
//! code lives in `tests/fidelity-native/`; contracts for continuing it are in
//! `.local/orchestration/native-contract.md`.
//!
//! Status (native-live checkpoint): `e2e_fidelity_flow` runs a private display supervisor
//! (`tests/fidelity-native/{supervisor,live}.rs`: private dbus, headless sway, fcitx5, the composed
//! window and wtype) over the phases with a page scenario, parent driver and evaluator
//! (compose-mount, lazy-editor, native-keys, native-ime). It writes an INCOMPLETE report and fails
//! while any phase is pending, failed or missing; it never passes a partial flow.

#[allow(dead_code)]
#[path = "../../tests/fidelity-native/mod.rs"]
mod support;

#[allow(dead_code)]
#[path = "../../scripts/feature-harness/native.rs"]
mod native_harness;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::support::plan::{self, Actor, PhaseOutcome, PhaseSpec, Readiness, FLOW};
use serde_json::{json, Value};

/// Evaluator of the phases whose parent steps are linked in `live.rs` (paste-selection,
/// mouse-scroll-links); `None` for every other phase.
#[cfg(target_os = "linux")]
fn linked_checks(
    phase: &str,
    report: &Value,
    live: &support::live::LiveRun,
) -> Option<Result<Vec<(&'static str, bool)>, String>> {
    use support::{mouse_flow, paste_flow, view_flow};
    let view = phase == view_flow::RESIZE_PHASE || phase == view_flow::A11Y_PHASE;
    if phase != paste_flow::PHASE && phase != mouse_flow::PHASE && !view {
        return None;
    }
    if report["phase"] != phase {
        return Some(Err(format!("{phase}: report of phase {}", report["phase"])));
    }
    let ledger = &live.pointer_ledger;
    if view {
        let Some(e) = &live.view_expectations else {
            return Some(Err(format!(
                "{phase}: no parent expectations (setup failed)"
            )));
        };
        return Some(if phase == view_flow::RESIZE_PHASE {
            view_flow::resize_checks(phase, report, &live.view_ledger, e)
        } else {
            view_flow::a11y_checks(phase, report, &live.view_ledger, e)
        });
    }
    Some(if phase == paste_flow::PHASE {
        match &live.paste_expectations {
            Some(exp) => paste_flow::checks(phase, report, ledger, exp),
            None => Err(format!("{phase}: no parent expectations (setup failed)")),
        }
    } else {
        match &live.mouse_expectations {
            Some(exp) => mouse_flow::checks(phase, report, ledger, exp),
            None => Err(format!("{phase}: no parent expectations (setup failed)")),
        }
    })
}

/// Parent evaluator of one phase: named checks from the page's raw observations. A phase without
/// an evaluator fails the flow.
fn checks_for(phase: &str, report: &Value, local_session: &str) -> Result<PhaseOutcome, String> {
    if !report["error"].is_null() {
        return Err(format!("{phase}: page error {}", report["error"]));
    }
    if report["phase"] != phase {
        return Err(format!("{phase}: report of phase {}", report["phase"]));
    }
    match phase {
        "compose-mount" => {
            let text = |k: &str| report[k].as_str().unwrap_or("").trim().to_owned();
            Ok(PhaseOutcome::with(&[
                (
                    "app_mounted_without_fake_bridge",
                    report["app_mounted"] == true && report["fake_bridge"] == false,
                ),
                (
                    "local_disposable_session_live",
                    report["status_phase"] == "live"
                        && text("session_text").contains(local_session),
                ),
                (
                    "terminal_active",
                    report["terminal_canvas"] == true && report["terminal_visible"] == true,
                ),
                (
                    "status_has_text_not_only_color",
                    !text("status_text").is_empty(),
                ),
            ]))
        }
        "lazy-editor" => {
            let map = support::chunks::ChunkMap::parse(
                report["chunk_map"]
                    .as_str()
                    .ok_or("lazy-editor: parent must attach the dist chunk map")?,
            )?;
            let snap = |k: &str| support::chunks::Snapshot::from_json(&report["probes"][k]);
            let verdict = support::chunks::lazy_editor_verdict(
                &map,
                &snap("mounted")?,
                &snap("idle")?,
                &snap("opened")?,
            )?;
            Ok(PhaseOutcome::with(&verdict.checks()))
        }
        "native-keys" => {
            let corpus = corpus_fixture();
            let items = report["parent"]["items"]
                .as_array()
                .ok_or("native-keys: parent must attach the PTY measurements")?;
            let exact = |id: &str| measured_equals(&corpus, items, id, "observed_hex");
            Ok(PhaseOutcome::with(&[
                (
                    "trusted_native_events",
                    report["trusted_events"].as_u64().is_some_and(|n| n > 0)
                        && report["untrusted_events"] == 0,
                ),
                (
                    "accents_bytes_once",
                    exact("accents-keysym") && exact("deadkey-gtk-compose"),
                ),
                ("emoji_bytes_once", exact("emoji-keysym")),
                (
                    "return_is_cr",
                    exact("return-is-cr")
                        && item_hex(items, "return-is-cr", "observed_hex") == Some("0d"),
                ),
            ]))
        }
        "native-ime" => {
            let items = report["parent"]["items"]
                .as_array()
                .ok_or("native-ime: parent must attach the PTY measurements")?;
            let events = report["commit_events"]
                .as_array()
                .ok_or("native-ime: page must report its commit events")?;
            let ids = support::corpus::IME_ITEMS;
            Ok(PhaseOutcome::with(&[
                (
                    "preedit_zero_pty_bytes",
                    ids.iter()
                        .all(|id| item_hex(items, id, "preedit_hex") == Some("")),
                ),
                (
                    "cjk_candidates_commit_once",
                    support::corpus::native_ime_variant(items, events).is_ok(),
                ),
                (
                    "keyboard_normal_after_ime",
                    report["parent"]["after_ime"]["observed_hex"] == "6f6b",
                ),
            ]))
        }
        other => Err(format!("{other}: no parent evaluator yet")),
    }
}

fn corpus_fixture() -> support::corpus::Corpus {
    support::corpus::parse(&read(
        &repo().join("tests/fidelity-native/fixtures/corpus.json"),
    ))
    .expect("corpus fixture")
}

fn item_hex<'a>(items: &'a [Value], id: &str, key: &str) -> Option<&'a str> {
    items.iter().find(|i| i["id"] == id)?[key].as_str()
}

/// The bytes measured at the PTY for corpus item `id` equal its commits exactly (once, nothing
/// else), with the expectation taken from the corpus fixture, never from the report.
fn measured_equals(corpus: &support::corpus::Corpus, items: &[Value], id: &str, key: &str) -> bool {
    let Some(spec) = corpus.items.iter().find(|i| i.id == id) else {
        return false;
    };
    let expected: String = spec.commits.iter().map(|c| c.pty_hex.as_str()).collect();
    !expected.is_empty() && item_hex(items, id, key) == Some(expected.as_str())
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

// ---------------------------------------------------------------------------------------
// Plan and evaluator
// ---------------------------------------------------------------------------------------

mod flow_plan {
    use super::*;

    // Would catch: a phase marked Prepared that the parent never drives (reported missing as if
    // pending work were done), a driven phase still Pending, or SSH phases whose plan checks
    // drifted from the evaluator in ssh_flow (a check nobody computes).
    #[test]
    fn driven_phases_are_exactly_the_prepared_ones_and_ssh_checks_match() {
        let prepared: Vec<&str> = FLOW
            .iter()
            .filter(|p| matches!(p.readiness, Readiness::Prepared))
            .map(|p| p.name)
            .collect();
        assert_eq!(prepared, DRIVEN_PHASES.to_vec());
        for (name, checks) in support::ssh_flow::PHASES {
            let spec = FLOW.iter().find(|p| p.name == name).expect(name);
            assert_eq!(spec.checks, checks, "{name}");
            assert!(DRIVEN_PHASES.contains(&name), "{name}");
        }
    }

    fn prepared(name: &'static str, checks: &'static [&'static str]) -> PhaseSpec {
        PhaseSpec {
            name,
            acs: &["AC-007-02", "AC-007-03", "AC-007-04"],
            actor: Actor::Window,
            readiness: Readiness::Prepared,
            checks,
        }
    }

    #[test]
    fn the_flow_plan_is_well_formed_and_covers_the_e2e_acs() {
        assert_eq!(plan::plan_problems(FLOW), Vec::<String>::new());
        assert!(FLOW.iter().all(|p| !p.acs.contains(&"AC-007-01")));
    }

    #[test]
    fn plan_problems_catch_duplicates_bad_names_and_uncovered_acs() {
        let bad = [
            PhaseSpec {
                name: "Bad_Name",
                acs: &["AC-007-02"],
                ..prepared("x", &["a"])
            },
            PhaseSpec {
                acs: &["AC-007-02"],
                ..prepared("dup", &["a", "a"])
            },
            PhaseSpec {
                acs: &["AC-007-02"],
                ..prepared("dup", &["b"])
            },
        ];
        let problems = plan::plan_problems(&bad);
        for expected in [
            "Bad_Name: invalid phase name",
            "dup: duplicate phase",
            "dup: duplicate check a",
            "AC-007-03: no phase",
            "AC-007-04: no phase",
        ] {
            assert!(
                problems.iter().any(|p| p == expected),
                "{expected} in {problems:?}"
            );
        }
    }

    #[test]
    fn a_pending_phase_never_passes_even_with_every_check_true() {
        let pending = [PhaseSpec {
            readiness: Readiness::Pending("seam"),
            ..prepared("native-ime", &["preedit_zero_pty_bytes"])
        }];
        let outcomes = BTreeMap::from([(
            "native-ime".to_owned(),
            PhaseOutcome::with(&[("preedit_zero_pty_bytes", true)]),
        )]);
        let verdict = plan::evaluate(&pending, &outcomes);
        assert!(verdict.passed.is_empty());
        assert_eq!(
            verdict.pending.get("native-ime").map(String::as_str),
            Some("seam")
        );
        assert!(!verdict.is_pass(&pending));
    }

    #[test]
    fn a_prepared_phase_passes_only_with_exactly_its_checks_true() {
        let flow = [prepared("p", &["a", "b"])];
        let run = |checks: &[(&str, bool)]| {
            plan::evaluate(
                &flow,
                &BTreeMap::from([("p".to_owned(), PhaseOutcome::with(checks))]),
            )
        };
        assert!(run(&[("a", true), ("b", true)]).is_pass(&flow));
        assert_eq!(
            run(&[("a", true), ("b", false)]).failed["p"],
            vec!["b: false"]
        );
        assert_eq!(run(&[("a", true)]).failed["p"], vec!["b: missing"]);
        assert_eq!(
            run(&[("a", true), ("b", true), ("c", true)]).failed["p"],
            vec!["c: undeclared"]
        );
    }

    #[test]
    fn missing_and_unknown_outcomes_fail_the_flow() {
        let flow = [prepared("p", &["a"])];
        let verdict = plan::evaluate(
            &flow,
            &BTreeMap::from([("q".to_owned(), PhaseOutcome::with(&[("a", true)]))]),
        );
        assert_eq!(verdict.missing, vec!["p"]);
        assert_eq!(verdict.unknown, vec!["q"]);
        assert!(!verdict.is_pass(&flow));
    }

    #[test]
    fn the_page_scenario_lists_exactly_the_plan_phases_in_order() {
        let source = read(&repo().join("src/features/fidelity/e2e.ts"));
        let start = source.find("phases:begin").expect("phases:begin marker");
        let end = source.find("phases:end").expect("phases:end marker");
        let names: Vec<&str> = source[start..end].split('"').skip(1).step_by(2).collect();
        let plan: Vec<&str> = FLOW.iter().map(|p| p.name).collect();
        assert_eq!(names, plan);
    }

    #[test]
    fn a_phase_without_parent_evaluator_or_with_page_error_is_an_error() {
        let ok = json!({ "phase": "resize-dpi", "error": null });
        assert!(checks_for("resize-dpi", &ok, "hd007-a")
            .unwrap_err()
            .contains("no parent evaluator"));
        let failed = json!({ "phase": "compose-mount", "error": "boom" });
        assert!(checks_for("compose-mount", &failed, "hd007-a").is_err());
        let other = json!({ "phase": "lazy-editor", "error": null });
        assert!(checks_for("compose-mount", &other, "hd007-a").is_err());
    }

    #[test]
    fn compose_mount_checks_distinguish_session_fake_bridge_and_color_only_state() {
        let base = json!({
            "phase": "compose-mount", "error": null, "app_mounted": true, "fake_bridge": false,
            "status_phase": "live", "status_text": "Conectado", "session_text": "sessão hd007-local-1",
            "terminal_canvas": true, "terminal_visible": true,
        });
        let checks = |report: &Value| {
            checks_for("compose-mount", report, "hd007-local-1")
                .unwrap()
                .checks
        };
        assert!(checks(&base).values().all(|v| *v));
        let mut other_session = base.clone();
        other_session["session_text"] = json!("sessão hd007-ssh-1");
        assert!(!checks(&other_session)["local_disposable_session_live"]);
        let mut fake = base.clone();
        fake["fake_bridge"] = json!(true);
        assert!(!checks(&fake)["app_mounted_without_fake_bridge"]);
        let mut color_only = base.clone();
        color_only["status_text"] = json!("  ");
        assert!(!checks(&color_only)["status_has_text_not_only_color"]);
        let mut hidden = base;
        hidden["terminal_visible"] = json!(false);
        assert!(!checks(&hidden)["terminal_active"]);
    }
}

mod native_input_checks {
    use super::*;

    fn keys_report() -> Value {
        json!({
            "phase": "native-keys", "error": null, "trusted_events": 42, "untrusted_events": 0,
            "parent": { "items": [
                { "id": "accents-keysym", "observed_hex": "61c3a7c3a36f20c3a920c3bc" },
                { "id": "deadkey-gtk-compose", "observed_hex": "c3a9c3a3" },
                { "id": "return-is-cr", "observed_hex": "0d" },
                { "id": "emoji-keysym", "observed_hex": "f09f9982f09f918df09f8fbd" },
            ]}
        })
    }

    fn failing(report: &Value) -> Vec<String> {
        let phase = report["phase"].as_str().unwrap();
        checks_for(phase, report, "hd007-a")
            .unwrap()
            .checks
            .into_iter()
            .filter(|(_, ok)| !ok)
            .map(|(k, _)| k)
            .collect()
    }

    #[test]
    fn native_keys_pass_only_with_exact_corpus_bytes_once_and_trusted_events() {
        assert_eq!(failing(&keys_report()), Vec::<String>::new());
        let mut doubled = keys_report();
        doubled["parent"]["items"][3]["observed_hex"] =
            json!("f09f9982f09f918df09f8fbdf09f9982f09f918df09f8fbd");
        assert_eq!(failing(&doubled), vec!["emoji_bytes_once"]);
        let mut lf = keys_report();
        lf["parent"]["items"][2]["observed_hex"] = json!("0a");
        assert_eq!(failing(&lf), vec!["return_is_cr"]);
        let mut dead_once = keys_report();
        dead_once["parent"]["items"][1]["observed_hex"] = json!("c3a9");
        assert_eq!(failing(&dead_once), vec!["accents_bytes_once"]);
        let mut synthetic = keys_report();
        synthetic["untrusted_events"] = json!(1);
        assert_eq!(failing(&synthetic), vec!["trusted_native_events"]);
        let mut report_expectation = keys_report();
        report_expectation["parent"]["items"][0]["expected_hex"] = json!("");
        report_expectation["parent"]["items"][0]["observed_hex"] = json!("");
        assert_eq!(failing(&report_expectation), vec!["accents_bytes_once"]);
        let no_parent = json!({ "phase": "native-keys", "error": null });
        assert!(checks_for("native-keys", &no_parent, "hd007-a").is_err());
    }

    const IDS: [&str; 4] = [
        "ime-pinyin-nihao",
        "ime-pinyin-candidate-1",
        "ime-pinyin-candidate-2",
        "ime-pinyin-zhongwen",
    ];

    /// Native IME report: `events[i]` is the trusted commit seen in item i's window, `pty[i]` the
    /// text whose UTF-8 bytes the PTY received for item i (windows 1 s apart, epoch-like ms).
    fn ime_with(events: [&str; 4], pty: [&str; 4]) -> Value {
        let items: Vec<Value> = (0..4)
            .map(|i| {
                let base = 1_000_000.0 + 1000.0 * i as f64;
                json!({
                    "id": IDS[i], "preedit_hex": "",
                    "preedit_window_ms": [base + 100.0, base + 400.0],
                    "commit_window_ms": [base + 500.0, base + 900.0],
                    "commit_hex": support::corpus::hex(pty[i].as_bytes()),
                })
            })
            .collect();
        let commit_events: Vec<Value> = (0..4)
            .map(|i| json!({ "data": events[i], "trusted": true, "t": 1_000_700.0 + 1000.0 * i as f64 }))
            .collect();
        json!({
            "phase": "native-ime", "error": null, "commit_events": commit_events,
            "parent": { "items": items, "after_ime": { "observed_hex": "6f6b" } },
        })
    }

    const RUN4: [&str; 4] = ["你好", "品饮", "拼音", "中文"];
    const RUN5: [&str; 4] = ["你好", "拼音", "品饮", "中文"];

    fn ime_report() -> Value {
        ime_with(RUN4, RUN4)
    }

    #[test]
    fn native_ime_distinguishes_preedit_bytes_wrong_candidate_double_commit_and_stuck_ime() {
        assert_eq!(failing(&ime_report()), Vec::<String>::new());
        let mut preedit = ime_report();
        preedit["parent"]["items"][0]["preedit_hex"] = json!("6e6968616f");
        assert_eq!(failing(&preedit), vec!["preedit_zero_pty_bytes"]);
        let mut same_candidate = ime_report();
        same_candidate["parent"]["items"][2]["commit_hex"] = json!("e59381e9a5ae");
        assert_eq!(failing(&same_candidate), vec!["cjk_candidates_commit_once"]);
        let mut twice = ime_report();
        twice["parent"]["items"][3]["commit_hex"] = json!("e4b8ade69687e4b8ade69687");
        assert_eq!(failing(&twice), vec!["cjk_candidates_commit_once"]);
        let mut missing = ime_report();
        missing["parent"]["items"].as_array_mut().unwrap().remove(1);
        assert_eq!(
            failing(&missing),
            vec!["cjk_candidates_commit_once", "preedit_zero_pty_bytes"]
        );
        let mut stuck = ime_report();
        stuck["parent"]["after_ime"]["observed_hex"] = json!("");
        assert_eq!(failing(&stuck), vec!["keyboard_normal_after_ime"]);
    }

    fn variant(report: &Value) -> Result<usize, String> {
        support::corpus::native_ime_variant(
            report["parent"]["items"].as_array().unwrap(),
            report["commit_events"].as_array().unwrap(),
        )
    }

    #[test]
    fn both_native_candidate_orders_pass_and_are_identified() {
        assert_eq!(variant(&ime_with(RUN4, RUN4)), Ok(0));
        assert_eq!(variant(&ime_with(RUN5, RUN5)), Ok(1));
        assert_eq!(failing(&ime_with(RUN5, RUN5)), Vec::<String>::new());
    }

    #[test]
    fn native_variants_are_corpus_commits_with_the_historical_items_unchanged() {
        let corpus = corpus_fixture();
        let committed: Vec<String> = support::corpus::IME_ITEMS
            .iter()
            .map(|id| {
                corpus.items.iter().find(|i| i.id == *id).unwrap().commits[0]
                    .text
                    .clone()
            })
            .collect();
        assert_eq!(committed, ["你好", "品饮", "拼音", "中文"]);
        for v in support::corpus::NATIVE_IME_VARIANTS {
            let mut sorted = v.to_vec();
            sorted.sort();
            let mut expected = committed.clone();
            expected.sort();
            assert_eq!(sorted, expected);
            assert_eq!((v[0], v[3]), ("你好", "中文"));
            assert_ne!(v[1], v[2]);
        }
    }

    #[test]
    fn native_ime_rejects_divergent_foreign_repeated_reordered_or_untrusted_commits() {
        let cjk_fails = |report: &Value| failing(report) == vec!["cjk_candidates_commit_once"];
        // PTY bytes differ from the trusted commit event of the same item.
        assert!(cjk_fails(&ime_with(RUN4, RUN5)));
        // Word outside the corpus, consistent on both sides.
        assert!(cjk_fails(&ime_with(
            ["你好", "拼", "品饮", "中文"],
            ["你好", "拼", "品饮", "中文"]
        )));
        // Same candidate twice.
        let same = ["你好", "品饮", "品饮", "中文"];
        assert!(cjk_fails(&ime_with(same, same)));
        // Bytes of the commit twice at the PTY.
        let mut doubled = ime_with(RUN5, RUN5);
        doubled["parent"]["items"][1]["commit_hex"] = json!("e68bbce99fb3e68bbce99fb3");
        assert!(cjk_fails(&doubled));
        // First/last items are fixed: any other order of the four words fails.
        let reordered = ["中文", "品饮", "拼音", "你好"];
        assert!(cjk_fails(&ime_with(reordered, reordered)));
        // Untrusted commit event.
        let mut untrusted = ime_report();
        untrusted["commit_events"][2]["trusted"] = json!(false);
        assert!(cjk_fails(&untrusted));
        // A second commit event in one window (duplicate end) and a commit during preedit.
        let mut duplicate = ime_report();
        duplicate["commit_events"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "data": "品饮", "trusted": true, "t": 1_001_800.0 }));
        assert!(cjk_fails(&duplicate));
        let mut early = ime_report();
        early["commit_events"][1]["t"] = json!(1_001_200.0);
        assert!(cjk_fails(&early));
        // Page did not report commit events at all.
        let mut silent = ime_report();
        silent.as_object_mut().unwrap().remove("commit_events");
        assert!(checks_for("native-ime", &silent, "hd007-a").is_err());
        // Non-zero preedit still fails its own check under the native variants.
        let mut preedit = ime_with(RUN5, RUN5);
        preedit["parent"]["items"][2]["preedit_hex"] = json!("70696e79696e");
        assert_eq!(failing(&preedit), vec!["preedit_zero_pty_bytes"]);
    }
}

#[cfg(target_os = "linux")]
mod supervisor_helpers {
    use crate::support::supervisor::{environ_in_runtime, parse_starttime, same_process, Tracked};
    use crate::support::window::{route, valid_step, Route};

    #[test]
    fn starttime_survives_spaces_in_comm_and_pid_reuse_is_not_ours() {
        let stat = "4242 (Web Content x) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 987654 20";
        assert_eq!(parse_starttime(stat), Some(987654));
        assert_eq!(parse_starttime("garbage"), None);
        let tracked = Tracked {
            name: "sway".into(),
            pid: 4242,
            starttime: 987654,
        };
        assert!(same_process(&tracked, Some(987654)));
        assert!(!same_process(&tracked, Some(987655)));
        assert!(!same_process(&tracked, None));
    }

    #[test]
    fn leftovers_are_matched_by_the_exact_private_runtime_only() {
        let rt = std::path::Path::new("/tmp/hd7L-1");
        assert!(environ_in_runtime(
            b"A=1\0XDG_RUNTIME_DIR=/tmp/hd7L-1\0",
            rt
        ));
        assert!(!environ_in_runtime(b"XDG_RUNTIME_DIR=/tmp/hd7L-10\0", rt));
        assert!(!environ_in_runtime(b"XDG_RUNTIME_DIR=/run/user/1000\0", rt));
    }

    // Spec 009 round 3: the hidden half of the output cells needs the window itself to stop being
    // visible, which is what `src/shell/interest.ts` reacts to. The product grants the WebView only
    // close/minimize/toggle-maximize/start-dragging (`src-tauri/capabilities/default.json`), so the
    // hide/show is a harness command in this binary, never a product capability.
    #[test]
    fn the_window_visibility_helper_is_harness_routed_and_is_not_a_product_command() {
        use crate::support::window::HARNESS_COMMANDS;
        assert_eq!(route("harness_window_visible"), Route::Harness);
        assert!(HARNESS_COMMANDS.contains(&"harness_window_visible"));
        let product: Vec<&str> = herdr_desktop::command_registry()
            .into_iter()
            .flat_map(|(_, commands)| commands.iter().copied())
            .collect();
        assert!(!product.contains(&"harness_window_visible"));
    }

    #[test]
    fn the_step_helper_is_harness_routed_and_rejects_path_like_steps() {
        assert_eq!(route("harness_await"), Route::Harness);
        assert!(valid_step("native-ime"));
        for bad in ["", "../x", "a/b", "Native", "x.json", &"a".repeat(65)] {
            assert!(!valid_step(bad), "{bad:?}");
        }
    }
}

// ---------------------------------------------------------------------------------------
// Lazy editor probe
// ---------------------------------------------------------------------------------------

mod lazy_editor {
    use crate::support::chunks::{lazy_editor_verdict, ChunkMap, Snapshot};

    const MAP: &str = r#"{"version":1,"chunks":{
        "assets/index.js":["src/main.ts","src/App.svelte","node_modules/svelte/src/index.js"],
        "assets/editor.js":["src/editor/CodeEditor.svelte","node_modules/@codemirror/view/dist/index.js"],
        "assets/lang.js":["node_modules/@lezer/rust/dist/index.js"]}}"#;

    fn snap(at_ms: f64, scripts: &[&str], editor_dom: bool) -> Snapshot {
        Snapshot {
            at_ms,
            scripts: scripts.iter().map(|s| (*s).to_owned()).collect(),
            editor_dom,
        }
    }

    fn checks(mounted: Snapshot, idle: Snapshot, opened: Snapshot) -> Vec<(&'static str, bool)> {
        let map = ChunkMap::parse(MAP).unwrap();
        lazy_editor_verdict(&map, &mounted, &idle, &opened)
            .unwrap()
            .checks()
    }

    fn failing(checks: &[(&'static str, bool)]) -> Vec<&'static str> {
        checks
            .iter()
            .filter(|(_, ok)| !ok)
            .map(|(k, _)| *k)
            .collect()
    }

    #[test]
    fn idle_composed_window_without_editor_then_detected_after_open_passes() {
        let result = checks(
            snap(100.0, &["/assets/index.js"], false),
            snap(2100.0, &["/assets/index.js"], false),
            snap(2500.0, &["/assets/index.js", "/assets/editor.js"], true),
        );
        assert_eq!(failing(&result), Vec::<&str>::new());
    }

    #[test]
    fn an_editor_or_language_chunk_before_opening_fails_the_idle_check() {
        let result = checks(
            snap(100.0, &["/assets/index.js"], false),
            snap(2200.0, &["/assets/index.js", "/assets/lang.js"], false),
            snap(2500.0, &["/assets/index.js", "/assets/editor.js"], true),
        );
        assert_eq!(failing(&result), vec!["zero_editor_modules_idle"]);
    }

    #[test]
    fn an_idle_window_shorter_than_2000_ms_fails() {
        let result = checks(
            snap(100.0, &["/assets/index.js"], false),
            snap(2099.0, &["/assets/index.js"], false),
            snap(2500.0, &["/assets/index.js", "/assets/editor.js"], true),
        );
        assert_eq!(failing(&result), vec!["idle_window_at_least_2000_ms"]);
    }

    #[test]
    fn a_dead_probe_that_never_sees_the_editor_chunk_fails_the_positive_control() {
        let result = checks(
            snap(100.0, &["/assets/index.js"], false),
            snap(2200.0, &["/assets/index.js"], false),
            snap(2500.0, &["/assets/index.js"], true),
        );
        assert_eq!(failing(&result), vec!["editor_modules_detected_after_open"]);
    }

    #[test]
    fn unknown_scripts_stale_maps_and_structural_errors_are_errors_not_zero() {
        let map = ChunkMap::parse(MAP).unwrap();
        let ok = snap(2500.0, &["/assets/index.js", "/assets/editor.js"], true);
        let foreign = snap(100.0, &["/assets/index-OLD.js"], false);
        let idle = snap(2200.0, &["/assets/index.js"], false);
        assert!(lazy_editor_verdict(&map, &foreign, &idle, &ok)
            .unwrap_err()
            .contains("not a chunk"));
        let empty = snap(100.0, &[], false);
        assert!(lazy_editor_verdict(&map, &empty, &idle, &ok).is_err());
        let dom_early = snap(100.0, &["/assets/index.js"], true);
        assert!(lazy_editor_verdict(&map, &dom_early, &idle, &ok).is_err());
        let backwards = snap(3000.0, &["/assets/index.js"], false);
        assert!(lazy_editor_verdict(&map, &backwards, &idle, &ok).is_err());
        assert!(ChunkMap::parse(r#"{"version":2,"chunks":{"a":[]}}"#).is_err());
        assert!(ChunkMap::parse(r#"{"version":1,"chunks":{}}"#).is_err());
    }
}

// ---------------------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------------------

mod sessions {
    use std::path::PathBuf;

    use crate::support::session::{disposable, may_stop, pair_problems, HostFixture};

    fn host(session: &str, pane: &str, root: &str, boot: &str) -> HostFixture {
        HostFixture {
            session: session.into(),
            pane_id: pane.into(),
            root: PathBuf::from(root),
            boot_id: boot.into(),
        }
    }

    #[test]
    fn only_hd007_sessions_are_disposable_and_only_created_ones_may_stop() {
        assert!(disposable("hd007-1789-42").is_ok());
        for refused in ["default", "hd004-1789-42", "hd007-", "work", "hd007-a/b"] {
            assert!(disposable(refused).is_err(), "{refused}");
        }
        let created = vec!["hd007-1".to_owned()];
        assert!(may_stop("hd007-1", &created));
        assert!(!may_stop("hd007-2", &created));
        assert!(!may_stop("default", &["default".to_owned()]));
    }

    #[test]
    fn the_host_pair_must_share_w1_p1_with_distinct_sessions_roots_and_boots() {
        let local = host("hd007-l", "w1:p1", "/var/tmp/hd007/local", "boot-local");
        let ssh = host("hd007-s", "w1:p1", "/var/tmp/hd007/remote", "boot-ssh");
        assert_eq!(pair_problems(&local, &ssh), Vec::<String>::new());
        let other_pane = host("hd007-s", "w1:p2", "/var/tmp/hd007/remote", "boot-ssh");
        assert!(pair_problems(&local, &other_pane)
            .iter()
            .any(|p| p.contains("is not w1:p1")));
        let nested = host("hd007-s", "w1:p1", "/var/tmp/hd007/local/sub", "boot-ssh");
        assert!(pair_problems(&local, &nested)
            .contains(&"project roots are equal or nested".to_owned()));
        let same_boot = host("hd007-s", "w1:p1", "/var/tmp/hd007/remote", "boot-local");
        assert!(
            pair_problems(&local, &same_boot).contains(&"hosts share the same boot id".to_owned())
        );
        let same_session = host("hd007-l", "w1:p1", "/var/tmp/hd007/remote", "boot-ssh");
        assert!(pair_problems(&local, &same_session)
            .contains(&"hosts share the same session".to_owned()));
    }
}

// ---------------------------------------------------------------------------------------
// Private display
// ---------------------------------------------------------------------------------------

mod private_display {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use crate::support::display::{
        guard_window_env, validate_runtime_dir, PrivateDisplay, Resources,
    };

    const UID: u32 = 1000;

    fn display() -> PrivateDisplay {
        PrivateDisplay::new(
            PathBuf::from("/tmp/hd007-rt"),
            PathBuf::from("/var/tmp/hd007-run/home"),
            Resources::under(Path::new("/opt/prepared/.local")),
            "tester".into(),
            UID,
        )
        .unwrap()
    }

    #[test]
    fn a_launched_process_sees_exactly_the_private_environment() {
        let display = display();
        let socket = Path::new("/tmp/hd007-rt/wayland-1");
        let mut launch = display
            .window(Path::new("/usr/bin/env"), socket, true, &[])
            .unwrap();
        launch.program = PathBuf::from("/usr/bin/env");
        let output = launch.command().output().expect("run env");
        assert!(output.status.success());
        let seen: BTreeMap<String, String> = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        let expected: BTreeMap<String, String> = launch.env.iter().cloned().collect();
        assert_eq!(seen, expected);
        assert_eq!(seen["WAYLAND_DISPLAY"], "/tmp/hd007-rt/wayland-1");
        assert_eq!(seen["GTK_IM_MODULE"], "wayland");
        assert!(!seen.contains_key("DISPLAY"));
        assert!(!seen.contains_key("DBUS_SESSION_BUS_ADDRESS"));
    }

    #[test]
    fn window_ime_module_is_opt_in_and_extras_cannot_override_the_display() {
        let display = display();
        let socket = Path::new("/tmp/hd007-rt/wayland-1");
        let plain = display.window(Path::new("/x"), socket, false, &[]).unwrap();
        assert_eq!(plain.var("GTK_IM_MODULE"), None);
        assert!(display
            .window(
                Path::new("/x"),
                socket,
                false,
                &[("WAYLAND_DISPLAY", "wayland-0")]
            )
            .is_err());
        let with_session = display
            .window(
                Path::new("/x"),
                socket,
                false,
                &[("HERDR_DESKTOP_E2E_SESSION", "hd007-1")],
            )
            .unwrap();
        assert_eq!(
            with_session.var("HERDR_DESKTOP_E2E_SESSION"),
            Some("hd007-1")
        );
    }

    #[test]
    fn the_user_display_runtime_and_relative_sockets_are_refused() {
        let display = display();
        for socket in [
            "wayland-1",
            "/run/user/1000/wayland-1",
            "/tmp/hd007-rt/other",
        ] {
            assert!(
                display
                    .window(Path::new("/x"), Path::new(socket), false, &[])
                    .is_err(),
                "{socket}"
            );
            assert!(display.wtype(Path::new(socket), &[]).is_err(), "{socket}");
        }
        assert!(validate_runtime_dir(Path::new("/run/user/1000/hd007"), UID).is_err());
        assert!(validate_runtime_dir(Path::new("relative"), UID).is_err());
        let long = format!("/tmp/{}", "x".repeat(100));
        assert!(validate_runtime_dir(Path::new(&long), UID).is_err());
        assert!(PrivateDisplay::new(
            PathBuf::from("/tmp/hd007-rt"),
            PathBuf::from("/run/user/1000/home"),
            Resources::under(Path::new("/opt")),
            "tester".into(),
            UID
        )
        .is_err());
    }

    #[test]
    fn wtype_waits_for_the_keyboard_before_the_first_key() {
        let launch = display()
            .wtype(
                Path::new("/tmp/hd007-rt/wayland-1"),
                &["-k".into(), "Return".into()],
            )
            .unwrap();
        assert_eq!(launch.args, vec!["-s", "250", "-k", "Return"]);
        assert_eq!(
            launch.var("WAYLAND_DISPLAY"),
            Some("/tmp/hd007-rt/wayland-1")
        );
    }

    #[test]
    fn sway_is_headless_pixman_from_the_prepared_prefix_and_fcitx5_loads_pinyin() {
        let display = display();
        let sway = display.sway();
        assert_eq!(
            sway.program,
            Path::new("/opt/prepared/.local/native-input/prefix/usr/bin/sway")
        );
        assert_eq!(sway.var("WLR_BACKENDS"), Some("headless"));
        assert_eq!(sway.var("WLR_RENDERER"), Some("pixman"));
        assert_eq!(sway.var("WAYLAND_DISPLAY"), None);
        let fcitx5 = display.fcitx5();
        assert!(fcitx5
            .var("FCITX_ADDON_DIRS")
            .unwrap()
            .starts_with("/opt/prepared/.local/prep-cjk/prefix/usr/lib/fcitx5:"));
        let bus = display.with_private_bus(&sway);
        assert_eq!(bus.program, Path::new("/usr/bin/dbus-run-session"));
        assert_eq!(bus.args[0], "--");
        assert_eq!(bus.env, sway.env);
    }

    #[test]
    fn the_window_process_refuses_anything_but_the_flow_private_display() {
        let ok: BTreeMap<&str, &str> = BTreeMap::from([
            ("XDG_RUNTIME_DIR", "/tmp/hd007-rt"),
            ("HERDR_DESKTOP_E2E_RUNTIME", "/tmp/hd007-rt"),
            ("WAYLAND_DISPLAY", "/tmp/hd007-rt/wayland-1"),
            ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/tmp/dbus-abc"),
        ]);
        let check = |env: &BTreeMap<&str, &str>| {
            guard_window_env(&|k| env.get(k).map(|v| (*v).to_owned()), UID)
        };
        assert!(check(&ok).is_ok());
        let cases: [(&str, Option<&str>); 5] = [
            ("DISPLAY", Some(":0")),
            ("WAYLAND_DISPLAY", Some("wayland-1")),
            ("HERDR_DESKTOP_E2E_RUNTIME", Some("/tmp/other")),
            (
                "DBUS_SESSION_BUS_ADDRESS",
                Some("unix:path=/run/user/1000/bus"),
            ),
            ("HYPRLAND_INSTANCE_SIGNATURE", Some("abc")),
        ];
        for (key, value) in cases {
            let mut env = ok.clone();
            if let Some(value) = value {
                env.insert(key, value);
            }
            assert!(check(&env).is_err(), "{key}={value:?}");
        }
        let mut no_runtime = ok.clone();
        no_runtime.remove("XDG_RUNTIME_DIR");
        assert!(check(&no_runtime).is_err());
    }
}

// ---------------------------------------------------------------------------------------
// Input corpus fixture
// ---------------------------------------------------------------------------------------

mod input_corpus {
    use super::{read, repo};
    use crate::support::corpus::{parse, pinyin_standalone_hex, problems};

    fn corpus_raw() -> String {
        read(&repo().join("tests/fidelity-native/fixtures/corpus.json"))
    }

    #[test]
    fn the_corpus_fixture_is_consistent_and_covers_every_category() {
        let corpus = parse(&corpus_raw()).unwrap();
        assert_eq!(problems(&corpus), Vec::<String>::new());
    }

    #[test]
    fn pinyin_commits_reproduce_the_bytes_observed_by_the_standalone_probe() {
        let corpus = parse(&corpus_raw()).unwrap();
        assert_eq!(
            pinyin_standalone_hex(&corpus),
            corpus.cjk_standalone_final_hex
        );
        assert_eq!(
            corpus.cjk_standalone_final_hex,
            "e4bda0e5a5bd0ae59381e9a5ae0ae68bbce99fb30ae4b8ade69687"
        );
    }

    #[test]
    fn tampered_bytes_unreferenced_observations_and_preedit_bytes_are_problems() {
        let mut corpus = parse(&corpus_raw()).unwrap();
        let item = |c: &mut crate::support::corpus::Corpus, id: &str| {
            c.items.iter().position(|i| i.id == id).unwrap()
        };
        let accents = item(&mut corpus, "accents-keysym");
        corpus.items[accents].commits[0].pty_hex = "61".into();
        corpus.items[accents].evidence = None;
        let ime = item(&mut corpus, "ime-pinyin-nihao");
        corpus.items[ime].preedit_pty_bytes = Some(5);
        let emoji = item(&mut corpus, "emoji-keysym");
        corpus.items.remove(emoji);
        let found = problems(&corpus);
        for expected in [
            "accents-keysym: pty_hex does not encode \"ação é ü\"",
            "accents-keysym: observed without a preflight reference",
            "ime-pinyin-nihao: IME item must expect zero preedit bytes",
            "category emoji has no item",
        ] {
            assert!(
                found.iter().any(|p| p == expected),
                "{expected} in {found:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// Composed window seam
// ---------------------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod window_seam {
    use std::collections::BTreeMap;

    use crate::support::window::{
        desktop_config, route, selection_script, Route, HARNESS_COMMANDS,
    };
    use serde_json::{json, Value};

    #[test]
    fn only_the_harness_report_bypasses_the_product_handler() {
        assert_eq!(route("harness_report"), Route::Harness);
        let product: Vec<&str> = herdr_desktop::command_registry()
            .into_iter()
            .flat_map(|(_, commands)| commands.iter().copied())
            .collect();
        assert!(product.len() > 50, "composed registry: {product:?}");
        for command in &product {
            assert_eq!(route(command), Route::Product, "{command}");
        }
        for unknown in ["Harness_report", "harness_report ", "shell_exec", ""] {
            assert_eq!(route(unknown), Route::Product, "{unknown:?}");
        }
        for harness in HARNESS_COMMANDS {
            assert!(
                !product.contains(&harness),
                "product exposes harness command {harness}"
            );
        }
        assert!(product.iter().all(|c| !c.starts_with("harness")));
    }

    #[test]
    fn the_injected_selection_is_the_fidelity_feature_with_escaped_params() {
        let params = json!({ "session": "hd007-1", "text": "</script>\"'\n" });
        let script = selection_script("compose-mount", &params);
        let body = script
            .strip_prefix("window.__HERDR_HARNESS__ = Object.freeze(")
            .and_then(|s| s.strip_suffix(");"))
            .expect("script shape");
        let value: Value = serde_json::from_str(body).unwrap();
        assert_eq!(
            value,
            json!({ "feature": "fidelity", "phase": "compose-mount", "params": params })
        );
    }

    fn env() -> BTreeMap<&'static str, &'static str> {
        BTreeMap::from([
            ("HERDR_DESKTOP_E2E_SESSION", "hd007-1789-1"),
            (
                "HERDR_DESKTOP_E2E_HERDR_CONFIG_DIR",
                "/home/u/.config/herdr",
            ),
            ("HERDR_DESKTOP_E2E_PREFS_DIR", "/var/tmp/hd007-1789-1/prefs"),
            ("HERDR_DESKTOP_E2E_STATE_DIR", "/var/tmp/hd007-1789-1/state"),
            ("HERDR_DESKTOP_HERDR_BIN", "/opt/bin/herdr-03749ae"),
        ])
    }

    fn config(env: &BTreeMap<&str, &str>) -> Result<herdr_desktop::DesktopConfig, String> {
        desktop_config(&|k| env.get(k).map(|v| (*v).to_owned()))
    }

    #[test]
    fn the_window_config_comes_only_from_explicit_private_variables() {
        let config = config(&env()).unwrap();
        assert_eq!(
            config.bootstrap.session.as_ref().unwrap().as_str(),
            "hd007-1789-1"
        );
        assert_eq!(
            config.bootstrap.config_dir,
            std::path::Path::new("/home/u/.config/herdr")
        );
        assert_eq!(
            config.prefs_dir.as_deref(),
            Some(std::path::Path::new("/var/tmp/hd007-1789-1/prefs"))
        );
        assert_eq!(
            config.herdr_state_dir,
            std::path::Path::new("/var/tmp/hd007-1789-1/state")
        );
        assert!(config.isolated_ssh.is_none());
        assert!(config.bootstrap.surface_trace.is_none());
    }

    #[test]
    fn the_window_config_refuses_defaults_foreign_sessions_and_overlapping_dirs() {
        let mutate = |key: &'static str, value: Option<&'static str>| {
            let mut env = env();
            match value {
                Some(v) => env.insert(key, v),
                None => env.remove(key),
            };
            config(&env)
        };
        assert!(mutate("HERDR_DESKTOP_E2E_SESSION", None).is_err());
        assert!(mutate("HERDR_DESKTOP_E2E_SESSION", Some("default")).is_err());
        assert!(mutate("HERDR_DESKTOP_E2E_SESSION", Some("hd005-1")).is_err());
        assert!(mutate("HERDR_DESKTOP_E2E_PREFS_DIR", Some("prefs")).is_err());
        assert!(mutate(
            "HERDR_DESKTOP_E2E_PREFS_DIR",
            Some("/home/u/.config/herdr/desktop")
        )
        .is_err());
        assert!(mutate("HERDR_DESKTOP_E2E_STATE_DIR", Some("/home/u/.config")).is_err());
        assert!(mutate("HERDR_DESKTOP_HERDR_BIN", None).is_err());
        assert!(mutate("HERDR_DESKTOP_E2E_SSH_IDENTITY", Some("/k/id")).is_err());
        let mut both = env();
        both.insert("HERDR_DESKTOP_E2E_SSH_IDENTITY", "/k/id");
        both.insert("HERDR_DESKTOP_E2E_SSH_KNOWN_HOSTS", "/k/known");
        let ssh = config(&both).unwrap().isolated_ssh.unwrap();
        assert_eq!(ssh.identity_file, std::path::Path::new("/k/id"));
    }
}

// ---------------------------------------------------------------------------------------
// The single native flow
// ---------------------------------------------------------------------------------------

/// Window process of one phase (never an E2E case by itself; started by the flow on the private
/// display). Refuses to open a window anywhere else.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "window process of e2e_fidelity_flow; refuses to run outside its private display"]
fn fidelity_window_phase() {
    let env = |k: &str| std::env::var(k).ok();
    // SAFETY of the user's session: checked before any GUI object exists.
    let uid = native_harness::required("HERDR_DESKTOP_E2E_USER_UID")
        .parse::<u32>()
        .expect("uid");
    support::display::guard_window_env(&env, uid).unwrap_or_else(|e| panic!("{e}"));
    let phase = native_harness::current_phase();
    let params: Value = serde_json::from_str(&native_harness::required("HERDR_DESKTOP_E2E_PARAMS"))
        .expect("params");
    let config = support::window::desktop_config(&env).unwrap_or_else(|e| panic!("{e}"));
    support::window::run_app_window(
        tauri::generate_context!(),
        support::window::WindowPhase {
            phase,
            params,
            report_path: PathBuf::from(native_harness::required(native_harness::RESULT_ENV)),
            timeout: std::time::Duration::from_secs(840),
            config,
        },
    );
    panic!("the composed window returned without reporting done");
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "single native E2E of spec 007; needs the disposable session and private display; run by just check-spec 007"]
fn e2e_fidelity_flow() {
    let absolute = |key: &str| {
        let path = PathBuf::from(native_harness::required(key));
        assert!(path.is_absolute(), "{key} must be absolute");
        path
    };
    let session_dir = absolute("HERDR_DESKTOP_E2E_DIR");
    let evidence = absolute("HERDR_DESKTOP_E2E_REPORT").join("native-live");
    std::fs::create_dir_all(&evidence).expect("evidence dir");
    let herdr_bin = absolute("HERDR_DESKTOP_HERDR_BIN");

    // Private Local/SSH/legacy hosts for every phase (never the gate's or a default session).
    // Short workdir: the engine socket path must stay within the fixture's 100-byte limit.
    let workdir = tempfile::Builder::new()
        .prefix("hd7S")
        .tempdir_in("/tmp")
        .expect("fixture workdir");
    let config =
        support::remote::RemoteFixtureConfig::new(workdir.path()).with_reference_binary(&herdr_bin);
    let mut fx = support::remote::RemoteFixture::start_pair(config)
        .unwrap_or_else(|e| panic!("remote fixture pair (already cleaned up): {e}"));
    let ledger = support::remote::Ledger::in_workdir(workdir.path());
    // From here every failure is collected; the fixture is cleaned before any assertion (its
    // guard also stops everything on unwind).
    let mut errors = Vec::new();
    if let Err(e) = fx.start_legacy_host() {
        errors.push(format!("legacy host: {e}"));
    }
    let fixture_params = fx.harness_params_json();
    let local = fx.local_fixture();
    let local_session = local.session.clone();
    support::session::disposable(&local_session).unwrap_or_else(|e| panic!("{e}"));
    let nonce = format!(
        "{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_micros() % 0xffff_ffff)
            .unwrap_or_default()
            | 0x1000_0000
    );
    let expectations = fx
        .legacy_fixture()
        .ok_or("legacy host not started".to_owned())
        .and_then(|leg| {
            support::ssh_flow::Expectations::from_fixture_params(
                &fixture_params,
                &leg.boot_id,
                &leg.root.display().to_string(),
                &nonce,
            )
        });
    let expectations = match expectations {
        Ok(e) => Some(e),
        Err(e) => {
            errors.push(format!("ssh expectations: {e}"));
            None
        }
    };
    // Positive lazy-editor control in its own Local root (the fixture root and notas.txt untouched).
    let work = session_dir.join("work");
    std::fs::create_dir_all(&work).expect("work dir");
    let fixture_file = "fidelity_fixture.rs";
    std::fs::write(
        work.join(fixture_file),
        "// spec 007 lazy-editor positive control\nfn main() {\n    println!(\"fidelidade\");\n}\n",
    )
    .expect("fixture file");
    assert_ne!(
        work, local.root,
        "lazy-editor root must differ from the fixture Local root"
    );
    let text = |k: &str| fixture_params[k].as_str().map(PathBuf::from).expect(k);
    let flow_env = support::live::FlowEnv {
        session: local_session.clone(),
        herdr_env: fx
            .local_session
            .namespace
            .env()
            .expect("local namespace env"),
        local_boot: local.boot_id.clone(),
        pane: local.pane_id.clone(),
        session_dir,
        herdr_bin,
        herdr_config_dir: text("local_herdr_config_dir"),
        resources_root: absolute("HERDR_DESKTOP_NATIVE_RESOURCES"),
        evidence: evidence.clone(),
        corpus: corpus_fixture(),
        user: native_harness::required("USER"),
        uid: support::live::uid(),
    };
    let driven: Vec<&str> = FLOW
        .iter()
        .map(|p| p.name)
        .filter(|n| DRIVEN_PHASES.contains(n))
        .collect();
    let exe = std::env::current_exe().expect("test executable");
    let observer = support::ssh_flow::fixture::FixtureObserver(&fx);
    let link = expectations.as_ref().map(|exp| support::live::SshLink {
        observer: &observer,
        page_params: exp.page_params(&fixture_params),
        identity: text("ssh_identity"),
        known_hosts: text("ssh_known_hosts"),
    });
    let live = if errors.is_empty() {
        support::live::run(&flow_env, &exe, &driven, fixture_file, link.as_ref())
    } else {
        support::live::LiveRun::default()
    };
    drop(link);
    let raw_fixture = fx.raw_observations();
    let cleanup = fx.cleanup();
    let ledger_clean = support::remote::verify_ledger_clean(&ledger);
    if !cleanup.failures.is_empty() {
        errors.push(format!("fixture cleanup failures: {:?}", cleanup.failures));
    }
    if !ledger_clean.is_clean() {
        errors.push(format!("fixture ledger not clean: {ledger_clean:?}"));
    }

    let chunk_map = read(&repo().join("dist/.vite/module-chunks.json"));
    let mut outcomes = BTreeMap::new();
    let mut observed = serde_json::Map::new();
    errors.extend(live.errors.clone());
    for name in &driven {
        let Some(report) = live.reports.get(*name) else {
            errors.push(format!("{name}: no report from the window"));
            continue;
        };
        let mut report = report.clone();
        if (*name == "lazy-editor" || *name == support::visual_files::PHASE)
            && report["error"].is_null()
        {
            report["chunk_map"] = json!(chunk_map);
        }
        let outcome = if support::ssh_flow::PHASES.iter().any(|(p, _)| p == name) {
            match &expectations {
                Some(exp) => support::ssh_flow::checks(name, &report, &live.ledger, exp)
                    .map(|c| PhaseOutcome::with(&c)),
                None => Err(format!("{name}: no ssh expectations")),
            }
        } else if *name == support::visual_frame::PHASE {
            support::visual_frame::checks(&report, &live.visual_ledger)
                .map(|c| PhaseOutcome::with(&c))
        } else if *name == support::visual_agents::PHASE {
            support::visual_agents::checks(&report, &live.agents_ledger)
                .map(|c| PhaseOutcome::with(&c))
        } else if *name == support::visual_projects::PHASE {
            support::visual_projects::checks(&report, &live.visual_projects_ledger)
                .map(|c| PhaseOutcome::with(&c))
        } else if *name == support::visual_files::PHASE {
            support::visual_files::checks(&report, &live.visual_files_ledger)
                .map(|c| PhaseOutcome::with(&c))
        } else if *name == support::visual_center::PHASE {
            support::visual_center::checks(&report, &live.center_ledger)
                .map(|c| PhaseOutcome::with(&c))
        } else if let Some(linked) = linked_checks(name, &report, &live) {
            linked.map(|c| PhaseOutcome::with(&c))
        } else {
            checks_for(name, &report, &local_session)
        };
        match outcome {
            Ok(outcome) => {
                observed.insert((*name).into(), json!(outcome.checks));
                outcomes.insert((*name).to_owned(), outcome);
            }
            Err(e) => errors.push(e),
        }
    }
    let native_ime_contract = live.reports.get("native-ime").map(|report| {
        let items = report["parent"]["items"].as_array().cloned().unwrap_or_default();
        let events = report["commit_events"].as_array().cloned().unwrap_or_default();
        let matched = support::corpus::native_ime_variant(&items, &events);
        json!({
            "variant_index": matched.as_ref().ok(),
            "variant_words": matched.as_ref().ok().map(|i| support::corpus::NATIVE_IME_VARIANTS[*i]),
            "mismatch": matched.as_ref().err(),
            "selection_space_candidate_1": matched.as_ref().ok().map(|i| support::corpus::NATIVE_IME_VARIANTS[*i][1]),
            "selection_key_2_candidate_2": matched.as_ref().ok().map(|i| support::corpus::NATIVE_IME_VARIANTS[*i][2]),
            "trusted_commit_events": events,
            "items": items.iter().map(|i| json!({
                "id": i["id"], "commit_hex": i["commit_hex"], "preedit_hex": i["preedit_hex"],
                "commit_window_ms": i["commit_window_ms"], "preedit_png": i["preedit_png"],
            })).collect::<Vec<_>>(),
        })
    });
    let verdict = plan::evaluate(FLOW, &outcomes);
    let complete = verdict.is_pass(FLOW) && errors.is_empty();
    let summary = json!({
        "status": if complete { "COMPLETE" } else { "INCOMPLETE" },
        "session": local_session,
        "driven_phases": driven,
        "observed_checks": observed,
        "native_ime_contract": native_ime_contract,
        "verdict": {
            "passed": verdict.passed,
            "failed": verdict.failed,
            "pending": verdict.pending,
            "missing": verdict.missing,
            "unknown": verdict.unknown,
        },
        "errors": errors,
        "ssh_fixture": {
            "params": fixture_params,
            "expectations": expectations,
            "raw_observations": raw_fixture,
            "cleanup": cleanup,
            "ledger_verification": ledger_clean,
        },
        "window_exit": live.window_exit,
        "final_report": live.final_report,
        "isolation_unix_sockets": live.isolation,
        "supervisor_log": live.supervisor_log,
        "pointer_expectations": {
            "paste-selection": live.paste_expectations.as_ref().map(|e| format!("{e:?}")),
            "mouse-scroll-links": live.mouse_expectations.as_ref().map(|e| format!("{e:?}")),
        },
        "view_expectations": live.view_expectations.as_ref().map(|e| format!("{e:?}")),
    });
    std::fs::write(
        evidence.join("flow-report.json"),
        serde_json::to_vec_pretty(&summary).unwrap(),
    )
    .expect("flow report");
    std::fs::write(
        evidence.join("ssh-parent-ledger.json"),
        serde_json::to_vec_pretty(&live.ledger.steps).unwrap(),
    )
    .expect("ssh parent ledger");
    std::fs::write(
        evidence.join("pointer-parent-ledger.json"),
        serde_json::to_vec_pretty(&live.pointer_ledger.steps).unwrap(),
    )
    .expect("pointer parent ledger");
    std::fs::write(
        evidence.join("view-parent-ledger.json"),
        serde_json::to_vec_pretty(&live.view_ledger.steps).unwrap(),
    )
    .expect("view parent ledger");
    std::fs::write(
        evidence.join("visual-parent-ledger.json"),
        serde_json::to_vec_pretty(&live.visual_ledger.steps).unwrap(),
    )
    .expect("visual parent ledger");
    std::fs::write(
        evidence.join("agents-parent-ledger.json"),
        serde_json::to_vec_pretty(&live.agents_ledger.steps).unwrap(),
    )
    .expect("agents parent ledger");
    std::fs::write(
        evidence.join("visual-files-parent-ledger.json"),
        serde_json::to_vec_pretty(&live.visual_files_ledger.steps).unwrap(),
    )
    .expect("visual-files parent ledger");
    std::fs::write(
        evidence.join("center-parent-ledger.json"),
        serde_json::to_vec_pretty(&live.center_ledger.steps).unwrap(),
    )
    .expect("center parent ledger");
    std::fs::write(
        evidence.join("phase-reports.json"),
        serde_json::to_vec_pretty(&live.reports).unwrap(),
    )
    .expect("phase reports");
    assert!(
        complete,
        "fidelity flow INCOMPLETE (never a pass while any phase is pending, failed or missing): {}",
        serde_json::to_string_pretty(&summary["verdict"]).unwrap()
    );
}

/// Phases with a page scenario, a parent driver and a parent evaluator.
const DRIVEN_PHASES: [&str; 18] = [
    "compose-mount",
    "lazy-editor",
    "hosts-identity",
    "ssh-agent-actions",
    "ssh-files-readonly",
    "legacy-server",
    "host-switch-dirty",
    "native-keys",
    "native-ime",
    "paste-selection",
    "mouse-scroll-links",
    "resize-dpi",
    "a11y-navigation",
    "visual-frame",
    "visual-agents",
    "visual-projects",
    "visual-files",
    "visual-center",
];

/// Adapter seams of the linked paste/mouse phases (no display, engine, GUI, clipboard or pointer).
#[cfg(target_os = "linux")]
mod pointer_live_seams {
    use super::*;
    use support::live::{self, ClientOffset, LiveRun};
    use support::mouse_flow::{self, LinkHandler, Point};
    use support::paste_flow;

    const WINDOW_PID: u32 = 4242;

    fn view(pid: u32, rect: [i64; 4], window_rect: [i64; 4]) -> Value {
        let r = |v: [i64; 4]| json!({ "x": v[0], "y": v[1], "width": v[2], "height": v[3] });
        json!({ "type": "con", "pid": pid, "app_id": "herdr-desktop", "rect": r(rect),
                "window_rect": r(window_rect), "nodes": [], "floating_nodes": [] })
    }

    fn tree(views: Vec<Value>) -> Value {
        json!({ "type": "root", "nodes": [ { "type": "output", "name": "HEADLESS-1",
            "nodes": [ { "type": "workspace", "nodes": views, "floating_nodes": [] } ] } ] })
    }

    fn client(width: f64, height: f64, dpr: f64) -> Value {
        json!({ "coordinate_space": "client", "width": width, "height": height, "dpr": dpr,
                "screen_x": 0, "screen_y": 0 })
    }

    // Would catch: assuming the client origin is the output origin (GTK header bar ignored),
    // reading the decoy view's rect, or dropping the container/window_rect position.
    #[test]
    fn client_offset_is_the_observed_window_origin_plus_the_header_bar() {
        let decoy = view(99, [640, 0, 640, 720], [0, 0, 640, 720]);
        let ours = view(WINDOW_PID, [0, 3, 1280, 717], [2, 5, 1276, 710]);
        let off = live::client_offset(
            &tree(vec![decoy, ours]),
            WINDOW_PID,
            &client(1276.0, 663.0, 1.0),
        )
        .unwrap();
        assert_eq!((off.x, off.y), (2.0, 55.0));
        assert_eq!(off.observation["window_pid"], WINDOW_PID);
        assert_eq!(off.observation["header_height"], 47.0);
        let out = live::to_output(Point { x: 10.0, y: 20.0 }, &off).unwrap();
        assert_eq!(out, Point { x: 12.0, y: 75.0 });
        assert!(live::to_output(
            Point {
                x: 1279.0,
                y: 700.0
            },
            &off
        )
        .is_err());
    }

    // Would catch: guessing when the tree does not identify one window or when the client does not
    // fit the observed geometry (side CSD, scaled output, client taller than the window).
    #[test]
    fn client_offset_refuses_what_it_cannot_observe() {
        let ours = || view(WINDOW_PID, [0, 0, 1280, 720], [0, 0, 1280, 720]);
        let ok = client(1280.0, 673.0, 1.0);
        assert!(live::client_offset(&tree(vec![ours()]), WINDOW_PID, &ok).is_ok());
        assert!(live::client_offset(&tree(vec![ours()]), 7, &ok).is_err());
        assert!(live::client_offset(&tree(vec![ours(), ours()]), WINDOW_PID, &ok).is_err());
        for bad in [
            client(1270.0, 673.0, 1.0),
            client(1280.0, 721.0, 1.0),
            client(1280.0, 673.0, 2.0),
            json!({ "coordinate_space": "client", "width": 1280.0 }),
            // Points of another (or an undeclared) space would be offset by an unknown amount.
            json!({ "coordinate_space": "screen", "width": 1280.0, "height": 673.0, "dpr": 1.0 }),
            json!({ "width": 1280.0, "height": 673.0, "dpr": 1.0, "screen_x": 0, "screen_y": 0 }),
        ] {
            assert!(
                live::client_offset(&tree(vec![ours()]), WINDOW_PID, &bad).is_err(),
                "{bad}"
            );
        }
    }

    // Would catch: rejecting (or adding) WebKit's bogus window.screenX/Y (r2: 20,20 while sway
    // placed the window at 0,0), ignoring a nonzero output/window origin, or a fixed header.
    #[test]
    fn client_points_get_the_observed_origin_and_header_once_screen_xy_is_audit_only() {
        let mut c = client(1280.0, 673.0, 1.0);
        c["screen_x"] = json!(20);
        c["screen_y"] = json!(20);
        let r2 = view(WINDOW_PID, [0, 0, 1280, 720], [0, 0, 1280, 720]);
        let off = live::client_offset(&tree(vec![r2]), WINDOW_PID, &c).unwrap();
        assert_eq!((off.x, off.y), (0.0, 47.0));
        assert_eq!(
            off.observation["screen_audit"],
            json!({ "screen_x": 20, "screen_y": 20 })
        );
        let at = live::to_output(Point { x: 383.5, y: 137.5 }, &off).unwrap();
        assert_eq!(at, Point { x: 383.5, y: 184.5 });
        // Another window origin and a 37 px header: offset differs accordingly.
        let moved = view(WINDOW_PID, [100, 0, 1180, 720], [0, 10, 1180, 700]);
        let off = live::client_offset(&tree(vec![moved]), WINDOW_PID, &client(1180.0, 663.0, 1.0))
            .unwrap();
        assert_eq!((off.x, off.y), (100.0, 47.0));
        assert_eq!(off.observation["header_height"], 37.0);
    }

    // Would catch: reading the scroll of another pane shape, or defaulting a missing scroll to 0.
    #[test]
    fn pane_scroll_is_the_engine_pane_get_scroll() {
        let raw = json!({ "id": "cli:pane:get", "result": { "type": "pane_info", "pane": {
            "pane_id": "w1:p1", "scroll": { "offset_from_bottom": 0, "max_offset_from_bottom": 181,
            "viewport_rows": 36 } } } });
        let scroll = live::pane_scroll(&raw.to_string()).unwrap();
        assert_eq!(
            scroll,
            json!({ "offset_from_bottom": 0, "max_offset_from_bottom": 181, "viewport_rows": 36 })
        );
        let missing = json!({ "result": { "type": "pane_info", "pane": { "pane_id": "w1:p1" } } });
        assert!(live::pane_scroll(&missing.to_string()).is_err());
        assert!(live::pane_scroll("not json").is_err());
    }

    // Would catch: acting on the SSH host's w1:p1 (other boot) or on a page without a pane.
    #[test]
    fn local_target_rejects_other_boot_or_missing_pane_before_action() {
        let want = |pane: &str, boot: &str| json!({ "pane_id": pane, "generation": "3", "boot_prefix": boot, "endpoint": "Este computador · Local" });
        assert_eq!(
            live::local_target("b0a1c2d3e4", &want("w1:p1", "b0a1c2")).unwrap(),
            "w1:p1"
        );
        assert!(live::local_target("b0a1c2d3e4", &want("w1:p1", "ffee01")).is_err());
        assert!(live::local_target("b0a1c2d3e4", &want("w1:p1", "")).is_err());
        assert!(live::local_target("b0a1c2d3e4", &want("", "b0a1c2")).is_err());
    }

    // Would catch: answering a step twice (replayed action), answering a step never requested,
    // or a torn ack the page could read half-written.
    #[test]
    fn due_steps_are_answered_once_and_acks_are_whole() {
        let dir = std::env::temp_dir().join(format!("hd007-due-steps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let result = dir.join("report.json");
        let mut handled = Vec::new();
        // Every file below is written by this test, after the run start.
        let since = std::time::SystemTime::UNIX_EPOCH;
        let steps = ["paste-native", "alt-screen-mouse"];
        assert!(live::due_steps(&result, &steps, &mut handled, since).is_empty());
        std::fs::write(result.with_extension("want-alt-screen-mouse"), "{broken").unwrap();
        std::fs::write(
            result.with_extension("want-paste-native"),
            r#"{"pane_id":"w1:p1"}"#,
        )
        .unwrap();
        std::fs::write(
            result.with_extension("want-other"),
            r#"{"pane_id":"w1:p1"}"#,
        )
        .unwrap();
        let due = live::due_steps(&result, &steps, &mut handled, since);
        assert_eq!(
            due,
            vec![("paste-native".to_owned(), json!({ "pane_id": "w1:p1" }))]
        );
        assert!(live::due_steps(&result, &steps, &mut handled, since).is_empty());
        live::write_ack(&result, "paste-native", &json!({ "step": "paste-native" }));
        let ack = std::fs::read_to_string(result.with_extension("ack-paste-native")).unwrap();
        assert_eq!(ack, r#"{"step":"paste-native"}"#);
        assert!(!result.with_extension("tmp").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn page(phase: &str) -> Value {
        json!({ "phase": phase, "error": null })
    }

    // Would catch: a linked phase evaluated by the generic evaluator (always Err "no evaluator"),
    // a phase passing without the parent's expectations or ledger, or phases routed crosswise.
    #[test]
    fn linked_checks_route_each_phase_to_its_own_evaluator() {
        let mut run = LiveRun::default();
        assert!(linked_checks("native-keys", &page("native-keys"), &run).is_none());
        for phase in [
            paste_flow::PHASE,
            mouse_flow::PHASE,
            support::view_flow::RESIZE_PHASE,
            support::view_flow::A11Y_PHASE,
        ] {
            let err = linked_checks(phase, &page(phase), &run)
                .expect(phase)
                .unwrap_err();
            assert!(err.contains("expectations"), "{phase}: {err}");
        }
        run.paste_expectations = Some(
            paste_flow::Expectations::new(
                &corpus_fixture(),
                "/tmp/hd7L-1",
                "/tmp/hd7L-1/wayland-1",
                "Este computador · Local",
                "hd007-clipboard-sentinel-abc123",
            )
            .unwrap(),
        );
        let handler =
            LinkHandler::new(Path::new("/tmp/hd007-home"), Path::new("/home/u"), 1000).unwrap();
        run.mouse_expectations = Some(
            mouse_flow::Expectations::new(
                "Este computador · Local",
                "/tmp/hd7L-1",
                "/tmp/hd7L-1/sway-ipc.1000.7.sock",
                &handler,
                "abc123",
            )
            .unwrap(),
        );
        // Same report shape errors come from each module's own evaluator (phase name mismatch).
        let err = linked_checks(paste_flow::PHASE, &page(mouse_flow::PHASE), &run)
            .unwrap()
            .unwrap_err();
        assert!(err.contains("report of phase"), "{err}");
        let err = linked_checks(paste_flow::PHASE, &page(paste_flow::PHASE), &run)
            .unwrap()
            .unwrap_err();
        assert!(
            !err.contains("expectations") && !err.contains("report of phase"),
            "{err}"
        );
        let err = linked_checks(mouse_flow::PHASE, &page(mouse_flow::PHASE), &run)
            .unwrap()
            .unwrap_err();
        assert!(
            !err.contains("expectations") && !err.contains("report of phase"),
            "{err}"
        );
        let _unused: Option<ClientOffset> = None;
    }

    // Would catch: MouseWorld failing to forward Pointer::seat_commands, leaving the step ledger empty.
    #[test]
    fn mouse_world_forwards_seat_commands_to_pointer() {
        let expected = vec![json!({
            "argv": ["seat", "seat0", "cursor", "set", "100", "100"],
            "command": "[\"seat\", \"seat0\", \"cursor\", \"set\", \"100\", \"100\"]",
            "exit": 0,
            "stdout": "[{\"success\":true}]"
        })];
        let mut pointer = mouse_flow::PrivatePointer::with_commands(expected.clone());
        let forwarded = live::mouse_world_seat_commands(&mut pointer);
        assert_eq!(forwarded, expected);
    }
}

// ---------------------------------------------------------------------------------------
// Handshake freshness: files of an earlier run are never read, and a want of another engine boot
// is refused without an answer (gate r12: tracked want-native-keys of boot 3163381- was answered).
// ---------------------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod handshake_freshness {
    use super::*;
    use std::cell::Cell;
    use std::time::{Duration, SystemTime};
    use support::live;

    const ENGINE_BOOT: &str = "7f3a9c01d2e4b5a6";

    fn dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hd007-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_at(path: &Path, body: &str, mtime: SystemTime) {
        std::fs::write(path, body).unwrap();
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
    }

    fn want(boot: &str) -> Value {
        json!({ "pane_id": "w1:p1", "generation": "1", "boot_prefix": boot, "endpoint": "local · Local" })
    }

    // Would catch: the parent answering a want-/ack-/report file left by an earlier run (tracked
    // evidence), deleting non-handshake evidence, or deleting a file this run already wrote.
    #[test]
    fn stale_handshake_files_are_removed_and_never_read() {
        let results = dir("stale-handshake");
        let run_started = SystemTime::now();
        let old = run_started - Duration::from_secs(3600);
        let fresh = run_started + Duration::from_secs(1);
        let result = results.join("report.json");
        let stale = [
            "report.want-native-keys",
            "report.want-native-ime",
            "report.want-paste-native",
            "report.ack-native-keys",
            "report.ack-native-ime",
            "report.json",
            "report.jsonl",
            "report.tmp",
        ];
        for name in stale {
            write_at(&results.join(name), &want("3163381").to_string(), old);
        }
        write_at(&results.join("keep.txt"), "not a handshake file", old);
        write_at(
            &results.join("report.want-alt-screen-mouse"),
            &want(ENGINE_BOOT).to_string(),
            fresh,
        );

        // Before clearing, an old want is invisible to the parent; a fresh one is due.
        assert_eq!(live::fresh_want(&result, "native-keys", run_started), None);
        assert_eq!(
            live::fresh_want(&result, "alt-screen-mouse", run_started),
            Some(want(ENGINE_BOOT))
        );
        let mut handled = Vec::new();
        assert_eq!(
            live::due_steps(
                &result,
                &["paste-native", "alt-screen-mouse"],
                &mut handled,
                run_started
            ),
            vec![("alt-screen-mouse".to_owned(), want(ENGINE_BOOT))]
        );

        let mut removed = live::clear_stale_handshake(&results, run_started).unwrap();
        removed.sort();
        let mut expected: Vec<PathBuf> = stale.iter().map(|n| results.join(n)).collect();
        expected.sort();
        assert_eq!(removed, expected);
        for name in stale {
            assert!(
                !results.join(name).exists(),
                "{name} survived the run start"
            );
        }
        assert!(results.join("keep.txt").exists());
        assert!(results.join("report.want-alt-screen-mouse").exists());
        // Idempotent and a missing results dir is not an error.
        assert!(live::clear_stale_handshake(&results, run_started)
            .unwrap()
            .is_empty());
        assert!(
            live::clear_stale_handshake(&results.join("absent"), run_started)
                .unwrap()
                .is_empty()
        );
        assert!(live::is_handshake_file("report.ack-resize-dpi"));
        assert!(!live::is_handshake_file("report-notes.json"));
        assert!(!live::is_handshake_file("flow-report.json"));
        std::fs::remove_dir_all(&results).unwrap();
    }

    // Would catch: answering (writing an ack for) a native want of another engine boot, acting on
    // it, or a refusal without the named error; and refusing this engine's own want.
    #[test]
    fn native_want_of_another_boot_is_refused_without_an_answer() {
        let results = dir("foreign-boot");
        let result = results.join("report.json");
        let ack = result.with_extension("ack-native-keys");
        let mut errors = Vec::new();
        for foreign in [want("3163381"), want(""), json!({ "pane_id": "w1:p1" })] {
            let acted = Cell::new(false);
            let err = live::answer_native_step(
                &result,
                ENGINE_BOOT,
                "native-keys",
                &foreign,
                &mut errors,
                |_| {
                    acted.set(true);
                    Ok(json!({ "items": [] }))
                },
            )
            .unwrap_err();
            assert!(
                err.starts_with(live::FOREIGN_BOOT_WANT),
                "unnamed refusal: {err}"
            );
            assert!(err.contains(ENGINE_BOOT), "{err}");
            assert!(!acted.get(), "acted on a foreign want");
            assert!(!ack.exists(), "answered a foreign want");
        }
        assert!(
            errors.is_empty(),
            "refusal is returned, not mixed into action errors"
        );

        let acted = Cell::new(false);
        let answer = live::answer_native_step(
            &result,
            ENGINE_BOOT,
            "native-keys",
            &want("7f3a9c"),
            &mut errors,
            |w| {
                acted.set(true);
                Ok(json!({ "pane_id": w["pane_id"] }))
            },
        )
        .unwrap();
        assert!(acted.get());
        assert_eq!(answer, json!({ "pane_id": "w1:p1" }));
        assert_eq!(
            std::fs::read_to_string(&ack).unwrap(),
            r#"{"pane_id":"w1:p1"}"#
        );
        // A failing action on this run's own want is still answered (the page must not hang).
        let ime_ack = result.with_extension("ack-native-ime");
        let answer = live::answer_native_step(
            &result,
            ENGINE_BOOT,
            "native-ime",
            &want("7f3a9c"),
            &mut errors,
            |_| Err("pane gone".into()),
        )
        .unwrap();
        assert_eq!(answer, json!({ "error": "pane gone" }));
        assert_eq!(
            std::fs::read_to_string(&ime_ack).unwrap(),
            r#"{"error":"pane gone"}"#
        );
        assert_eq!(errors, vec!["native-ime: pane gone".to_owned()]);
        std::fs::remove_dir_all(&results).unwrap();
    }
}

// ---------------------------------------------------------------------------------------
// Spec 010 — visual-frame evaluator and parent helpers (no display, engine or GUI)
// ---------------------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod visual_frame_eval {
    use super::*;
    use support::visual_frame::{self as vf, Ledger};

    const HOST: &str = "Este computador · Local";

    fn small(scope: &str, text: &str, color: &str, background: &str) -> Value {
        json!({ "scope": scope, "text": text, "color": color, "background": background, "opacity": 1,
                "font_size": 13, "font_weight": 400, "visibility": "visible" })
    }

    /// An open menu with its items and the small texts measured while it was open.
    fn menu(label: &str, items: Value) -> Value {
        let muted = "rgb(140, 147, 163)";
        let texts: Vec<Value> = items
            .as_array()
            .unwrap()
            .iter()
            .map(|i| {
                small(
                    &format!("menu:{label}"),
                    i["label"].as_str().unwrap(),
                    muted,
                    "rgba(23, 26, 33, 1)",
                )
            })
            .collect();
        json!({ "label": label, "expanded": "true", "items": items, "small_texts": texts })
    }

    fn report() -> Value {
        let muted = "rgb(140, 147, 163)";
        let surface = "rgba(17, 19, 24, 1)";
        let tokens: serde_json::Map<String, Value> = vf::GUIDE_TOKENS
            .iter()
            .map(|(k, v)| {
                let value = match *k {
                    "--font-ui" => "\"Inter\", system-ui, sans-serif".to_owned(),
                    "--font-mono" => "\"JetBrains Mono\", monospace".to_owned(),
                    _ => (*v).to_owned(),
                };
                ((*k).to_owned(), json!(value))
            })
            .collect();
        let item = |kind: &str, text: &str, disabled: bool| json!({ "item": kind, "text": text, "label": format!("{kind} label"), "title": null, "disabled": disabled });
        let enabled = |id: &str, action: &str| json!({ "id": id, "label": id, "action": action, "disabled": false, "reason": id });
        let disabled = |id: &str| json!({ "id": id, "label": id, "action": null, "disabled": true, "reason": "Indisponível: motivo" });
        json!({
            "phase": "visual-frame", "error": null,
            "identity": { "pane_id": "w3:p1", "generation": "5", "boot_prefix": "2040265-", "endpoint": HOST },
            "viewport": { "inner_width": 1440, "inner_height": 900, "dpr": 1 },
            "frame": {
                "regions": {
                    "topbar": { "left": 0, "top": 0, "width": 1440, "height": 44 },
                    "activity": { "left": 0, "top": 44, "width": 52, "height": 830 },
                    "projects": { "left": 52, "top": 44, "width": 272, "height": 830 },
                    "center": { "left": 324, "top": 44, "width": 860, "height": 830 },
                    "agents": { "left": 1184, "top": 44, "width": 256, "height": 830 },
                    "status": { "left": 0, "top": 874, "width": 1440, "height": 26 }
                },
                "tokens": tokens,
                "fonts": { "topbar": "Inter, system-ui", "status": "Inter, system-ui", "terminal": "\"JetBrains Mono\", monospace", "canvas": "15px \"JetBrains Mono\", \"Fira Code\"" },
                "topbar_items": [
                    item("logo", "herdr", false), item("menu", "Arquivo", false), item("menu", "Editar", false),
                    item("menu", "Ver", false), item("menu", "Agentes", false), item("menu", "Janela", false),
                    item("search", "Buscar projetos, panes, agentes e comandos\nCtrl K", false), item("search-hint", "Ctrl K", false),
                    item("host", HOST, false), item("notifications", "", true), item("window-minimize", "", false),
                    item("window-maximize", "", false), item("window-close", "", false)
                ],
                "host": { "title": format!("{HOST} — Conectado"), "text": HOST, "text_overflow": "ellipsis", "overflow": "hidden", "white_space": "nowrap", "dot": true },
                "status_items": [
                    { "item": "server", "text": "herdr server 0.9.0 · conectado", "phase": "live" },
                    { "item": "host", "text": HOST, "phase": null },
                    { "item": "session", "text": "sessão hd007-local-1", "phase": null },
                    { "item": "branch", "text": "hd010-branch", "phase": null },
                    { "item": "counts", "text": "1 pane · 3 abas", "phase": null },
                    { "item": "channel", "text": "canal stable", "phase": null }
                ],
                "small_texts": [
                    small("topbar", "herdr", "rgb(231, 233, 238)", surface), small("topbar", "Arquivo", muted, surface),
                    small("topbar", "Ctrl K", muted, "rgba(30, 34, 43, 1)"), small("topbar", HOST, "rgb(231, 233, 238)", "rgba(23, 26, 33, 1)"),
                    small("status", "herdr server 0.9.0 · conectado", muted, surface), small("status", "canal stable", muted, surface)
                ]
            },
            "menus": [
                menu("Arquivo", json!([enabled("projects", "showProjects"), disabled("open-file")])),
                menu("Editar", json!([disabled("undo"), enabled("paste", "paste")])),
                menu("Ver", json!([enabled("search", "openPalette")])),
                menu("Agentes", json!([enabled("split", "split")])),
                menu("Janela", json!([disabled("minimize")]))
            ],
            "ctrlk_outside": {
                "opener": "host",
                "keys": [{ "trusted": true, "target": "host", "palette_at_first_frame": true, "frames_waited": 1 }],
                "palette": { "input_focused": true, "sections": [
                    { "title": "Projetos", "entries": [
                        { "id": "project:p1", "label": "Fidelidade Local" }, { "id": "project:p2", "label": "Fidelidade SSH" }
                    ] },
                    { "title": "Panes", "entries": [{ "id": "pane:w3:p1", "label": "w3:p1" }] },
                    { "title": "Agentes", "entries": [{ "id": "agent:w1:p1", "label": "pi" }] },
                    { "title": "Comandos", "entries": [{ "id": "command:split", "label": "Dividir pane" }] }
                ], "small_texts": [
                    small("palette", "Esc", muted, "rgba(30, 34, 43, 1)"), small("palette", "PROJETOS", muted, "rgba(23, 26, 33, 1)"),
                    small("palette", "PANES", muted, "rgba(23, 26, 33, 1)"), small("palette", "AGENTES", muted, "rgba(23, 26, 33, 1)"),
                    small("palette", "COMANDOS", muted, "rgba(23, 26, 33, 1)"),
                    small("palette", "Fidelidade Local", "rgb(231, 233, 238)", "rgba(23, 26, 33, 1)"),
                    small("palette", "Local · /tmp/work", muted, "rgba(23, 26, 33, 1)"),
                    small("palette", "Fidelidade SSH", "rgb(231, 233, 238)", "rgba(23, 26, 33, 1)"),
                    small("palette", "w3:p1", "rgb(231, 233, 238)", "rgba(23, 26, 33, 1)"),
                    small("palette", "pi", "rgb(231, 233, 238)", "rgba(23, 26, 33, 1)"),
                    small("palette", "Ocioso · w1:p1", muted, "rgba(23, 26, 33, 1)"),
                    small("palette", "Dividir pane", "rgb(231, 233, 238)", "rgba(23, 26, 33, 1)")
                ] }
            },
            "escape": { "palette": null, "focus_after": "host" },
            "ctrlk_terminal": {
                "focused": true, "palette": null,
                "keys": [{ "trusted": true, "target": "Terminal Herdr", "palette_at_first_frame": false, "frames_waited": 1 }]
            }
        })
    }

    fn engine() -> Value {
        vf::engine_expectations(
            "herdr 0.9.0\n",
            &json!({ "result": { "tabs": [{ "tab_id": "w1:t1" }, { "tab_id": "w2:t1" }, { "tab_id": "w3:t1" }] } }),
            &json!({ "result": { "panes": [
                { "pane_id": "w1:p1", "tab_id": "w1:t1" }, { "pane_id": "w2:p1", "tab_id": "w2:t1" },
                { "pane_id": "w3:p1", "tab_id": "w3:t1" }
            ] } }),
            "w3:p1",
            Some(vf::FIXTURE_BRANCH),
        )
        .unwrap()
    }

    /// `herdr agent list` of the session: one detected agent in another tab.
    fn agent_list(panes: &[&str]) -> Value {
        let agents: Vec<Value> = panes
            .iter()
            .map(|p| json!({ "pane_id": p, "tab_id": "w1:t1", "workspace_id": "w1", "agent": "pi", "agent_status": "idle", "terminal_id": "term_1", "focused": false, "revision": 1 }))
            .collect();
        json!({ "id": "cli:agent:list", "result": { "type": "agent_list", "agents": agents } })
    }

    /// The window's projects.json as the parent reads it.
    fn catalog(ids: &[&str]) -> Value {
        let projects: Vec<Value> = ids
            .iter()
            .map(|id| json!({ "id": id, "label": id, "endpoint_profile_id": "local", "session": "hd007-x", "root": "/tmp/work" }))
            .collect();
        json!({ "version": 1, "projects": projects, "collections": [] })
    }

    fn ledger(engine: Value) -> Ledger {
        ledger_with(engine, agent_list(&["w1:p1"]), catalog(&["p2", "p1"]))
    }

    fn ledger_with(engine: Value, agents: Value, projects: Value) -> Ledger {
        let mut ledger = Ledger::default();
        for step in vf::STEPS {
            let mut answer = json!({ "step": step, "pty_hex": "" });
            if step == "visual-engine" {
                answer["engine"] = engine.clone();
            }
            if step == "visual-ctrlk-outside" {
                answer["engine_agent_list"] = agents.clone();
                answer["project_catalog"] = projects.clone();
            }
            if step == "visual-ctrlk-terminal" {
                answer["pty_hex"] = json!("0b");
            }
            ledger.record(step, answer).unwrap();
        }
        ledger
    }

    fn failing(report: &Value, ledger: &Ledger) -> Vec<&'static str> {
        vf::checks(report, ledger)
            .unwrap()
            .into_iter()
            .filter(|(_, ok)| !ok)
            .map(|(name, _)| name)
            .collect()
    }

    // Would catch: the plan and the evaluator naming different checks, or the phase not driven.
    #[test]
    fn plan_checks_steps_and_actions_match_the_page_and_the_evaluator() {
        let spec = FLOW.iter().find(|p| p.name == vf::PHASE).unwrap();
        assert_eq!(spec.checks, vf::CHECKS);
        assert!(DRIVEN_PHASES.contains(&vf::PHASE));
        let names: Vec<&str> = vf::checks(&report(), &ledger(engine()))
            .unwrap()
            .iter()
            .map(|c| c.0)
            .collect();
        assert_eq!(names, vf::CHECKS);
        let page = read(&repo().join("src/features/fidelity/visual-frame.ts"));
        let steps = format!("[{}]", vf::STEPS.map(|s| format!("\"{s}\"")).join(", "));
        assert!(
            page.contains(&format!("VISUAL_STEPS = {steps} as const")),
            "page steps drifted"
        );
        let menus = read(&repo().join("src/components/frame/menus.ts"));
        for action in vf::MENU_ACTIONS {
            assert!(
                menus.contains(&format!("  \"{action}\",\n")),
                "{action} missing in menus.ts"
            );
        }
        assert_eq!(
            menus.matches("\n  \"").count(),
            vf::MENU_ACTIONS.len(),
            "menus.ts lists other actions"
        );
    }

    #[test]
    fn a_faithful_frame_passes_every_check() {
        assert_eq!(failing(&report(), &ledger(engine())), Vec::<&str>::new());
    }

    type Mutation = Box<dyn Fn(&mut Value)>;

    // Would catch each wrong behaviour flipping only its own check.
    #[test]
    fn each_wrong_frame_fails_its_own_check() {
        let cases: Vec<(&str, Mutation)> = vec![
            (
                "frame_dimensions_at_1440x900",
                Box::new(|r| r["frame"]["regions"]["topbar"]["height"] = json!(46)),
            ),
            (
                "frame_dimensions_at_1440x900",
                Box::new(|r| r["frame"]["regions"]["projects"]["width"] = json!(240)),
            ),
            (
                "frame_dimensions_at_1440x900",
                Box::new(|r| r["viewport"]["inner_height"] = json!(853)),
            ),
            (
                "guide_tokens_on_root",
                Box::new(|r| r["frame"]["tokens"]["--idle"] = json!("#8C93A3")),
            ),
            (
                "guide_tokens_on_root",
                Box::new(|r| r["frame"]["tokens"]["--font-ui"] = json!("system-ui")),
            ),
            (
                "ui_inter_terminal_jetbrains_mono",
                Box::new(|r| r["frame"]["fonts"]["canvas"] = json!("15px monospace")),
            ),
            (
                "top_bar_order_menus_and_host",
                Box::new(|r| {
                    r["frame"]["topbar_items"]
                        .as_array_mut()
                        .unwrap()
                        .swap(1, 2)
                }),
            ),
            (
                "top_bar_order_menus_and_host",
                Box::new(|r| r["menus"][0]["items"][1]["reason"] = json!("")),
            ),
            (
                "top_bar_order_menus_and_host",
                Box::new(|r| r["menus"][2]["items"][0]["action"] = json!("runShell")),
            ),
            (
                "top_bar_order_menus_and_host",
                Box::new(|r| r["frame"]["host"]["text_overflow"] = json!("clip")),
            ),
            (
                "status_bar_real_session",
                Box::new(|r| {
                    r["frame"]["status_items"][0]["text"] = json!("herdr server 0.8.0 · conectado")
                }),
            ),
            (
                "status_bar_real_session",
                Box::new(|r| r["frame"]["status_items"][3]["text"] = json!("main")),
            ),
            (
                "status_bar_real_session",
                Box::new(|r| r["frame"]["status_items"][4]["text"] = json!("3 panes · 3 abas")),
            ),
            (
                "small_text_contrast_at_least_4_5",
                Box::new(|r| r["frame"]["small_texts"][1]["color"] = json!("rgb(91, 98, 114)")),
            ),
            (
                "small_text_contrast_at_least_4_5",
                Box::new(|r| r["frame"]["small_texts"][5]["opacity"] = json!(0.55)),
            ),
            (
                "palette_opens_within_one_frame_with_engine_lists",
                Box::new(|r| {
                    r["ctrlk_outside"]["keys"][0]["palette_at_first_frame"] = json!(false)
                }),
            ),
            (
                "palette_opens_within_one_frame_with_engine_lists",
                Box::new(|r| r["ctrlk_outside"]["palette"]["sections"][1]["entries"] = json!([])),
            ),
            (
                "palette_opens_within_one_frame_with_engine_lists",
                Box::new(|r| r["ctrlk_outside"]["keys"][0]["trusted"] = json!(false)),
            ),
            (
                "escape_closes_and_restores_focus",
                Box::new(|r| r["escape"]["focus_after"] = json!("body")),
            ),
            (
                "ctrl_k_in_terminal_reaches_pty_once",
                Box::new(|r| r["ctrlk_terminal"]["palette"] = json!({ "sections": [] })),
            ),
        ];
        for (check, mutate) in cases {
            let mut r = report();
            mutate(&mut r);
            assert_eq!(
                failing(&r, &ledger(engine())),
                vec![check],
                "mutation for {check}"
            );
        }
        // The r1 hole: a cwd outside git makes the expected branch "—", the same text shown when
        // App passes no branch. The fixture repository's branch is required, not only equality.
        let mut no_git = report();
        no_git["frame"]["status_items"][3]["text"] = json!("—");
        let mut e = engine();
        e["branch"] = json!("—");
        assert_eq!(
            failing(&no_git, &ledger(e)),
            vec!["status_bar_real_session"]
        );
        // Agents and projects compared with the parent's engine agent list and catalog (equal,
        // non-empty), never only "every shown id exists".
        let agents_cases: Vec<(Value, Value, Mutation)> = vec![
            // agents=[] in App: the palette lists none while the engine has one.
            (
                agent_list(&["w1:p1"]),
                catalog(&["p1", "p2"]),
                Box::new(|r| r["ctrlk_outside"]["palette"]["sections"][2]["entries"] = json!([])),
            ),
            // An empty engine list is not a pass even when the palette agrees.
            (
                agent_list(&[]),
                catalog(&["p1", "p2"]),
                Box::new(|r| r["ctrlk_outside"]["palette"]["sections"][2]["entries"] = json!([])),
            ),
            // One engine agent missing from the palette (contained, not equal).
            (
                agent_list(&["w1:p1", "w2:p1"]),
                catalog(&["p1", "p2"]),
                Box::new(|_| {}),
            ),
            // projects=[] in App.
            (
                agent_list(&["w1:p1"]),
                catalog(&["p1", "p2"]),
                Box::new(|r| r["ctrlk_outside"]["palette"]["sections"][0]["entries"] = json!([])),
            ),
            // An empty catalog is not a pass even when the palette agrees.
            (
                agent_list(&["w1:p1"]),
                catalog(&[]),
                Box::new(|r| r["ctrlk_outside"]["palette"]["sections"][0]["entries"] = json!([])),
            ),
            // A project shown that the catalog does not have.
            (agent_list(&["w1:p1"]), catalog(&["p1"]), Box::new(|_| {})),
        ];
        for (i, (agents, projects, mutate)) in agents_cases.into_iter().enumerate() {
            let mut r = report();
            mutate(&mut r);
            assert_eq!(
                failing(&r, &ledger_with(engine(), agents, projects)),
                vec!["palette_opens_within_one_frame_with_engine_lists"],
                "agents/projects case {i}"
            );
        }
        let mut l = ledger(engine());
        l.steps.get_mut("visual-ctrlk-outside").unwrap()["engine_agent_list"] = Value::Null;
        l.steps.get_mut("visual-ctrlk-outside").unwrap()["project_catalog"] = Value::Null;
        let mut r = report();
        r["ctrlk_outside"]["palette"]["sections"][0]["entries"] = json!([]);
        r["ctrlk_outside"]["palette"]["sections"][2]["entries"] = json!([]);
        assert_eq!(
            failing(&r, &l),
            vec!["palette_opens_within_one_frame_with_engine_lists"],
            "parent never read the agents or the catalog"
        );
        // Contrast covers the open menus and the palette.
        let contrast_cases: Vec<Mutation> = vec![
            Box::new(|r| {
                r["ctrlk_outside"]["palette"]["small_texts"][10]["color"] =
                    json!("rgb(91, 98, 114)")
            }),
            Box::new(|r| r["menus"][0]["small_texts"][1]["color"] = json!("rgb(91, 98, 114)")),
            Box::new(|r| r["ctrlk_outside"]["palette"]["small_texts"] = json!([])),
            Box::new(|r| r["menus"][3]["small_texts"] = json!([])),
            Box::new(|r| {
                r["ctrlk_outside"]["palette"]["small_texts"]
                    .as_array_mut()
                    .unwrap()
                    .retain(|t| t["text"] != "pi")
            }),
        ];
        for (i, mutate) in contrast_cases.into_iter().enumerate() {
            let mut r = report();
            mutate(&mut r);
            assert_eq!(
                failing(&r, &ledger(engine())),
                vec!["small_text_contrast_at_least_4_5"],
                "contrast case {i}"
            );
        }
        // PTY bytes come from the parent ledger only.
        let mut l = ledger(engine());
        l.steps.get_mut("visual-ctrlk-terminal").unwrap()["pty_hex"] = json!("0b0b");
        assert_eq!(
            failing(&report(), &l),
            vec!["ctrl_k_in_terminal_reaches_pty_once"]
        );
        let mut l = ledger(engine());
        l.steps.get_mut("visual-ctrlk-outside").unwrap()["pty_hex"] = json!("0b");
        assert_eq!(
            failing(&report(), &l),
            vec!["palette_opens_within_one_frame_with_engine_lists"]
        );
        let mut l = ledger(engine());
        l.steps.get_mut("visual-escape").unwrap()["pty_hex"] = json!("1b");
        assert_eq!(
            failing(&report(), &l),
            vec!["escape_closes_and_restores_focus"]
        );
    }

    // Would catch: a missing parent answer or page error counted as a result.
    #[test]
    fn missing_parent_steps_or_page_errors_are_errors() {
        let mut l = ledger(engine());
        l.steps.remove("visual-restore");
        assert!(vf::checks(&report(), &l)
            .unwrap_err()
            .contains("visual-restore"));
        let mut r = report();
        r["error"] = json!("boom");
        assert!(vf::checks(&r, &ledger(engine())).is_err());
    }

    #[test]
    fn engine_expectations_come_from_the_engine_lists_and_git() {
        let e = engine();
        assert_eq!(e["version"], "0.9.0");
        assert_eq!(e["channel"], "stable");
        assert_eq!(e["tabs"], 3);
        assert_eq!(e["pane_ids_in_tab"], json!(["w3:p1"]));
        assert_eq!(e["branch"], "hd010-branch");
        let outside_git = vf::engine_expectations(
            "herdr 0.9.0\n",
            &json!({ "result": { "tabs": [] } }),
            &json!({ "result": { "panes": [{ "pane_id": "w3:p1", "tab_id": "w3:t1" }] } }),
            "w3:p1",
            None,
        )
        .unwrap();
        assert_eq!(outside_git["branch"], "—");
        let preview = vf::engine_expectations(
            "herdr 0.9.1-preview.7",
            &json!({ "result": { "tabs": [] } }),
            &json!({ "result": { "panes": [{ "pane_id": "w1:p1", "tab_id": "w1:t1" }, { "pane_id": "w1:p2", "tab_id": "w1:t1" }] } }),
            "w1:p2",
            Some("feat/login"),
        )
        .unwrap();
        assert_eq!(
            (preview["channel"].as_str(), preview["branch"].as_str()),
            (Some("preview"), Some("feat/login"))
        );
        assert_eq!(preview["pane_ids_in_tab"], json!(["w1:p1", "w1:p2"]));
        assert!(vf::engine_expectations("herdr", &json!({}), &json!({}), "w1:p1", None).is_err());
    }

    // Would catch: resizing by the output size instead of the observed viewport (window decorations),
    // or commanding a scaled output.
    #[test]
    fn viewport_command_adds_the_observed_decoration_to_the_target() {
        let outputs = json!([{ "name": "HEADLESS-1", "scale": 1.0, "current_mode": { "width": 1280, "height": 720 } }]);
        assert_eq!(
            vf::viewport_command(&outputs, 1280.0, 673.0).unwrap(),
            [
                "output",
                "HEADLESS-1",
                "resolution",
                "1440x947",
                "scale",
                "1"
            ]
        );
        let scaled = json!([{ "name": "HEADLESS-1", "scale": 2.0, "current_mode": { "width": 1600, "height": 900 } }]);
        assert!(vf::viewport_command(&scaled, 800.0, 427.0).is_err());
        assert!(vf::viewport_command(&json!([]), 1280.0, 673.0).is_err());
        assert_eq!(
            vf::wtype_args("ctrl+k").unwrap(),
            ["-M", "ctrl", "-k", "k", "-m", "ctrl"]
        );
        assert!(vf::wtype_args("ctrl+c").is_err());
    }

    // Would catch: agents read from another field than the engine's `agent list` result, projects
    // from another place than the catalog's `projects[].id`, or the agent started in the pane whose
    // PTY the phase measures.
    #[test]
    fn parent_reads_agent_ids_catalog_ids_and_the_agent_pane() {
        assert_eq!(
            vf::engine_agent_ids(&agent_list(&["w2:p1", "w1:p1"])).unwrap(),
            ["agent:w1:p1", "agent:w2:p1"]
        );
        assert_eq!(
            vf::engine_agent_ids(&agent_list(&[])).unwrap(),
            Vec::<String>::new()
        );
        assert!(vf::engine_agent_ids(&json!({ "result": { "panes": [] } })).is_err());
        assert_eq!(
            vf::catalog_project_ids(&catalog(&["p2", "p1"])).unwrap(),
            ["project:p1", "project:p2"]
        );
        assert!(vf::catalog_project_ids(&json!({ "version": 1 })).is_err());
        let panes = json!({ "result": { "panes": [
            { "pane_id": "w3:p1", "tab_id": "w3:t1" }, { "pane_id": "w1:p1", "tab_id": "w1:t1" }
        ] } });
        assert_eq!(vf::agent_host_pane(&panes, "w3:p1").unwrap(), "w1:p1");
        assert!(vf::agent_host_pane(
            &json!({ "result": { "panes": [{ "pane_id": "w3:p1", "tab_id": "w3:t1" }] } }),
            "w3:p1"
        )
        .is_err());
        assert_eq!(vf::FIXTURE_BRANCH, "hd010-branch");
        assert_ne!(vf::FIXTURE_BRANCH, "—");
    }

    #[test]
    fn contrast_matches_the_guide() {
        let r = |fg: &str, bg: &str| (vf::contrast(fg, bg, 1.0).unwrap() * 1000.0).round() / 1000.0;
        assert_eq!(r("#5B6272", "#0B0C10"), 3.197);
        assert_eq!(r("#8C93A3", "#1E222B"), 5.169);
        assert_eq!(r("rgb(140, 147, 163)", "rgba(11, 12, 16, 1)"), 6.346);
        assert!(vf::contrast("rgb(140, 147, 163)", "rgb(11, 12, 16)", 0.55).unwrap() < 4.5);
    }
}

// ---------------------------------------------------------------------------------------
// Spec 014 — visual-agents evaluator and parent helpers (no display, engine or GUI)
// ---------------------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod visual_agents_eval {
    use super::*;
    use support::visual_agents::{self as va, Ledger};

    const AGENT_PANE: &str = "w1:p2";
    const CONFIRMED: &str = "w1:p1";
    const LINE: &str = "> hd014 aprovar migracao 0042?";

    fn small(text: &str) -> Value {
        json!({ "text": text, "color": "rgb(140, 147, 163)", "background": "rgba(23, 26, 33, 1)",
                "opacity": 1, "font_size": 11, "font_weight": 400, "visibility": "visible" })
    }

    /// `herdr agent list`: the fixture agent waiting for the user plus one working agent.
    fn agent_list() -> Value {
        json!({ "id": "cli:agent:list", "result": { "type": "agent_list", "agents": [
            { "pane_id": CONFIRMED, "workspace_id": "w1", "tab_id": "w1:t1", "name": "claude",
              "agent": "claude", "agent_status": "working", "terminal_title_stripped": "Refatorando faturamento",
              "state_change_seq": 4, "focused": true, "revision": 2 },
            { "pane_id": AGENT_PANE, "workspace_id": "w1", "tab_id": "w1:t2", "name": "pi",
              "agent": "pi", "agent_status": "blocked", "state_change_seq": 9, "focused": false, "revision": 3 },
        ] } })
    }

    fn tab_list() -> Value {
        json!({ "id": "cli:tab:list", "result": { "type": "tab_list", "tabs": [
            { "tab_id": "w1:t1", "workspace_id": "w1", "label": "api", "focused": true },
            { "tab_id": "w1:t2", "workspace_id": "w1", "label": "", "focused": false },
        ] } })
    }

    fn catalog() -> Value {
        json!({ "version": 1, "projects": [
            { "id": "p-erp", "label": "erp-api", "endpoint_profile_id": "local", "session_name": "hd007-x",
              "root": "/tmp/work" },
        ], "collections": [] })
    }

    /// `herdr workspace list`: the workspace the window tagged with the project UUID.
    fn workspace_list() -> Value {
        json!({ "id": "cli:workspace:list", "result": { "type": "workspace_list", "workspaces": [
            { "workspace_id": "w1", "label": "erp-api",
              "tokens": { va::PROJECT_TOKEN: "p-erp" } },
        ] } })
    }

    fn detection(line: &str) -> Value {
        json!({ "id": "cli:agent:read", "result": { "type": "pane_read", "read": {
            "pane_id": AGENT_PANE, "source": "detection", "format": "text",
            "text": format!("{line}\n"), "revision": 3, "truncated": true } } })
    }

    fn ledger() -> Ledger {
        ledger_with(agent_list(), detection(LINE), json!(LINE))
    }

    fn ledger_with(agents: Value, read: Value, after: Value) -> Ledger {
        let mut ledger = Ledger::default();
        for step in va::STEPS {
            let mut answer = json!({ "step": step, "pane_id": CONFIRMED });
            match step {
                "visual-agents-arm" => {
                    answer["agent_pane"] = json!(AGENT_PANE);
                    answer["nonce"] = json!(va::NONCE);
                    answer["detection"] = read.clone();
                    answer["agent_list"] = agents.clone();
                    answer["project_catalog"] = catalog();
                }
                "visual-agents-block" => {
                    answer["reported_at_ms"] = json!(1_700_000_000_000.0_f64);
                    answer["agent_list"] = agents.clone();
                    answer["tab_list"] = tab_list();
                    answer["workspace_list"] = workspace_list();
                    answer["detection"] = read.clone();
                }
                "visual-agents-focus" => {
                    answer["focused_pane"] = json!(AGENT_PANE);
                    answer["agent_status_after"] = json!("blocked");
                    answer["detection_after"] = after.clone();
                }
                _ => {}
            }
            ledger.record(step, answer).unwrap();
        }
        ledger
    }

    fn report() -> Value {
        json!({
            "phase": va::PHASE,
            "error": null,
            "agent_pane": AGENT_PANE,
            "counters": { "active": 1, "waiting": 1, "idle": 0, "unknown": 0 },
            "counter_labels": va::COUNTER_LABELS,
            "card_seen_at_ms": 1_700_000_000_180.0_f64,
            "attention": { "cards": [ { "pane": AGENT_PANE, "name": "pi", "path": "erp-api › w1:t2",
                "time": va::FRESH_TIME, "status": "blocked", "label": "Aguardando você",
                "last_line": LINE, "disabled": false } ], "overflow": null },
            "enter": { "keys": [ { "trusted": true, "key": "Enter", "card": AGENT_PANE } ],
                       "activations": 1, "focused": true },
            "running": [
                { "pane": AGENT_PANE, "name": "pi", "path": "erp-api › w1:t2", "summary": "Aguardando você",
                  "time": va::FRESH_TIME, "status": "blocked", "label": "Aguardando você" },
                { "pane": CONFIRMED, "name": "claude", "path": "erp-api › api", "summary": "Refatorando faturamento",
                  "time": "3 min", "status": "working", "label": "Trabalhando" },
            ],
            "layout": { "agents_width_before": 256.0, "center_width_before": 604.0,
                        "agents_present_after": false, "center_width_after": 860.0,
                        "collapse_label": "Recolher painel de agentes" },
            "small_texts": [
                small("ativos"), small("esperando"), small("ociosos"),
                small("✋ Aguardando você"), small(LINE), small("erp-api › api"),
            ],
        })
    }

    fn failing(report: &Value, ledger: &Ledger) -> Vec<&'static str> {
        va::checks(report, ledger)
            .unwrap()
            .into_iter()
            .filter(|(_, ok)| !ok)
            .map(|(name, _)| name)
            .collect()
    }

    // Would catch: the plan and the evaluator naming different checks, the phase not driven, or
    // the page's steps drifting from the parent's.
    #[test]
    fn plan_checks_steps_and_the_page_match_the_evaluator() {
        let spec = FLOW.iter().find(|p| p.name == va::PHASE).unwrap();
        assert_eq!(spec.checks, va::CHECKS);
        assert!(DRIVEN_PHASES.contains(&va::PHASE));
        let names: Vec<&str> = va::checks(&report(), &ledger())
            .unwrap()
            .iter()
            .map(|c| c.0)
            .collect();
        assert_eq!(names, va::CHECKS);
        let page = read(&repo().join("src/features/fidelity/visual-agents.ts"));
        let steps = format!("[{}]", va::STEPS.map(|s| format!("\"{s}\"")).join(", "));
        assert!(
            page.contains(&format!("AGENTS_STEPS = {steps} as const")),
            "page steps drifted"
        );
        // The panel's state texts are the ones the product publishes for the engine states —
        // since spec 067 that is the one i18n table, read by the panel, the center, the palette
        // and the sidebar (the window of this phase runs with `HERDR_DESKTOP_LOCALE=pt`).
        let status = read(&repo().join("src/i18n/areas/core.ts"));
        for (state, label) in va::STATE_LABELS {
            assert!(
                status.contains(&format!("\"agent.status.{state}\": \"{label}\"")),
                "{state}: {label} is not a product label"
            );
        }
        assert!(failing(&report(), &ledger()).is_empty());
    }

    // Would catch: counters, times or paths computed from the page's own report instead of the
    // engine's answers, and any check that cannot fail.
    #[test]
    fn each_wrong_panel_fails_its_own_check() {
        let cases: Vec<(&str, Value, Vec<&'static str>)> = vec![
            (
                "counter of another tally",
                {
                    let mut r = report();
                    r["counters"]["idle"] = json!(1);
                    r
                },
                vec![va::CHECKS[0]],
            ),
            (
                "waiting counter not from the engine",
                {
                    let mut r = report();
                    r["counters"]["waiting"] = json!(2);
                    r
                },
                vec![va::CHECKS[0]],
            ),
            (
                "counter labels of another design",
                {
                    let mut r = report();
                    r["counter_labels"] = json!(["ativos", "bloqueados", "ociosos"]);
                    r
                },
                vec![va::CHECKS[0]],
            ),
            (
                "card later than the deadline",
                {
                    let mut r = report();
                    r["card_seen_at_ms"] = json!(1_700_000_000_501.0_f64);
                    r
                },
                vec![va::CHECKS[1]],
            ),
            (
                "card seen before the engine changed",
                {
                    let mut r = report();
                    r["card_seen_at_ms"] = json!(1_699_999_999_000.0_f64);
                    r
                },
                vec![va::CHECKS[1]],
            ),
            (
                "card with another project › tab",
                {
                    let mut r = report();
                    r["attention"]["cards"][0]["path"] = json!("w1 › w1:t2");
                    r
                },
                vec![va::CHECKS[2]],
            ),
            (
                "card without the engine line",
                {
                    let mut r = report();
                    r["attention"]["cards"][0]["last_line"] = Value::Null;
                    r
                },
                vec![va::CHECKS[2]],
            ),
            (
                "card with a line the engine never sent",
                {
                    let mut r = report();
                    r["attention"]["cards"][0]["last_line"] = json!("Aprovar?");
                    r
                },
                vec![va::CHECKS[2]],
            ),
            (
                "card time invented",
                {
                    let mut r = report();
                    r["attention"]["cards"][0]["time"] = json!("—");
                    r
                },
                vec![va::CHECKS[2]],
            ),
            (
                "card state as done",
                {
                    let mut r = report();
                    r["attention"]["cards"][0]["status"] = json!("done");
                    r["attention"]["cards"][0]["label"] = json!("Concluído");
                    r
                },
                vec![va::CHECKS[2]],
            ),
            (
                "focus disabled on a live host",
                {
                    let mut r = report();
                    r["attention"]["cards"][0]["disabled"] = json!(true);
                    r
                },
                vec![va::CHECKS[2]],
            ),
            (
                "key not trusted",
                {
                    let mut r = report();
                    r["enter"]["keys"][0]["trusted"] = json!(false);
                    r
                },
                vec![va::CHECKS[3]],
            ),
            (
                "two activations of the same card",
                {
                    let mut r = report();
                    r["enter"]["activations"] = json!(2);
                    r
                },
                vec![va::CHECKS[3]],
            ),
            (
                "no key on the card",
                {
                    let mut r = report();
                    r["enter"]["keys"] = json!([]);
                    r
                },
                vec![va::CHECKS[3]],
            ),
            (
                "rows in engine list order",
                {
                    let mut r = report();
                    let rows = r["running"].as_array().unwrap().clone();
                    r["running"] = json!([rows[1].clone(), rows[0].clone()]);
                    r
                },
                vec![va::CHECKS[4]],
            ),
            (
                "row summary invented",
                {
                    let mut r = report();
                    r["running"][1]["summary"] = json!("Trabalhando duro");
                    r
                },
                vec![va::CHECKS[4]],
            ),
            (
                "an engine agent missing from the rows",
                {
                    let mut r = report();
                    r["running"] = json!([r["running"][0].clone()]);
                    r
                },
                vec![va::CHECKS[4]],
            ),
            (
                "unknown row shown as done",
                {
                    let mut r = report();
                    r["running"][1]["status"] = json!("done");
                    r["running"][1]["label"] = json!("Concluído");
                    r
                },
                vec![va::CHECKS[4]],
            ),
            (
                "agents column of another width",
                {
                    let mut r = report();
                    r["layout"]["agents_width_before"] = json!(272.0);
                    r
                },
                vec![va::CHECKS[5]],
            ),
            (
                "center that did not grow by the column",
                {
                    let mut r = report();
                    r["layout"]["center_width_after"] = json!(700.0);
                    r
                },
                vec![va::CHECKS[5]],
            ),
            (
                "panel still mounted after collapsing",
                {
                    let mut r = report();
                    r["layout"]["agents_present_after"] = json!(true);
                    r
                },
                vec![va::CHECKS[5]],
            ),
            (
                "collapse control without an accessible name",
                {
                    let mut r = report();
                    r["layout"]["collapse_label"] = json!("  ");
                    r
                },
                vec![va::CHECKS[5]],
            ),
            (
                "small text below 4.5:1",
                {
                    let mut r = report();
                    r["small_texts"][0]["color"] = json!("rgb(91, 98, 114)");
                    r
                },
                vec![va::CHECKS[6]],
            ),
            (
                "counter label never measured",
                {
                    let mut r = report();
                    r["small_texts"] = json!([
                        small("ativos"),
                        small("esperando"),
                        small(LINE),
                        small("✋ Aguardando você")
                    ]);
                    r
                },
                vec![va::CHECKS[6]],
            ),
        ];
        for (name, bad, expected) in cases {
            assert_eq!(failing(&bad, &ledger()), expected, "case: {name}");
        }

        // The engine's own answers move the expectations: another focused pane, a changed screen
        // (a key that reached the agent) or a state that is no longer waiting all fail.
        let mut wrong_focus = ledger_with(agent_list(), detection(LINE), json!(LINE));
        wrong_focus.steps.get_mut("visual-agents-focus").unwrap()["focused_pane"] =
            json!(CONFIRMED);
        assert_eq!(failing(&report(), &wrong_focus), vec![va::CHECKS[3]]);
        let changed = ledger_with(agent_list(), detection(LINE), json!("trabalhando: hd014"));
        assert_eq!(failing(&report(), &changed), vec![va::CHECKS[3]]);
        let mut left = ledger_with(agent_list(), detection(LINE), json!(LINE));
        left.steps.get_mut("visual-agents-focus").unwrap()["agent_status_after"] = json!("idle");
        assert_eq!(failing(&report(), &left), vec![va::CHECKS[3]]);
        // A line without the nonce is not the screen this phase armed.
        let foreign = ledger_with(agent_list(), detection("Aprovar?"), json!("Aprovar?"));
        let mut foreign_report = report();
        foreign_report["attention"]["cards"][0]["last_line"] = json!("Aprovar?");
        foreign_report["small_texts"] = json!([
            small("ativos"),
            small("esperando"),
            small("ociosos"),
            small("✋ Aguardando você"),
            small("Aprovar?")
        ]);
        assert_eq!(failing(&foreign_report, &foreign), vec![va::CHECKS[2]]);
    }

    // Would catch: a missing parent step, a page error or a page report of another pane passing.
    #[test]
    fn missing_parent_steps_page_errors_and_other_panes_are_errors() {
        let mut r = report();
        r["error"] = json!("boom");
        assert!(va::checks(&r, &ledger()).is_err());
        let mut other = report();
        other["phase"] = json!("visual-frame");
        assert!(va::checks(&other, &ledger()).is_err());
        let mut pane = report();
        pane["agent_pane"] = json!("w9:p9");
        assert!(va::checks(&pane, &ledger()).is_err());
        for step in va::STEPS {
            let mut short = ledger();
            short.steps.remove(step);
            assert!(
                va::checks(&report(), &short)
                    .unwrap_err()
                    .contains("never answered"),
                "{step} was not required"
            );
        }
        // No detection line at all is an error, never a pass.
        let empty = ledger_with(agent_list(), detection(""), json!(null));
        assert!(va::checks(&report(), &empty).is_err());
    }

    // Would catch: parent helpers reading the wrong engine field (states, titles, tabs, project
    // bindings), or a key other than Enter being allowed in this phase.
    #[test]
    fn parent_helpers_read_the_engine_and_allow_only_enter() {
        assert_eq!(
            va::engine_counters(&agent_list()).unwrap(),
            json!({ "active": 1, "waiting": 1, "idle": 0, "unknown": 0 })
        );
        let odd = json!({ "result": { "agents": [
            { "pane_id": "w1:p1", "agent_status": "finished" },
            { "pane_id": "w1:p2", "agent_status": "done" },
            { "pane_id": "w1:p3", "agent_status": "idle" },
        ] } });
        assert_eq!(
            va::engine_counters(&odd).unwrap(),
            json!({ "active": 0, "waiting": 0, "idle": 2, "unknown": 1 }),
            "a state the engine never published is unknown, never done"
        );
        assert_eq!(
            va::agent_pane(&agent_list(), CONFIRMED).unwrap(),
            AGENT_PANE
        );
        assert!(va::agent_pane(&agent_list(), "w1:p3").is_ok());
        let single = json!({ "result": { "agents": [{ "pane_id": CONFIRMED }] } });
        assert!(va::agent_pane(&single, CONFIRMED).is_err());
        let listed = agent_list();
        let agent = va::engine_agent(&listed, AGENT_PANE).unwrap();
        assert!(va::engine_agent(&listed, "w9:p9").is_err());
        assert_eq!(
            va::engine_path(agent, &tab_list(), &workspace_list(), &catalog()).unwrap(),
            "erp-api › w1:t2",
            "a tab the engine left unlabeled shows its id"
        );
        let working = va::engine_agent(&listed, CONFIRMED).unwrap();
        assert_eq!(
            va::engine_path(working, &tab_list(), &workspace_list(), &catalog()).unwrap(),
            "erp-api › api"
        );
        assert_eq!(
            va::engine_path(
                working,
                &tab_list(),
                &workspace_list(),
                &json!({ "projects": [] })
            )
            .unwrap(),
            "w1 › api",
            "a project the catalog does not have shows the workspace id"
        );
        assert_eq!(
            va::engine_path(
                working,
                &tab_list(),
                &json!({ "result": { "workspaces": [{ "workspace_id": "w1" }] } }),
                &catalog()
            )
            .unwrap(),
            "w1 › api",
            "a workspace the window never tagged shows its id"
        );
        let row = va::engine_row(working, &tab_list(), &workspace_list(), &catalog()).unwrap();
        assert_eq!(row["summary"], "Refatorando faturamento");
        assert_eq!(row["label"], "Trabalhando");
        let waiting = va::engine_row(agent, &tab_list(), &workspace_list(), &catalog()).unwrap();
        assert_eq!(
            waiting["summary"], "Aguardando você",
            "without a terminal title the row falls back to the state text"
        );
        assert_eq!(
            va::last_snapshot_line(&detection("  linha  ")).as_deref(),
            Some("  linha")
        );
        assert_eq!(va::last_snapshot_line(&detection("")), None);
        assert_eq!(va::wtype_args("Return").unwrap(), vec!["-k", "Return"]);
        assert!(va::wtype_args("y").is_err());
        assert_eq!(va::AGENTS_WIDTH, 256.0);
        assert_eq!(va::ATTENTION_DEADLINE_MS, 500.0);
    }
}

// Spec 015 — visual-files evaluator, parent helpers and plan drift (no display, engine or GUI)
// ---------------------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod visual_files_eval {
    use super::*;
    use support::visual_files::{self as vf, Ledger};

    // Keys without the leading slash, as `ChunkMap::modules` looks them up.
    const CHUNK_MAP: &str = r#"{"version":1,"chunks":{
        "assets/main.js":["src/main.ts","src/components/files/review.ts"],
        "assets/editor.js":["src/editor/editor.ts","node_modules/@codemirror/view/index.js"]}}"#;

    fn probe(at_ms: f64, editor_dom: bool) -> Value {
        json!({ "at_ms": at_ms, "scripts": ["/assets/main.js", "/assets/editor.js"], "editor_dom": editor_dom })
    }

    #[allow(clippy::too_many_arguments)]
    fn cell(
        row: u64,
        side: &str,
        op: &str,
        line: u64,
        text: &str,
        mark: &str,
        top: u64,
        background: &str,
    ) -> Value {
        json!({
            "row": row, "side": side, "op": op, "line": line, "text": text, "mark": mark,
            "top": top, "height": 18, "background": background, "color": "rgb(242, 119, 122)",
        })
    }

    const REMOVED_BG: &str = "rgba(242, 119, 122, 0.15)";
    const ADDED_BG: &str = "rgba(91, 214, 138, 0.15)";

    /// Cells of the ORIGINAL → BUFFER diff (five rows) as the page must report them.
    fn local_cells() -> Vec<Value> {
        vec![
            cell(
                1,
                "base",
                "removed",
                1,
                "// spec 007 lazy-editor positive control",
                "-",
                100,
                REMOVED_BG,
            ),
            cell(
                1,
                "current",
                "added",
                1,
                "// spec 015 review",
                "+",
                100,
                ADDED_BG,
            ),
            cell(
                2,
                "base",
                "same",
                2,
                "fn main() {",
                " ",
                118,
                "rgba(0, 0, 0, 0)",
            ),
            cell(
                2,
                "current",
                "same",
                2,
                "fn main() {",
                " ",
                118,
                "rgba(0, 0, 0, 0)",
            ),
            cell(
                3,
                "base",
                "removed",
                3,
                "    println!(\"fidelidade\");",
                "-",
                136,
                REMOVED_BG,
            ),
            cell(
                3,
                "current",
                "added",
                3,
                "    println!(\"revisao 015\");",
                "+",
                136,
                ADDED_BG,
            ),
            cell(
                4,
                "current",
                "added",
                4,
                "    println!(\"linha nova\");",
                "+",
                154,
                ADDED_BG,
            ),
            cell(5, "base", "same", 4, "}", " ", 172, "rgba(0, 0, 0, 0)"),
            cell(5, "current", "same", 5, "}", " ", 172, "rgba(0, 0, 0, 0)"),
        ]
    }

    /// Cells of the REMOTE_A → REMOTE_B diff (four rows).
    fn remote_cells() -> Vec<Value> {
        vec![
            cell(
                1,
                "base",
                "same",
                1,
                "linha um remota",
                " ",
                100,
                "rgba(0, 0, 0, 0)",
            ),
            cell(
                1,
                "current",
                "same",
                1,
                "linha um remota",
                " ",
                100,
                "rgba(0, 0, 0, 0)",
            ),
            cell(2, "base", "removed", 2, "linha dois", "-", 118, REMOVED_BG),
            cell(
                2,
                "current",
                "added",
                2,
                "linha dois ALTERADA",
                "+",
                118,
                ADDED_BG,
            ),
            cell(
                3,
                "base",
                "same",
                3,
                "linha tres",
                " ",
                136,
                "rgba(0, 0, 0, 0)",
            ),
            cell(
                3,
                "current",
                "same",
                3,
                "linha tres",
                " ",
                136,
                "rgba(0, 0, 0, 0)",
            ),
            cell(4, "current", "added", 4, "linha quatro", "+", 154, ADDED_BG),
        ]
    }

    fn small(scope: &str, text: &str, size: f64) -> Value {
        json!({ "scope": scope, "text": text, "color": "rgb(140, 147, 163)", "background": "rgba(17, 19, 24, 1)",
                "opacity": 1, "font_size": size, "font_weight": 400, "visibility": "visible" })
    }

    fn report() -> Value {
        json!({
            "phase": "visual-files", "error": null,
            "chunk_map": CHUNK_MAP,
            "probes": { "mount": probe(1.0, false), "opened": probe(2.0, true), "diff": probe(3.0, false) },
            "mount": { "tabs": [], "empty_text": "Abra um arquivo. Nenhum arquivo aberto: …", "open_file_label": "+ Abrir arquivo", "probe": probe(1.0, false) },
            "open_focus": "fidelity_fixture.rs",
            "buffer_applied": vf::BUFFER,
            "review": {
                "title": "Revisar alterações",
                "breadcrumb": "work / fidelity_fixture.rs",
                "tabs": [
                    { "kind": "file", "label": "fidelity_fixture.rs", "active": false },
                    { "kind": "diff", "label": "fidelity_fixture.rs · diff", "active": true }
                ],
                "ordered": ["fidelity_fixture.rs", "fidelity_fixture.rs · diff", "+ Abrir arquivo"],
                "diff": {
                    "base": "original",
                    "sources": "conteúdo-base 11111111 → buffer atual",
                    "summary": "+3 −2",
                    "headings": { "base": "Snapshot anterior", "current": "Versão atual" },
                    "cells": local_cells(),
                },
                "editor_dom": false,
                "dock": { "rect": { "left": 0, "top": 0, "width": 900, "height": 190 }, "label": "TERMINAL · pi / Fidelidade Local" },
                "small_texts": [ small("review", "Revisar alterações", 20.0), small("review", "Snapshot anterior", 11.0), small("dock", "TERMINAL · pi / Fidelidade Local", 11.0) ],
            },
            "remote": {
                "banner": { "text": "SSH A · SSH · Somente leitura · Comparação entre snapshots", "background": "rgba(143, 168, 255, 0.122)", "color": "rgb(231, 233, 238)" },
                "breadcrumb": "remote-root / notas.txt",
                "tabs": [
                    { "kind": "file", "label": "notas.txt", "active": false },
                    { "kind": "diff", "label": "notas.txt · diff", "active": true }
                ],
                "ordered": ["notas.txt", "notas.txt · diff", "+ Abrir arquivo"],
                "diff": {
                    "base": null,
                    "sources": "leitura aaaaaaaa · geração 2 → leitura bbbbbbbb · geração 2",
                    "summary": "+2 −1",
                    "headings": { "base": "Snapshot anterior", "current": "Versão atual" },
                    "cells": remote_cells(),
                },
                "editor_dom": false,
                "small_texts": [ small("remote-review", "SSH A · SSH · Somente leitura · Comparação entre snapshots", 12.0) ],
                "read_only": { "contenteditable": "false", "text_before": "linha um remota", "text_after": "linha um remota", "typed_ignored": true, "save_buttons": 0 },
            },
            "closed": { "present": false, "dock_height": 190, "dock_band": 202, "diff_before": 200, "diff_after": 402, "workspace": true },
            "dock": {
                "focused": true,
                "frames": {
                    "before": { "raf_requests": 4, "raf_callbacks": 4, "paint_calls": 2, "painted_rows": 12, "full_frames": 1 },
                    "after": { "raf_requests": 6, "raf_callbacks": 6, "paint_calls": 3, "painted_rows": 20, "full_frames": 1 },
                },
            },
        })
    }

    fn ledger() -> Ledger {
        let mut ledger = Ledger::default();
        let mut record = |step: &str, answer: Value| ledger.record(step, answer).unwrap();
        record(
            "visual-files-viewport",
            json!({ "step": "visual-files-viewport", "command": ["output", "HEADLESS-1", "resolution", "1500x900"] }),
        );
        record(
            "visual-files-local",
            json!({
                "step": "visual-files-local", "file": "fidelity_fixture.rs", "root": "/var/tmp/hd007/work",
                "original": vf::ORIGINAL, "buffer": vf::BUFFER,
                "removed": vf::only_in(vf::ORIGINAL, vf::BUFFER), "added": vf::only_in(vf::BUFFER, vf::ORIGINAL),
            }),
        );
        record(
            "visual-files-engine",
            json!({ "step": "visual-files-engine", "agent": "pi", "expected_label": "TERMINAL · pi / Fidelidade Local",
                    "engine_agent_list": json!({ "result": { "agents": [{ "pane_id": "w1:p1", "agent": "pi", "focused": false }] } }) }),
        );
        record(
            "visual-files-keys",
            json!({ "step": "visual-files-keys", "items": [
                { "id": "accents-keysym", "expected_hex": "61c3a7c3a36f20c3a920c3bc", "observed_hex": "61c3a7c3a36f20c3a920c3bc" },
                { "id": "emoji-keysym", "expected_hex": "f09f9982", "observed_hex": "f09f9982" },
            ], "capture_pids_stopped": [4242] }),
        );
        record(
            "visual-files-frames",
            json!({ "step": "visual-files-frames", "marker": "visual-files-marker-015", "pane_read": "prompt $ echo visual-files-marker-015\nvisual-files-marker-015" }),
        );
        record(
            "visual-files-remote-before",
            json!({ "step": "visual-files-remote-before", "root": "/var/tmp/hd007/remote-root", "banner_label": "SSH A", "content_a": vf::REMOTE_A }),
        );
        record(
            "visual-files-remote-change",
            json!({ "step": "visual-files-remote-change", "root": "/var/tmp/hd007/remote-root", "banner_label": "SSH A",
                    "content_a": vf::REMOTE_A, "content_b": vf::REMOTE_B }),
        );
        record(
            "visual-files-evidence",
            json!({ "step": "visual-files-evidence", "screenshot": "/tmp/x.png" }),
        );
        record(
            "visual-files-restore",
            json!({ "step": "visual-files-restore", "command": ["output", "HEADLESS-1", "resolution", "1280x720"] }),
        );
        ledger
    }

    fn failing(report: &Value, ledger: &Ledger) -> Vec<&'static str> {
        vf::checks(report, ledger)
            .unwrap()
            .into_iter()
            .filter(|(_, ok)| !ok)
            .map(|(name, _)| name)
            .collect()
    }

    // Would catch: plan and evaluator naming different checks, the phase not driven, the page's
    // steps drifting from the parent's, or the review pulling the editor in.
    #[test]
    fn plan_checks_steps_and_sources_match_the_page_and_the_evaluator() {
        let spec = FLOW.iter().find(|p| p.name == vf::PHASE).unwrap();
        assert_eq!(spec.checks, vf::CHECKS);
        assert!(DRIVEN_PHASES.contains(&vf::PHASE));
        let names: Vec<&str> = vf::checks(&report(), &ledger())
            .unwrap()
            .iter()
            .map(|c| c.0)
            .collect();
        assert_eq!(names, vf::CHECKS);
        let page = read(&repo().join("src/features/fidelity/visual-files.ts"));
        let steps = format!(
            "VISUAL_FILES_STEPS = [\n{}] as const;",
            vf::STEPS.map(|s| format!("  \"{s}\",\n")).join("")
        );
        assert!(page.contains(&steps), "page steps drifted");
        assert!(page.contains("FIXTURE_FILE = \"fidelity_fixture.rs\""));
        let e2e = read(&repo().join("src/features/fidelity/e2e.ts"));
        assert!(e2e.contains("runVisualFiles(domVisualFilesPage(root), awaitParent, params)"));
        for name in [
            "ReviewHeader.svelte",
            "FileTabs.svelte",
            "SideBySideDiff.svelte",
            "TerminalDock.svelte",
            "review.ts",
        ] {
            assert!(!e2e.contains(name), "{name} must stay out of the flow page");
        }
        for file in [
            "ReviewHeader.svelte",
            "FileTabs.svelte",
            "SideBySideDiff.svelte",
            "TerminalDock.svelte",
            "FilesRegion.svelte",
            "review.ts",
        ] {
            let source = read(&repo().join("src/components/files").join(file));
            for forbidden in ["@codemirror", "src/editor", "../editor/"] {
                assert!(
                    !source.contains(forbidden),
                    "{file} must not import {forbidden}"
                );
            }
        }
        let workspace = read(&repo().join("src/components/FilesWorkspace.svelte"));
        assert_eq!(
            workspace.matches("import(\"../editor/editor\")").count(),
            1,
            "the editor stays a single dynamic import"
        );
    }

    #[test]
    fn every_check_passes_on_the_reviewed_fixture() {
        assert_eq!(failing(&report(), &ledger()), Vec::<&'static str>::new());
    }

    // Would catch: renamed title/order, a missing `arquivo · diff` tab, the wrong breadcrumb or
    // `+ Abrir arquivo` not landing on the file entry.
    #[test]
    fn title_breadcrumb_tabs_and_open_file_are_required() {
        for mutate in [
            |r: &mut Value| r["review"]["title"] = json!("Revisar"),
            |r: &mut Value| r["review"]["breadcrumb"] = json!("work / notas.txt"),
            |r: &mut Value| {
                r["review"]["ordered"] = json!(["fidelity_fixture.rs", "+ Abrir arquivo"])
            },
            |r: &mut Value| r["open_focus"] = json!("sub"),
            |r: &mut Value| r["mount"]["empty_text"] = json!("Editor vazio"),
            |r: &mut Value| {
                r["mount"]["tabs"] =
                    json!([{ "kind": "file", "label": "fidelity_fixture.rs", "active": true }])
            },
        ] {
            let mut report = report();
            mutate(&mut report);
            assert_eq!(
                failing(&report, &ledger()),
                vec!["review_title_breadcrumb_and_tabs"]
            );
        }
        assert!(fails_without_ledger("visual-files-local").contains("visual-files-local"));
        assert!(fails_without_ledger("visual-files-frames").contains("visual-files-frames"));
    }

    // Would catch: the editor mounted in the empty review or in the diff view, or a new editor
    // module loaded by the review (the spec 005 probe kept as a set delta).
    #[test]
    fn the_editor_stays_lazy_before_the_file_is_opened() {
        for mutate in [
            |r: &mut Value| r["probes"]["mount"]["editor_dom"] = json!(true),
            |r: &mut Value| r["probes"]["diff"]["editor_dom"] = json!(true),
            |r: &mut Value| r["probes"]["opened"]["editor_dom"] = json!(false),
            |r: &mut Value| r["review"]["editor_dom"] = json!(true),
            |r: &mut Value| r["buffer_applied"] = json!("outro texto"),
        ] {
            let mut report = report();
            mutate(&mut report);
            assert_eq!(
                failing(&report, &ledger()),
                vec!["editor_lazy_before_open_in_review"]
            );
        }
        let mut report = report();
        report["probes"]["diff"]["scripts"] = json!([
            "/assets/main.js",
            "/assets/editor.js",
            "/assets/lang-python.js"
        ]);
        let map = support::chunks::ChunkMap::parse(CHUNK_MAP).unwrap();
        let mut chunks: serde_json::Map<String, Value> = map_modules(&map);
        chunks.insert(
            "assets/lang-python.js".into(),
            json!(["node_modules/@codemirror/lang-python/index.js"]),
        );
        report["chunk_map"] =
            json!(serde_json::to_string(&json!({ "version": 1, "chunks": chunks })).unwrap());
        assert_eq!(
            failing(&report, &ledger()),
            vec!["editor_lazy_before_open_in_review"]
        );
    }

    fn map_modules(map: &support::chunks::ChunkMap) -> serde_json::Map<String, Value> {
        let mut chunks = serde_json::Map::new();
        for script in ["assets/main.js", "assets/editor.js"] {
            let modules = map.modules(&[script.to_owned()]).unwrap();
            chunks.insert(script.into(), json!(modules));
        }
        chunks
    }

    fn fails_without_ledger(step: &str) -> String {
        let mut ledger = ledger();
        ledger.steps.remove(step);
        vf::checks(&report(), &ledger).unwrap_err()
    }

    // Would catch: headers swapped, a wrong removed/added set, or a missing `-`/`+` prefix.
    #[test]
    fn side_by_side_headers_prefixes_and_sets_are_required() {
        for mutate in [
            |r: &mut Value| r["review"]["diff"]["headings"]["base"] = json!("Versão atual"),
            |r: &mut Value| r["review"]["diff"]["headings"]["current"] = json!("Snapshot anterior"),
            |r: &mut Value| {
                r["review"]["diff"]["cells"][0]["text"] = json!("    println!(\"fidelidade\");")
            },
            |r: &mut Value| r["review"]["diff"]["cells"][1]["mark"] = json!("-"),
            |r: &mut Value| r["review"]["diff"]["cells"][2]["op"] = json!("removed"),
            |r: &mut Value| r["review"]["diff"]["base"] = json!("disk"),
        ] {
            let mut report = report();
            mutate(&mut report);
            let failed = failing(&report, &ledger());
            assert!(
                failed.contains(&"side_by_side_headers_prefixes_and_sets")
                    || failed.contains(&"diff_line_numbers_match_their_sides"),
                "{failed:?}"
            );
        }
    }

    // Would catch: numbering taken from the wrong side, an off-by-one or rows not aligned.
    #[test]
    fn line_numbers_must_match_their_side() {
        for mutate in [
            |r: &mut Value| r["review"]["diff"]["cells"][0]["line"] = json!(9),
            |r: &mut Value| r["review"]["diff"]["cells"][4]["line"] = json!(2),
            |r: &mut Value| r["review"]["diff"]["cells"][6]["line"] = json!(5),
            |r: &mut Value| r["review"]["diff"]["cells"][3]["top"] = json!(140),
            |r: &mut Value| r["review"]["diff"]["cells"][2]["row"] = json!(0),
        ] {
            let mut report = report();
            mutate(&mut report);
            assert_eq!(
                failing(&report, &ledger()),
                vec!["diff_line_numbers_match_their_sides"]
            );
        }
    }

    // Would catch: a tint other than error/working at 15 %, or an opaque background.
    #[test]
    fn removed_and_added_backgrounds_are_error_and_working_at_15_percent() {
        for mutate in [
            |r: &mut Value| {
                r["review"]["diff"]["cells"][0]["background"] = json!("rgba(242, 119, 122, 0.25)")
            },
            |r: &mut Value| r["review"]["diff"]["cells"][1]["background"] = json!("#14251a"),
            |r: &mut Value| {
                r["review"]["diff"]["cells"][6]["background"] = json!("rgba(91, 214, 138, 0.5)")
            },
        ] {
            let mut report = report();
            mutate(&mut report);
            assert_eq!(
                failing(&report, &ledger()),
                vec!["diff_error_and_working_at_15_percent"]
            );
        }
    }

    // Would catch: a small text below 4,5:1 (text-dim) or an unmeasured title/heading/dock label.
    #[test]
    fn small_text_contrast_is_recomputed_from_the_tokens() {
        let mut dim = report();
        dim["review"]["small_texts"][1]["color"] = json!("rgb(91, 98, 114)");
        assert_eq!(
            failing(&dim, &ledger()),
            vec!["small_text_contrast_at_least_4_5"]
        );
        let mut invisible = report();
        invisible["review"]["small_texts"] = json!([]);
        assert_eq!(
            failing(&invisible, &ledger()),
            vec!["small_text_contrast_at_least_4_5"]
        );
        let mut hidden = report();
        hidden["review"]["small_texts"][1]["visibility"] = json!("hidden");
        assert_eq!(
            failing(&hidden, &ledger()),
            vec!["small_text_contrast_at_least_4_5"]
        );
    }

    // Would catch: a banner not in accent-soft, an editable remote editor, a save action or a
    // remote diff whose sides do not match the two snapshots.
    #[test]
    fn remote_banner_read_only_and_snapshot_diff_are_required() {
        for mutate in [
            |r: &mut Value| r["remote"]["banner"]["text"] = json!("SSH A · Somente leitura"),
            |r: &mut Value| r["remote"]["banner"]["background"] = json!("rgba(23, 26, 33, 1)"),
            |r: &mut Value| r["remote"]["read_only"]["typed_ignored"] = json!(false),
            |r: &mut Value| r["remote"]["read_only"]["contenteditable"] = json!("true"),
            |r: &mut Value| r["remote"]["read_only"]["save_buttons"] = json!(1),
            |r: &mut Value| r["remote"]["diff"]["cells"][6]["text"] = json!("linha cinco"),
            |r: &mut Value| r["remote"]["diff"]["sources"] = json!("leitura aaaaaaaa · geração 2"),
            |r: &mut Value| r["remote"]["breadcrumb"] = json!("remote-root / outro.txt"),
        ] {
            let mut report = report();
            mutate(&mut report);
            assert_eq!(
                failing(&report, &ledger()),
                vec!["remote_banner_read_only_and_snapshot_diff"]
            );
        }
    }

    // Would catch: a dock that is not 190 px, a label naming another agent/project, or a close that
    // does not hand the dock's height back to the diff.
    #[test]
    fn dock_is_190px_labelled_and_returns_its_height() {
        type Mutation = fn(&mut Value);
        let cases: [(Mutation, &str); 4] = [
            (
                |r: &mut Value| r["review"]["dock"]["rect"]["height"] = json!(210),
                "dock_height_label_and_close_returns_height",
            ),
            (
                |r: &mut Value| {
                    r["review"]["dock"]["label"] = json!("TERMINAL · claude / Fidelidade Local")
                },
                "dock_height_label_and_close_returns_height",
            ),
            (
                |r: &mut Value| r["closed"]["present"] = json!(true),
                "dock_height_label_and_close_returns_height",
            ),
            (
                |r: &mut Value| r["closed"]["diff_after"] = json!(250),
                "dock_height_label_and_close_returns_height",
            ),
        ];
        for (mutate, expected) in cases {
            let mut report = report();
            mutate(&mut report);
            assert_eq!(failing(&report, &ledger()), vec![expected]);
        }
    }

    // Would catch: a dock that never took the focus, corpus bytes missing or doubled, no capture.
    #[test]
    fn the_dock_surface_keeps_receiving_input() {
        let mut unfocused = report();
        unfocused["dock"]["focused"] = json!(false);
        assert_eq!(
            failing(&unfocused, &ledger()),
            vec!["dock_surface_keeps_input"]
        );
        let mut short = ledger();
        short.steps.remove("visual-files-keys");
        short
            .record(
                "visual-files-keys",
                json!({ "step": "visual-files-keys", "items": [
                    { "id": "accents-keysym", "expected_hex": "61c3a7c3a36f20c3a920c3bc", "observed_hex": "61" },
                ], "capture_pids_stopped": [4242] }),
            )
            .unwrap();
        assert_eq!(failing(&report(), &short), vec!["dock_surface_keeps_input"]);
        let mut doubled = ledger();
        doubled.steps.remove("visual-files-keys");
        doubled
            .record(
                "visual-files-keys",
                json!({ "step": "visual-files-keys", "items": [
                    { "id": "accents-keysym", "expected_hex": "61c3a7c3a36f20c3a920c3bc", "observed_hex": "61c3a7c3a36f20c3a920c3bc61c3a7c3a36f20c3a920c3bc" },
                ], "capture_pids_stopped": [4242] }),
            )
            .unwrap();
        assert_eq!(
            failing(&report(), &doubled),
            vec!["dock_surface_keeps_input"]
        );
    }

    // Would catch: frames not painted while the dock is visible or a marker that never reached the pane.
    #[test]
    fn the_dock_surface_paints_frames() {
        for mutate in [
            |r: &mut Value| r["dock"]["frames"]["after"]["painted_rows"] = json!(12),
            |r: &mut Value| r["dock"]["frames"]["after"]["paint_calls"] = json!(2),
        ] {
            let mut report = report();
            mutate(&mut report);
            assert_eq!(
                failing(&report, &ledger()),
                vec!["dock_surface_frames_painted"]
            );
        }
        let mut no_marker = ledger();
        no_marker.steps.remove("visual-files-frames");
        no_marker
            .record("visual-files-frames", json!({ "step": "visual-files-frames", "marker": "visual-files-marker-015", "pane_read": "prompt $ " }))
            .unwrap();
        assert_eq!(
            failing(&report(), &no_marker),
            vec!["dock_surface_frames_painted"]
        );
    }

    // Would catch: a dock label computed from the page instead of the engine's agent list.
    #[test]
    fn the_expected_dock_label_comes_from_the_engine() {
        let focused = json!({ "result": { "agents": [
            { "pane_id": "w2:p1", "agent": "pi", "focused": false },
            { "pane_id": "w1:p1", "agent": "claude", "name": "Claude", "focused": true },
        ] } });
        assert_eq!(
            vf::expected_dock_label(&focused, "erp-api").unwrap(),
            "TERMINAL · Claude / erp-api"
        );
        let idle = json!({ "result": { "agents": [{ "pane_id": "w2:p1", "agent": "pi", "focused": false }] } });
        assert_eq!(
            vf::expected_dock_label(&idle, "erp-api").unwrap(),
            "TERMINAL · pi / erp-api"
        );
        assert!(
            vf::expected_dock_label(&json!({ "result": { "agents": [] } }), "erp-api").is_err()
        );
    }
}

// Spec 013 — visual-center evaluator (no display, engine or GUI)
// ---------------------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod visual_center_eval {
    use super::*;
    use support::visual_center::{self as vc, Ledger};

    const ROOT: &str = "/tmp/hd013/work";
    const HOST: &str = "Este computador · Local";

    fn rect(left: f64, top: f64, width: f64, height: f64) -> Value {
        json!({ "left": left, "top": top, "width": width, "height": height })
    }

    /// A frame of `pane` measured exactly at `inner_rect × cell` (cell 9×19, canvas at 10,25).
    fn frame(
        pane: &str,
        inner: (f64, f64, f64, f64),
        focused: bool,
        name: &str,
        path: &str,
    ) -> Value {
        let (cw, ch) = (9.0, 19.0);
        let (ox, oy) = (10.0, 25.0);
        let (x, y, w, h) = inner;
        json!({
            "pane_id": pane,
            "rect": rect(ox + x * cw, oy + y * ch, w * cw, h * ch),
            "band": rect(ox + x * cw, oy + y * ch - ch, w * cw, ch),
            "name": name,
            "state": "Ocioso",
            "state_tone": "idle",
            "path": path,
            "edge": if focused { "accent" } else { "border" },
            "edge_color": if focused { vc::ACCENT_RGB } else { vc::BORDER_RGB },
            "edge_width": vc::EDGE_WIDTH,
            "focused": focused,
            "cache": false,
        })
    }

    fn frames() -> Vec<Value> {
        vec![
            frame(
                "w1:p1",
                (1.0, 1.0, 40.0, 20.0),
                false,
                "shell",
                "/tmp/hd013/work",
            ),
            frame(
                "w1:p2",
                (42.0, 1.0, 40.0, 20.0),
                true,
                "pi",
                "/tmp/hd013/other",
            ),
        ]
    }

    fn panes_meta() -> Value {
        json!([
            { "pane_id": "w1:p1", "inner_rect": { "x": 1, "y": 1, "width": 40, "height": 20 } },
            { "pane_id": "w1:p2", "inner_rect": { "x": 42, "y": 1, "width": 40, "height": 20 } }
        ])
    }

    fn stage(name: &str, dpr: f64) -> Value {
        json!({
            "stage": name,
            "inner_width": if name == "grow" || name == "scale2" { 1600 } else { 1280 },
            "inner_height": if name == "grow" || name == "scale2" { 900 } else { 720 },
            "dpr": dpr,
            "canvas": rect(10.0, 25.0, 83.0 * 9.0, 22.0 * 19.0),
            "stage_rect": rect(0.0, 0.0, 900.0, 500.0),
            "surface": { "cols": 83, "rows": 22 },
            "panes": panes_meta(),
            "frames": frames(),
            "toolbar": Value::Null,
            "probe": { "raf_requests": 9, "raf_callbacks": 9, "paint_calls": 9, "painted_rows": 90, "full_frames": 2 },
        })
    }

    fn report() -> Value {
        json!({
            "phase": vc::PHASE, "error": null,
            "identity": { "pane_id": "w1:p1", "generation": "3", "boot_prefix": "boot-013", "endpoint": HOST },
            "before": {
                "header": {
                    "crumbs": ["Fidelidade", "Fidelidade Local"],
                    "branch": format!("⎇ {}", vc::FIXTURE_BRANCH),
                    "path": ROOT,
                    "actions": ["split", "newTab", "newAgent"],
                    "empty": false,
                },
                "tabs": [{ "tab_id": "w1:t1", "label": "1", "panes": "1", "dot": null, "active": true }],
                "panes": ["w1:p1"],
            },
            "actions": {
                "shell_click": "[data-center-header] [data-action=\"newTab\"]",
                "plus_click": "[data-center-tabs] [data-new-tab]",
                "split_click": "[data-center-header] [data-action=\"split\"]",
                "agent_click": "[data-center-header] [data-action=\"newAgent\"]",
                "agent_pane": "w1:p2",
                "panes_after_split": ["w1:p1", "w1:p2"],
                "focus_target": "w1:p2",
                "focused_after_click": "w1:p2",
                "zoom_click": "[data-pane-band] [data-zoom]",
                "panes_zoomed": ["w1:p2"],
            },
            "seen": {
                "header": { "crumbs": ["Fidelidade", "Fidelidade Local"], "branch": format!("⎇ {}", vc::FIXTURE_BRANCH), "path": ROOT, "actions": ["split", "newTab", "newAgent"], "empty": false },
                "tabs": [
                    { "tab_id": "w1:t1", "label": "1", "panes": "2", "dot": null, "active": true },
                    { "tab_id": "w1:t2", "label": "2", "panes": "1", "dot": null, "active": false },
                    { "tab_id": "w1:t3", "label": "3", "panes": "1", "dot": null, "active": false }
                ],
                "frames": frames(),
                "panes": ["w1:p1", "w1:p2"],
                "focused": "w1:p2",
            },
            "stages": [stage("start", 1.0), stage("grow", 1.0), stage("scale2", 2.0), stage("restore", 1.0)],
            "hidden": {
                "before": { "raf_requests": 9, "raf_callbacks": 9, "paint_calls": 9, "painted_rows": 90, "full_frames": 2 },
                "after": { "raf_requests": 9, "raf_callbacks": 9, "paint_calls": 9, "painted_rows": 90, "full_frames": 2 },
                "frames": 0,
                "ms": vc::HIDDEN_MS,
            },
        })
    }

    fn pane_list(panes: &[(&str, &str, &str, bool)]) -> Value {
        json!({ "result": { "panes": panes.iter().map(|(pane, tab, cwd, focused)| json!({
            "pane_id": pane, "tab_id": tab, "workspace_id": "w1", "cwd": cwd, "focused": focused,
            "agent_status": "unknown", "terminal_id": format!("term-{pane}"),
        })).collect::<Vec<_>>() } })
    }

    fn tab_list(tabs: &[(&str, bool)]) -> Value {
        json!({ "result": { "tabs": tabs.iter().map(|(tab, focused)| json!({
            "tab_id": tab, "workspace_id": "w1", "label": "1", "focused": focused, "number": 1,
            "pane_count": 1, "agent_status": "idle",
        })).collect::<Vec<_>>() } })
    }

    fn agent_list(agents: &[(&str, &str, &str)]) -> Value {
        json!({ "result": { "type": "agent_list", "agents": agents.iter().map(|(pane, agent, status)| json!({
            "pane_id": pane, "tab_id": "w1:t1", "workspace_id": "w1", "agent": agent,
            "agent_status": status, "terminal_id": "term_1", "focused": false, "revision": 1,
        })).collect::<Vec<_>>() } })
    }

    fn center_ledger() -> Ledger {
        let catalog = json!({
            "version": 1,
            "projects": [{ "id": "p1", "label": "Fidelidade Local", "endpoint_profile_id": "local",
                           "session_name": "hd013", "root": ROOT, "binding": null }],
            "collections": [{ "id": "c1", "name": "Fidelidade", "project_ids": ["p1"] }],
        });
        let before = json!({
            "step": "center-engine-before",
            "project_catalog": catalog,
            "focused_cwd": "/tmp/hd013/repo",
            "branch": vc::FIXTURE_BRANCH,
            "engine_pane_list": pane_list(&[("w1:p1", "w1:t1", ROOT, true)]),
            "engine_tab_list": tab_list(&[("w1:t1", true)]),
            "engine_agent_list": agent_list(&[]),
        });
        let after = json!({
            "step": "center-actions",
            "engine_pane_list": pane_list(&[
                ("w1:p1", "w1:t1", "/tmp/hd013/work", false),
                ("w1:p2", "w1:t1", "/tmp/hd013/other", true),
                ("w1:p3", "w1:t2", ROOT, false),
                ("w1:p4", "w1:t3", ROOT, false),
            ]),
            "engine_tab_list": tab_list(&[("w1:t1", true), ("w1:t2", false), ("w1:t3", false)]),
            "engine_agent_list": agent_list(&[("w1:p2", "pi", "idle")]),
            // Shape of `herdr pane zoom --off` as the reference engine answers (checked against
            // .local/bin/herdr-03749ae in a disposable session, gate r7): the result nests `zoom`.
            "zoom_off": { "id": "cli:pane:zoom", "result": { "type": "pane_zoom", "zoom": {
                "changed": true, "zoom_changed": true, "focus_changed": false,
                "pane_id": "w1:p2", "focused_pane_id": "w1:p2", "zoomed": false,
            } } },
        });
        let mut ledger = Ledger::default();
        for step in vc::STEPS {
            let answer = match step {
                "center-engine-before" => before.clone(),
                "center-actions" => after.clone(),
                other => json!({ "step": other }),
            };
            ledger.record(step, answer).unwrap();
        }
        ledger
    }

    /// A wrong report/ledger built from the faithful one.
    type Mutation = Box<dyn Fn(&mut Value, &mut Ledger)>;

    fn failing(report: &Value, ledger: &Ledger) -> Vec<&'static str> {
        vc::checks(report, ledger)
            .unwrap()
            .into_iter()
            .filter(|(_, ok)| !ok)
            .map(|(name, _)| name)
            .collect()
    }

    // Would catch: the plan and the evaluator naming different checks, the phase not driven, or the
    // page's step list drifting from the parent's.
    #[test]
    fn plan_checks_and_steps_match_the_page_and_the_evaluator() {
        let spec = FLOW.iter().find(|p| p.name == vc::PHASE).unwrap();
        assert_eq!(spec.checks, vc::CHECKS);
        assert!(DRIVEN_PHASES.contains(&vc::PHASE));
        let names: Vec<&str> = vc::checks(&report(), &center_ledger())
            .unwrap()
            .iter()
            .map(|c| c.0)
            .collect();
        assert_eq!(names, vc::CHECKS);
        let page = read(&repo().join("src/features/fidelity/visual-center.ts"));
        let steps = format!(
            "[\n  {},\n]",
            vc::STEPS.map(|s| format!("\"{s}\"")).join(",\n  ")
        );
        assert!(
            page.contains(&format!("CENTER_STEPS = {steps} as const")),
            "page steps drifted"
        );
        assert!(page.contains(&format!("HIDDEN_MS = {}", vc::HIDDEN_MS)));
    }

    // Would catch (gate r6): the mode passed as `--mode off`, which the engine's CLI refuses with
    // "unknown option", so the zoom is never restored and the frames stay at one pane.
    #[test]
    fn the_zoom_restore_uses_the_engine_cli_flag() {
        // The mode is a flag of `herdr pane zoom` (--toggle/--on/--off), never `--mode VALUE`.
        assert_eq!(
            vc::zoom_off_command("w3:p4"),
            vec![
                "pane".to_owned(),
                "zoom".to_owned(),
                "w3:p4".to_owned(),
                "--off".to_owned()
            ]
        );
    }

    // Would catch: an evaluator that passes a faithful run only by accident (every check false).
    #[test]
    fn a_faithful_center_passes_every_check() {
        assert_eq!(failing(&report(), &center_ledger()), Vec::<&str>::new());
    }

    // Would catch: a check that ignores its own evidence. Each wrong report must fail exactly the
    // check that owns it (the mutations are the ones a wrong implementation would produce).
    #[test]
    fn each_wrong_center_fails_its_own_check() {
        // Each mutation lists the checks it must kill; a mutation of the engine's own snapshot can
        // legitimately break two checks that read the same snapshot.
        type Case = (&'static str, Mutation, Vec<&'static str>);
        let cases: Vec<Case> = vec![
            (
                "branch missing in the header",
                Box::new(|r: &mut Value, _: &mut Ledger| {
                    r["before"]["header"]["branch"] = Value::Null
                }),
                vec![vc::CHECKS[0]],
            ),
            (
                "path that is not the root of the project in the breadcrumb",
                Box::new(|r: &mut Value, _: &mut Ledger| {
                    r["before"]["header"]["path"] = json!("/outro")
                }),
                vec![vc::CHECKS[0]],
            ),
            (
                "a group that does not contain the project",
                Box::new(|r: &mut Value, _: &mut Ledger| {
                    r["before"]["header"]["crumbs"] = json!(["Outra coleção", "Fidelidade Local"])
                }),
                vec![vc::CHECKS[0]],
            ),
            (
                "a branch other than the one git reports for the focused workspace",
                Box::new(|r: &mut Value, _: &mut Ledger| {
                    r["before"]["header"]["branch"] = json!("⎇ main")
                }),
                vec![vc::CHECKS[0]],
            ),
            (
                "only the project in the breadcrumb",
                Box::new(|r: &mut Value, _: &mut Ledger| {
                    r["before"]["header"]["crumbs"] = json!(["Fidelidade Local"])
                }),
                vec![vc::CHECKS[0]],
            ),
            (
                "the engine created no tab for Shell/+",
                Box::new(|_: &mut Value, l: &mut Ledger| {
                    l.steps.get_mut("center-actions").unwrap()["engine_tab_list"] =
                        tab_list(&[("w1:t1", true)]);
                }),
                vec![vc::CHECKS[1], vc::CHECKS[2]],
            ),
            (
                "two agents started by one click",
                Box::new(|_: &mut Value, l: &mut Ledger| {
                    l.steps.get_mut("center-actions").unwrap()["engine_agent_list"] =
                        agent_list(&[("w1:p2", "pi", "idle"), ("w1:p3", "pi", "idle")]);
                }),
                vec![vc::CHECKS[1]],
            ),
            (
                "a tab bar that counts panes on its own",
                Box::new(|r: &mut Value, _: &mut Ledger| {
                    r["seen"]["tabs"][0]["panes"] = json!("1")
                }),
                vec![vc::CHECKS[2]],
            ),
            (
                "a tab the engine does not have",
                Box::new(|r: &mut Value, _: &mut Ledger| {
                    r["seen"]["tabs"] = json!([{ "tab_id": "w1:t1", "label": "1", "panes": "2", "dot": null, "active": true }]);
                }),
                vec![vc::CHECKS[2]],
            ),
            (
                "the active tab following the global focus flag",
                Box::new(|r: &mut Value, _: &mut Ledger| {
                    r["seen"]["tabs"][0]["active"] = json!(false);
                    r["seen"]["tabs"][2]["active"] = json!(true);
                }),
                vec![vc::CHECKS[2]],
            ),
            (
                "a dot invented without an agent",
                Box::new(|r: &mut Value, _: &mut Ledger| {
                    r["seen"]["tabs"][1]["dot"] = json!("working")
                }),
                vec![vc::CHECKS[2]],
            ),
            (
                "a frame two pixels off its inner_rect at one stage",
                Box::new(|r: &mut Value, _: &mut Ledger| {
                    r["stages"][2]["frames"][1]["rect"]["left"] = json!(
                        r["stages"][2]["frames"][1]["rect"]["left"]
                            .as_f64()
                            .unwrap()
                            + 2.0
                    );
                }),
                vec![vc::CHECKS[3]],
            ),
            (
                "the frame sized from the engine border box instead of inner_rect",
                Box::new(|r: &mut Value, _: &mut Ledger| {
                    r["stages"][0]["frames"][0]["rect"]["width"] = json!(42.0 * 9.0);
                }),
                vec![vc::CHECKS[3]],
            ),
            (
                "the DPI stage never applied",
                Box::new(|r: &mut Value, _: &mut Ledger| r["stages"][2]["dpr"] = json!(1.0)),
                vec![vc::CHECKS[3]],
            ),
            (
                "the band drawn over the pane's first content row",
                Box::new(|r: &mut Value, _: &mut Ledger| {
                    let top = r["stages"][1]["frames"][0]["rect"]["top"].as_f64().unwrap();
                    r["stages"][1]["frames"][0]["band"]["top"] = json!(top);
                }),
                vec![vc::CHECKS[3]],
            ),
            (
                "a toolbar over the content cells of a pane",
                Box::new(|r: &mut Value, _: &mut Ledger| {
                    r["stages"][0]["toolbar"] = rect(20.0, 25.0 + 19.0 * 2.0, 200.0, 19.0);
                }),
                vec![vc::CHECKS[3]],
            ),
            (
                "a pane path taken from the project root",
                Box::new(|r: &mut Value, _: &mut Ledger| {
                    r["seen"]["frames"][1]["path"] = json!(ROOT)
                }),
                vec![vc::CHECKS[4]],
            ),
            (
                "the agent's pane labelled shell",
                Box::new(|r: &mut Value, _: &mut Ledger| {
                    r["seen"]["frames"][1]["name"] = json!("shell")
                }),
                vec![vc::CHECKS[4]],
            ),
            (
                "a state text outside the five engine states",
                Box::new(|r: &mut Value, _: &mut Ledger| {
                    r["seen"]["frames"][0]["state"] = json!("Rodando")
                }),
                vec![vc::CHECKS[4]],
            ),
            (
                "the focus edge painted on every frame",
                Box::new(|r: &mut Value, _: &mut Ledger| {
                    r["seen"]["frames"][0]["edge"] = json!("accent");
                    r["seen"]["frames"][0]["edge_color"] = json!(vc::ACCENT_RGB);
                }),
                vec![vc::CHECKS[4]],
            ),
            (
                "the click that never reached the engine",
                Box::new(|_: &mut Value, l: &mut Ledger| {
                    l.steps.get_mut("center-actions").unwrap()["engine_pane_list"] = pane_list(&[
                        ("w1:p1", "w1:t1", "/tmp/hd013/work", true),
                        ("w1:p2", "w1:t1", "/tmp/hd013/other", false),
                        ("w1:p3", "w1:t2", ROOT, false),
                        ("w1:p4", "w1:t3", ROOT, false),
                    ]);
                }),
                vec![vc::CHECKS[5]],
            ),
            (
                "an expand button that zoomed nothing in the engine",
                Box::new(|_: &mut Value, l: &mut Ledger| {
                    l.steps.get_mut("center-actions").unwrap()["zoom_off"]["result"]["zoom"]
                        ["zoom_changed"] = json!(false)
                }),
                vec![vc::CHECKS[5]],
            ),
            (
                "a repaint while the terminal is hidden",
                Box::new(|r: &mut Value, _: &mut Ledger| {
                    r["hidden"]["after"]["raf_requests"] = json!(11)
                }),
                vec![vc::CHECKS[6]],
            ),
            (
                "frames still mounted over a hidden terminal",
                Box::new(|r: &mut Value, _: &mut Ledger| r["hidden"]["frames"] = json!(2)),
                vec![vc::CHECKS[6]],
            ),
        ];
        for (what, mutate, expected) in cases {
            let (mut wrong, mut wrong_ledger) = (report(), center_ledger());
            mutate(&mut wrong, &mut wrong_ledger);
            assert_eq!(failing(&wrong, &wrong_ledger), expected, "{what}");
        }
    }

    // Would catch: a phase counted as passed while the parent never answered a step, or while the
    // window reported the phase as failed.
    #[test]
    fn missing_parent_steps_or_page_errors_are_errors() {
        let mut ledger = center_ledger();
        ledger.steps.remove("center-observe-scale2");
        assert!(vc::checks(&report(), &ledger)
            .unwrap_err()
            .contains("center-observe-scale2"));
        let mut broken = report();
        broken["error"] = json!("Dividir not found");
        assert!(vc::checks(&broken, &center_ledger())
            .unwrap_err()
            .contains("Dividir not found"));
        let mut failed_step = center_ledger();
        failed_step.steps.get_mut("center-actions").unwrap()["error"] = json!("swaymsg failed");
        assert!(vc::checks(&report(), &failed_step)
            .unwrap_err()
            .contains("swaymsg failed"));
    }
}

// ---------------------------------------------------------------------------------------
// Spec 011 — visual-projects evaluator (no display, engine or GUI)
// ---------------------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod visual_projects_eval {
    use super::*;
    use support::visual_projects::{self as vp, Ledger};

    fn baseline_report() -> Value {
        json!({
            "phase": "visual-projects",
            "error": null,
            "dialog_closed": true,
            "sidebar": {
                "groups": [
                    { "name": "Open source", "color": "rgb(244, 180, 84)", "count": 2, "collapsed": false },
                    { "name": "Trabalho", "color": "rgb(91, 214, 138)", "count": 1, "collapsed": false },
                ],
                "projects": [
                    {
                        "name": "herdr-desktop",
                        "branch": "main",
                        "host_badge": null,
                        "active": true,
                        "bg_color": "rgb(30, 34, 43)",
                        "accent_bar_width": 2.0,
                        "accent_bar_color": "rgb(143, 168, 255)",
                        "agent_dots": [{ "status": "working", "label": "Agente trabalhando" }],
                        "has_menu": true,
                    },
                    {
                        "name": "herdr-remote",
                        "branch": "feat-ssh",
                        "host_badge": "dev-box",
                        "active": false,
                        "bg_color": "rgba(0, 0, 0, 0)",
                        "accent_bar_width": 0.0,
                        "accent_bar_color": "",
                        "agent_dots": [],
                        "has_menu": true,
                    },
                ],
                "connections": [
                    {
                        "label": "Este computador · Local",
                        "type_badge": "Local",
                        "latency_text": "Local",
                        "status_text": "Online",
                        "tone": "ok",
                    },
                    {
                        "label": "dev-box",
                        "type_badge": "SSH",
                        "latency_text": "45 ms",
                        "status_text": "45 ms",
                        "tone": "ok",
                    },
                ],
                "small_texts": [
                    {
                        "scope": "projects-sidebar",
                        "text": "Open source",
                        "color": "rgb(231, 233, 238)",
                        "background": "rgba(17, 19, 24, 1)",
                        "opacity": 1.0,
                        "font_size": 13.0,
                        "font_weight": 500.0,
                        "visibility": "visible",
                    }
                ],
            },
            "dialog": {
                "open": true,
                "width": 620.0,
                "height": 520.0,
                "border_radius": 12.0,
                "cards": [
                    { "kind": "local", "role": "radio", "tabindex": -1, "selected": false },
                    { "kind": "ssh", "role": "radio", "tabindex": 0, "selected": true },
                ],
                "fields": [
                    "Host",
                    "Porta",
                    "Autenticação",
                    "Nome de exibição",
                    "Adicionar projetos ao grupo",
                    "Sessão",
                ],
                "progress_items": [
                    { "text": "Host alcançável · 45 ms", "step": 1, "status": "ok" },
                    { "text": "Autenticado como dev", "step": 2, "status": "ok" },
                    { "text": "herdr 0.9.0 encontrado · endpoint geração 1", "step": 3, "status": "ok" },
                    { "text": "Lendo workspaces remotos", "step": 4, "status": "ok" },
                ],
                "small_texts": [
                    {
                        "scope": "connection-dialog",
                        "text": "Conectar a um servidor herdr",
                        "color": "rgb(231, 233, 238)",
                        "background": "rgba(23, 26, 33, 1)",
                        "opacity": 1.0,
                        "font_size": 16.0,
                        "font_weight": 600.0,
                        "visibility": "visible",
                    }
                ],
            }
        })
    }

    fn baseline_ledger() -> Ledger {
        let mut ledger = Ledger::default();
        // The sidebar answer carries the validated pane and the engine list of the phase: one
        // agent in the pane's workspace (`w3`), which is the agent the active project must show.
        for step in vp::STEPS {
            let answer = if step == "visual-projects-sidebar" {
                json!({
                    "step": step,
                    "pane_id": "w3:p1",
                    "agent_pane": "w3:p1",
                    "agent_list_after_start": {
                        "result": {
                            "agents": [
                                { "pane_id": "w3:p1", "workspace_id": "w3", "agent_status": "idle" }
                            ]
                        }
                    },
                })
            } else {
                json!({ "step": step })
            };
            ledger.record(step, answer).unwrap();
        }
        ledger
    }

    #[test]
    fn plan_checks_and_steps_match_page_and_evaluator() {
        let spec = FLOW.iter().find(|p| p.name == vp::PHASE).unwrap();
        assert_eq!(spec.checks, vp::CHECKS);
        assert!(DRIVEN_PHASES.contains(&vp::PHASE));
        let names: Vec<&str> = vp::checks(&baseline_report(), &baseline_ledger())
            .unwrap()
            .iter()
            .map(|c| c.0)
            .collect();
        assert_eq!(names, vp::CHECKS);
        let page = read(&repo().join("src/features/fidelity/visual-projects.ts"));
        let steps = format!("[{}]", vp::STEPS.map(|s| format!("\"{s}\"")).join(", "));
        assert!(
            page.contains(&format!("VISUAL_PROJECTS_STEPS = {steps} as const")),
            "page steps drifted"
        );
    }

    #[test]
    fn baseline_report_and_ledger_pass_every_check() {
        let checks = vp::checks(&baseline_report(), &baseline_ledger()).unwrap();
        for (name, ok) in checks {
            assert!(ok, "check {name} failed on baseline");
        }
    }

    #[test]
    fn missing_step_in_ledger_is_an_error() {
        let mut ledger = Ledger::default();
        ledger
            .record(
                "visual-projects-sidebar",
                json!({ "step": "visual-projects-sidebar" }),
            )
            .unwrap();
        assert!(vp::checks(&baseline_report(), &ledger).is_err());
    }

    #[test]
    fn dialog_dimension_deviation_fails_check() {
        let mut rep = baseline_report();
        rep["dialog"]["width"] = json!(600.0);
        let checks = vp::checks(&rep, &baseline_ledger()).unwrap();
        let dialog_check = checks
            .iter()
            .find(|(n, _)| *n == "connection_dialog_layout_and_progress")
            .unwrap();
        assert!(!dialog_check.1);
    }

    #[test]
    fn unclosed_dialog_fails_check() {
        let mut rep = baseline_report();
        rep["dialog_closed"] = json!(false);
        let checks = vp::checks(&rep, &baseline_ledger()).unwrap();
        let dialog_check = checks
            .iter()
            .find(|(n, _)| *n == "connection_dialog_layout_and_progress")
            .unwrap();
        assert!(!dialog_check.1);
    }

    #[test]
    fn active_project_without_dots_fails_tree_check() {
        let mut rep = baseline_report();
        rep["sidebar"]["projects"][0]["agent_dots"] = json!([]);
        let checks = vp::checks(&rep, &baseline_ledger()).unwrap();
        let tree_check = checks
            .iter()
            .find(|(n, _)| *n == "project_tree_groups_and_projects")
            .unwrap();
        assert!(!tree_check.1);
    }

    #[test]
    fn inactive_project_with_unexpected_dots_fails_tree_check() {
        let mut rep = baseline_report();
        rep["sidebar"]["projects"][1]["agent_dots"] =
            json!([{ "status": "working", "label": "extra" }]);
        let checks = vp::checks(&rep, &baseline_ledger()).unwrap();
        let tree_check = checks
            .iter()
            .find(|(n, _)| *n == "project_tree_groups_and_projects")
            .unwrap();
        assert!(!tree_check.1);
    }

    /// The session fallback (agents of another workspace shown on the active project) must fail:
    /// the active row cannot show more dots than the agents the engine lists for its workspace.
    #[test]
    fn active_project_with_session_agents_beyond_the_workspace_fails_tree_check() {
        let mut rep = baseline_report();
        rep["sidebar"]["projects"][0]["agent_dots"] = json!([
            { "status": "working", "label": "Agente trabalhando" },
            { "status": "idle", "label": "outro workspace" },
        ]);
        let checks = vp::checks(&rep, &baseline_ledger()).unwrap();
        let tree_check = checks
            .iter()
            .find(|(n, _)| *n == "project_tree_groups_and_projects")
            .unwrap();
        assert!(!tree_check.1);
    }

    #[test]
    fn sidebar_answer_without_the_started_agent_fails_tree_check() {
        let mut ledger = baseline_ledger();
        ledger.steps.insert(
            "visual-projects-sidebar".into(),
            json!({ "step": "visual-projects-sidebar" }),
        );
        let checks = vp::checks(&baseline_report(), &ledger).unwrap();
        let tree_check = checks
            .iter()
            .find(|(n, _)| *n == "project_tree_groups_and_projects")
            .unwrap();
        assert!(!tree_check.1);
    }
}
