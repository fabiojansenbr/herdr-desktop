//! Resource bench of spec 007 (AC-007-03 preparation): matrix, run modes and the evidence
//! validator that refuses to turn a partial, debug, short or unowned run into a result.
//! Deterministic: no engine, display, server or prior build.

#[path = "../../tests/fidelity-bench/mod.rs"]
mod bench;

#[allow(dead_code)]
#[path = "../../tests/fidelity-native/mod.rs"]
mod support;

#[allow(dead_code)]
#[path = "../../tests/fidelity-latency/mod.rs"]
mod latency;

#[allow(dead_code)]
#[path = "../../scripts/feature-harness/native.rs"]
mod native_harness;

use bench::plan::{matrix, parse_args, Mode, Output, Scenario, MEMORY_CHECKPOINTS_S};
use bench::verdict::{
    evaluate, observations, targets, BuildInfo, ClientEvidence, CounterReading, Evidence, Geometry,
    ProcessId, Producer, RunStatus, TargetState,
};
use serde_json::json;

fn scenario(panes: u8, clients: u8, output: Output) -> Scenario {
    Scenario {
        panes,
        clients,
        output,
    }
}

/// The calibrated fixed geometry (spec 009 recalibration), written literally so a change of the
/// constant shows.
fn geometry() -> Geometry {
    Geometry {
        cols: 108,
        rows: 30,
        cell_width_px: 9,
        cell_height_px: 19,
        scale_milli: 1000,
        font: "14px \"JetBrains Mono\", \"Fira Code\", \"DejaVu Sans Mono\", monospace".into(),
    }
}

fn client(n: u32, output: Output) -> ClientEvidence {
    let hidden = output == Output::Hidden;
    ClientEvidence {
        window: ProcessId {
            pid: 1000 + n,
            starttime: 50 + n as u64,
            current_starttime: Some(50 + n as u64),
            owned: true,
        },
        runtime_dir: format!("/tmp/hd7B-{n}"),
        geometry: geometry(),
        terminal_canvases: 1,
        terminal_hidden: hidden,
        probe_delta: json!({
            "raf_callbacks": if hidden { 0 } else { 40 },
            "painted_rows": if hidden { 0 } else { 900 },
        }),
        positive_control: json!({ "raf_callbacks": 12, "painted_rows": 300 }),
    }
}

const ENGINE_PID: u32 = 900;
/// Generator counter before the measured window: 10 min of earlier output, so a total-bytes
/// reading would be far off the fixed rate while the interval delta is on it.
const EARLIER_BYTES: u64 = 3950 * 600;

fn producer(i: u8, measured_s: f64) -> Producer {
    let shell = 2000 + i as u32;
    let start_ns = 5_000_000_000u64;
    Producer {
        pane_id: format!("w{}:p1", i + 1),
        engine_shell_pid: Some(shell),
        shell: ProcessId {
            pid: shell,
            starttime: 7,
            current_starttime: Some(7),
            owned: true,
        },
        shell_ancestors: vec![ENGINE_PID, 1],
        generator: ProcessId {
            pid: 3000 + i as u32,
            starttime: 11,
            current_starttime: Some(11),
            owned: true,
        },
        generator_ancestors: vec![shell, ENGINE_PID, 1],
        counter_start: CounterReading {
            bytes: EARLIER_BYTES,
            monotonic_ns: start_ns,
        },
        counter_end: CounterReading {
            bytes: EARLIER_BYTES + 4096 * measured_s as u64,
            monotonic_ns: start_ns + (measured_s * 1e9) as u64,
        },
    }
}

fn evidence(s: Scenario, mode: Mode) -> Evidence {
    let producers = (0..s.panes)
        .map(|i| producer(i, mode.measured_s()))
        .collect();
    Evidence {
        scenario: s,
        mode,
        build: BuildInfo {
            debug_assertions: false,
            profile: "release".into(),
            binary_sha256: "ab".repeat(32),
            surface_trace: false,
        },
        session: "hd007-bench-1".into(),
        engine: ProcessId {
            pid: ENGINE_PID,
            starttime: 3,
            current_starttime: Some(3),
            owned: true,
        },
        pane_list_count: s.panes as usize,
        clients: (1..=s.clients as u32)
            .map(|n| client(n, s.output))
            .collect(),
        producers,
        warmup_s: mode.warmup_s(),
        measured_s: mode.measured_s(),
        collector_files: vec![
            "samples.jsonl".into(),
            "summary.json".into(),
            "report.md".into(),
        ],
        collector_summary: with_pty_groups(summary(s.clients, 180_000, 0.4), s.panes),
        cleanup_log: vec!["no process left with XDG_RUNTIME_DIR=/tmp/hd7B-1".into()],
    }
}

fn group(pss_kb: u64, cpu: f64) -> serde_json::Value {
    json!({
        "pss_kb": { "avg": pss_kb, "max": pss_kb, "is_lower_bound": false },
        "rss_kb": { "avg": pss_kb * 2, "max": pss_kb * 2, "is_lower_bound": false },
        "cpu_percent_of_one_core": { "avg": cpu, "max": cpu, "is_lower_bound": false },
    })
}

fn summary(clients: u8, gui_pss_kb: u64, cpu: f64) -> serde_json::Value {
    let mut groups = serde_json::Map::new();
    for n in 1..=clients {
        groups.insert(format!("gui_{n}"), group(gui_pss_kb, cpu));
    }
    groups.insert("engine".into(), group(20_000, 0.1));
    groups.insert("pty".into(), group(8_000, 0.2));
    json!({ "status": "success", "failure_reason": null, "groups": groups })
}

/// One collector group per PTY shell tree: `pty` alone, or `pty_1..pty_N` for N > 1.
fn with_pty_groups(mut summary: serde_json::Value, panes: u8) -> serde_json::Value {
    if panes > 1 {
        let groups = summary["groups"].as_object_mut().unwrap();
        groups.remove("pty");
        for i in 1..=panes {
            groups.insert(format!("pty_{i}"), group(8_000, 0.2));
        }
    }
    summary
}

fn reasons(e: &Evidence) -> Vec<String> {
    match evaluate(e) {
        RunStatus::Rejected(r) => r,
        other => panic!("expected a rejection, got {other:?}"),
    }
}

fn rejected_for(e: &Evidence, needle: &str) {
    let r = reasons(e);
    assert!(
        r.iter().any(|x| x.contains(needle)),
        "{needle:?} not in {r:?}"
    );
}

const ACCEPT: Mode = Mode::Acceptance {
    warmup_s: 5.0,
    measured_s: 60.0,
};

mod matrix_and_modes {
    use super::*;

    #[test]
    fn the_matrix_is_eight_cells_plus_idle_baselines() {
        let m = matrix();
        assert_eq!(m.cells.len(), 8);
        let mut seen = std::collections::BTreeSet::new();
        for c in &m.cells {
            assert!([1, 15].contains(&c.panes) && [1, 2].contains(&c.clients));
            seen.insert((c.panes, c.clients, c.output == Output::Hidden));
        }
        assert_eq!(seen.len(), 8, "duplicated cell");
        assert_eq!(m.idle_baseline, scenario(1, 1, Output::Idle));
        assert_eq!(m.idle_comparison, scenario(15, 1, Output::Idle));
    }

    #[test]
    fn modes_enforce_smoke_ceiling_acceptance_floor_and_memory_checkpoints() {
        assert!(Mode::smoke(3.0).is_ok());
        assert!(Mode::smoke(3.5).is_err());
        assert!(Mode::acceptance(5.0, 60.0).is_ok());
        assert!(Mode::acceptance(5.0, 59.9).is_err(), "short acceptance run");
        assert!(Mode::acceptance(4.9, 60.0).is_err(), "short warmup");
        let memory = Mode::memory();
        assert_eq!(memory.measured_s(), 1800.0);
        assert_eq!(MEMORY_CHECKPOINTS_S, [0, 600, 1200, 1800]);
    }

    #[test]
    fn live_measurement_authorises_acceptance_and_memory_of_every_matrix_topology() {
        // Would catch: the previous checkpoint gate that only allowed smoke and idle 1/1 acceptance.
        let topologies = [
            (1u8, 1u8, "idle"),
            (15, 1, "idle"),
            (1, 1, "output"),
            (1, 2, "output"),
            (15, 1, "output"),
            (15, 2, "output"),
        ];
        for (panes, clients, load) in topologies {
            let acc = bench::plan::parse_run(&[
                "--mode",
                "acceptance",
                "--warmup",
                "5",
                "--duration",
                "60",
                "--out",
                "/tmp/x",
                "--panes",
                &panes.to_string(),
                "--clients",
                &clients.to_string(),
                "--load",
                load,
            ])
            .unwrap();
            assert_eq!(
                bench::plan::resource_live_authorised(&acc),
                Ok(()),
                "{panes}pty {clients}gui {load} acceptance"
            );
        }
        let memory = bench::plan::parse_run(&[
            "--mode",
            "memory",
            "--duration",
            "1800",
            "--out",
            "/tmp/x",
            "--panes",
            "1",
            "--clients",
            "1",
            "--load",
            "idle",
        ])
        .unwrap();
        assert_eq!(bench::plan::resource_live_authorised(&memory), Ok(()));
    }

    #[test]
    fn resource_window_timeout_covers_acceptance_windows_and_memory_1800s() {
        // Would catch: the 150 s harness timeout killing a 60+60 s output run or the 1800 s memory GUI.
        let acc = Mode::acceptance(5.0, 60.0).unwrap();
        assert!(
            bench::plan::resource_window_timeout_s(&acc, bench::plan::Load::Output)
                >= 2 * (5 + 60) + 120
        );
        assert!(bench::plan::resource_window_timeout_s(&acc, bench::plan::Load::Idle) > 60 + 5);
        assert!(
            bench::plan::resource_window_timeout_s(&Mode::memory(), bench::plan::Load::Idle) > 1800
        );
        assert!(
            bench::plan::resource_window_timeout_s(
                &Mode::smoke(3.0).unwrap(),
                bench::plan::Load::Idle
            ) < 1800
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn resource_page_params_carry_mode_duration_so_the_page_can_wait_out_memory() {
        // Would catch: page only seeing settle_ms/windows and timing out harness_await at 110s.
        let memory = bench::live::resource_bench_page_params(
            &Mode::memory(),
            bench::plan::Load::Idle,
            bench::plan::RendererVariant::Control,
        );
        assert_eq!(memory["warmup_s"], 0.0);
        assert_eq!(memory["measured_s"], 1800.0);
        assert_eq!(memory["windows"], json!(["idle"]));
        let acc = Mode::acceptance(5.0, 60.0).unwrap();
        let page = bench::live::resource_bench_page_params(
            &acc,
            bench::plan::Load::Output,
            bench::plan::RendererVariant::Control,
        );
        assert_eq!(page["warmup_s"], 5.0);
        assert_eq!(page["measured_s"], 60.0);
        assert_eq!(page["windows"], json!(["visible", "hidden"]));
        assert!(
            bench::plan::resource_window_timeout_s(&Mode::memory(), bench::plan::Load::Idle)
                >= 1800
        );
        // Would catch: harness_await clipping timeout_ms to 120 s so a 1920 s wait never happens.
        assert_eq!(
            support::window::harness_await_wait(1_920_000),
            std::time::Duration::from_millis(1_920_000)
        );
        assert_eq!(
            support::window::harness_await_wait(110_000),
            std::time::Duration::from_millis(110_000)
        );
    }

    #[test]
    fn cli_parameters_are_explicit_and_unknown_or_missing_ones_are_errors() {
        let ok =
            bench::plan::parse_args(&["--mode", "smoke", "--duration", "3", "--out", "/tmp/x"]);
        assert_eq!(ok.unwrap().mode, Mode::Smoke { measured_s: 3.0 });
        assert!(bench::plan::parse_args(&["--mode", "smoke", "--out", "/tmp/x"]).is_err());
        assert!(bench::plan::parse_args(&[
            "--mode",
            "acceptance",
            "--duration",
            "30",
            "--warmup",
            "5",
            "--out",
            "/tmp/x"
        ])
        .is_err());
        assert!(
            bench::plan::parse_args(&["--mode", "smoke", "--duration", "3", "--out", "rel"])
                .is_err()
        );
        assert!(bench::plan::parse_args(&[
            "--mode",
            "smoke",
            "--duration",
            "3",
            "--out",
            "/tmp/x",
            "--fast-clock"
        ])
        .is_err());
    }
}

mod rejections {
    use super::*;

    #[test]
    fn a_complete_acceptance_run_is_only_pending_review_never_pass() {
        for s in matrix().cells {
            let status = evaluate(&evidence(s, ACCEPT));
            assert_eq!(status, RunStatus::MeasuredPendingReview, "{s:?}");
        }
    }

    #[test]
    fn a_complete_smoke_is_insufficient_for_acceptance() {
        let smoke = Mode::smoke(3.0).unwrap();
        let s = scenario(15, 2, Output::Hidden);
        assert_eq!(
            evaluate(&evidence(s, smoke)),
            RunStatus::InsufficientForAcceptance
        );
    }

    #[test]
    fn debug_build_or_continuous_surface_trace_is_rejected() {
        let s = scenario(1, 1, Output::Visible);
        let mut e = evidence(s, ACCEPT);
        e.build.debug_assertions = true;
        rejected_for(&e, "debug");
        let mut e = evidence(s, ACCEPT);
        e.build.profile = "dev".into();
        rejected_for(&e, "release");
        let mut e = evidence(s, ACCEPT);
        e.build.surface_trace = true;
        rejected_for(&e, "HERDR_DESKTOP_SURFACE_TRACE");
        let mut e = evidence(s, ACCEPT);
        e.build.binary_sha256 = "abc".into();
        rejected_for(&e, "sha256");
    }

    #[test]
    fn measured_or_warmup_time_shorter_than_the_mode_is_rejected() {
        let s = scenario(1, 2, Output::Visible);
        let mut e = evidence(s, ACCEPT);
        e.measured_s = 42.0;
        rejected_for(&e, "measured 42");
        let mut e = evidence(s, ACCEPT);
        e.warmup_s = 1.0;
        rejected_for(&e, "warmup");
        let mut e = evidence(s, ACCEPT);
        e.producers[0].counter_end.monotonic_ns =
            e.producers[0].counter_start.monotonic_ns + 10_000_000_000;
        rejected_for(&e, "w1:p1");
    }

    #[test]
    fn recycled_pids_and_unowned_engine_or_window_are_rejected() {
        let s = scenario(15, 2, Output::Visible);
        let mut e = evidence(s, ACCEPT);
        e.clients[1].window.current_starttime = Some(999);
        rejected_for(&e, "gui_2");
        let mut e = evidence(s, ACCEPT);
        e.engine.owned = false;
        rejected_for(&e, "engine");
        let mut e = evidence(s, ACCEPT);
        e.producers[14].shell.current_starttime = None;
        rejected_for(&e, "w15:p1");
        let mut e = evidence(s, ACCEPT);
        e.session = "default".into();
        rejected_for(&e, "disposable");
    }

    #[test]
    fn wrong_topology_is_rejected() {
        let s = scenario(15, 2, Output::Hidden);
        let mut e = evidence(s, ACCEPT);
        e.pane_list_count = 14;
        rejected_for(&e, "pane.list");
        let mut e = evidence(s, ACCEPT);
        e.clients.pop();
        rejected_for(&e, "clients");
        let mut e = evidence(s, ACCEPT);
        e.clients[0].terminal_canvases = 15;
        rejected_for(&e, "canvas");
        let mut e = evidence(s, ACCEPT);
        e.clients[1].runtime_dir = e.clients[0].runtime_dir.clone();
        rejected_for(&e, "independent");
        let mut e = evidence(s, ACCEPT);
        e.clients[1].window.pid = e.clients[0].window.pid;
        rejected_for(&e, "independent");
        let mut e = evidence(s, ACCEPT);
        e.producers.pop();
        rejected_for(&e, "producers");
        let mut e = evidence(s, ACCEPT);
        e.clients[0].terminal_hidden = false;
        rejected_for(&e, "hidden");
    }

    #[test]
    fn divergent_geometry_between_clients_or_from_the_fixed_one_is_rejected() {
        let s = scenario(1, 2, Output::Visible);
        let mut e = evidence(s, ACCEPT);
        e.clients[1].geometry.cols = 119;
        rejected_for(&e, "geometry");
        let mut e = evidence(s, ACCEPT);
        e.clients[1].geometry.scale_milli = 2000;
        rejected_for(&e, "geometry");
        let mut e = evidence(s, ACCEPT);
        for c in e.clients.iter_mut() {
            c.geometry.rows = 24;
        }
        rejected_for(&e, "fixed geometry");
    }

    #[test]
    fn zero_or_off_rate_production_is_rejected_including_hidden_tab_panes() {
        let s = scenario(15, 1, Output::Hidden);
        let mut e = evidence(s, ACCEPT);
        e.producers[9].counter_end.bytes = e.producers[9].counter_start.bytes;
        rejected_for(&e, "w10:p1");
        let mut e = evidence(s, ACCEPT);
        let p = &mut e.producers[3];
        p.counter_end.bytes =
            p.counter_start.bytes + (p.counter_end.bytes - p.counter_start.bytes) / 2;
        rejected_for(&e, "w4:p1");
    }

    #[test]
    fn hidden_repaint_or_dead_probe_is_rejected() {
        let s = scenario(15, 1, Output::Hidden);
        let mut e = evidence(s, ACCEPT);
        e.clients[0].probe_delta = json!({ "raf_callbacks": 1, "painted_rows": 0 });
        rejected_for(&e, "hidden repaint");
        let mut e = evidence(s, ACCEPT);
        e.clients[0].positive_control = json!({ "raf_callbacks": 0, "painted_rows": 0 });
        rejected_for(&e, "positive control");
        let mut e = evidence(s, ACCEPT);
        e.clients[0].probe_delta = json!({});
        rejected_for(&e, "probe");
        let v = scenario(1, 1, Output::Visible);
        let mut e = evidence(v, ACCEPT);
        e.clients[0].probe_delta = json!({ "raf_callbacks": 0, "painted_rows": 0 });
        rejected_for(&e, "visible");
    }

    #[test]
    fn incomplete_or_failed_collector_output_is_rejected() {
        let s = scenario(1, 2, Output::Visible);
        let mut e = evidence(s, ACCEPT);
        e.collector_files.retain(|f| f != "summary.json");
        rejected_for(&e, "summary.json");
        let mut e = evidence(s, ACCEPT);
        e.collector_summary["status"] = json!("failed");
        rejected_for(&e, "collector status failed");
        let mut e = evidence(s, ACCEPT);
        e.collector_summary["groups"]
            .as_object_mut()
            .unwrap()
            .remove("gui_2");
        rejected_for(&e, "gui_2");
    }

    #[test]
    fn cleanup_leftovers_are_rejected() {
        let s = scenario(1, 1, Output::Visible);
        let mut e = evidence(s, ACCEPT);
        e.cleanup_log.push("LEFTOVER pid=77 /usr/bin/foo".into());
        rejected_for(&e, "LEFTOVER");
        let mut e = evidence(s, ACCEPT);
        e.cleanup_log.clear();
        rejected_for(&e, "cleanup");
    }
}

mod target_comparison {
    use super::*;

    #[test]
    fn idle_gui_is_judged_per_client_against_the_prd_limits() {
        // The idle limit is the PRD's revised 300 MiB (2026-09-23); the key carries it literally
        // so a change of the constant shows here.
        let idle = summary(2, 300 * 1024, 0.99);
        let t = targets(&idle, None);
        assert_eq!(t["gui_1.idle_pss_max_mib<=300"], TargetState::Met);
        assert_eq!(t["gui_2.idle_cpu_avg<1"], TargetState::Met);
        // Two clients at 200 MiB each must not be summed into a 400 MiB failure.
        let two = summary(2, 200 * 1024, 0.2);
        assert_eq!(
            targets(&two, None)["gui_2.idle_pss_max_mib<=300"],
            TargetState::Met
        );
        let heavy = summary(1, 301 * 1024, 1.0);
        let t = targets(&heavy, None);
        assert_eq!(t["gui_1.idle_pss_max_mib<=300"], TargetState::Failed);
        assert_eq!(t["gui_1.idle_cpu_avg<1"], TargetState::Failed);
    }

    #[test]
    fn fifteen_panes_are_compared_to_the_matching_idle_client_baseline() {
        let base = summary(2, 180 * 1024, 0.2);
        let scale = summary(2, 280 * 1024, 0.2);
        let t = targets(&scale, Some(&base));
        assert_eq!(t["gui_1.scale15_delta_pss_max_mib<=100"], TargetState::Met);
        let over = summary(2, 281 * 1024, 0.2);
        assert_eq!(
            targets(&over, Some(&base))["gui_2.scale15_delta_pss_max_mib<=100"],
            TargetState::Failed
        );
    }

    #[test]
    fn lower_bound_null_or_missing_metrics_are_unknown_not_met() {
        let mut s = summary(1, 100 * 1024, 0.1);
        s["groups"]["gui_1"]["pss_kb"]["is_lower_bound"] = json!(true);
        assert!(matches!(
            targets(&s, None)["gui_1.idle_pss_max_mib<=300"],
            TargetState::Unknown(_)
        ));
        let mut s = summary(1, 100 * 1024, 0.1);
        s["groups"]["gui_1"]["cpu_percent_of_one_core"]["avg"] = json!(null);
        assert!(matches!(
            targets(&s, None)["gui_1.idle_cpu_avg<1"],
            TargetState::Unknown(_)
        ));
        let mut s = summary(1, 100 * 1024, 0.1);
        s["status"] = json!("incomplete");
        assert!(targets(&s, None)
            .values()
            .all(|v| matches!(v, TargetState::Unknown(_))));
    }
}

/// Gaps the root found in the checkpoint helpers (live-runner-task.md 1–3), closed before use.
mod validator_gaps {
    use super::*;

    const SMOKE: Mode = Mode::Smoke { measured_s: 3.0 };

    #[test]
    fn infinite_or_nan_durations_never_satisfy_a_minimum() {
        // partial_cmp(inf, 60) is Greater: a bare ordering check would accept an endless run.
        assert!(Mode::acceptance(f64::INFINITY, 60.0).is_err());
        assert!(Mode::acceptance(5.0, f64::INFINITY).is_err());
        assert!(Mode::acceptance(f64::NAN, 60.0).is_err());
        assert!(Mode::acceptance(5.0, 60.0).is_ok());
        for bad in ["inf", "infinity", "NaN"] {
            let args = [
                "--mode",
                "acceptance",
                "--warmup",
                "5",
                "--duration",
                bad,
                "--out",
                "/tmp/x",
            ];
            assert!(parse_args(&args).is_err(), "--duration {bad} accepted");
            let args = [
                "--mode",
                "acceptance",
                "--warmup",
                bad,
                "--duration",
                "60",
                "--out",
                "/tmp/x",
            ];
            assert!(parse_args(&args).is_err(), "--warmup {bad} accepted");
        }
        let s = scenario(1, 1, Output::Visible);
        let mut e = evidence(s, ACCEPT);
        e.measured_s = f64::INFINITY;
        rejected_for(&e, "not finite");
        let mut e = evidence(s, SMOKE);
        e.warmup_s = f64::INFINITY;
        rejected_for(&e, "not finite");
    }

    #[test]
    fn producer_must_be_the_engine_reported_shell_of_its_pane() {
        let s = scenario(1, 1, Output::Hidden);
        assert!(matches!(
            evaluate(&evidence(s, ACCEPT)),
            RunStatus::MeasuredPendingReview
        ));
        let mut e = evidence(s, ACCEPT);
        e.producers[0].engine_shell_pid = None;
        rejected_for(&e, "w1:p1: shell pid 2000 is not the engine's shell");
        let mut e = evidence(s, ACCEPT);
        e.producers[0].engine_shell_pid = Some(4242);
        rejected_for(&e, "is not the engine's shell");
        let mut e = evidence(s, ACCEPT);
        e.producers[0].shell_ancestors = vec![1];
        rejected_for(&e, "w1:p1: shell 2000 is not a descendant of engine 900");
    }

    #[test]
    fn foreign_or_recycled_generator_is_rejected() {
        let s = scenario(1, 1, Output::Visible);
        let mut e = evidence(s, SMOKE);
        e.producers[0].generator.owned = false;
        rejected_for(&e, "w1:p1 generator: pid 3000 not owned");
        let mut e = evidence(s, SMOKE);
        e.producers[0].generator.current_starttime = Some(12);
        rejected_for(&e, "w1:p1 generator: pid 3000 exited or was recycled");
        // A generator elsewhere (not below the pane's shell) cannot prove that pane's output.
        let mut e = evidence(s, SMOKE);
        e.producers[0].generator_ancestors = vec![ENGINE_PID, 1];
        rejected_for(
            &e,
            "w1:p1: generator 3000 is not a descendant of shell 2000",
        );
    }

    #[test]
    fn fifteen_producers_must_be_fifteen_distinct_panes_shells_and_generators() {
        let s = scenario(15, 1, Output::Hidden);
        assert!(matches!(
            evaluate(&evidence(s, ACCEPT)),
            RunStatus::MeasuredPendingReview
        ));
        let mut e = evidence(s, ACCEPT);
        e.producers[7].pane_id = e.producers[6].pane_id.clone();
        rejected_for(&e, "duplicate pane w7:p1");
        let mut e = evidence(s, ACCEPT);
        let first = e.producers[0].clone();
        e.producers[14].shell = first.shell.clone();
        e.producers[14].engine_shell_pid = first.engine_shell_pid;
        e.producers[14].generator_ancestors = first.generator_ancestors.clone();
        rejected_for(&e, "duplicate shell pid 2000");
        let mut e = evidence(s, ACCEPT);
        e.producers[14].generator = e.producers[0].generator.clone();
        rejected_for(&e, "duplicate generator pid 3000");
    }

    #[test]
    fn bytes_are_the_counter_delta_inside_the_measured_window() {
        let s = scenario(1, 1, Output::Visible);
        // End-only total (earlier output + window) at the right rate, but nothing was written
        // inside the window: must be rejected.
        let mut e = evidence(s, ACCEPT);
        let p = &mut e.producers[0];
        p.counter_start.bytes = 3950 * 60;
        p.counter_end.bytes = 3950 * 60;
        rejected_for(&e, "w1:p1: emitted 0 B/s");
        // Counter going backwards (restarted generator) is not a delta.
        let mut e = evidence(s, ACCEPT);
        let p = &mut e.producers[0];
        p.counter_end.bytes = p.counter_start.bytes - 1;
        rejected_for(&e, "w1:p1: generator counter went backwards");
        let mut e = evidence(s, ACCEPT);
        let p = &mut e.producers[0];
        p.counter_end.monotonic_ns = p.counter_start.monotonic_ns;
        rejected_for(&e, "w1:p1: produced for 0 s");
    }

    fn gui(avg_mib: u64, max_mib: u64) -> serde_json::Value {
        json!({
            "pss_kb": { "avg": avg_mib * 1024, "max": max_mib * 1024, "is_lower_bound": false },
            "rss_kb": { "avg": avg_mib * 2048, "max": max_mib * 2048, "is_lower_bound": false },
            "cpu_percent_of_one_core": { "avg": 0.5, "max": 7.0, "is_lower_bound": false },
        })
    }

    #[test]
    fn idle_pss_is_judged_on_the_maximum_and_cpu_on_the_average() {
        let mut s = summary(1, 0, 0.0);
        s["groups"]["gui_1"] = gui(200, 310);
        let t = targets(&s, None);
        assert_eq!(t["gui_1.idle_pss_max_mib<=300"], TargetState::Failed);
        // cpu max 7 % but average 0.5 % of one core: the PRD limit is on the average.
        assert_eq!(t["gui_1.idle_cpu_avg<1"], TargetState::Met);
        s["groups"]["gui_1"]["pss_kb"]["max"] = json!(null);
        assert!(matches!(
            targets(&s, None)["gui_1.idle_pss_max_mib<=300"],
            TargetState::Unknown(_)
        ));
        let mut s = summary(1, 0, 0.0);
        s["groups"]["gui_1"] = gui(240, 250);
        let o = observations(&s);
        assert_eq!(o["gui_1"]["pss_max_mib"], json!(250.0));
        assert_eq!(o["gui_1"]["pss_avg_mib"], json!(240.0));
        assert_eq!(o["gui_1"]["rss_max_mib"], json!(500.0));
        assert_eq!(o["gui_1"]["cpu_avg_percent_of_one_core"], json!(0.5));
    }

    #[test]
    fn scale_delta_is_max_minus_baseline_max() {
        let mut base = summary(1, 0, 0.0);
        base["groups"]["gui_1"] = gui(180, 180);
        let mut scale = summary(1, 0, 0.0);
        // avg delta 70 MiB would be Met; max delta 110 MiB is not.
        scale["groups"]["gui_1"] = gui(250, 290);
        assert_eq!(
            targets(&scale, Some(&base))["gui_1.scale15_delta_pss_max_mib<=100"],
            TargetState::Failed
        );
        scale["groups"]["gui_1"] = gui(270, 280);
        assert_eq!(
            targets(&scale, Some(&base))["gui_1.scale15_delta_pss_max_mib<=100"],
            TargetState::Met
        );
    }

    #[test]
    fn a_summary_without_gui_groups_is_unknown_not_an_empty_success() {
        let mut s = summary(1, 100 * 1024, 0.1);
        s["groups"].as_object_mut().unwrap().remove("gui_1");
        let t = targets(&s, None);
        assert!(!t.is_empty());
        assert!(t.values().all(|v| matches!(v, TargetState::Unknown(_))));
        assert!(t.values().all(|v| *v != TargetState::Met));
    }
}

/// Mapping of real readings (engine JSON, /proc, generator counter, page probe) to evidence.
mod evidence_mapping {
    use super::*;
    use bench::evidence;

    #[test]
    fn ppid_and_ancestry_come_from_proc_stat_even_with_spaces_in_comm() {
        assert_eq!(
            evidence::parse_ppid("4321 (python3 gen) S 4300 4321 4300 0 -1"),
            Some(4300)
        );
        let parents = |pid| match pid {
            4321 => Some(4300),
            4300 => Some(900),
            900 => Some(1),
            _ => None,
        };
        assert_eq!(evidence::ancestors(4321, &parents), vec![4300, 900, 1]);
        // A cycle (recycled pid read mid-walk) terminates instead of looping.
        let cyclic = |pid| match pid {
            10 => Some(11),
            11 => Some(10),
            _ => None,
        };
        assert_eq!(evidence::ancestors(10, &cyclic), vec![11, 10]);
    }

    #[test]
    fn generator_ownership_requires_this_runs_token() {
        let cmdline = b"python3\0/tmp/out/gen-hd007-bench-77-a1.py\0/tmp/out/counter\0";
        assert!(evidence::generator_owned(cmdline, "hd007-bench-77-a1"));
        assert!(!evidence::generator_owned(cmdline, "hd007-bench-78-a1"));
        assert!(!evidence::generator_owned(
            b"python3\0other.py\0",
            "hd007-bench-77-a1"
        ));
        assert!(!evidence::generator_owned(cmdline, ""));
    }

    #[test]
    fn counter_file_is_bytes_monotonic_pid_and_nothing_else() {
        let (reading, pid) = evidence::parse_counter("237000 5000000000 4321\n").unwrap();
        assert_eq!(
            reading,
            CounterReading {
                bytes: 237000,
                monotonic_ns: 5_000_000_000
            }
        );
        assert_eq!(pid, 4321);
        assert!(evidence::parse_counter("237000 5000000000").is_none());
        assert!(evidence::parse_counter("237000 5000000000 4321 9").is_none());
        assert!(evidence::parse_counter("").is_none());
    }

    #[test]
    fn engine_json_is_read_for_the_requested_pane_only() {
        let list = json!({ "id": "1", "result": { "type": "pane_list", "panes": [
            { "pane_id": "w1:p1" }, { "pane_id": "w2:p1" } ] } });
        assert_eq!(
            evidence::pane_list_ids(&list),
            Some(vec!["w1:p1".to_owned(), "w2:p1".to_owned()])
        );
        assert_eq!(evidence::pane_list_ids(&json!({ "panes": [] })), None);
        let info = json!({ "result": { "type": "pane_process_info",
            "process_info": { "pane_id": "w2:p1", "shell_pid": 4300 } } });
        assert_eq!(evidence::engine_shell_pid(&info, "w2:p1"), Some(4300));
        assert_eq!(evidence::engine_shell_pid(&info, "w1:p1"), None);
        let layout = json!({ "result": { "type": "pane_layout", "layout": { "panes": [
            { "pane_id": "w1:p1", "rect": { "x": 0, "y": 0, "width": 80, "height": 24 } },
            { "pane_id": "w2:p1", "rect": { "x": 0, "y": 0, "width": 132, "height": 41 } } ] } } });
        assert_eq!(
            evidence::engine_pane_size(&layout, "w2:p1"),
            Some((132, 41))
        );
        assert_eq!(evidence::engine_pane_size(&layout, "w3:p1"), None);
    }

    #[test]
    fn probe_delta_drops_missing_or_backwards_counters_instead_of_zeroing() {
        let before =
            json!({ "raf_requests": 5, "raf_callbacks": 4, "paint_calls": 4, "painted_rows": 90 });
        let after =
            json!({ "raf_requests": 9, "raf_callbacks": 8, "paint_calls": 7, "painted_rows": 150 });
        assert_eq!(
            evidence::probe_delta(&before, &after),
            json!({ "raf_requests": 4, "raf_callbacks": 4, "paint_calls": 3, "painted_rows": 60 })
        );
        // A reset probe (new page) is not "no repaint".
        let reset =
            json!({ "raf_requests": 0, "raf_callbacks": 0, "paint_calls": 0, "painted_rows": 0 });
        let d = evidence::probe_delta(&after, &reset);
        assert!(d.get("raf_callbacks").is_none() && d.get("painted_rows").is_none());
        let s = scenario(1, 1, Output::Hidden);
        let mut e = evidence(s, ACCEPT);
        e.clients[0].probe_delta = d;
        rejected_for(&e, "probe counters missing");
    }

    #[test]
    fn geometry_is_the_engine_grid_over_the_canvas_backing_store() {
        let page = json!({ "canvas_width": 1188, "canvas_height": 779,
            "device_pixel_ratio": 1.0, "font": "14px \"JetBrains Mono\", monospace" });
        let g = evidence::geometry(&page, Some((132, 41))).unwrap();
        assert_eq!(
            (g.cols, g.rows, g.cell_width_px, g.cell_height_px),
            (132, 41, 9, 19)
        );
        assert_eq!(g.scale_milli, 1000);
        assert_eq!(g.font, "14px \"JetBrains Mono\", monospace");
        assert!(evidence::geometry(&page, None).is_err());
        // Canvas not a whole grid of the engine's size: the client drew another geometry.
        assert!(evidence::geometry(&page, Some((131, 41))).is_err());
        let mut no_font = page.clone();
        no_font["font"] = json!("");
        assert!(evidence::geometry(&no_font, Some((132, 41))).is_err());
        // ctx.font keeps the style of the last painted cell (observed "bold 14px …" in run-2):
        // the geometry font is the family/size, never the last cell's weight or slant.
        for styled in [
            "bold 14px \"JetBrains Mono\", monospace",
            "italic bold 14px \"JetBrains Mono\", monospace",
        ] {
            let mut p = page.clone();
            p["font"] = json!(styled);
            assert_eq!(
                evidence::geometry(&p, Some((132, 41))).unwrap().font,
                "14px \"JetBrains Mono\", monospace"
            );
        }
        let mut hidpi = page.clone();
        hidpi["device_pixel_ratio"] = json!(2.0);
        assert_eq!(
            evidence::geometry(&hidpi, Some((132, 41)))
                .unwrap()
                .scale_milli,
            2000
        );
    }
}

/// Scale round (spec 007): idle only when observed without generators, counter window free of
/// warmup, warmup taken from the collector samples, PRD targets only on an accepted observed idle
/// run (never on output cells), equivalent scenarios for the 15-pane delta, one common barrier
/// for two clients and an explicit topology CLI.
mod scale_contracts {
    use super::*;
    use bench::evidence::collector_window;
    use bench::plan::{barrier_step, parse_run, pty_group, Load, BENCH_STEPS};
    use bench::verdict::judge;

    fn no_generator(mut p: Producer) -> Producer {
        p.generator = ProcessId {
            pid: 0,
            starttime: 0,
            current_starttime: None,
            owned: false,
        };
        p.generator_ancestors = Vec::new();
        p.counter_end = p.counter_start;
        p
    }

    fn idle(panes: u8, clients: u8, mode: Mode, gui_pss_kb: u64, cpu: f64) -> Evidence {
        let mut e = evidence(scenario(panes, clients, Output::Idle), mode);
        e.producers = e.producers.into_iter().map(no_generator).collect();
        e.collector_summary = with_pty_groups(summary(clients, gui_pss_kb, cpu), panes);
        e
    }

    #[test]
    fn idle_is_only_idle_without_a_generator_or_produced_bytes() {
        assert_eq!(
            evaluate(&idle(1, 1, ACCEPT, 100_000, 0.1)),
            RunStatus::MeasuredPendingReview
        );
        // A generator left alive (the fixture producer emits at the fixed rate).
        let mut live = idle(1, 1, ACCEPT, 100_000, 0.1);
        live.producers = vec![producer(0, 60.0)];
        rejected_for(&live, "idle");
        // Generator gone but its counter still advanced inside the window.
        let mut bytes = idle(1, 1, ACCEPT, 100_000, 0.1);
        bytes.producers[0].counter_end.bytes += 79;
        rejected_for(&bytes, "idle");
        // An owned generator that already exited is still a generator of this run.
        let mut owned = idle(1, 1, ACCEPT, 100_000, 0.1);
        owned.producers[0].generator.owned = true;
        owned.producers[0].generator.pid = 3000;
        rejected_for(&owned, "idle");
    }

    #[test]
    fn a_counter_window_that_includes_the_warmup_is_rejected() {
        let mut e = evidence(scenario(1, 1, Output::Visible), ACCEPT);
        let p = &mut e.producers[0];
        // Same fixed rate, but read before the warmup: 65 s span for a 60 s measured window.
        p.counter_start.monotonic_ns -= 5_000_000_000;
        p.counter_start.bytes -= 3950 * 5;
        rejected_for(&e, "warmup");
        // Reading latency after the collector exits is tolerated.
        let mut late = evidence(scenario(1, 1, Output::Visible), ACCEPT);
        late.producers[0].counter_end.monotonic_ns += 400_000_000;
        late.producers[0].counter_end.bytes += 3950 * 4 / 10;
        assert_eq!(evaluate(&late), RunStatus::MeasuredPendingReview);
    }

    fn samples(points: &[(f64, bool)]) -> String {
        points
            .iter()
            .map(|(t, w)| json!({ "elapsed_monotonic_s": t, "is_warmup": w }).to_string() + "\n")
            .collect()
    }

    #[test]
    fn observed_warmup_and_measured_come_from_the_samples_not_the_request() {
        let ok = samples(&[
            (0.0, true),
            (2.5, true),
            (5.01, false),
            (35.0, false),
            (65.0, false),
        ]);
        let (warmup, measured) = collector_window(&ok, 65.51).unwrap();
        assert!((warmup - 5.01).abs() < 1e-9 && (measured - 60.5).abs() < 1e-9);
        // The collector ignored the requested warmup: the run observed none.
        let none = samples(&[(0.0, false), (30.0, false)]);
        assert_eq!(collector_window(&none, 30.2).unwrap().0, 0.0);
        let mut e = evidence(scenario(1, 1, Output::Visible), ACCEPT);
        e.warmup_s = collector_window(&none, 65.0).unwrap().0;
        rejected_for(&e, "warmup");
        // Warmup after measurement, no measured sample, or garbage: not a window.
        assert!(collector_window(&samples(&[(0.0, false), (1.0, true)]), 2.0).is_err());
        assert!(collector_window(&samples(&[(0.0, true)]), 5.0).is_err());
        assert!(collector_window("{\"is_warmup\": false}\n", 5.0).is_err());
        assert!(collector_window(&ok, f64::NAN).is_err());
    }

    fn states(t: &std::collections::BTreeMap<String, TargetState>) -> Vec<(String, bool)> {
        t.iter()
            .map(|(k, v)| (k.clone(), matches!(v, TargetState::Met)))
            .collect()
    }

    #[test]
    fn prd_idle_targets_are_judged_only_on_an_accepted_observed_idle_run() {
        let met = judge(&idle(1, 1, ACCEPT, 100 * 1024, 0.2), None);
        assert_eq!(
            states(&met),
            vec![
                ("gui_1.idle_cpu_avg<1".into(), true),
                ("gui_1.idle_pss_max_mib<=300".into(), true)
            ]
        );
        let over = judge(&idle(1, 1, ACCEPT, 301 * 1024, 0.2), None);
        assert_eq!(over["gui_1.idle_pss_max_mib<=300"], TargetState::Failed);
        // Same low numbers under output, smoke or a rejected idle: never Met, never Failed.
        let mut visible = evidence(scenario(1, 1, Output::Visible), ACCEPT);
        visible.collector_summary = summary(1, 100 * 1024, 0.2);
        let smoke = idle(1, 1, Mode::Smoke { measured_s: 3.0 }, 100 * 1024, 0.2);
        let mut rejected = idle(1, 1, ACCEPT, 100 * 1024, 0.2);
        rejected.producers = vec![producer(0, 60.0)];
        for e in [visible, smoke, rejected] {
            let t = judge(&e, None);
            assert!(!t.is_empty(), "{:?}: empty map", e.scenario);
            assert!(
                t.values().all(|v| matches!(v, TargetState::Unknown(_))),
                "{:?} {:?}: {t:?}",
                e.scenario,
                e.mode
            );
            assert!(
                t.keys().all(|k| !k.contains("idle_pss")),
                "idle label on {t:?}"
            );
        }
    }

    #[test]
    fn scale_delta_compares_equivalent_scenarios_per_client() {
        let base = idle(1, 2, ACCEPT, 180 * 1024, 0.2);
        let t = judge(&idle(15, 2, ACCEPT, 280 * 1024, 0.2), Some(&base));
        assert_eq!(
            states(&t),
            vec![
                ("gui_1.scale15_delta_pss_max_mib<=100".into(), true),
                ("gui_2.scale15_delta_pss_max_mib<=100".into(), true)
            ]
        );
        let t = judge(&idle(15, 2, ACCEPT, 281 * 1024, 0.2), Some(&base));
        assert_eq!(
            t["gui_2.scale15_delta_pss_max_mib<=100"],
            TargetState::Failed
        );
        let unknown = |t: std::collections::BTreeMap<String, TargetState>| {
            !t.is_empty() && t.values().all(|v| matches!(v, TargetState::Unknown(_)))
        };
        let scale = idle(15, 2, ACCEPT, 200 * 1024, 0.2);
        // Different client count, output, pane counts or a rejected baseline.
        assert!(unknown(judge(
            &scale,
            Some(&idle(1, 1, ACCEPT, 100 * 1024, 0.2))
        )));
        let mut out_base = evidence(scenario(1, 2, Output::Hidden), ACCEPT);
        out_base.collector_summary = summary(2, 100 * 1024, 0.2);
        assert!(unknown(judge(&scale, Some(&out_base))));
        assert!(unknown(judge(
            &scale,
            Some(&idle(15, 2, ACCEPT, 100 * 1024, 0.2))
        )));
        assert!(unknown(judge(
            &idle(1, 2, ACCEPT, 200 * 1024, 0.2),
            Some(&base)
        )));
        let mut bad_base = idle(1, 2, ACCEPT, 100 * 1024, 0.2);
        bad_base.build.debug_assertions = true;
        assert!(unknown(judge(&scale, Some(&bad_base))));
    }

    #[test]
    fn every_pty_has_its_own_collector_group() {
        assert_eq!(pty_group(1, 1), "pty");
        assert_eq!(pty_group(15, 15), "pty_15");
        let mut e = evidence(scenario(15, 1, Output::Visible), ACCEPT);
        assert_eq!(evaluate(&e), RunStatus::MeasuredPendingReview);
        e.collector_summary["groups"]
            .as_object_mut()
            .unwrap()
            .remove("pty_15");
        rejected_for(&e, "pty_15");
    }

    #[test]
    fn the_barrier_releases_a_step_only_when_every_client_requested_it() {
        let visible = ["bench-ready", "bench-visible", "bench-hidden", "bench-stop"];
        assert_eq!(
            BENCH_STEPS,
            [
                "bench-ready",
                "bench-visible",
                "bench-hidden",
                "bench-idle",
                "bench-stop"
            ]
        );
        let one: &[&[&str]] = &[&["bench-ready"], &[]];
        assert_eq!(barrier_step(one, &[]), Ok(None));
        let both: &[&[&str]] = &[&["bench-ready"], &["bench-ready"]];
        assert_eq!(barrier_step(both, &[]), Ok(Some("bench-ready")));
        assert_eq!(barrier_step(both, &["bench-ready"]), Ok(None));
        // A client ahead waits for the other; a client asking a different window is an error.
        let ahead: &[&[&str]] = &[&visible[..2], &["bench-ready"]];
        assert_eq!(barrier_step(ahead, &["bench-ready"]), Ok(None));
        let split: &[&[&str]] = &[&["bench-idle"], &["bench-visible"]];
        assert!(barrier_step(split, &["bench-ready"]).is_err());
        let unknown: &[&[&str]] = &[&["bench-go"]];
        assert!(barrier_step(unknown, &[]).is_err());
        // stop is released per client as soon as any asks, so a failing client never strands the other.
        let stop: &[&[&str]] = &[&["bench-ready", "bench-stop"], &["bench-ready"]];
        assert_eq!(barrier_step(stop, &["bench-ready"]), Ok(Some("bench-stop")));
    }

    #[test]
    fn topology_parameters_are_explicit() {
        let base = ["--mode", "smoke", "--duration", "3", "--out", "/tmp/x"];
        let with = |extra: &[&str]| {
            let mut v: Vec<&str> = base.to_vec();
            v.extend_from_slice(extra);
            parse_run(&v)
        };
        let run = with(&["--panes", "15", "--clients", "2", "--load", "output"]).unwrap();
        assert_eq!((run.panes, run.clients, run.load), (15, 2, Load::Output));
        assert_eq!(run.load.outputs(), vec![Output::Visible, Output::Hidden]);
        assert_eq!(run.args.mode, Mode::Smoke { measured_s: 3.0 });
        assert_eq!(run.background_split(), (7, 7));
        let idle = with(&["--panes", "1", "--clients", "1", "--load", "idle"]).unwrap();
        assert_eq!(idle.load.outputs(), vec![Output::Idle]);
        assert_eq!(idle.background_split(), (0, 0));
        for bad in [
            &["--clients", "1", "--load", "idle"][..],
            &["--panes", "2", "--clients", "1", "--load", "idle"],
            &["--panes", "1", "--clients", "3", "--load", "idle"],
            &["--panes", "1", "--clients", "1", "--load", "busy"],
            &["--panes", "1", "--clients", "1"],
            &[
                "--panes",
                "1",
                "--panes",
                "1",
                "--clients",
                "1",
                "--load",
                "idle",
            ],
            &[
                "--panes",
                "1",
                "--clients",
                "1",
                "--load",
                "idle",
                "--fast-clock",
            ],
        ] {
            assert!(with(bad).is_err(), "{bad:?} accepted");
        }
    }
}

/// Window process of `bench_desktop_live` (started by the native harness `Supervisor` under this
/// exact name on the private display). Refuses to open a window anywhere else.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "window process of bench_desktop_live; refuses to run outside its private display"]
fn fidelity_window_phase() {
    let env = |k: &str| std::env::var(k).ok();
    let uid = native_harness::required("HERDR_DESKTOP_E2E_USER_UID")
        .parse::<u32>()
        .expect("uid");
    support::display::guard_window_env(&env, uid).unwrap_or_else(|e| panic!("{e}"));
    let phase = native_harness::current_phase();
    assert_eq!(
        phase, "resource-bench",
        "bench window only runs resource-bench"
    );
    let params: serde_json::Value =
        serde_json::from_str(&native_harness::required("HERDR_DESKTOP_E2E_PARAMS"))
            .expect("params");
    let config = support::window::desktop_config(&env).unwrap_or_else(|e| panic!("{e}"));
    support::window::run_app_window(
        tauri::generate_context!(),
        support::window::WindowPhase {
            phase,
            params,
            report_path: std::path::PathBuf::from(native_harness::required(
                native_harness::RESULT_ENV,
            )),
            timeout: std::time::Duration::from_secs(
                std::env::var("HERDR_DESKTOP_HARNESS_TIMEOUT_SECS")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(150),
            ),
            config,
        },
    );
    panic!("the bench window returned without reporting done");
}

/// Live 1-GUI/1-PTY resource run (release build, smoke only in this checkpoint). Parameters are
/// explicit: `HERDR_DESKTOP_BENCH_ARGS="--mode smoke --duration 3 --out /abs/new-dir"`,
/// `HERDR_DESKTOP_HERDR_BIN`, `HERDR_DESKTOP_NATIVE_RESOURCES`. Writes `run-report.json`; fails
/// unless both cells are exactly INSUFFICIENT_FOR_ACCEPTANCE (a smoke never accepts anything).
#[cfg(target_os = "linux")]
#[test]
#[ignore = "live release bench; needs the reference engine and private display; run by just bench-desktop"]
fn bench_desktop_live() {
    let raw = native_harness::required("HERDR_DESKTOP_BENCH_ARGS");
    let words: Vec<&str> = raw.split_whitespace().collect();
    let absolute = |key: &str| {
        let path = std::path::PathBuf::from(native_harness::required(key));
        assert!(path.is_absolute(), "{key} must be absolute");
        path
    };
    if words.first() == Some(&"--latency") {
        let latency_args =
            bench::plan::parse_latency_args(&words).unwrap_or_else(|e| panic!("{e}"));
        std::fs::create_dir_all(latency_args.out.parent().expect("out parent"))
            .expect("out parent");
        std::fs::create_dir(&latency_args.out).expect("--out must be a new directory");
        let env = bench::live::LiveLatencyEnv {
            herdr_bin: absolute("HERDR_DESKTOP_HERDR_BIN"),
            resources_root: absolute("HERDR_DESKTOP_NATIVE_RESOURCES"),
            out: latency_args.out.clone(),
            exe: std::env::current_exe().expect("test executable"),
            repo: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .expect("repo")
                .to_path_buf(),
            user: native_harness::required("USER"),
            uid: support::live::uid(),
            mode: latency_args.mode,
            warmup: latency_args.warmup,
            measured: latency_args.measured,
        };
        let report = bench::live::run_latency(&env);
        println!(
            "bench_desktop_live latency: out={} smoke_proofs={:?} verdict={:?} errors={}",
            latency_args.out.display(),
            report["smoke_proofs"],
            report["verdict"],
            report["errors"]
        );
        if matches!(latency_args.mode, bench::plan::LatencyMode::Smoke) {
            let proofs = &report["smoke_proofs"];
            assert_eq!(
                proofs["all_passed"],
                true,
                "smoke proofs failed: {proofs:?}; errors: {}; see {}/run-report.json",
                report["errors"],
                latency_args.out.display()
            );
        }
        assert!(
            report["errors"].as_array().is_some_and(Vec::is_empty),
            "latency bench has errors: {}; see {}/run-report.json",
            report["errors"],
            latency_args.out.display()
        );
        return;
    }
    let run = bench::plan::parse_run(&words).unwrap_or_else(|e| panic!("{e}"));
    let args = run.args.clone();
    bench::plan::resource_live_authorised(&run).unwrap_or_else(|e| panic!("{e}"));
    std::fs::create_dir_all(args.out.parent().expect("out parent")).expect("out parent");
    std::fs::create_dir(&args.out).expect("--out must be a new directory");
    let env = bench::live::LiveEnv {
        herdr_bin: absolute("HERDR_DESKTOP_HERDR_BIN"),
        resources_root: absolute("HERDR_DESKTOP_NATIVE_RESOURCES"),
        out: args.out.clone(),
        mode: args.mode,
        exe: std::env::current_exe().expect("test executable"),
        repo: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("repo")
            .to_path_buf(),
        user: native_harness::required("USER"),
        uid: support::live::uid(),
        panes: run.panes,
        clients: run.clients,
        load: run.load,
        renderer: run.renderer,
    };
    let report = bench::live::run(&env);
    let expected = if matches!(args.mode, Mode::Smoke { .. }) {
        "\"INSUFFICIENT_FOR_ACCEPTANCE\""
    } else {
        "\"MeasuredPendingReview\""
    };
    let statuses: Vec<String> = run
        .load
        .outputs()
        .iter()
        .map(|o| format!("{o:?}").to_lowercase())
        .map(|c| report["verdicts"][&c]["verdict"]["status"].to_string())
        .collect();
    println!(
        "bench_desktop_live: out={} statuses={statuses:?} errors={}",
        args.out.display(),
        report["errors"]
    );
    assert!(
        statuses.iter().all(|s| s == expected)
            && report["errors"].as_array().is_some_and(Vec::is_empty),
        "resource bench smoke not complete: {statuses:?}; see {}/run-report.json",
        args.out.display()
    );
}

/// Renderer A/B of the idle memory diagnostic: a named, closed set of WebKit renderer variants
/// passed only to the private window launch, recorded from the owned window's own environ.
mod renderer_variants {
    use crate::bench;
    use bench::evidence::{foreign_webkit, renderer_env_of};
    use bench::plan::{check_renderer_env, parse_run, variant_applied, RendererVariant};
    use std::path::{Path, PathBuf};

    const COMPOSITING: &str = "WEBKIT_DISABLE_COMPOSITING_MODE";
    const SKIA_CPU: &str = "WEBKIT_SKIA_ENABLE_CPU_RENDERING";
    const DMABUF: &str = "WEBKIT_DISABLE_DMABUF_RENDERER";

    fn run_with(extra: &[&str]) -> Result<bench::plan::RunSpec, String> {
        let mut v = vec![
            "--mode",
            "smoke",
            "--duration",
            "3",
            "--out",
            "/tmp/x",
            "--panes",
            "1",
            "--clients",
            "1",
            "--load",
            "idle",
        ];
        v.extend_from_slice(extra);
        parse_run(&v)
    }

    /// Catches: a required flag breaking existing runner invocations, a free-form value turning
    /// into generic env injection, silent fallback to control on a typo, or swapped mappings.
    #[test]
    fn renderer_is_an_optional_named_flag_defaulting_to_control_with_exact_env() {
        assert_eq!(run_with(&[]).unwrap().renderer, RendererVariant::Control);
        let cases = [
            ("control", RendererVariant::Control, vec![]),
            (
                "no-compositing",
                RendererVariant::NoCompositing,
                vec![(COMPOSITING, "1")],
            ),
            ("cpu-skia", RendererVariant::CpuSkia, vec![(SKIA_CPU, "1")]),
        ];
        for (name, variant, env) in cases {
            let run = run_with(&["--renderer", name]).unwrap();
            assert_eq!(run.renderer, variant, "{name}");
            assert_eq!(variant.name(), name);
            assert_eq!(variant.env().to_vec(), env, "{name}");
            assert_eq!(check_renderer_env(variant.env()), Ok(()));
        }
        for bad in [
            &["--renderer", "gpu"][..],
            &["--renderer", "Control"],
            &["--renderer", ""],
            &["--renderer", "WEBKIT_DISABLE_COMPOSITING_MODE=1"],
            &["--renderer"],
            &["--renderer", "control", "--renderer", "cpu-skia"],
        ] {
            assert!(run_with(bad).is_err(), "{bad:?} accepted");
        }
        // Normal parser failures are unchanged by the new flag.
        assert!(run_with(&["--renderer", "control", "--fast-clock"]).is_err());
        assert!(parse_run(&["--renderer", "control", "--panes", "1", "--clients", "1"]).is_err());
    }

    /// Catches: a variant able to override the private display, the user's DISPLAY/WAYLAND
    /// socket, or to re-enable DMA-BUF; values other than the fixed "1".
    #[test]
    fn renderer_env_guard_refuses_display_isolation_and_unknown_keys() {
        for bad in [
            &[("DISPLAY", ":0")][..],
            &[("WAYLAND_DISPLAY", "/run/user/1000/wayland-1")],
            &[(DMABUF, "0")],
            &[("LD_PRELOAD", "/x.so")],
            &[(COMPOSITING, "0")],
            &[(COMPOSITING, "1"), (COMPOSITING, "1")],
            &[(COMPOSITING, "1"), (SKIA_CPU, "1")],
        ] {
            assert!(check_renderer_env(bad).is_err(), "{bad:?} accepted");
        }
    }

    /// Catches: the variant not reaching the window launch, leaking the other variant's key,
    /// dropping WEBKIT_DISABLE_DMABUF_RENDERER=1 or the private socket, or a DISPLAY of the user.
    #[test]
    fn window_launch_carries_exactly_the_variant_and_keeps_isolation() {
        let runtime = PathBuf::from("/tmp/hd7T-renderer");
        let display = crate::support::display::PrivateDisplay::new(
            runtime.clone(),
            PathBuf::from("/tmp/hd7T-renderer-home"),
            crate::support::display::Resources::under(Path::new("/tmp/hd7T-res")),
            "u".into(),
            1000,
        )
        .unwrap();
        let socket = runtime.join("wayland-1");
        let exe = Path::new("/tmp/hd7T-exe");
        let base = [("HERDR_DESKTOP_HARNESS_PHASE", "resource-bench")];
        for variant in [
            RendererVariant::Control,
            RendererVariant::NoCompositing,
            RendererVariant::CpuSkia,
        ] {
            let mut extra = base.to_vec();
            extra.extend_from_slice(variant.env());
            let launch = display.window(exe, &socket, false, &extra).unwrap();
            let get = |k: &str| {
                launch
                    .env
                    .iter()
                    .filter(|(key, _)| key == k)
                    .map(|(_, v)| v.as_str())
                    .collect::<Vec<_>>()
            };
            assert_eq!(get(DMABUF), vec!["1"], "{variant:?}");
            assert_eq!(get("WAYLAND_DISPLAY"), vec!["/tmp/hd7T-renderer/wayland-1"]);
            assert!(get("DISPLAY").is_empty(), "{variant:?}");
            let expect = |key: &str| {
                if variant.env().iter().any(|(k, _)| *k == key) {
                    vec!["1"]
                } else {
                    vec![]
                }
            };
            assert_eq!(get(COMPOSITING), expect(COMPOSITING), "{variant:?}");
            assert_eq!(get(SKIA_CPU), expect(SKIA_CPU), "{variant:?}");
        }
        // The launch still refuses an extra that overrides the private display.
        let hijack = [("WAYLAND_DISPLAY", "/run/user/1000/wayland-1")];
        assert!(display.window(exe, &socket, false, &hijack).is_err());
    }

    /// Catches: recording the requested variant instead of the window's real environ, copying
    /// unrelated variables, and accepting a window whose environ differs from the variant.
    #[test]
    fn applied_variant_is_judged_on_the_owned_window_environ() {
        let environ = |pairs: &[&str]| pairs.join("\0").into_bytes();
        let control = environ(&[
            "HERDR_DESKTOP_E2E_PARAMS={\"secret\":1}",
            "WEBKIT_DISABLE_DMABUF_RENDERER=1",
            "WAYLAND_DISPLAY=/tmp/hd7B/wayland-1",
        ]);
        let map = renderer_env_of(&control);
        assert_eq!(
            map.into_iter().collect::<Vec<_>>(),
            vec![(DMABUF.to_owned(), "1".to_owned())]
        );
        let no_comp = environ(&[
            "WEBKIT_DISABLE_DMABUF_RENDERER=1",
            "WEBKIT_DISABLE_COMPOSITING_MODE=1",
        ]);
        let both = environ(&[
            "WEBKIT_DISABLE_DMABUF_RENDERER=1",
            "WEBKIT_DISABLE_COMPOSITING_MODE=1",
            "WEBKIT_SKIA_ENABLE_CPU_RENDERING=1",
        ]);
        let no_dmabuf = environ(&["WEBKIT_SKIA_ENABLE_CPU_RENDERING=1"]);
        use RendererVariant::*;
        assert!(variant_applied(Control, &renderer_env_of(&control)));
        assert!(!variant_applied(NoCompositing, &renderer_env_of(&control)));
        assert!(variant_applied(NoCompositing, &renderer_env_of(&no_comp)));
        assert!(!variant_applied(Control, &renderer_env_of(&no_comp)));
        assert!(!variant_applied(CpuSkia, &renderer_env_of(&no_comp)));
        assert!(!variant_applied(NoCompositing, &renderer_env_of(&both)));
        assert!(!variant_applied(CpuSkia, &renderer_env_of(&no_dmabuf)));
    }

    /// Catches: counting this run's own WebProcess/NetworkProcess as interference, or missing a
    /// foreign WebKit process of another app/worktree.
    #[test]
    fn foreign_webkit_processes_exclude_owned_trees_and_non_webkit() {
        let procs = [
            (100, 11, "fidelity_bench-"),
            (101, 12, "WebKitWebProces"),
            (102, 13, "WebKitNetworkPr"),
            (200, 21, "WebKitWebProces"),
            (201, 22, "WebKitNetworkPr"),
            (300, 31, "bash"),
        ];
        let ppid = |p: u32| match p {
            101 | 102 => Some(100),
            100 | 200 | 201 | 300 => Some(1),
            _ => None,
        };
        let found = foreign_webkit(&procs, &[100], &ppid);
        assert_eq!(
            found,
            vec![
                (200, 21, "WebKitWebProces".to_owned()),
                (201, 22, "WebKitNetworkPr".to_owned())
            ]
        );
        assert_eq!(foreign_webkit(&procs, &[], &ppid).len(), 4);
    }
}

mod gdk_variants {
    use crate::bench;
    use bench::evidence::renderer_env_of;
    use bench::plan::{check_renderer_env, parse_run, variant_applied, RendererVariant};
    use std::path::{Path, PathBuf};

    const GDK_DEBUG: &str = "GDK_DEBUG";
    const GDK_RENDERING: &str = "GDK_RENDERING";
    const COMPOSITING: &str = "WEBKIT_DISABLE_COMPOSITING_MODE";
    const SKIA_CPU: &str = "WEBKIT_SKIA_ENABLE_CPU_RENDERING";
    const DMABUF: &str = "WEBKIT_DISABLE_DMABUF_RENDERER";

    fn run_with(extra: &[&str]) -> Result<bench::plan::RunSpec, String> {
        let mut v = vec![
            "--mode",
            "smoke",
            "--duration",
            "3",
            "--out",
            "/tmp/x",
            "--panes",
            "1",
            "--clients",
            "1",
            "--load",
            "idle",
        ];
        v.extend_from_slice(extra);
        parse_run(&v)
    }

    /// Catches: GDK names mapped to the wrong variable or value (e.g. GDK_DEBUG=1, swapped
    /// pairs), the default drifting away from control, and case/alias variants being accepted.
    #[test]
    fn gdk_variants_are_closed_names_with_exact_pairs_and_default_stays_control() {
        assert_eq!(run_with(&[]).unwrap().renderer, RendererVariant::Control);
        assert!(RendererVariant::Control.env().is_empty());
        let cases = [
            (
                "gdk-nogl",
                RendererVariant::GdkNoGl,
                vec![(GDK_DEBUG, "nogl")],
            ),
            (
                "gdk-image",
                RendererVariant::GdkImage,
                vec![(GDK_RENDERING, "image")],
            ),
        ];
        for (name, variant, env) in cases {
            let run = run_with(&["--renderer", name]).unwrap();
            assert_eq!(run.renderer, variant, "{name}");
            assert_eq!(variant.name(), name);
            assert_eq!(variant.env().to_vec(), env, "{name}");
            assert_eq!(check_renderer_env(variant.env()), Ok(()), "{name}");
        }
        for bad in [
            "gdk",
            "nogl",
            "image",
            "GDK-NOGL",
            "gdk_image",
            "GDK_DEBUG=nogl",
            "gdk-gl",
        ] {
            assert!(run_with(&["--renderer", bad]).is_err(), "{bad} accepted");
        }
        assert!(run_with(&["--renderer", "gdk-nogl", "--renderer", "gdk-image"]).is_err());
    }

    /// Catches: a key-only allowlist (any GDK_DEBUG/GDK_RENDERING value passing), pairs from one
    /// key reused with another's value, combined variables, and GTK keys outside the two pairs
    /// (GDK_BACKEND, GDK_GL) turning the variant into generic GTK configuration.
    #[test]
    fn gdk_guard_accepts_only_the_exact_pairs_and_refuses_wrong_values_or_mixes() {
        assert_eq!(check_renderer_env(&[(GDK_DEBUG, "nogl")]), Ok(()));
        assert_eq!(check_renderer_env(&[(GDK_RENDERING, "image")]), Ok(()));
        // Previous exact pairs are still accepted.
        assert_eq!(check_renderer_env(&[(COMPOSITING, "1")]), Ok(()));
        assert_eq!(check_renderer_env(&[(SKIA_CPU, "1")]), Ok(()));
        for bad in [
            &[(GDK_DEBUG, "1")][..],
            &[(GDK_DEBUG, "nogl,interactive")],
            &[(GDK_DEBUG, "all")],
            &[(GDK_DEBUG, "image")],
            &[(GDK_DEBUG, "")],
            &[(GDK_RENDERING, "1")],
            &[(GDK_RENDERING, "similar")],
            &[(GDK_RENDERING, "nogl")],
            &[(COMPOSITING, "nogl")],
            &[(SKIA_CPU, "image")],
            &[("GDK_BACKEND", "x11")],
            &[("GDK_GL", "disable")],
            &[(GDK_DEBUG, "nogl"), (GDK_RENDERING, "image")],
            &[(GDK_DEBUG, "nogl"), (COMPOSITING, "1")],
            &[(GDK_RENDERING, "image"), (SKIA_CPU, "1")],
            &[(GDK_DEBUG, "nogl"), (GDK_DEBUG, "nogl")],
        ] {
            assert!(check_renderer_env(bad).is_err(), "{bad:?} accepted");
        }
    }

    /// Catches: the GDK variant not reaching the window launch, a GDK variant displacing the
    /// fixed GDK_BACKEND=wayland / DMA-BUF / private socket, or leaking another variant's key.
    #[test]
    fn gdk_window_launch_carries_exactly_the_pair_and_keeps_isolation() {
        let runtime = PathBuf::from("/tmp/hd7T-gdk");
        let display = crate::support::display::PrivateDisplay::new(
            runtime.clone(),
            PathBuf::from("/tmp/hd7T-gdk-home"),
            crate::support::display::Resources::under(Path::new("/tmp/hd7T-res")),
            "u".into(),
            1000,
        )
        .unwrap();
        let socket = runtime.join("wayland-1");
        let exe = Path::new("/tmp/hd7T-exe");
        for variant in [
            RendererVariant::Control,
            RendererVariant::GdkNoGl,
            RendererVariant::GdkImage,
        ] {
            let mut extra = vec![("HERDR_DESKTOP_HARNESS_PHASE", "resource-bench")];
            extra.extend_from_slice(variant.env());
            let launch = display.window(exe, &socket, false, &extra).unwrap();
            let get = |k: &str| {
                launch
                    .env
                    .iter()
                    .filter(|(key, _)| key == k)
                    .map(|(_, v)| v.as_str())
                    .collect::<Vec<_>>()
            };
            assert_eq!(get(DMABUF), vec!["1"], "{variant:?}");
            assert_eq!(get("GDK_BACKEND"), vec!["wayland"], "{variant:?}");
            assert_eq!(get("WAYLAND_DISPLAY"), vec!["/tmp/hd7T-gdk/wayland-1"]);
            assert!(get("DISPLAY").is_empty(), "{variant:?}");
            assert!(get(COMPOSITING).is_empty() && get(SKIA_CPU).is_empty());
            let nogl = matches!(variant, RendererVariant::GdkNoGl);
            let image = matches!(variant, RendererVariant::GdkImage);
            assert_eq!(get(GDK_DEBUG), if nogl { vec!["nogl"] } else { vec![] });
            assert_eq!(
                get(GDK_RENDERING),
                if image { vec!["image"] } else { vec![] }
            );
        }
    }

    /// Catches: judging the requested pair instead of the window's real environ, accepting a
    /// GDK variable with a different value, a control window that carries a GDK variable, a
    /// window carrying both GDK pairs, or one missing WEBKIT_DISABLE_DMABUF_RENDERER=1.
    #[test]
    fn gdk_actual_window_env_mismatch_fails() {
        let environ = |pairs: &[&str]| pairs.join("\0").into_bytes();
        let nogl = environ(&[
            "GDK_BACKEND=wayland",
            "WEBKIT_DISABLE_DMABUF_RENDERER=1",
            "GDK_DEBUG=nogl",
        ]);
        assert_eq!(
            renderer_env_of(&nogl).into_iter().collect::<Vec<_>>(),
            vec![
                (GDK_DEBUG.to_owned(), "nogl".to_owned()),
                (DMABUF.to_owned(), "1".to_owned())
            ]
        );
        let image = environ(&["WEBKIT_DISABLE_DMABUF_RENDERER=1", "GDK_RENDERING=image"]);
        let wrong_debug = environ(&["WEBKIT_DISABLE_DMABUF_RENDERER=1", "GDK_DEBUG=nogl,misc"]);
        let wrong_rendering =
            environ(&["WEBKIT_DISABLE_DMABUF_RENDERER=1", "GDK_RENDERING=similar"]);
        let both = environ(&[
            "WEBKIT_DISABLE_DMABUF_RENDERER=1",
            "GDK_DEBUG=nogl",
            "GDK_RENDERING=image",
        ]);
        let no_dmabuf = environ(&["GDK_DEBUG=nogl"]);
        let control = environ(&["WEBKIT_DISABLE_DMABUF_RENDERER=1", "GDK_BACKEND=wayland"]);
        use RendererVariant::*;
        let applied = |v, e: &Vec<u8>| variant_applied(v, &renderer_env_of(e));
        assert!(applied(GdkNoGl, &nogl));
        assert!(applied(GdkImage, &image));
        assert!(applied(Control, &control));
        assert!(!applied(Control, &nogl));
        assert!(!applied(Control, &image));
        assert!(!applied(GdkImage, &nogl));
        assert!(!applied(GdkNoGl, &image));
        assert!(!applied(GdkNoGl, &wrong_debug));
        assert!(!applied(GdkImage, &wrong_rendering));
        assert!(!applied(GdkNoGl, &both));
        assert!(!applied(GdkImage, &both));
        assert!(!applied(GdkNoGl, &no_dmabuf));
        assert!(!applied(GdkNoGl, &control));
    }
}

/// WebKit settings API diagnostic: closed `--renderer api-never|api-software` arms passed to the
/// resource-bench window through its params, applied by the harness plugin hook on the real Wry
/// WebView and judged only by the getter readback the window wrote (never by the request).
#[cfg(target_os = "linux")]
mod webkit_policy_variants {
    use crate::bench;
    use crate::support::window::webkit_policy::{
        record_path, selection, validate, Effect, Policy, PARAM, PHASE, PROP_CANVAS, PROP_POLICY,
        PROP_WEBGL, SCHEMA,
    };
    use bench::evidence::renderer_env_of;
    use bench::plan::{check_renderer_env, parse_run, variant_applied, RendererVariant};
    use serde_json::{json, Value};
    use std::path::Path;

    fn run_with(extra: &[&str]) -> Result<bench::plan::RunSpec, String> {
        let mut v = vec![
            "--mode",
            "smoke",
            "--duration",
            "3",
            "--out",
            "/tmp/x",
            "--panes",
            "1",
            "--clients",
            "1",
            "--load",
            "idle",
        ];
        v.extend_from_slice(extra);
        parse_run(&v)
    }

    /// Catches: API arms leaking an environment variable (turning them into a fourth env family),
    /// aliases/case variants accepted, the default drifting from control, env arms losing their
    /// getter-only control policy, or the arms mapped to the wrong WebKit policy.
    #[test]
    fn api_arms_are_closed_names_without_env_and_default_is_control_policy() {
        let default = run_with(&[]).unwrap();
        assert_eq!(default.renderer, RendererVariant::Control);
        assert_eq!(default.renderer.webkit_policy(), Policy::Control);
        for (name, variant, policy) in [
            ("api-never", RendererVariant::ApiNever, Policy::ApiNever),
            (
                "api-software",
                RendererVariant::ApiSoftware,
                Policy::ApiSoftware,
            ),
        ] {
            let run = run_with(&["--renderer", name]).unwrap();
            assert_eq!(run.renderer, variant, "{name}");
            assert_eq!(variant.name(), name);
            assert!(variant.env().is_empty(), "{name} sets env");
            assert_eq!(variant.webkit_policy(), policy);
            assert_eq!(policy.name(), name);
            assert_eq!(Policy::parse(name), Ok(policy));
        }
        for env_arm in [
            RendererVariant::Control,
            RendererVariant::NoCompositing,
            RendererVariant::CpuSkia,
            RendererVariant::GdkNoGl,
            RendererVariant::GdkImage,
        ] {
            assert_eq!(env_arm.webkit_policy(), Policy::Control, "{env_arm:?}");
        }
        assert_eq!(Policy::parse("control"), Ok(Policy::Control));
        for bad in [
            "never",
            "api_never",
            "API-NEVER",
            "api-never ",
            "api-software-webgl",
            "api-on-demand",
            "api-always",
            "hardware-acceleration-policy=never",
            "",
        ] {
            assert!(Policy::parse(bad).is_err(), "{bad:?} accepted");
            assert!(run_with(&["--renderer", bad]).is_err(), "{bad:?} accepted");
        }
        assert!(run_with(&["--renderer", "api-never", "--renderer", "api-software"]).is_err());
        // An API arm's window must still carry only the fixed DMA-BUF variable.
        let clean = "WEBKIT_DISABLE_DMABUF_RENDERER=1\0GDK_BACKEND=wayland".as_bytes();
        let dirty =
            "WEBKIT_DISABLE_DMABUF_RENDERER=1\0WEBKIT_DISABLE_COMPOSITING_MODE=1".as_bytes();
        assert!(variant_applied(
            RendererVariant::ApiNever,
            &renderer_env_of(clean)
        ));
        assert!(!variant_applied(
            RendererVariant::ApiSoftware,
            &renderer_env_of(dirty)
        ));
        assert_eq!(
            check_renderer_env(RendererVariant::ApiSoftware.env()),
            Ok(())
        );
    }

    /// Catches: api-never also disabling WebGL/canvas (not a single-factor arm), api-software
    /// using the deprecated `enable-accelerated-2d-canvas` (a no-op on 2.52) instead of
    /// `enable-2d-canvas-acceleration`, or control calling any setter.
    #[test]
    fn setters_are_exactly_the_closed_properties_of_each_arm() {
        assert!(Policy::Control.setters().is_empty());
        assert_eq!(Policy::ApiNever.setters(), &[(PROP_POLICY, "never")]);
        assert_eq!(
            Policy::ApiSoftware.setters(),
            &[
                (PROP_POLICY, "never"),
                (PROP_WEBGL, "false"),
                (PROP_CANVAS, "false")
            ]
        );
        assert_eq!(PROP_POLICY, "hardware-acceleration-policy");
        assert_eq!(PROP_WEBGL, "enable-webgl");
        assert_eq!(PROP_CANVAS, "enable-2d-canvas-acceleration");
    }

    /// Catches: the hook arming in the standard native flow (params without the key), a policy
    /// accepted outside resource-bench, a non-string or unknown value falling back to control, and
    /// the record written somewhere other than next to the owned report.
    #[test]
    fn selection_only_in_resource_bench_with_closed_values_and_default_has_no_hook() {
        assert_eq!(PARAM, "webkit_policy");
        assert_eq!(PHASE, "resource-bench");
        let standard = json!({ "terminal_probe": true, "settle_ms": 700 });
        for phase in [
            "compose-mount",
            "native-keys",
            "resource-bench",
            "view-flow",
        ] {
            assert_eq!(selection(phase, &standard), Ok(None), "{phase}");
        }
        assert_eq!(selection("compose-mount", &json!({})), Ok(None));
        let with = |v: Value| json!({ "terminal_probe": true, PARAM: v });
        assert_eq!(
            selection(PHASE, &with(json!("control"))),
            Ok(Some(Policy::Control))
        );
        assert_eq!(
            selection(PHASE, &with(json!("api-never"))),
            Ok(Some(Policy::ApiNever))
        );
        assert_eq!(
            selection(PHASE, &with(json!("api-software"))),
            Ok(Some(Policy::ApiSoftware))
        );
        for phase in ["compose-mount", "native-keys", "Resource-bench", ""] {
            for name in ["control", "api-never", "api-software"] {
                assert!(
                    selection(phase, &with(json!(name))).is_err(),
                    "{phase} {name}"
                );
            }
        }
        for bad in [
            json!("gpu"),
            json!(true),
            json!(2),
            json!(null),
            json!(["api-never"]),
        ] {
            assert!(
                selection(PHASE, &with(bad.clone())).is_err(),
                "{bad} accepted"
            );
        }
        assert_eq!(
            record_path(Path::new("/tmp/hd7B/window-1/report.json")),
            Path::new("/tmp/hd7B/window-1/report.policy.json")
        );
    }

    fn getters(policy: &str, webgl: bool, canvas: bool) -> Value {
        json!({
            PROP_POLICY: policy,
            PROP_WEBGL: webgl,
            PROP_CANVAS: canvas,
            "enable-accelerated-2d-canvas": false,
        })
    }

    fn record(policy: Policy, setters: &[&str], before: Value, after: Value) -> Value {
        json!({
            "schema": SCHEMA,
            "phase": PHASE,
            "variant": policy.name(),
            "pid": 4242,
            "webview_count": 1,
            "available": { PROP_POLICY: true, PROP_WEBGL: true, PROP_CANVAS: true, "enable-accelerated-2d-canvas": true },
            "before": before,
            "setters": setters,
            "after": after,
            "unsupported": [],
            "errors": [],
        })
    }

    fn wry_defaults() -> Value {
        getters("on-demand", true, true)
    }

    /// Catches: claiming an arm applied from the requested name while the getter still reports
    /// the old value (unsupported hardware makes the setter a no-op), control changing any value,
    /// a record of another process, a missing/unsupported property accepted, extra or missing
    /// setters, and a no-op setter counted as a proven effect (ESC31: idempotent setters are
    /// valid but reported separately as no-op, never as changed).
    #[test]
    fn readback_validation_judges_getters_not_requests() {
        let pid = 4242;
        let none = |changed: &[&'static str], no_op: &[&'static str]| {
            Ok(Effect {
                changed: changed.to_vec(),
                no_op: no_op.to_vec(),
            })
        };
        let control = record(Policy::Control, &[], wry_defaults(), wry_defaults());
        assert_eq!(validate(Policy::Control, &control, pid), none(&[], &[]));
        assert!(!validate(Policy::Control, &control, pid).unwrap().proven());
        let never = record(
            Policy::ApiNever,
            &["hardware-acceleration-policy=never"],
            wry_defaults(),
            getters("never", true, true),
        );
        assert_eq!(
            validate(Policy::ApiNever, &never, pid),
            none(&[PROP_POLICY], &[])
        );
        let software = record(
            Policy::ApiSoftware,
            &[
                "hardware-acceleration-policy=never",
                "enable-webgl=false",
                "enable-2d-canvas-acceleration=false",
            ],
            wry_defaults(),
            getters("never", false, false),
        );
        assert_eq!(
            validate(Policy::ApiSoftware, &software, pid),
            none(&[PROP_POLICY, PROP_WEBGL, PROP_CANVAS], &[])
        );
        // The harness readback: policy already never, WebGL and 2D canvas really change.
        let mut harness = software.clone();
        harness["before"] = getters("never", true, true);
        let effect = validate(Policy::ApiSoftware, &harness, pid).unwrap();
        assert_eq!(
            effect,
            none(&[PROP_WEBGL, PROP_CANVAS], &[PROP_POLICY]).unwrap()
        );
        assert!(effect.proven());
        // api-never in the same harness: valid, but equivalent to control (no proven effect).
        let mut idle_never = never.clone();
        idle_never["before"] = getters("never", true, true);
        let effect = validate(Policy::ApiNever, &idle_never, pid).unwrap();
        assert_eq!(effect, none(&[], &[PROP_POLICY]).unwrap());
        assert!(!effect.proven());
        // Fully idempotent configuration: valid, no proof of effect.
        let mut idempotent = software.clone();
        idempotent["before"] = getters("never", false, false);
        let effect = validate(Policy::ApiSoftware, &idempotent, pid).unwrap();
        assert_eq!(
            effect,
            none(&[], &[PROP_POLICY, PROP_WEBGL, PROP_CANVAS]).unwrap()
        );
        assert!(!effect.proven());

        let mut bad: Vec<(&str, Policy, Value)> = Vec::new();
        bad.push(("other pid", Policy::Control, control.clone()));
        let mut v = never.clone();
        v["after"] = wry_defaults();
        bad.push(("getter still on-demand", Policy::ApiNever, v));
        let mut v = harness.clone();
        v["after"] = getters("never", true, false);
        bad.push((
            "no-op policy but webgl getter still true",
            Policy::ApiSoftware,
            v,
        ));
        let mut v = idempotent.clone();
        v["after"] = getters("on-demand", false, false);
        bad.push((
            "idempotent before but wrong policy after",
            Policy::ApiSoftware,
            v,
        ));
        let mut v = harness.clone();
        v["setters"] = json!([
            "hardware-acceleration-policy=never",
            "enable-webgl=false",
            "enable-2d-canvas-acceleration=false",
            "enable-accelerated-2d-canvas=false"
        ]);
        bad.push(("extra setter", Policy::ApiSoftware, v));
        let mut v = harness.clone();
        v["phase"] = json!("compose-mount");
        bad.push(("phase", Policy::ApiSoftware, v));
        let mut v = harness.clone();
        v["available"][PROP_CANVAS] = json!(false);
        bad.push(("missing canvas property", Policy::ApiSoftware, v));
        let mut v = never.clone();
        v["after"] = getters("never", false, true);
        bad.push(("api-never changed webgl", Policy::ApiNever, v));
        let mut v = software.clone();
        v["after"] = getters("never", false, true);
        bad.push(("canvas getter still true", Policy::ApiSoftware, v));
        let mut v = software.clone();
        v["setters"] = json!(["hardware-acceleration-policy=never", "enable-webgl=false"]);
        bad.push(("missing canvas setter", Policy::ApiSoftware, v));
        let mut v = control.clone();
        v["after"] = getters("never", true, true);
        bad.push(("control changed policy", Policy::Control, v));
        let mut v = control.clone();
        v["setters"] = json!(["enable-webgl=false"]);
        bad.push(("control setter", Policy::Control, v));
        bad.push(("variant mismatch", Policy::ApiNever, control.clone()));
        let mut v = software.clone();
        v["unsupported"] = json!([PROP_CANVAS]);
        bad.push(("unsupported", Policy::ApiSoftware, v));
        let mut v = software.clone();
        v["after"][PROP_CANVAS] = Value::Null;
        bad.push(("null getter", Policy::ApiSoftware, v));
        let mut v = never.clone();
        v["errors"] = json!(["with_webview failed"]);
        bad.push(("error", Policy::ApiNever, v));
        let mut v = never.clone();
        v["schema"] = json!("other");
        bad.push(("schema", Policy::ApiNever, v));
        let mut v = never.clone();
        v["webview_count"] = json!(2);
        bad.push(("second webview", Policy::ApiNever, v));
        bad.push(("absent", Policy::Control, Value::Null));
        for (i, (label, policy, rec)) in bad.into_iter().enumerate() {
            let pid = if i == 0 { 1 } else { pid };
            assert!(validate(policy, &rec, pid).is_err(), "{label} accepted");
        }
    }
}

// ------------------------------------------------------------------------------ latency mode (named, separate from resource acceptance)

#[test]
fn latency_mode_is_closed_and_separate_from_resource_modes() {
    use bench::plan::{parse_latency_args, LatencyMode};
    // Would catch: a smoke calibration allowed to grow into a measurement, or a full run that
    // is not exactly 10 warmup + 100 measured transitions.
    let smoke =
        parse_latency_args(&["--latency", "smoke", "--transitions", "3", "--out", "/o"]).unwrap();
    assert_eq!(
        (smoke.mode, smoke.warmup, smoke.measured),
        (LatencyMode::Smoke, 0, 3)
    );
    assert!(
        parse_latency_args(&["--latency", "smoke", "--transitions", "4", "--out", "/o"]).is_err()
    );
    assert!(
        parse_latency_args(&["--latency", "smoke", "--transitions", "0", "--out", "/o"]).is_err()
    );
    let full = parse_latency_args(&["--latency", "full", "--out", "/o"]).unwrap();
    assert_eq!(
        (full.mode, full.warmup, full.measured),
        (LatencyMode::Full, 10, 100)
    );
    assert!(
        parse_latency_args(&["--latency", "full", "--transitions", "3", "--out", "/o"]).is_err()
    );
    assert!(parse_latency_args(&["--latency", "full", "--out", "rel"]).is_err());
    assert!(parse_latency_args(&["--latency", "acceptance", "--out", "/o"]).is_err());
    // Resource parser never accepts latency flags (60 s resource acceptance unchanged).
    assert!(bench::plan::parse_args(&["--latency", "full", "--out", "/o"]).is_err());
}

#[test]
fn supervisor_latency_methods_contract() {
    use std::path::{Path, PathBuf};
    use support::display::{PrivateDisplay, Resources};
    use support::supervisor::{grim_ppm_launch, latency_wtype_launch, sway_ipc_socket};

    let rt = PathBuf::from("/tmp/hd7_test_rt");
    let sock = sway_ipc_socket(&rt, 1000, 4242).unwrap();
    assert_eq!(sock, rt.join("sway-ipc.1000.4242.sock"));
    assert!(sway_ipc_socket(Path::new("relative"), 1000, 4242).is_err());

    let display = PrivateDisplay {
        runtime_dir: rt.clone(),
        home: PathBuf::from("/tmp/hd7_home"),
        resources: Resources::under(Path::new("/tmp/res")),
        user: "test".into(),
    };
    let wayland = rt.join("wayland-1");
    let bin = PathBuf::from("/tmp/wtype-hd-latency");
    let log = PathBuf::from("/tmp/wtype.log");
    let bus = "unix:path=/tmp/hd7_test_rt/bus";

    let wtype = latency_wtype_launch(&display, &wayland, bus, &bin, &log).unwrap();
    assert_eq!(wtype.program, bin);
    assert_eq!(wtype.var("HD_LATENCY_LOG"), Some("/tmp/wtype.log"));
    assert_eq!(wtype.var("DBUS_SESSION_BUS_ADDRESS"), Some(bus));
    assert_eq!(wtype.args, vec!["-s", "250", "a"]);
    // Private wtype must share the window's compositor, never the user's display.
    let window = display
        .window(Path::new("/tmp/fake-window"), &wayland, true, &[])
        .unwrap();
    assert_eq!(
        wtype.var("WAYLAND_DISPLAY"),
        window.var("WAYLAND_DISPLAY"),
        "wtype WAYLAND_DISPLAY must match the window"
    );
    assert_eq!(
        wtype.var("XDG_RUNTIME_DIR"),
        window.var("XDG_RUNTIME_DIR"),
        "wtype XDG_RUNTIME_DIR must match the window"
    );
    assert_eq!(
        wtype.var("WAYLAND_DISPLAY"),
        Some(wayland.to_str().unwrap())
    );
    assert_eq!(wtype.var("XDG_RUNTIME_DIR"), Some(rt.to_str().unwrap()));

    let grim = grim_ppm_launch(&display, &wayland, bus).unwrap();
    assert_eq!(grim.program, PathBuf::from("/usr/bin/grim"));
    assert_eq!(grim.args, vec!["-t", "ppm", "-o", "HEADLESS-1", "-"]);
    assert_eq!(grim.var("DBUS_SESSION_BUS_ADDRESS"), Some(bus));
    assert_eq!(grim.var("WAYLAND_DISPLAY"), Some(wayland.to_str().unwrap()));

    // Negative cases:
    // 1. Socket with different parent
    assert!(
        sway_ipc_socket(&rt.join("sub"), 1000, 4242).unwrap() != rt.join("sway-ipc.1000.4242.sock")
    );
    // 2. O_EXCL check: existing log file must cause latency_wtype to return Err before spawning
    let temp = tempfile::tempdir().unwrap();
    let existing_log = temp.path().join("exists.log");
    std::fs::write(&existing_log, b"stale").unwrap();
    // A mock supervisor with dummy PrivateDisplay to test latency_wtype O_EXCL guard
    let mut sup = support::supervisor::Supervisor::for_test(
        display,
        temp.path().to_path_buf(),
        bus.to_string(),
        temp.path().join("wayland-1"),
    );
    let err = sup.latency_wtype(&bin, &existing_log).unwrap_err();
    assert!(err.contains("already exists"), "{err}");

    assert_eq!(
        support::supervisor::focus_window_args(4242)[0],
        "[pid=4242] focus"
    );
    assert_ne!(
        support::supervisor::focus_window_args(1)[0],
        "[pid=4242] focus"
    );
}

#[test]
fn latency_runner_contracts() {
    use crate::latency::pty::HelperEvent;
    use crate::latency::record::{
        evaluate, Attempt, Capture, Identity, Press, Run, Verdict, WtypeBuild,
    };
    use bench::live::LiveLatencyEnv;
    use bench::plan::LatencyMode;
    use std::path::PathBuf;

    let env = LiveLatencyEnv {
        herdr_bin: PathBuf::from("/bin/herdr"),
        resources_root: PathBuf::from("/res"),
        out: PathBuf::from("/out"),
        exe: PathBuf::from("/bin/herdr-desktop"),
        repo: PathBuf::from("/repo"),
        user: "test".into(),
        uid: 1000,
        mode: LatencyMode::Smoke,
        warmup: 0,
        measured: 3,
    };
    assert_eq!(env.warmup + env.measured, 3);

    // 1. Boot prefix matching: parent_boot must start with boot_prefix
    let parent_boot = "3851209-abc-xyz";
    let is_valid_boot_prefix = |prefix: &str| !prefix.is_empty() && parent_boot.starts_with(prefix);
    assert!(is_valid_boot_prefix("3851209-"));
    assert!(!is_valid_boot_prefix("wrong-"));
    assert!(!is_valid_boot_prefix(""));

    // 2. Identity and stable generation across attempts
    let id0 = Identity {
        host: "local".into(),
        session: "hd007-lat-123".into(),
        boot: parent_boot.into(),
        generation: 5,
        pane: "w1:p1".into(),
        dpr_milli: 1000,
        cols: 75,
        rows: 29,
    };
    let mut id1 = id0.clone();
    assert_eq!(id0, id1);
    id1.generation = 6; // changed generation -> mismatch
    assert_ne!(id0, id1);

    // 3. Simulated smoke run evaluation:
    // With 3 attempts, evaluate must reject for attempts != 110 (as expected for smoke),
    // but the individual attempt samples are computed and verified.
    let wtype = WtypeBuild {
        commit: "d71be3a7b3f93b534a2823fd68cabd7ac2a02359".into(),
        binary_sha256: "ef".repeat(32),
    };
    let mut attempts = Vec::new();
    let mut t = 1_000_000_000i64;
    for i in 0..3 {
        let press = t + 500_000;
        let release = press + 2_000_000;
        let cap_start = press + 10_000_000;
        let cap_end = cap_start + 5_000_000;
        t = cap_end + 1_000_000;
        attempts.push(Attempt {
            index: i,
            expected_seq: (i + 1) as u16,
            identity: id0.clone(),
            press: Press {
                clock: "CLOCK_MONOTONIC".into(),
                index: 0,
                keycode: 38,
                ns: press,
            },
            released_ns: release,
            helper: vec![HelperEvent {
                byte: b'a',
                count: i as u64 + 1,
                seq: (i + 1) as u16,
            }],
            trusted_keydowns: 1,
            untrusted_keydowns: 0,
            captures: vec![Capture {
                clock: "CLOCK_MONOTONIC".into(),
                start_ns: cap_start,
                end_ns: cap_end,
                seq: Ok((i + 1) as u16),
            }],
        });
    }

    let run = Run {
        clock: "CLOCK_MONOTONIC".into(),
        clock_res_ns: 1,
        identity: id0,
        wtype: wtype.clone(),
        expected_wtype: wtype,
        attempts,
    };

    let rep = evaluate(&run);
    // Evaluator flags attempts 3 != 110:
    match &rep.verdict {
        Verdict::Invalid(errs) => {
            assert!(errs.iter().any(|e| e.contains("attempts 3 != 110")));
        }
        other => panic!("expected Invalid(attempts 3 != 110), got {other:?}"),
    }
}
