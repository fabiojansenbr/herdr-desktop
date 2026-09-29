//! Evidence validation of one bench run. Never produces PASS: a complete acceptance run is
//! `MeasuredPendingReview`, a complete smoke is `InsufficientForAcceptance`, anything else is
//! `Rejected` with every reason found.

use std::collections::BTreeMap;

use serde_json::Value;

use super::plan::{
    producer_bytes_per_s, pty_group, Mode, Output, Scenario, COUNTER_WINDOW_SLACK_S,
    RATE_TOLERANCE_PERMILLE,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessId {
    pub pid: u32,
    /// Starttime recorded when the run created/identified the process.
    pub starttime: u64,
    /// Starttime read again at the end (None = gone).
    pub current_starttime: Option<u64>,
    /// Created by this run (window, generator) or identified as the disposable session's own.
    pub owned: bool,
}

impl ProcessId {
    fn problem(&self, label: &str) -> Option<String> {
        if !self.owned {
            Some(format!("{label}: pid {} not owned by this run", self.pid))
        } else if self.current_starttime != Some(self.starttime) {
            Some(format!(
                "{label}: pid {} exited or was recycled (starttime {} → {:?})",
                self.pid, self.starttime, self.current_starttime
            ))
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildInfo {
    pub debug_assertions: bool,
    pub profile: String,
    pub binary_sha256: String,
    pub surface_trace: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Geometry {
    pub cols: u16,
    pub rows: u16,
    pub cell_width_px: u16,
    pub cell_height_px: u16,
    pub scale_milli: u16,
    /// Canvas 2D context font as painted by the terminal.
    pub font: String,
}

/// Fixed geometry every GUI must report (private display output, font and scale pinned).
///
/// Calibrated from the product: the private headless sway output and the composed layout give a
/// terminal canvas at devicePixelRatio 1, and TerminalView uses
/// cell = ceil(measureText("M")) x ceil(14 * 1.3) = 9x19 px with the family list below.
///
/// First calibration (spec 007, evidencias/007/bench-live/run-2): 675x551 px, engine grid 75x29
/// (the 120x32, 9x18, "14px monospace" proposal was never observed). Recalibrated for spec 009
/// (evidencias/009/base1-control, `geometry_after` of the complete idle run, and confirmed by
/// evidencias/009/base2-control): the composed layout changed between the two specs and now gives
/// 972x570 px, engine grid 108x30. The cell size, scale and font family are unchanged. Only the
/// calibrated constant moved: equality stays strict, for every client and against this constant.
pub fn fixed_geometry() -> Geometry {
    Geometry {
        cols: 108,
        rows: 30,
        cell_width_px: 9,
        cell_height_px: 19,
        scale_milli: 1000,
        font: "14px \"JetBrains Mono\", \"Fira Code\", \"DejaVu Sans Mono\", monospace".into(),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClientEvidence {
    pub window: ProcessId,
    pub runtime_dir: String,
    pub geometry: Geometry,
    /// Terminal canvases mounted in this GUI (the active terminal is the only one).
    pub terminal_canvases: u32,
    /// Terminal layer hidden by the UI (Files activity, no file opened) during measurement.
    pub terminal_hidden: bool,
    /// Probe counters (after − before) over the measured window.
    pub probe_delta: Value,
    /// Probe counters (after − before) of the visible positive control in the same window.
    pub positive_control: Value,
}

/// One reading of a generator's own byte counter (bytes it really wrote to the PTY so far,
/// CLOCK_MONOTONIC of that write).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CounterReading {
    pub bytes: u64,
    pub monotonic_ns: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Producer {
    pub pane_id: String,
    /// `shell_pid` the engine reported for `pane_id` (`pane.process_info`).
    pub engine_shell_pid: Option<u32>,
    pub shell: ProcessId,
    /// Parent chain of the shell read from /proc (nearest first).
    pub shell_ancestors: Vec<u32>,
    /// Generator started by this run inside the pane.
    pub generator: ProcessId,
    /// Parent chain of the generator read from /proc (nearest first).
    pub generator_ancestors: Vec<u32>,
    /// Counter read before the measured window starts and after it ends.
    pub counter_start: CounterReading,
    pub counter_end: CounterReading,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Evidence {
    pub scenario: Scenario,
    pub mode: Mode,
    pub build: BuildInfo,
    pub session: String,
    pub engine: ProcessId,
    /// Panes returned by the engine's `pane.list` for the session (not DOM).
    pub pane_list_count: usize,
    pub clients: Vec<ClientEvidence>,
    pub producers: Vec<Producer>,
    pub warmup_s: f64,
    pub measured_s: f64,
    pub collector_files: Vec<String>,
    pub collector_summary: Value,
    pub cleanup_log: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RunStatus {
    Rejected(Vec<String>),
    InsufficientForAcceptance,
    MeasuredPendingReview,
}

fn counter(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(Value::as_u64)
}

pub fn evaluate(e: &Evidence) -> RunStatus {
    let mut r = Vec::new();
    let s = e.scenario;

    if e.build.debug_assertions {
        r.push("build has debug assertions (debug build presented as release)".into());
    }
    if e.build.profile != "release" {
        r.push(format!(
            "build profile {:?} is not release",
            e.build.profile
        ));
    }
    if e.build.surface_trace {
        r.push("HERDR_DESKTOP_SURFACE_TRACE was enabled (per-frame I/O)".into());
    }
    if e.build.binary_sha256.len() != 64
        || !e.build.binary_sha256.bytes().all(|b| b.is_ascii_hexdigit())
    {
        r.push("binary sha256 missing or malformed".into());
    }

    for (name, value, minimum) in [
        ("warmup", e.warmup_s, e.mode.warmup_s()),
        ("measured", e.measured_s, e.mode.measured_s()),
    ] {
        if !value.is_finite() {
            r.push(format!("{name} {value} s is not finite"));
        } else if value < minimum {
            r.push(format!("{name} {value} s < {minimum} s"));
        }
    }

    if !e.session.starts_with("hd007-") || e.session.len() <= "hd007-".len() {
        r.push(format!(
            "session {:?} is not a disposable hd007-* session",
            e.session
        ));
    }
    r.extend(e.engine.problem("engine"));

    if e.pane_list_count != s.panes as usize {
        r.push(format!(
            "pane.list has {} panes, scenario needs {}",
            e.pane_list_count, s.panes
        ));
    }
    if e.clients.len() != s.clients as usize {
        r.push(format!(
            "{} clients, scenario needs {}",
            e.clients.len(),
            s.clients
        ));
    }
    for (i, a) in e.clients.iter().enumerate() {
        for b in &e.clients[i + 1..] {
            if a.window.pid == b.window.pid || a.runtime_dir == b.runtime_dir {
                r.push("GUI clients are not independent processes/runtime dirs".into());
            }
        }
    }
    let first_geometry = e.clients.first().map(|c| &c.geometry);
    for (i, c) in e.clients.iter().enumerate() {
        let label = format!("gui_{}", i + 1);
        r.extend(c.window.problem(&label));
        if c.terminal_canvases != 1 {
            r.push(format!(
                "{label}: {} terminal canvas(es), expected exactly 1",
                c.terminal_canvases
            ));
        }
        if Some(&c.geometry) != first_geometry {
            r.push(format!("{label}: geometry diverges from gui_1"));
        } else if c.geometry != fixed_geometry() {
            r.push(format!("{label}: geometry differs from the fixed geometry"));
        }
        if c.terminal_hidden != (s.output == Output::Hidden) {
            r.push(format!(
                "{label}: terminal hidden={} does not match the scenario",
                c.terminal_hidden
            ));
        }
        let control = (
            counter(&c.positive_control, "raf_callbacks"),
            counter(&c.positive_control, "painted_rows"),
        );
        if !matches!(control, (Some(a), Some(b)) if a > 0 && b > 0) {
            r.push(format!(
                "{label}: probe positive control saw no terminal frame/rows"
            ));
        }
        match (
            counter(&c.probe_delta, "raf_callbacks"),
            counter(&c.probe_delta, "painted_rows"),
        ) {
            (Some(raf), Some(rows)) => match s.output {
                Output::Hidden if raf != 0 || rows != 0 => {
                    r.push(format!("{label}: hidden repaint (raf {raf}, rows {rows})"))
                }
                Output::Visible if raf == 0 || rows == 0 => {
                    r.push(format!("{label}: visible output painted nothing"))
                }
                _ => {}
            },
            _ => r.push(format!("{label}: probe counters missing")),
        }
    }

    if e.producers.len() != s.panes as usize {
        r.push(format!(
            "{} producers for {} panes",
            e.producers.len(),
            s.panes
        ));
    }
    let mut seen_panes = std::collections::BTreeSet::new();
    let mut seen_shells = std::collections::BTreeSet::new();
    let mut seen_generators = std::collections::BTreeSet::new();
    for p in &e.producers {
        let id = &p.pane_id;
        if !seen_panes.insert(id.clone()) {
            r.push(format!("duplicate pane {id} among producers"));
        }
        if !seen_shells.insert(p.shell.pid) {
            r.push(format!("{id}: duplicate shell pid {}", p.shell.pid));
        }
        if s.output != Output::Idle && !seen_generators.insert(p.generator.pid) {
            r.push(format!("{id}: duplicate generator pid {}", p.generator.pid));
        }
        r.extend(p.shell.problem(&format!("{id} shell")));
        if p.engine_shell_pid != Some(p.shell.pid) {
            r.push(format!(
                "{id}: shell pid {} is not the engine's shell for the pane ({:?})",
                p.shell.pid, p.engine_shell_pid
            ));
        }
        if !p.shell_ancestors.contains(&e.engine.pid) {
            r.push(format!(
                "{id}: shell {} is not a descendant of engine {}",
                p.shell.pid, e.engine.pid
            ));
        }
        if s.output == Output::Idle {
            // Idle means observed without any generator of this run and without produced bytes.
            let g = &p.generator;
            if g.owned || g.pid != 0 || g.current_starttime.is_some() {
                r.push(format!(
                    "{id}: idle scenario has a generator (pid {})",
                    g.pid
                ));
            }
            if p.counter_end != p.counter_start {
                r.push(format!("{id}: idle scenario counter advanced"));
            }
            continue;
        }
        r.extend(p.generator.problem(&format!("{id} generator")));
        if !p.generator_ancestors.contains(&p.shell.pid) {
            r.push(format!(
                "{id}: generator {} is not a descendant of shell {}",
                p.generator.pid, p.shell.pid
            ));
        }
        let (start, end) = (p.counter_start, p.counter_end);
        if end.bytes < start.bytes || end.monotonic_ns < start.monotonic_ns {
            r.push(format!("{id}: generator counter went backwards"));
            continue;
        }
        // Only what the generator wrote between the two readings (never its lifetime total).
        let bytes = end.bytes - start.bytes;
        let seconds = (end.monotonic_ns - start.monotonic_ns) as f64 / 1e9;
        if seconds < e.measured_s || !e.measured_s.is_finite() {
            r.push(format!(
                "{id}: produced for {seconds} s < measured {} s",
                e.measured_s
            ));
        } else if seconds > e.measured_s + COUNTER_WINDOW_SLACK_S {
            r.push(format!(
                "{id}: counter window {seconds} s exceeds measured {} s (warmup mixed into the rate)",
                e.measured_s
            ));
        }
        let expected = producer_bytes_per_s() as f64;
        let rate = if seconds > 0.0 {
            bytes as f64 / seconds
        } else {
            0.0
        };
        if (rate - expected).abs() * 1000.0 > expected * RATE_TOLERANCE_PERMILLE as f64 {
            r.push(format!(
                "{id}: emitted {rate:.0} B/s, fixed rate is {expected} B/s"
            ));
        }
    }

    for f in ["samples.jsonl", "summary.json", "report.md"] {
        if !e.collector_files.iter().any(|x| x == f) {
            r.push(format!("collector output incomplete: {f} missing"));
        }
    }
    let status = e.collector_summary["status"].as_str().unwrap_or("missing");
    if status != "success" {
        r.push(format!("collector status {status}"));
    }
    let groups = &e.collector_summary["groups"];
    let mut required: Vec<String> = (1..=s.clients).map(|n| format!("gui_{n}")).collect();
    required.push("engine".into());
    required.extend((1..=s.panes).map(|i| pty_group(i, s.panes)));
    for g in required {
        if groups.get(&g).is_none() {
            r.push(format!("collector group {g} missing"));
        }
    }

    if e.cleanup_log.is_empty() {
        r.push("cleanup log missing".into());
    }
    r.extend(
        e.cleanup_log
            .iter()
            .filter(|l| l.starts_with("LEFTOVER"))
            .cloned(),
    );

    if !r.is_empty() {
        RunStatus::Rejected(r)
    } else if matches!(e.mode, Mode::Smoke { .. }) {
        RunStatus::InsufficientForAcceptance
    } else {
        RunStatus::MeasuredPendingReview
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TargetState {
    Met,
    Failed,
    Unknown(String),
}

/// One statistic (`max`/`avg`) of a collector aggregate, refusing lower bounds and nulls.
fn metric(summary: &Value, group: &str, metric: &str, stat: &str) -> Result<f64, String> {
    let m = &summary["groups"][group][metric];
    if m.is_null() {
        return Err(format!("{group}.{metric} missing"));
    }
    if m["is_lower_bound"].as_bool() != Some(false) {
        return Err(format!("{group}.{metric} is a lower bound or undeclared"));
    }
    m[stat]
        .as_f64()
        .filter(|v| v.is_finite())
        .ok_or_else(|| format!("{group}.{metric}.{stat} is null"))
}

fn gui_groups(summary: &Value) -> Vec<String> {
    summary["groups"]
        .as_object()
        .map(|g| {
            g.keys()
                .filter(|k| k.starts_with("gui_"))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// Recorded next to the targets (not judged): PSS max/avg, RSS max/avg and CPU avg/max per GUI,
/// in MiB and % of one core; null when missing or a lower bound.
pub fn observations(summary: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for gui in gui_groups(summary) {
        let mib = |m: &str, stat: &str| metric(summary, &gui, m, stat).ok().map(|kb| kb / 1024.0);
        let pct = |stat: &str| metric(summary, &gui, "cpu_percent_of_one_core", stat).ok();
        out.insert(
            gui.clone(),
            serde_json::json!({
                "pss_max_mib": mib("pss_kb", "max"),
                "pss_avg_mib": mib("pss_kb", "avg"),
                "rss_max_mib": mib("rss_kb", "max"),
                "rss_avg_mib": mib("rss_kb", "avg"),
                "cpu_avg_percent_of_one_core": pct("avg"),
                "cpu_max_percent_of_one_core": pct("max"),
            }),
        );
    }
    Value::Object(out)
}

/// Idle PSS maximum of one GUI tree, in MiB (PRD). Revised from 250 to 300 by the user's decision
/// of 2026-09-23, recorded in docs/PRD-herdr-desktop.md and the log of spec 009, with the
/// methodology of evidencias/009/results.md: whole GUI tree, one instance, shared pages counted at
/// their proportional share, maximum after warmup. The name of the reported target carries the
/// number, so moving it is visible in every report.
pub const IDLE_PSS_MAX_MIB: u32 = 300;

/// PRD targets, per GUI client (never summed across clients), on the post-warmup summary:
/// PSS maximum <= [`IDLE_PSS_MAX_MIB`], CPU average < 1 % of one core; `baseline` = the idle
/// summary with the same client count, for the 15-pane delta of maxima <= 100 MiB. A summary
/// without any GUI group yields an Unknown entry, never an empty (vacuously met) map.
pub fn targets(summary: &Value, baseline: Option<&Value>) -> BTreeMap<String, TargetState> {
    let mut out = BTreeMap::new();
    let clients = gui_groups(summary);
    let unusable = |s: &Value| s["status"].as_str() != Some("success");
    if clients.is_empty() {
        out.insert(
            "gui".into(),
            TargetState::Unknown("summary has no gui_N group".into()),
        );
    }
    for gui in clients {
        let judge = |value: Result<f64, String>, ok: &dyn Fn(f64) -> bool| match value {
            _ if unusable(summary) => {
                TargetState::Unknown("collector status is not success".into())
            }
            Ok(v) if ok(v) => TargetState::Met,
            Ok(_) => TargetState::Failed,
            Err(e) => TargetState::Unknown(e),
        };
        match baseline {
            None => {
                let pss = metric(summary, &gui, "pss_kb", "max").map(|kb| kb / 1024.0);
                out.insert(
                    format!("{gui}.idle_pss_max_mib<={IDLE_PSS_MAX_MIB}"),
                    judge(pss, &|v| v <= f64::from(IDLE_PSS_MAX_MIB)),
                );
                let cpu = metric(summary, &gui, "cpu_percent_of_one_core", "avg");
                out.insert(format!("{gui}.idle_cpu_avg<1"), judge(cpu, &|v| v < 1.0));
            }
            Some(base) => {
                let delta = match (
                    metric(summary, &gui, "pss_kb", "max"),
                    metric(base, &gui, "pss_kb", "max"),
                ) {
                    _ if unusable(base) => Err("baseline status is not success".into()),
                    (Ok(a), Ok(b)) => Ok((a - b) / 1024.0),
                    (Err(e), _) | (_, Err(e)) => Err(e),
                };
                out.insert(
                    format!("{gui}.scale15_delta_pss_max_mib<=100"),
                    judge(delta, &|v| v <= 100.0),
                );
            }
        }
    }
    out
}

/// PRD targets of one run, gated by its evidence: idle limits only for an accepted (not smoke,
/// not rejected) idle run; with `baseline`, the 15-pane delta only against an accepted 1-pane run
/// of the same clients and output. Anything else is Unknown, never Met/Failed.
pub fn judge(e: &Evidence, baseline: Option<&Evidence>) -> BTreeMap<String, TargetState> {
    let refuse =
        |why: String| BTreeMap::from([("prd_targets".to_owned(), TargetState::Unknown(why))]);
    let accepted = |x: &Evidence, label: &str| match evaluate(x) {
        RunStatus::MeasuredPendingReview => Ok(()),
        RunStatus::InsufficientForAcceptance => Err(format!("{label} is a smoke run")),
        RunStatus::Rejected(r) => Err(format!("{label} evidence rejected: {}", r.join("; "))),
    };
    if let Err(why) = accepted(e, "run") {
        return refuse(why);
    }
    match baseline {
        None if e.scenario.output != Output::Idle => refuse(format!(
            "idle targets need an observed idle run, scenario is {:?}",
            e.scenario
        )),
        None => targets(&e.collector_summary, None),
        Some(b) => {
            let (s, bs) = (e.scenario, b.scenario);
            if s.panes != 15 || bs.panes != 1 || s.clients != bs.clients || s.output != bs.output {
                return refuse(format!(
                    "baseline {bs:?} is not the 1-pane equivalent of {s:?}"
                ));
            }
            if let Err(why) = accepted(b, "baseline") {
                return refuse(why);
            }
            targets(&e.collector_summary, Some(&b.collector_summary))
        }
    }
}
