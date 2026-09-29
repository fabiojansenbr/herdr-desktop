//! Live runner of the resource bench: ONE release GUI (this test binary as the composed window on
//! a private display, via the native harness `Supervisor`) and ONE PTY of a disposable `hd007-*`
//! engine started here. The page (`src/features/fidelity/resource-bench.ts`) switches the real
//! Files layer; this parent starts a generator inside the pane, runs `bench/collector.py`
//! (untouched) over each window and maps real readings into [`Evidence`] for the visible and the
//! hidden 1-PTY/1-GUI cells. Only smoke durations are run in this checkpoint; the result is never
//! better than `InsufficientForAcceptance`.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::evidence;
use super::plan::{
    background_split, barrier_step, check_renderer_env, pty_group, resource_window_timeout_s,
    variant_applied, Load, Mode, Output, RendererVariant, Scenario, BENCH_STEPS, SYNTHETIC_LINE,
    SYNTHETIC_LINES_PER_S,
};
use super::verdict::{
    evaluate, judge, observations, BuildInfo, ClientEvidence, CounterReading, Evidence, ProcessId,
    Producer, RunStatus,
};
use crate::support::display::{PrivateDisplay, Resources};
use crate::support::supervisor::{same_process, starttime, Supervisor, Tracked};
use herdr_client::RuntimeGateway;

pub struct LiveEnv {
    pub herdr_bin: PathBuf,
    pub resources_root: PathBuf,
    /// New, absolute run directory (refused if it exists).
    pub out: PathBuf,
    pub mode: Mode,
    /// This test binary (window process and runner).
    pub exe: PathBuf,
    pub repo: PathBuf,
    pub user: String,
    pub uid: u32,
    pub panes: u8,
    pub clients: u8,
    pub load: Load,
    /// Named WebKit/GDK renderer variant of every window of the run (memory A/B diagnostic).
    pub renderer: RendererVariant,
}

impl LiveEnv {
    fn background_split(&self) -> (u8, u8) {
        background_split(self.panes)
    }
}

pub struct LiveLatencyEnv {
    pub herdr_bin: PathBuf,
    pub resources_root: PathBuf,
    pub out: PathBuf,
    pub exe: PathBuf,
    pub repo: PathBuf,
    pub user: String,
    pub uid: u32,
    pub mode: super::plan::LatencyMode,
    pub warmup: usize,
    pub measured: usize,
}

const COLLECTOR_INTERVAL_S: f64 = 0.5;
fn renderer_label(variant: RendererVariant) -> String {
    let extra: String = variant
        .env()
        .iter()
        .map(|(k, v)| format!(" {k}={v}"))
        .collect();
    // Control and env arms keep the label they always had; only API arms name their policy.
    let api = match variant.webkit_policy() {
        crate::support::window::webkit_policy::Policy::Control => String::new(),
        policy => format!("; webkit_policy={}", policy.name()),
    };
    format!(
        "private headless sway WLR_RENDERER=pixman; WEBKIT_DISABLE_DMABUF_RENDERER=1{extra}; variant={}{api}",
        variant.name()
    )
}

/// Read-only census of WebKit processes outside this run's windows: pid, starttime and comm only
/// (no environ, no cmdline), to label host interference with PSS of shared pages.
fn foreign_webkit_census(windows: &[u32]) -> Value {
    let procs: Vec<(u32, u64, String)> = std::fs::read_dir("/proc")
        .map(|d| {
            d.flatten()
                .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
                .filter_map(|pid| {
                    let comm = proc_text(pid, "comm")?.trim().to_owned();
                    Some((pid, starttime(pid)?, comm))
                })
                .collect()
        })
        .unwrap_or_default();
    let view: Vec<(u32, u64, &str)> = procs.iter().map(|(p, s, c)| (*p, *s, c.as_str())).collect();
    let found = evidence::foreign_webkit(&view, windows, &ppid_of);
    json!({
        "count": found.len(),
        "processes": found.iter().map(|(pid, st, comm)| json!({ "pid": pid, "starttime": st, "comm": comm })).collect::<Vec<_>>(),
        "interference": !found.is_empty(),
    })
}

fn proc_text(pid: u32, file: &str) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/{file}")).ok()
}

fn ppid_of(pid: u32) -> Option<u32> {
    proc_text(pid, "stat")
        .as_deref()
        .and_then(evidence::parse_ppid)
}

fn now_identity(t: &Tracked) -> Option<u64> {
    starttime(t.pid)
}

fn wait_until(timeout: Duration, mut probe: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if probe() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

/// Disposable engine of this run: spawned here, identity recorded right after spawn, stopped by
/// `session stop` and then only by pid+starttime; the session is deleted.
struct Engine {
    session: String,
    config: PathBuf,
    herdr_bin: PathBuf,
    tracked: Tracked,
    child: Option<Child>,
    log: Vec<String>,
}

impl Engine {
    pub fn start_for_session(
        herdr_bin: &Path,
        out: &Path,
        session: &str,
        dir: &Path,
    ) -> Result<Self, String> {
        crate::support::session::disposable(session)?;
        let work = dir.join("work");
        std::fs::create_dir_all(&work).map_err(|e| format!("session dir: {e}"))?;
        let config = dir.join("config.toml");
        std::fs::write(
            &config,
            "# Test-only configuration of a disposable resource bench session.\n[experimental]\nallow_nested = true\n[ui]\npane_scrollbars = false\n",
        )
        .map_err(|e| e.to_string())?;
        let log_file =
            |name: &str| std::fs::File::create(out.join(name)).map_err(|e| e.to_string());
        let mut command = Command::new(herdr_bin);
        command
            .args(["--session", session, "server"])
            .env_remove("HERDR_SOCKET_PATH")
            .env_remove("HERDR_CLIENT_SOCKET_PATH")
            .env_remove("HERDR_SESSION")
            .env_remove("HERDR_WORKSPACE_ID")
            .env_remove("HERDR_TAB_ID")
            .env_remove("HERDR_PANE_ID")
            .env_remove("HERDR_DESKTOP_SURFACE_TRACE")
            .env("HERDR_CONFIG_PATH", &config)
            .env("HERDR_STARTUP_CWD", &work)
            .current_dir(&work)
            .stdin(Stdio::null())
            .stdout(log_file("engine.out.log")?)
            .stderr(log_file("engine.err.log")?);
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let child = command.spawn().map_err(|e| format!("engine spawn: {e}"))?;
        let pid = child.id();
        let st = starttime(pid);
        let mut engine = Self {
            session: session.into(),
            config,
            herdr_bin: herdr_bin.to_path_buf(),
            tracked: Tracked {
                name: "engine".into(),
                pid,
                starttime: st.unwrap_or(0),
            },
            child: Some(child),
            log: vec![format!(
                "spawn engine pid={pid} starttime={st:?} session={session}"
            )],
        };
        if st.is_none() {
            return Err("engine exited immediately".into());
        }
        if !wait_until(Duration::from_secs(15), || {
            engine.cli(&["pane", "list"]).is_ok()
        }) {
            return Err(format!("engine {session} never answered pane list"));
        }
        let panes = engine.pane_ids()?;
        if panes.is_empty() {
            engine.cli(&[
                "workspace",
                "create",
                "--cwd",
                &work.display().to_string(),
                "--focus",
            ])?;
        }
        engine
            .log
            .push(format!("engine ready, panes {:?}", engine.pane_ids()?));
        Ok(engine)
    }

    fn start(env: &LiveEnv, session: &str, dir: &Path) -> Result<Self, String> {
        Self::start_for_session(&env.herdr_bin, &env.out, session, dir)
    }

    fn cli(&self, args: &[&str]) -> Result<String, String> {
        let output = Command::new(&self.herdr_bin)
            .args(args)
            .env_remove("HERDR_SOCKET_PATH")
            .env_remove("HERDR_CLIENT_SOCKET_PATH")
            .env_remove("HERDR_WORKSPACE_ID")
            .env_remove("HERDR_TAB_ID")
            .env_remove("HERDR_PANE_ID")
            .env("HERDR_SESSION", &self.session)
            .env("HERDR_CONFIG_PATH", &self.config)
            .output()
            .map_err(|e| format!("herdr {args:?}: {e}"))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Err(format!(
                "herdr {args:?}: {} {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ))
        }
    }

    fn json(&self, args: &[&str]) -> Result<Value, String> {
        serde_json::from_str(&self.cli(args)?).map_err(|e| format!("herdr {args:?} json: {e}"))
    }

    fn pane_ids(&self) -> Result<Vec<String>, String> {
        let list = self.json(&["pane", "list"])?;
        evidence::pane_list_ids(&list).ok_or_else(|| format!("pane list shape: {list}"))
    }

    fn stop(&mut self) -> Vec<String> {
        let Some(mut child) = self.child.take() else {
            return Vec::new();
        };
        let stop = self.cli(&["session", "stop", &self.session]);
        self.log.push(format!(
            "session stop {}: {:?}",
            self.session,
            stop.map(|_| "ok")
        ));
        let exited = wait_until(Duration::from_secs(5), || {
            matches!(child.try_wait(), Ok(Some(_)))
        });
        if !exited && same_process(&self.tracked, now_identity(&self.tracked)) {
            let _ = child.kill();
            let _ = child.wait();
            self.log.push(format!(
                "engine pid={} killed after stop timeout",
                self.tracked.pid
            ));
        }
        let delete = Command::new(&self.herdr_bin)
            .args(["session", "delete", &self.session])
            .env_remove("HERDR_SOCKET_PATH")
            .env_remove("HERDR_CLIENT_SOCKET_PATH")
            .env("HERDR_CONFIG_PATH", &self.config)
            .output();
        self.log.push(format!(
            "session delete {}: {:?}",
            self.session,
            delete.map(|o| o.status.to_string())
        ));
        if same_process(&self.tracked, now_identity(&self.tracked)) {
            self.log
                .push(format!("LEFTOVER engine pid={}", self.tracked.pid));
        } else {
            self.log
                .push(format!("engine pid={} gone", self.tracked.pid));
        }
        self.log.clone()
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Generator source: fixed-rate `SYNTHETIC_LINE` writes to the PTY, counting only bytes
/// `os.write` really accepted, publishing `<bytes> <CLOCK_MONOTONIC ns> <pid>` after each write.
pub fn generator_script() -> String {
    let line = serde_json::to_string(SYNTHETIC_LINE).unwrap();
    format!(
        r#"import os, sys, time
LINE = {line}.encode()
PERIOD_NS = 1_000_000_000 // {SYNTHETIC_LINES_PER_S}
counter = sys.argv[1]
tmp = counter + ".tmp"
written = 0
def publish():
    with open(tmp, "w") as f:
        f.write(f"{{written}} {{time.monotonic_ns()}} {{os.getpid()}}\n")
    os.replace(tmp, counter)
start = time.monotonic_ns()
publish()
n = 0
while True:
    target = start + n * PERIOD_NS
    now = time.monotonic_ns()
    if target > now:
        time.sleep((target - now) / 1e9)
    view = memoryview(LINE)
    while view:
        k = os.write(1, view)
        written += k
        view = view[k:]
    publish()
    n += 1
"#
    )
}

/// One PTY of the run: engine-reported shell and, under output load, its own generator/counter.
#[derive(Clone)]
struct Pty {
    pane_id: String,
    engine_shell_pid: Option<u32>,
    shell: Tracked,
    shell_ancestors: Vec<u32>,
    generator: Option<(Tracked, bool)>,
    generator_ancestors: Vec<u32>,
    counter: PathBuf,
}

#[derive(Default)]
struct State {
    ptys: Vec<Pty>,
    pane_list_count: usize,
    engine_size: Option<(u16, u16)>,
    raw: serde_json::Map<String, Value>,
    windows: serde_json::Map<String, Value>,
    errors: Vec<String>,
}

struct Ctx<'a> {
    env: &'a LiveEnv,
    engine: &'a Engine,
    windows: Vec<Tracked>,
    token: String,
    build: &'a BuildInfo,
}

fn read_counter(path: &Path) -> Option<(CounterReading, u32)> {
    std::fs::read_to_string(path)
        .ok()
        .as_deref()
        .and_then(evidence::parse_counter)
}

/// Adds the background PTYs of a 15-pane run (tabs in the active workspace, then workspaces),
/// none focused, so the clients keep showing the single active pane.
fn populate(engine: &Engine, tabs: u8, workspaces: u8, work: &Path) -> Result<Value, String> {
    let cwd = work.display().to_string();
    for _ in 0..tabs {
        engine.cli(&["tab", "create", "--cwd", &cwd])?;
    }
    for _ in 0..workspaces {
        engine.cli(&["workspace", "create", "--cwd", &cwd])?;
    }
    Ok(json!({
        "workspace_list": engine.json(&["workspace", "list"]).unwrap_or_else(|e| json!({ "error": e })),
        "tab_list": engine.json(&["tab", "list"]).unwrap_or_else(|e| json!({ "error": e })),
    }))
}

/// Barrier step `bench-ready` (all clients): every client's confirmed pane is in pane.list; each
/// engine pane gets its shell identity and, under output load, its own generator.
fn ready(ctx: &Ctx, st: &mut State, details: &[Value]) -> Result<Value, String> {
    let list = ctx.engine.json(&["pane", "list"])?;
    let ids = evidence::pane_list_ids(&list).ok_or("pane list shape")?;
    let mut shown = Vec::new();
    for detail in details {
        let pane = detail["pane_id"]
            .as_str()
            .filter(|p| !p.is_empty())
            .ok_or("page did not report its confirmed pane")?;
        if !ids.iter().any(|i| i == pane) {
            return Err(format!("pane {pane} not in engine pane.list {ids:?}"));
        }
        shown.push(pane.to_owned());
    }
    let active = shown.first().cloned().ok_or("no client")?;
    let layout = ctx.engine.json(&["pane", "layout", "--pane", &active]);
    st.engine_size = layout
        .as_ref()
        .ok()
        .and_then(|l| evidence::engine_pane_size(l, &active));
    st.raw.insert("pane_list".into(), list);
    st.raw.insert("panes_shown_by_clients".into(), json!(shown));
    st.raw.insert(
        "layout".into(),
        layout.unwrap_or_else(|e| json!({ "error": e })),
    );
    st.pane_list_count = ids.len();
    // Active pane first, so `pty` / `pty_1` is the pane the clients paint.
    let mut ordered = vec![active.clone()];
    ordered.extend(ids.iter().filter(|i| **i != active).cloned());
    let script = ctx.env.out.join(format!("gen-{}.py", ctx.token));
    std::fs::write(&script, generator_script()).map_err(|e| e.to_string())?;
    let mut infos = serde_json::Map::new();
    for (i, pane) in ordered.iter().enumerate() {
        let info = ctx.engine.json(&["pane", "process-info", "--pane", pane])?;
        let engine_shell_pid = evidence::engine_shell_pid(&info, pane);
        infos.insert(pane.clone(), info);
        let shell_pid = engine_shell_pid.ok_or(format!("{pane}: engine reported no shell pid"))?;
        let shell = Tracked {
            name: format!("shell-{}", i + 1),
            pid: shell_pid,
            starttime: starttime(shell_pid).ok_or(format!("{pane}: shell not alive"))?,
        };
        let mut pty = Pty {
            pane_id: pane.clone(),
            engine_shell_pid,
            shell_ancestors: evidence::ancestors(shell_pid, &ppid_of),
            shell,
            generator: None,
            generator_ancestors: Vec::new(),
            counter: ctx.env.out.join(format!("counter-{}-{}", ctx.token, i + 1)),
        };
        if ctx.env.load == Load::Output {
            let command = format!("python3 '{}' '{}'", script.display(), pty.counter.display());
            ctx.engine.cli(&["pane", "run", pane, &command])?;
        }
        st.ptys.push(pty.clone());
        if ctx.env.load == Load::Idle {
            continue;
        }
        let mut found = None;
        wait_until(Duration::from_secs(10), || {
            found = read_counter(&pty.counter);
            found.is_some()
        });
        let (_, gen_pid) = found.ok_or(format!("{pane}: generator never published"))?;
        let gen_st = starttime(gen_pid).ok_or(format!("{pane}: generator exited"))?;
        let cmdline = std::fs::read(format!("/proc/{gen_pid}/cmdline")).unwrap_or_default();
        let owned = evidence::generator_owned(&cmdline, &ctx.token);
        pty.generator = Some((
            Tracked {
                name: format!("generator-{}", i + 1),
                pid: gen_pid,
                starttime: gen_st,
            },
            owned,
        ));
        pty.generator_ancestors = evidence::ancestors(gen_pid, &ppid_of);
        *st.ptys.last_mut().unwrap() = pty;
    }
    st.raw.insert("process_info".into(), Value::Object(infos));
    if ctx.env.load == Load::Output {
        // Let output flow before the first window.
        std::thread::sleep(Duration::from_millis(700));
    }
    Ok(json!({
        "panes": ordered,
        "generators": st.ptys.iter().map(|p| p.generator.as_ref().map(|g| g.0.pid)).collect::<Vec<_>>(),
    }))
}

fn counters(st: &State) -> Vec<Option<[u64; 2]>> {
    st.ptys
        .iter()
        .map(|p| read_counter(&p.counter).map(|(c, _)| [c.bytes, c.monotonic_ns]))
        .collect()
}

/// One collector run over every GUI, the engine and each PTY tree at once (common barrier).
/// Generator counters are read when the warmup ends and after the collector exits, so the
/// producer rate never includes the warmup.
fn measure(ctx: &Ctx, st: &mut State, name: &str) -> Result<Value, String> {
    let dir = ctx.env.out.join(format!("collector-{name}"));
    let warmup = ctx.env.mode.warmup_s();
    // Acceptance requests one sampling interval more, so the observed window is >= the minimum.
    let duration = match ctx.env.mode {
        Mode::Acceptance { measured_s, .. } => measured_s + COLLECTOR_INTERVAL_S,
        other => other.measured_s(),
    };
    let roots: Vec<u32> = ctx.windows.iter().map(|w| w.pid).collect();
    let foreign_start = foreign_webkit_census(&roots);
    let mut command = Command::new("python3");
    command.arg(ctx.env.repo.join("bench/collector.py"));
    for (n, w) in ctx.windows.iter().enumerate() {
        command.args(["--tree", &format!("gui_{}:{}", n + 1, w.pid)]);
    }
    command.args(["--tree", &format!("engine:{}", ctx.engine.tracked.pid)]);
    let panes = st.ptys.len() as u8;
    for (i, p) in st.ptys.iter().enumerate() {
        command.args([
            "--tree",
            &format!("{}:{}", pty_group(i as u8 + 1, panes), p.shell.pid),
        ]);
    }
    let log =
        |ext: &str| std::fs::File::create(ctx.env.out.join(format!("collector-{name}.{ext}.log")));
    command
        .args(["--duration", &duration.to_string()])
        .args(["--interval", &COLLECTOR_INTERVAL_S.to_string()])
        .args(["--warmup", &warmup.to_string()])
        .arg("--output-dir")
        .arg(&dir)
        .args([
            "--app-build",
            &format!(
                "profile={} debug_assertions={} sha256={}",
                ctx.build.profile, ctx.build.debug_assertions, ctx.build.binary_sha256
            ),
        ])
        .args(["--hardware-renderer", &renderer_label(ctx.env.renderer)])
        .args([
            "--scenario",
            &format!("{}pty-{}gui-{name}", panes, ctx.windows.len()),
        ])
        .current_dir(&ctx.env.repo)
        .stdout(log("stdout").map_err(|e| e.to_string())?)
        .stderr(log("stderr").map_err(|e| e.to_string())?);
    let t0 = Instant::now();
    let mut child = command.spawn().map_err(|e| format!("collector: {e}"))?;
    std::thread::sleep(Duration::from_secs_f64(warmup));
    let start = counters(st);
    let mut smaps = Value::Null;
    if ctx.env.load == Load::Idle {
        // Mid-window memory detail of this run's own GUI trees (diagnosis if PSS is over target).
        std::thread::sleep(Duration::from_secs_f64(duration / 2.0));
        smaps = smaps_snapshot(&ctx.windows, &ctx.env.out.join(format!("smaps-{name}")));
    }
    let status = child.wait().map_err(|e| format!("collector wait: {e}"))?;
    let wall_s = t0.elapsed().as_secs_f64();
    let end = counters(st);
    let foreign_end = foreign_webkit_census(&roots);
    let window = json!({
        "foreign_webkit": { "collector_start": foreign_start, "collector_end": foreign_end },
        "collector_exit": status.code(),
        "smaps_snapshot": smaps,
        "collector_wall_s": wall_s,
        "collector_requested_duration_s": duration,
        "collector_dir": dir.display().to_string(),
        "counter_start": start,
        "counter_end": end,
        "current_starttime": {
            "engine": now_identity(&ctx.engine.tracked),
            "windows": ctx.windows.iter().map(now_identity).collect::<Vec<_>>(),
            "shells": st.ptys.iter().map(|p| now_identity(&p.shell)).collect::<Vec<_>>(),
            "generators": st.ptys.iter().map(|p| p.generator.as_ref().and_then(|g| now_identity(&g.0))).collect::<Vec<_>>(),
        },
    });
    st.windows.insert(name.into(), window.clone());
    Ok(window)
}

/// Copies `smaps_rollup` and `smaps` of each window and its descendants (identity checked by
/// starttime before and after reading) into `dir`. Only this run's own processes are read.
fn smaps_snapshot(windows: &[Tracked], dir: &Path) -> Value {
    let _ = std::fs::create_dir_all(dir);
    let all: Vec<u32> = std::fs::read_dir("/proc")
        .map(|d| {
            d.flatten()
                .filter_map(|e| e.file_name().to_str()?.parse().ok())
                .collect()
        })
        .unwrap_or_default();
    let mut read = Vec::new();
    for w in windows.iter().filter(|w| same_process(w, starttime(w.pid))) {
        for pid in all
            .iter()
            .copied()
            .filter(|p| *p == w.pid || evidence::ancestors(*p, &ppid_of).contains(&w.pid))
        {
            let before = starttime(pid);
            let rollup = proc_text(pid, "smaps_rollup");
            let full = proc_text(pid, "smaps");
            if before.is_none() || starttime(pid) != before {
                continue;
            }
            let comm = proc_text(pid, "comm").unwrap_or_default();
            let _ = std::fs::write(
                dir.join(format!("{pid}.smaps_rollup")),
                rollup.unwrap_or_default(),
            );
            let _ = std::fs::write(dir.join(format!("{pid}.smaps")), full.unwrap_or_default());
            read.push(
                json!({ "pid": pid, "starttime": before, "comm": comm.trim(), "root": w.pid }),
            );
        }
    }
    json!({ "dir": dir.display().to_string(), "processes": read })
}

fn stop_generators(st: &mut State) -> Value {
    let gens: Vec<Tracked> = st
        .ptys
        .iter()
        .filter_map(|p| p.generator.as_ref().map(|g| g.0.clone()))
        .collect();
    let mut alive = 0;
    for gen in &gens {
        if same_process(gen, starttime(gen.pid)) {
            alive += 1;
            let _ = Command::new("kill")
                .args(["-TERM", &gen.pid.to_string()])
                .status();
        }
    }
    let gone = wait_until(Duration::from_secs(3), || {
        gens.iter().all(|g| !same_process(g, starttime(g.pid)))
    });
    json!({ "generators": gens.len(), "were_alive": alive, "all_gone": gone })
}

fn counter_at(window: &Value, key: &str, i: usize) -> CounterReading {
    let v = &window[key][i];
    CounterReading {
        bytes: v[0].as_u64().unwrap_or(0),
        monotonic_ns: v[1].as_u64().unwrap_or(0),
    }
}

fn file_names(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// Build facts from the binary itself: compiled debug_assertions, target profile directory of
/// the executable, sha256 of its bytes, and whether the window's own environ had the trace.
pub fn build_info(exe: &Path, window_environ: Option<&[u8]>) -> BuildInfo {
    let profile = exe
        .parent()
        .and_then(Path::parent)
        .filter(|_| exe.parent().and_then(Path::file_name) == Some("deps".as_ref()))
        .and_then(Path::file_name)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".into());
    let sha = Command::new("sha256sum")
        .arg(exe)
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .split_whitespace()
                .next()
                .map(str::to_owned)
        })
        .unwrap_or_default();
    let surface_trace = window_environ.is_none_or(|env| {
        env.split(|b| *b == 0)
            .any(|kv| kv.starts_with(b"HERDR_DESKTOP_SURFACE_TRACE="))
    });
    BuildInfo {
        debug_assertions: cfg!(debug_assertions),
        profile,
        binary_sha256: sha,
        surface_trace,
    }
}

fn shell_out(program: &str, args: &[&str]) -> Value {
    Command::new(program)
        .args(args)
        .output()
        .map(|o| json!(String::from_utf8_lossy(&o.stdout).trim().to_owned()))
        .unwrap_or_else(|e| json!({ "error": e.to_string() }))
}

fn manifest(env: &LiveEnv, build: &BuildInfo, home: &Path) -> Value {
    let cpu = std::fs::read_to_string("/proc/cpuinfo").ok().and_then(|c| {
        c.lines()
            .find(|l| l.starts_with("model name"))
            .map(|l| l.split(':').nth(1).unwrap_or("").trim().to_owned())
    });
    let fc_match = Command::new("fc-match")
        .args(["-f", "%{family} %{style} %{file}", "monospace"])
        .env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .ok();
    json!({
        "binary": env.exe.display().to_string(),
        "build": {
            "profile_dir": build.profile,
            "debug_assertions": build.debug_assertions,
            "sha256": build.binary_sha256,
            "surface_trace_in_window_environ": build.surface_trace,
        },
        "git_head": shell_out("git", &["-C", &env.repo.display().to_string(), "rev-parse", "HEAD"]),
        "herdr_bin": env.herdr_bin.display().to_string(),
        "herdr_version": shell_out(&env.herdr_bin.display().to_string(), &["--version"]),
        "mode": format!("{:?}", env.mode),
        "os_release": shell_out("sh", &["-c", ". /etc/os-release; echo \"$PRETTY_NAME\""]),
        "kernel": shell_out("uname", &["-srm"]),
        "cpu": cpu,
        "cpus_online": shell_out("nproc", &[]),
        "gpu_drm": shell_out("sh", &["-c", "for d in /sys/class/drm/card*/device; do printf '%s %s %s\\n' \"$d\" \"$(cat $d/vendor)\" \"$(cat $d/device)\"; done"]),
        "webkit2gtk": shell_out("pkg-config", &["--modversion", "webkit2gtk-4.1"]),
        "private_fc_match_monospace": fc_match,
        "renderer": "private headless sway (WLR_BACKENDS=headless, WLR_RENDERER=pixman); WebKitGTK with WEBKIT_DISABLE_DMABUF_RENDERER=1; not a physical GPU presentation",
        "renderer_variant": { "name": env.renderer.name(), "requested_env": env.renderer.env().iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>() },
        "topology": { "panes": env.panes, "clients": env.clients, "load": format!("{:?}", env.load) },
        "probe_semantics": "JS requestAnimationFrame callbacks and canvas row submissions; not physical presentation and not input-to-paint",
        "rate": { "line_bytes": SYNTHETIC_LINE.len(), "lines_per_s": SYNTHETIC_LINES_PER_S },
    })
}

fn client_evidence(
    st: &State,
    page: &Value,
    window: &Tracked,
    current: Option<u64>,
    runtime: &Path,
    name: &str,
) -> Result<ClientEvidence, String> {
    let delta = |w: &str| evidence::probe_delta(&page[w]["before"], &page[w]["after"]);
    let geometry = evidence::geometry(&page["geometry"], st.engine_size)?;
    if evidence::geometry(&page["geometry_after"], st.engine_size).as_ref() != Ok(&geometry) {
        return Err("geometry changed between the start and the end of the page run".into());
    }
    let zero =
        json!({ "raf_requests": 0, "raf_callbacks": 0, "paint_calls": 0, "painted_rows": 0 });
    Ok(ClientEvidence {
        window: ProcessId {
            pid: window.pid,
            starttime: window.starttime,
            current_starttime: current,
            owned: true,
        },
        runtime_dir: runtime.display().to_string(),
        geometry,
        terminal_canvases: page["canvases"].as_u64().unwrap_or(0) as u32,
        // Observed by the page at the end of the window; visible is refused by the page if hidden.
        terminal_hidden: match name {
            "visible" => false,
            _ => page[name]["terminal_hidden"] == true,
        },
        probe_delta: delta(name),
        // Output: the visible window. Idle (no output): the terminal paints before the idle window.
        positive_control: match name {
            "idle" => evidence::probe_delta(&zero, &page["idle"]["before"]),
            _ => delta("visible"),
        },
    })
}

/// Renderer variables of each owned window read from its own `/proc/<pid>/environ` once the page
/// asked for `bench-ready` (after exec; right after spawn the child may still show the runner's
/// environ). Returns the record and whether every window carries exactly the variant.
fn renderer_record(variant: RendererVariant, windows: &[Tracked]) -> (Value, bool) {
    let per_window: Vec<Value> = windows
        .iter()
        .map(|w| {
            let same = same_process(w, starttime(w.pid));
            let found = std::fs::read(format!("/proc/{}/environ", w.pid))
                .ok()
                .map(|e| evidence::renderer_env_of(&e));
            json!({
                "pid": w.pid,
                "starttime": w.starttime,
                "identity_checked": same,
                "env": found,
                "applied": same && found.as_ref().is_some_and(|m| variant_applied(variant, m)),
            })
        })
        .collect();
    let all = per_window.iter().all(|w| w["applied"] == json!(true));
    let record = json!({
        "name": variant.name(),
        "requested_env": variant.env().iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>(),
        "read_at": "bench-ready",
        "windows": per_window,
        "applied": all,
    });
    (record, all)
}

/// WebKit settings readback of each owned window (`<report>.policy.json`, written by the harness
/// hook) validated at `bench-ready` against the arm; the request alone never counts as applied.
fn webkit_policy_record(variant: RendererVariant, clients: &[Client]) -> (Value, bool) {
    use crate::support::window::webkit_policy::{record_path, validate};
    let policy = variant.webkit_policy();
    let per_window: Vec<Value> = clients
        .iter()
        .map(|c| {
            let path = record_path(&c.result);
            let record = std::fs::read_to_string(&path)
                .ok()
                .and_then(|r| serde_json::from_str::<Value>(&r).ok())
                .unwrap_or(Value::Null);
            let checked = match c.window.as_ref() {
                Some(w) if same_process(w, starttime(w.pid)) => validate(policy, &record, w.pid),
                _ => Err("window identity not confirmed".into()),
            };
            let (effect, error) = match checked {
                Ok(e) => (
                    json!({ "changed": e.changed, "no_op": e.no_op, "effect_proven": e.proven() }),
                    None,
                ),
                Err(e) => (Value::Null, Some(e)),
            };
            json!({
                "path": path.display().to_string(),
                "record": record,
                "valid": error.is_none(),
                "effect": effect,
                "error": error,
            })
        })
        .collect();
    let all = !per_window.is_empty() && per_window.iter().all(|w| w["valid"] == json!(true));
    let record = json!({
        "name": policy.name(),
        "setters_requested": policy.setters().iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>(),
        "read_at": "bench-ready",
        "windows": per_window,
        "applied": all,
    });
    (record, all)
}

struct Client {
    sup: Supervisor,
    runtime: PathBuf,
    result: PathBuf,
    window: Option<Tracked>,
    acked: Vec<String>,
    exited: bool,
}

/// Page params for resource-bench: probe, windows, and the mode duration the page uses to wait
/// for collector windows (`warmup_s`+`measured_s` + ≥120 s, never below the 110 s harness default).
pub fn resource_bench_page_params(mode: &Mode, load: Load, renderer: RendererVariant) -> Value {
    let windows = match load {
        Load::Idle => json!(["idle"]),
        Load::Output => json!(["visible", "hidden"]),
    };
    json!({
        "terminal_probe": true,
        "settle_ms": 700,
        "windows": windows,
        "warmup_s": mode.warmup_s(),
        "measured_s": mode.measured_s(),
        crate::support::window::webkit_policy::PARAM: renderer.webkit_policy().name(),
    })
}

fn start_client(env: &LiveEnv, session: &str, session_dir: &Path, n: u8) -> Result<Client, String> {
    let runtime = PathBuf::from(format!("/tmp/hd7B-{}-{n}", std::process::id()));
    std::fs::create_dir(&runtime)
        .and_then(|_| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700))
        })
        .map_err(|e| format!("runtime dir: {e}"))?;
    let gui = session_dir.join(format!("gui-{n}"));
    let display = PrivateDisplay::new(
        runtime.clone(),
        gui.join("home"),
        Resources::under(&env.resources_root),
        env.user.clone(),
        env.uid,
    )?;
    let sup = Supervisor::start(display, env.out.join(format!("processes-{n}")))?;
    let mut client = Client {
        sup,
        runtime,
        result: env.out.join(format!("window-{n}/report.json")),
        window: None,
        acked: Vec::new(),
        exited: false,
    };
    let (prefs, state_dir) = (gui.join("prefs"), gui.join("state"));
    let _ = std::fs::create_dir_all(&prefs);
    let _ = std::fs::create_dir_all(&state_dir);
    let _ = std::fs::create_dir_all(client.result.parent().unwrap());
    let params = resource_bench_page_params(&env.mode, env.load, env.renderer);
    let p = |x: &Path| x.display().to_string();
    let timeout_s = resource_window_timeout_s(&env.mode, env.load).to_string();
    let extra_owned = [
        ("HERDR_DESKTOP_HARNESS_PHASE", "resource-bench".to_owned()),
        ("HERDR_DESKTOP_HARNESS_RESULT", p(&client.result)),
        ("HERDR_DESKTOP_E2E_USER_UID", env.uid.to_string()),
        ("HERDR_DESKTOP_HARNESS_TIMEOUT_SECS", timeout_s),
        ("HERDR_DESKTOP_E2E_PARAMS", params.to_string()),
        ("HERDR_DESKTOP_E2E_SESSION", session.to_owned()),
        (
            "HERDR_DESKTOP_E2E_HERDR_CONFIG_DIR",
            p(&herdr_client::session::herdr_config_dir(&|k: &str| {
                std::env::var(k).ok()
            })),
        ),
        ("HERDR_DESKTOP_E2E_PREFS_DIR", p(&prefs)),
        ("HERDR_DESKTOP_E2E_STATE_DIR", p(&state_dir)),
        ("HERDR_DESKTOP_HERDR_BIN", p(&env.herdr_bin)),
    ];
    let mut extra: Vec<(&str, &str)> = extra_owned.iter().map(|(k, v)| (*k, v.as_str())).collect();
    if let Err(e) = check_renderer_env(env.renderer.env()) {
        let left = client.sup.cleanup();
        return Err(format!("renderer variant: {e}; cleanup {left:?}"));
    }
    extra.extend_from_slice(env.renderer.env());
    match client.sup.start_window(&env.exe, &extra) {
        Ok(wpid) => {
            client.window = client.sup.tracked.iter().find(|t| t.pid == wpid).cloned();
            Ok(client)
        }
        Err(e) => {
            let left = client.sup.cleanup();
            Err(format!("window: {e}; cleanup {left:?}"))
        }
    }
}

fn start_latency_client(
    env: &LiveLatencyEnv,
    session: &str,
    session_dir: &Path,
    total_transitions: usize,
) -> Result<Client, String> {
    let runtime = PathBuf::from(format!("/tmp/hd7B-{}-lat", std::process::id()));
    if runtime.exists() {
        let _ = std::fs::remove_dir_all(&runtime);
    }
    std::fs::create_dir(&runtime)
        .and_then(|_| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700))
        })
        .map_err(|e| format!("runtime dir: {e}"))?;
    let gui = session_dir.join("gui-1");
    let display = PrivateDisplay::new(
        runtime.clone(),
        gui.join("home"),
        Resources::under(&env.resources_root),
        env.user.clone(),
        env.uid,
    )?;
    let sup = Supervisor::start(display, env.out.join("processes-lat"))?;
    let mut client = Client {
        sup,
        runtime,
        result: env.out.join("window-lat/report.json"),
        window: None,
        acked: Vec::new(),
        exited: false,
    };
    let (prefs, state_dir) = (gui.join("prefs"), gui.join("state"));
    let _ = std::fs::create_dir_all(&prefs);
    let _ = std::fs::create_dir_all(&state_dir);
    let _ = std::fs::create_dir_all(client.result.parent().unwrap());
    let params = json!({
        "latency": true,
        "transitions": total_transitions,
    });
    let p = |x: &Path| x.display().to_string();
    let extra_owned = [
        ("HERDR_DESKTOP_HARNESS_PHASE", "resource-bench".to_owned()),
        ("HERDR_DESKTOP_HARNESS_RESULT", p(&client.result)),
        ("HERDR_DESKTOP_E2E_USER_UID", env.uid.to_string()),
        ("HERDR_DESKTOP_E2E_PARAMS", params.to_string()),
        ("HERDR_DESKTOP_E2E_SESSION", session.to_owned()),
        (
            "HERDR_DESKTOP_E2E_HERDR_CONFIG_DIR",
            p(&herdr_client::session::herdr_config_dir(&|k: &str| {
                std::env::var(k).ok()
            })),
        ),
        ("HERDR_DESKTOP_E2E_PREFS_DIR", p(&prefs)),
        ("HERDR_DESKTOP_E2E_STATE_DIR", p(&state_dir)),
        ("HERDR_DESKTOP_HERDR_BIN", p(&env.herdr_bin)),
    ];
    let extra: Vec<(&str, &str)> = extra_owned.iter().map(|(k, v)| (*k, v.as_str())).collect();
    match client.sup.start_window(&env.exe, &extra) {
        Ok(wpid) => {
            client.window = client.sup.tracked.iter().find(|t| t.pid == wpid).cloned();
            Ok(client)
        }
        Err(e) => {
            let left = client.sup.cleanup();
            Err(format!("window: {e}; cleanup {left:?}"))
        }
    }
}

fn want(client: &Client, step: &str) -> Option<Value> {
    std::fs::read_to_string(client.result.with_extension(format!("want-{step}")))
        .ok()
        .and_then(|r| serde_json::from_str::<Value>(&r).ok())
}

fn ack(client: &mut Client, step: &str, outcome: &Value) {
    let path = client.result.with_extension(format!("ack-{step}"));
    let tmp = path.with_extension("tmp");
    let _ = std::fs::write(&tmp, outcome.to_string()).and_then(|_| std::fs::rename(&tmp, &path));
    client.acked.push(step.to_string());
}

/// Runs one topology (panes × clients × load). Returns the run report (also `out/run-report.json`).
pub fn run(env: &LiveEnv) -> Value {
    let mut report = serde_json::Map::new();
    let pid = std::process::id();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let session = format!("hd007-bench-{pid}-{stamp}");
    let session_dir = PathBuf::from(format!("/var/tmp/herdr-desktop-e2e/{session}"));
    let mut errors: Vec<String> = Vec::new();
    let mut cleanup: Vec<String> = Vec::new();
    report.insert("session".into(), json!(session));
    report.insert(
        "topology".into(),
        json!({ "panes": env.panes, "clients": env.clients, "load": format!("{:?}", env.load) }),
    );
    report.insert("foreign_webkit_before".into(), foreign_webkit_census(&[]));

    let mut engine = match Engine::start(env, &session, &session_dir) {
        Ok(e) => Some(e),
        Err(e) => {
            errors.push(format!("engine: {e}"));
            None
        }
    };
    let mut st = State::default();
    let mut clients: Vec<Client> = Vec::new();
    let mut pages: Vec<Value> = Vec::new();
    let mut build = build_info(&env.exe, None);
    if let Some(engine) = engine.as_ref() {
        let (tabs, workspaces) = env.background_split();
        match populate(engine, tabs, workspaces, &session_dir.join("work")) {
            Ok(v) => {
                report.insert("background".into(), v);
            }
            Err(e) => errors.push(format!("background panes: {e}")),
        }
        for n in 1..=env.clients {
            match start_client(env, &session, &session_dir, n) {
                Ok(c) => clients.push(c),
                Err(e) => {
                    errors.push(format!("client {n}: {e}"));
                    break;
                }
            }
        }
        let windows: Vec<Tracked> = clients.iter().filter_map(|c| c.window.clone()).collect();
        if errors.is_empty() && windows.len() == env.clients as usize {
            // Every client runs the same executable; the trace check applies to each environ.
            let environs: Vec<Option<Vec<u8>>> = windows
                .iter()
                .map(|w| std::fs::read(format!("/proc/{}/environ", w.pid)).ok())
                .collect();
            let infos: Vec<BuildInfo> = environs
                .iter()
                .map(|e| build_info(&env.exe, e.as_deref()))
                .collect();
            build = infos
                .iter()
                .find(|b| b.surface_trace)
                .cloned()
                .unwrap_or_else(|| infos[0].clone());
            report.insert(
                "window_exe_is_runner_exe".into(),
                json!(windows
                    .iter()
                    .map(|w| std::fs::read_link(format!("/proc/{}/exe", w.pid)).ok()
                        == std::fs::canonicalize(&env.exe).ok())
                    .collect::<Vec<_>>()),
            );
            let ctx = Ctx {
                env,
                engine,
                windows: windows.clone(),
                token: session.clone(),
                build: &build,
            };
            let per_window = env.mode.warmup_s() + env.mode.measured_s() + COLLECTOR_INTERVAL_S;
            let deadline = Instant::now() + Duration::from_secs_f64(180.0 + 2.5 * per_window * 2.0);
            let mut handled: Vec<&'static str> = Vec::new();
            loop {
                for c in clients.iter_mut().filter(|c| !c.exited) {
                    if let Some(status) = c.sup.try_exit("window") {
                        c.exited = true;
                        report.insert(
                            format!("window_exit_{}", c.runtime.display()),
                            json!(status.to_string()),
                        );
                    }
                }
                if clients.iter().all(|c| c.exited) {
                    break;
                }
                if Instant::now() > deadline {
                    errors.push("windows did not finish within the deadline".into());
                    break;
                }
                // Late requests of an already handled step (e.g. stop) are acknowledged again.
                for c in clients.iter_mut() {
                    for step in handled.clone() {
                        if !c.acked.iter().any(|s| s == step) && want(c, step).is_some() {
                            ack(c, step, &json!({ "already_handled": step }));
                        }
                    }
                }
                let pending: Vec<Vec<&str>> = clients
                    .iter()
                    .map(|c| {
                        BENCH_STEPS
                            .iter()
                            .copied()
                            .filter(|s| want(c, s).is_some())
                            .collect()
                    })
                    .collect();
                let views: Vec<&[&str]> = pending.iter().map(Vec::as_slice).collect();
                let step = match barrier_step(&views, &handled) {
                    Ok(Some(step)) => step,
                    Ok(None) => {
                        std::thread::sleep(Duration::from_millis(50));
                        continue;
                    }
                    Err(e) => {
                        errors.push(format!("barrier: {e}"));
                        break;
                    }
                };
                if step == "bench-ready" {
                    let (record, applied) = renderer_record(env.renderer, &windows);
                    report.insert("renderer_variant".into(), record.clone());
                    if !applied {
                        errors.push(format!(
                            "renderer variant {} not applied to every window: {record}",
                            env.renderer.name()
                        ));
                        break;
                    }
                    let (policy, valid) = webkit_policy_record(env.renderer, &clients);
                    report.insert("webkit_policy".into(), policy.clone());
                    if !valid {
                        errors.push(format!(
                            "webkit policy {} readback not valid for every window: {policy}",
                            env.renderer.webkit_policy().name()
                        ));
                        break;
                    }
                }
                handled.push(step);
                let details: Vec<Value> = clients.iter().filter_map(|c| want(c, step)).collect();
                let outcome = match step {
                    "bench-ready" => ready(&ctx, &mut st, &details),
                    "bench-stop" => Ok(stop_generators(&mut st)),
                    other => measure(&ctx, &mut st, other.trim_start_matches("bench-")),
                }
                .unwrap_or_else(|e| {
                    st.errors.push(format!("{step}: {e}"));
                    json!({ "error": e })
                });
                for c in clients.iter_mut() {
                    if want(c, step).is_some() {
                        ack(c, step, &outcome);
                    }
                }
            }
        }
        for c in clients.iter_mut() {
            pages.push(
                std::fs::read_to_string(&c.result)
                    .ok()
                    .and_then(|r| serde_json::from_str(&r).ok())
                    .unwrap_or(Value::Null),
            );
            report.insert(
                format!("isolation_unix_sockets_{}", c.runtime.display()),
                json!(c.sup.isolation()),
            );
        }
        let roots: Vec<u32> = clients
            .iter()
            .filter_map(|c| c.window.as_ref().map(|w| w.pid))
            .collect();
        report.insert(
            "foreign_webkit_after_windows_exit".into(),
            foreign_webkit_census(&roots),
        );
        report.insert("stop_generators_final".into(), stop_generators(&mut st));
        for c in clients.iter_mut() {
            cleanup.extend(c.sup.cleanup());
        }
        report.insert(
            "manifest".into(),
            manifest(env, &build, &session_dir.join("gui-1/home")),
        );
    }
    let engine_tracked = engine.as_ref().map(|e| e.tracked.clone());
    if let Some(mut e) = engine.take() {
        cleanup.extend(e.stop());
    }
    for p in &st.ptys {
        for tracked in std::iter::once(&p.shell).chain(p.generator.iter().map(|(t, _)| t)) {
            if same_process(tracked, starttime(tracked.pid)) {
                cleanup.push(format!("LEFTOVER {} pid={}", tracked.name, tracked.pid));
            }
        }
    }
    let _ = std::fs::remove_dir_all(&session_dir);
    cleanup.push(format!("session dir removed: {}", !session_dir.exists()));

    errors.extend(st.errors.iter().cloned());
    let mut verdicts = serde_json::Map::new();
    let complete = clients.len() == env.clients as usize
        && clients.iter().all(|c| c.window.is_some())
        && st.ptys.len() == env.panes as usize;
    if !complete {
        errors.push("incomplete clients/PTY identities: nothing to evaluate".into());
    }
    for output in env.load.outputs().into_iter().filter(|_| complete) {
        let name = match output {
            Output::Idle => "idle",
            Output::Visible => "visible",
            Output::Hidden => "hidden",
        };
        let Some(w) = st.windows.get(name).cloned() else {
            errors.push(format!("{name}: window not measured"));
            continue;
        };
        let dir = PathBuf::from(w["collector_dir"].as_str().unwrap_or(""));
        let summary: Value = std::fs::read_to_string(dir.join("summary.json"))
            .ok()
            .and_then(|r| serde_json::from_str(&r).ok())
            .unwrap_or(Value::Null);
        let observed = std::fs::read_to_string(dir.join("samples.jsonl"))
            .map_err(|e| e.to_string())
            .and_then(|s| {
                let actual = summary["metadata"]["actual_duration_s"]
                    .as_f64()
                    .unwrap_or(f64::NAN);
                evidence::collector_window(&s, actual)
            });
        let (warmup, measured) = observed.clone().unwrap_or((f64::NAN, f64::NAN));
        let mut client_ev = Vec::new();
        for (n, (c, page)) in clients.iter().zip(&pages).enumerate() {
            let window = c.window.as_ref().unwrap();
            let current = w["current_starttime"]["windows"][n].as_u64();
            match client_evidence(&st, page, window, current, &c.runtime, name) {
                Ok(ev) => client_ev.push(ev),
                Err(e) => errors.push(format!("{name} gui_{}: {e}", n + 1)),
            }
        }
        let producers = st
            .ptys
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let (gen, owned) = p
                    .generator
                    .clone()
                    .map_or((None, false), |(t, o)| (Some(t), o));
                Producer {
                    pane_id: p.pane_id.clone(),
                    engine_shell_pid: p.engine_shell_pid,
                    shell: ProcessId {
                        pid: p.shell.pid,
                        starttime: p.shell.starttime,
                        current_starttime: w["current_starttime"]["shells"][i].as_u64(),
                        owned: true,
                    },
                    shell_ancestors: p.shell_ancestors.clone(),
                    generator: ProcessId {
                        pid: gen.as_ref().map_or(0, |g| g.pid),
                        starttime: gen.as_ref().map_or(0, |g| g.starttime),
                        current_starttime: w["current_starttime"]["generators"][i].as_u64(),
                        owned,
                    },
                    generator_ancestors: p.generator_ancestors.clone(),
                    counter_start: counter_at(&w, "counter_start", i),
                    counter_end: counter_at(&w, "counter_end", i),
                }
            })
            .collect();
        let e = Evidence {
            scenario: Scenario {
                panes: env.panes,
                clients: env.clients,
                output,
            },
            mode: env.mode,
            build: build.clone(),
            session: session.clone(),
            engine: engine_identity(&w, engine_tracked.as_ref()),
            pane_list_count: st.pane_list_count,
            clients: client_ev,
            producers,
            warmup_s: warmup,
            measured_s: measured,
            collector_files: file_names(&dir),
            collector_summary: summary.clone(),
            cleanup_log: cleanup.clone(),
        };
        let status = match evaluate(&e) {
            RunStatus::Rejected(r) => json!({ "status": "Rejected", "reasons": r }),
            RunStatus::InsufficientForAcceptance => {
                json!({ "status": "INSUFFICIENT_FOR_ACCEPTANCE" })
            }
            RunStatus::MeasuredPendingReview => json!({ "status": "MeasuredPendingReview" }),
        };
        let targets: serde_json::Map<String, Value> = judge(&e, None)
            .into_iter()
            .map(|(k, v)| (k, json!(format!("{v:?}"))))
            .collect();
        verdicts.insert(
            name.into(),
            json!({
                "verdict": status,
                "observed_window_s": observed.map(|(w, m)| json!({ "warmup": w, "measured": m })).unwrap_or_else(|e| json!({ "error": e })),
                "prd_targets_gated": targets,
                "observations": observations(&summary),
                "evidence": format!("{e:#?}"),
            }),
        );
    }
    report.insert(
        "engine".into(),
        json!(engine_tracked.map(|t| json!({ "pid": t.pid, "starttime": t.starttime }))),
    );
    report.insert("state_raw".into(), Value::Object(st.raw.clone()));
    report.insert("windows".into(), Value::Object(st.windows.clone()));
    report.insert("page_reports".into(), json!(pages));
    report.insert("verdicts".into(), Value::Object(verdicts));
    report.insert("cleanup_log".into(), json!(cleanup));
    report.insert("errors".into(), json!(errors));
    let report = Value::Object(report);
    let _ = std::fs::write(
        env.out.join("run-report.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    );
    report
}

fn engine_identity(window: &Value, spawned: Option<&Tracked>) -> ProcessId {
    ProcessId {
        pid: spawned.map_or(0, |t| t.pid),
        starttime: spawned.map_or(0, |t| t.starttime),
        current_starttime: window["current_starttime"]["engine"].as_u64(),
        owned: spawned.is_some(),
    }
}

/// Sway keyboard focus must sit on the measured window before private wtype, or the key
/// is dropped (`wl_keyboard.enter` never happens) and the page records zero trusted keydowns.
fn require_compositor_focus(sup: &mut Supervisor, window_pid: u32) -> Result<(), String> {
    if window_pid == 0 {
        return Err("window pid missing".into());
    }
    let focused = |tree: &serde_json::Value| crate::latency::viewport::focused_window_pid(tree);
    if focused(&sup.get_tree()?)? == window_pid {
        return Ok(());
    }
    sup.focus_window(window_pid)?;
    let now = focused(&sup.get_tree()?)?;
    if now != window_pid {
        return Err(format!(
            "sway focused pid {now}, expected window {window_pid}"
        ));
    }
    Ok(())
}

pub fn run_latency(env: &LiveLatencyEnv) -> Value {
    let mut report = serde_json::Map::new();
    let pid = std::process::id();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let session = format!("hd007-lat-{pid}-{stamp}");
    let session_dir = PathBuf::from(format!("/var/tmp/herdr-desktop-e2e/{session}"));
    let mut errors: Vec<String> = Vec::new();
    let mut cleanup: Vec<String> = Vec::new();
    let total_transitions = env.warmup + env.measured;

    report.insert("session".into(), json!(session));
    report.insert(
        "mode".into(),
        json!({
            "name": format!("{:?}", env.mode),
            "warmup": env.warmup,
            "measured": env.measured,
            "total_transitions": total_transitions,
        }),
    );

    // 1. Start disposable engine
    let mut engine =
        match Engine::start_for_session(&env.herdr_bin, &env.out, &session, &session_dir) {
            Ok(e) => Some(e),
            Err(e) => {
                errors.push(format!("engine: {e}"));
                None
            }
        };

    // 2. Connect parent LocalGateway to verify boot identity
    let parent_session = match herdr_client::SessionName::parse(&session) {
        Ok(s) => Some(s),
        Err(e) => {
            errors.push(format!("parent session parse: {}", e.message));
            None
        }
    };
    let mut parent_gateway = parent_session.map(|s| {
        let config_dir = herdr_client::session::herdr_config_dir(&|k| std::env::var(k).ok());
        herdr_client::LocalGateway::new(&config_dir, s)
    });
    let mut parent_boot = String::new();
    if let Some(gw) = parent_gateway.as_mut() {
        match gw.connect(herdr_client::ConnectOptions {
            geometry: herdr_client::SurfaceGeometry {
                cols: 80,
                rows: 24,
                cell_width_px: 9,
                cell_height_px: 18,
            },
            surface_active: false,
        }) {
            Ok(_) => {
                if let Some(events) = gw.take_event_stream() {
                    std::thread::spawn(move || while events.recv().is_ok() {});
                }
                let boot_deadline = Instant::now() + Duration::from_secs(10);
                while Instant::now() < boot_deadline {
                    if let Some(id) = gw.identity() {
                        parent_boot = id.boot_id;
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                if parent_boot.is_empty() {
                    errors.push("parent gateway identity timeout".into());
                }
            }
            Err(e) => errors.push(format!("parent gateway connect: {e}")),
        }
    }

    // 3. Locate wtype binary and verify build
    let wtype_bin = [
        env.repo.join(".local/native-latency/bin/wtype-hd-latency"),
        env.resources_root
            .join("native-latency/bin/wtype-hd-latency"),
    ]
    .into_iter()
    .find(|p| p.exists());

    let wtype_sha256 = wtype_bin
        .as_ref()
        .and_then(|bin| {
            Command::new("sha256sum")
                .arg(bin)
                .output()
                .ok()
                .and_then(|o| {
                    String::from_utf8_lossy(&o.stdout)
                        .split_whitespace()
                        .next()
                        .map(str::to_owned)
                })
        })
        .unwrap_or_default();

    let expected_wtype = crate::latency::record::WtypeBuild {
        commit: "d71be3a7b3f93b534a2823fd68cabd7ac2a02359".into(),
        binary_sha256: "efe1ea92605f7872ff4a8d656a6e871eaf5ac820731b8e259a5521628a58ad4f".into(),
    };
    let wtype_build = crate::latency::record::WtypeBuild {
        commit: "d71be3a7b3f93b534a2823fd68cabd7ac2a02359".into(),
        binary_sha256: wtype_sha256,
    };

    let mut client = if errors.is_empty() {
        match start_latency_client(env, &session, &session_dir, total_transitions) {
            Ok(c) => Some(c),
            Err(e) => {
                errors.push(format!("client start: {e}"));
                None
            }
        }
    } else {
        None
    };

    let mut smoke_proofs = serde_json::Map::new();
    let mut run_identity: Option<crate::latency::record::Identity> = None;
    let mut run_attempts: Vec<crate::latency::record::Attempt> = Vec::new();
    let mut page_report_val = Value::Null;
    let helper_log = env.out.join("helper.log");

    if let (Some(engine), Some(client), Some(wtype_bin_path)) =
        (engine.as_ref(), client.as_mut(), wtype_bin.as_ref())
    {
        // Step 1: wait for latency-ready
        let ready_deadline = Instant::now() + Duration::from_secs(60);
        let mut ready_detail = None;
        while Instant::now() < ready_deadline {
            if let Some(status) = client.sup.try_exit("window") {
                client.exited = true;
                errors.push(format!(
                    "window exited prematurely before latency-ready: {status}"
                ));
                break;
            }
            if let Some(detail) = want(client, "latency-ready") {
                ready_detail = Some(detail);
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        if let Some(detail) = ready_detail {
            let pane_id = detail["pane_id"].as_str().unwrap_or("").to_owned();
            let page_boot_prefix = detail["boot_prefix"].as_str().unwrap_or("").to_owned();
            let page_gen_str = detail["generation"].as_str().unwrap_or("").to_owned();
            let page_gen = page_gen_str.parse::<u64>().unwrap_or(0);

            // Validations:
            let pane_list = engine.pane_ids().unwrap_or_default();
            if !pane_list.contains(&pane_id) {
                errors.push(format!(
                    "pane {pane_id} not in engine pane list: {pane_list:?}"
                ));
            }
            if page_boot_prefix.is_empty() || !parent_boot.starts_with(&page_boot_prefix) {
                errors.push(format!(
                    "page boot prefix {page_boot_prefix:?} does not prefix parent boot {parent_boot:?}"
                ));
            }
            if page_gen == 0 {
                errors.push(format!("invalid page generation: {page_gen_str:?}"));
            }

            let page_obs = match crate::latency::marker::PageObservation::from_json(&detail["page"])
            {
                Ok(p) => Some(p),
                Err(e) => {
                    errors.push(format!("page observation: {e}"));
                    None
                }
            };

            let client_w_css = detail["client_w_css"].as_f64().unwrap_or(1280.0);
            let client_h_css = detail["client_h_css"].as_f64().unwrap_or(720.0);

            if let Some(page) = page_obs {
                let identity = crate::latency::record::Identity {
                    host: "local".into(),
                    session: session.clone(),
                    boot: parent_boot.clone(),
                    generation: page_gen,
                    pane: pane_id.clone(),
                    dpr_milli: (page.dpr * 1000.0).round() as u32,
                    cols: page.cols,
                    rows: page.rows,
                };
                run_identity = Some(identity);

                // Run pty_marker.py inside the pane
                let pty_marker_py = env.repo.join("tests/fidelity-latency/pty_marker.py");
                let cmd = format!(
                    "python3 '{}' --log '{}'",
                    pty_marker_py.display(),
                    helper_log.display()
                );
                if let Err(e) = engine.cli(&["pane", "run", &pane_id, &cmd]) {
                    errors.push(format!("pane run pty_marker: {e}"));
                }

                // Wait for ready line in helper.log
                let helper_deadline = Instant::now() + Duration::from_secs(10);
                let mut ready_cols = 0u32;
                let mut ready_rows = 0u32;
                let mut found_ready = false;
                while Instant::now() < helper_deadline {
                    if let Ok(content) = std::fs::read_to_string(&helper_log) {
                        for line in content.lines() {
                            if line.starts_with("hd-pty v1 ready cols=") {
                                for part in line.split_whitespace() {
                                    if let Some(c) = part.strip_prefix("cols=") {
                                        ready_cols = c.parse().unwrap_or(0);
                                    } else if let Some(r) = part.strip_prefix("rows=") {
                                        ready_rows = r.parse().unwrap_or(0);
                                    }
                                }
                                found_ready = true;
                                break;
                            }
                        }
                        if found_ready {
                            break;
                        }
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }

                if !found_ready {
                    errors.push("pty_marker.py ready line not observed".into());
                } else {
                    if ready_cols < 52 || ready_rows < 5 {
                        errors.push(format!("pty size {ready_cols}x{ready_rows} < 52x5"));
                    }
                    if ready_cols != page.cols || ready_rows != page.rows {
                        errors.push(format!(
                            "pty size {ready_cols}x{ready_rows} != page {}x{}",
                            page.cols, page.rows
                        ));
                    }
                }

                // Viewport offset and seq-0 confirmation
                let cell_grid = match client.sup.get_tree() {
                    Ok(tree) => {
                        let wpid = client.window.as_ref().map(|w| w.pid).unwrap_or(0);
                        match crate::latency::viewport::content_rect(&tree, wpid) {
                            Ok(c_rect) => match crate::latency::viewport::offset_from_client(
                                c_rect,
                                client_w_css,
                                client_h_css,
                                page.dpr,
                            ) {
                                Ok(predicted) => {
                                    let seq0_deadline = Instant::now() + Duration::from_secs(5);
                                    let mut confirmed_offset = None;
                                    let mut last_err = String::new();
                                    while Instant::now() < seq0_deadline {
                                        if let Ok((raw_ppm, _, _)) = client.sup.grim_ppm() {
                                            if let Ok(img) =
                                                crate::support::view_flow::parse_ppm(&raw_ppm)
                                            {
                                                let rgb_view = crate::latency::marker::RgbView {
                                                    width: img.width,
                                                    height: img.height,
                                                    rgb: &img.rgb,
                                                };
                                                match crate::latency::viewport::locate_seq0(rgb_view, &page, predicted) {
                                                    Ok(located) => match crate::latency::viewport::confirm_offset(predicted, located) {
                                                        Ok(confirmed) => {
                                                            smoke_proofs.insert("seq0_confirmed".into(), json!(true));
                                                            smoke_proofs.insert("viewport_offset_px".into(), json!([confirmed.0, confirmed.1]));
                                                            confirmed_offset = Some(confirmed);
                                                            break;
                                                        }
                                                        Err(e) => {
                                                            last_err = format!("confirm_offset: {e}");
                                                        }
                                                    },
                                                    Err(e) => {
                                                        last_err = format!("locate_seq0: {e}");
                                                    }
                                                }
                                            }
                                        }
                                        std::thread::sleep(Duration::from_millis(25));
                                    }
                                    if confirmed_offset.is_none() {
                                        errors.push(if last_err.is_empty() {
                                            "locate_seq0: timeout waiting for seq-0 marker".into()
                                        } else {
                                            last_err
                                        });
                                    }
                                    confirmed_offset.and_then(|c| {
                                        crate::latency::marker::grid(&page, Some(c)).ok()
                                    })
                                }
                                Err(e) => {
                                    errors.push(format!("offset_from_client: {e}"));
                                    None
                                }
                            },
                            Err(e) => {
                                errors.push(format!("content_rect: {e}"));
                                None
                            }
                        }
                    }
                    Err(e) => {
                        errors.push(format!("get_tree: {e}"));
                        None
                    }
                };

                // Ack latency-ready
                ack(
                    client,
                    "latency-ready",
                    &json!({ "ok": errors.is_empty(), "transitions": total_transitions }),
                );

                // Run attempts loop if no errors so far and cell_grid is available
                if errors.is_empty() {
                    if let Some(grid) = cell_grid {
                        let mut attempts_data = Vec::new();

                        for i in 0..total_transitions {
                            let expected_seq = (i + 1) as u16;
                            let step = format!("latency-attempt-{i:03}");
                            let attempt_deadline = Instant::now() + Duration::from_secs(10);
                            let mut attempt_detail = None;
                            while Instant::now() < attempt_deadline {
                                if let Some(status) = client.sup.try_exit("window") {
                                    client.exited = true;
                                    errors.push(format!("window exited during {step}: {status}"));
                                    break;
                                }
                                if let Some(detail) = want(client, &step) {
                                    attempt_detail = Some(detail);
                                    break;
                                }
                                std::thread::sleep(Duration::from_millis(10));
                            }
                            let Some(detail) = attempt_detail else {
                                errors.push(format!("timeout waiting for {step}"));
                                break;
                            };

                            // Check attempt identity
                            let att_pane = detail["pane_id"].as_str().unwrap_or("");
                            let att_gen = detail["generation"].as_str().unwrap_or("");
                            let att_boot = detail["boot_prefix"].as_str().unwrap_or("");
                            if att_pane != pane_id
                                || att_gen != page_gen_str
                                || att_boot != page_boot_prefix
                            {
                                errors.push(format!("attempt {i} identity changed: pane={att_pane} gen={att_gen} boot={att_boot}"));
                                break;
                            }

                            let wtype_log = env.out.join(format!("wtype-{i:03}.log"));
                            if wtype_log.exists() {
                                errors.push(format!(
                                    "wtype log {} exists before wtype spawn",
                                    wtype_log.display()
                                ));
                                break;
                            }

                            let wpid = client.window.as_ref().map(|w| w.pid).unwrap_or(0);
                            if let Err(e) = require_compositor_focus(&mut client.sup, wpid) {
                                errors.push(format!("attempt {i} compositor focus: {e}"));
                                break;
                            }

                            // Fresh private wtype 'a'
                            if let Err(e) = client.sup.latency_wtype(wtype_bin_path, &wtype_log) {
                                errors.push(format!("attempt {i} latency_wtype: {e}"));
                                break;
                            }

                            let wtype_text = match std::fs::read_to_string(&wtype_log) {
                                Ok(t) => t,
                                Err(e) => {
                                    errors.push(format!("read wtype log {i}: {e}"));
                                    break;
                                }
                            };
                            let wtype_log_parsed =
                                match crate::latency::pty::parse_wtype_log(&wtype_text) {
                                    Ok(w) => w,
                                    Err(e) => {
                                        errors.push(format!("parse wtype log {i}: {e}"));
                                        break;
                                    }
                                };
                            if wtype_log_parsed.press.is_empty()
                                || wtype_log_parsed.released_ns.is_empty()
                            {
                                errors.push(format!("attempt {i}: empty wtype log press/release"));
                                break;
                            }
                            let press = wtype_log_parsed.press[0].clone();
                            let released_ns = wtype_log_parsed.released_ns[0];

                            // Grim capture loop (no sleep until positive or 2s)
                            let mut captures = Vec::new();
                            let loop_deadline = Instant::now() + Duration::from_secs(2);
                            loop {
                                let (raw_ppm, start_ns, end_ns) = match client.sup.grim_ppm() {
                                    Ok(p) => p,
                                    Err(e) => {
                                        errors.push(format!("attempt {i} grim_ppm: {e}"));
                                        break;
                                    }
                                };
                                let img = match crate::support::view_flow::parse_ppm(&raw_ppm) {
                                    Ok(im) => im,
                                    Err(e) => {
                                        errors.push(format!("attempt {i} parse_ppm: {e}"));
                                        break;
                                    }
                                };
                                let rgb_view = crate::latency::marker::RgbView {
                                    width: img.width,
                                    height: img.height,
                                    rgb: &img.rgb,
                                };
                                let decoded = crate::latency::marker::decode(rgb_view, &grid);
                                let is_positive = decoded == Ok(expected_seq);
                                let seq_res = decoded.map_err(|e| format!("{e:?}"));
                                captures.push(crate::latency::record::Capture {
                                    clock: crate::latency::clock::CLOCK_NAME.into(),
                                    start_ns,
                                    end_ns,
                                    seq: seq_res,
                                });
                                if is_positive {
                                    break;
                                }
                                if end_ns - press.ns > crate::latency::record::TIMEOUT_NS
                                    || Instant::now() > loop_deadline
                                {
                                    break;
                                }
                            }

                            attempts_data.push((press, released_ns, captures));

                            // Ack attempt
                            ack(client, &step, &json!({ "ok": true, "attempt": i }));
                        }

                        // Handle latency-stop
                        let stop_deadline = Instant::now() + Duration::from_secs(10);
                        while Instant::now() < stop_deadline {
                            if let Some(_status) = client.sup.try_exit("window") {
                                client.exited = true;
                                break;
                            }
                            if want(client, "latency-stop").is_some() {
                                ack(client, "latency-stop", &json!({ "ok": true }));
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(20));
                        }

                        // Wait for client to exit
                        let exit_deadline = Instant::now() + Duration::from_secs(10);
                        while !client.exited && Instant::now() < exit_deadline {
                            if let Some(_status) = client.sup.try_exit("window") {
                                client.exited = true;
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(50));
                        }

                        // Read page report
                        if let Ok(content) = std::fs::read_to_string(&client.result) {
                            page_report_val = serde_json::from_str(&content).unwrap_or(Value::Null);
                        }

                        // Read helper log events
                        let helper_content =
                            std::fs::read_to_string(&helper_log).unwrap_or_default();
                        let all_helper_events =
                            crate::latency::pty::parse_helper_log(&helper_content)
                                .unwrap_or_default();

                        let page_attempts = page_report_val["attempts"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default();

                        // Assemble attempts into run_attempts
                        for (i, (press, released_ns, captures)) in
                            attempts_data.into_iter().enumerate()
                        {
                            let expected_seq = (i + 1) as u16;
                            let h_events: Vec<crate::latency::pty::HelperEvent> = all_helper_events
                                .iter()
                                .filter(|e| e.count == (i as u64 + 1))
                                .cloned()
                                .collect();
                            let page_att = page_attempts.get(i);
                            let trusted_keydowns = page_att
                                .and_then(|a| a["trusted_keydowns"].as_u64())
                                .unwrap_or(0)
                                as u32;
                            let untrusted_keydowns = page_att
                                .and_then(|a| a["untrusted_keydowns"].as_u64())
                                .unwrap_or(0)
                                as u32;

                            run_attempts.push(crate::latency::record::Attempt {
                                index: i,
                                expected_seq,
                                identity: run_identity.clone().unwrap(),
                                press,
                                released_ns,
                                helper: h_events,
                                trusted_keydowns,
                                untrusted_keydowns,
                                captures,
                            });
                        }
                    }
                }
            }
        } else {
            errors.push("window never reported latency-ready".into());
        }

        cleanup.extend(client.sup.cleanup());
    }

    if let Some(mut e) = engine.take() {
        cleanup.extend(e.stop());
    }
    let _ = std::fs::remove_dir_all(&session_dir);
    cleanup.push(format!("session dir removed: {}", !session_dir.exists()));

    // Evaluate Run
    let mut eval_report = None;
    if let Some(identity) = run_identity.clone() {
        let clock_res_ns = crate::latency::clock::monotonic_res_ns();
        let run = crate::latency::record::Run {
            clock: crate::latency::clock::CLOCK_NAME.into(),
            clock_res_ns,
            identity: identity.clone(),
            wtype: wtype_build,
            expected_wtype,
            attempts: run_attempts.clone(),
        };

        let rep = crate::latency::record::evaluate(&run);
        eval_report = Some(rep);

        // Write raw.jsonl
        let raw_path = env.out.join("raw.jsonl");
        let mut raw_lines = Vec::new();
        raw_lines.push(
            json!({
                "header": true,
                "clock": run.clock,
                "clock_res_ns": run.clock_res_ns,
                "identity": {
                    "host": identity.host,
                    "session": identity.session,
                    "boot": identity.boot,
                    "generation": identity.generation,
                    "pane": identity.pane,
                    "dpr_milli": identity.dpr_milli,
                    "cols": identity.cols,
                    "rows": identity.rows,
                },
                "wtype": {
                    "commit": run.wtype.commit,
                    "binary_sha256": run.wtype.binary_sha256,
                },
                "mode": format!("{:?}", env.mode),
                "warmup": env.warmup,
                "measured": env.measured,
                "total_attempts": run.attempts.len(),
            })
            .to_string(),
        );

        for att in &run.attempts {
            let helper_json: Vec<Value> = att
                .helper
                .iter()
                .map(|h| {
                    json!({
                        "byte": h.byte,
                        "count": h.count,
                        "seq": h.seq,
                    })
                })
                .collect();
            let captures_json: Vec<Value> = att
                .captures
                .iter()
                .map(|c| {
                    json!({
                        "clock": c.clock,
                        "start_ns": c.start_ns,
                        "end_ns": c.end_ns,
                        "seq": c.seq.as_ref().ok(),
                        "error": c.seq.as_ref().err(),
                    })
                })
                .collect();
            raw_lines.push(
                json!({
                    "index": att.index,
                    "expected_seq": att.expected_seq,
                    "press": {
                        "clock": att.press.clock,
                        "index": att.press.index,
                        "keycode": att.press.keycode,
                        "ns": att.press.ns,
                    },
                    "released_ns": att.released_ns,
                    "helper": helper_json,
                    "trusted_keydowns": att.trusted_keydowns,
                    "untrusted_keydowns": att.untrusted_keydowns,
                    "captures": captures_json,
                })
                .to_string(),
            );
        }
        let _ = std::fs::write(&raw_path, raw_lines.join("\n") + "\n");
    }

    // Check smoke proofs
    let private_input_proven = run_attempts.iter().all(|a| {
        a.press.index == 0
            && a.press.clock == crate::latency::clock::CLOCK_NAME
            && a.trusted_keydowns == 1
            && a.untrusted_keydowns == 0
    }) && !run_attempts.is_empty();

    let colours_valid = run_attempts.iter().all(|a| {
        a.captures
            .iter()
            .any(|c| c.seq.as_ref().is_ok_and(|s| *s == a.expected_seq))
    }) && !run_attempts.is_empty();

    let geometry_valid = run_identity
        .as_ref()
        .is_some_and(|id| id.cols >= 52 && id.rows >= 5);

    let cleanup_clean = !cleanup.iter().any(|c| c.starts_with("LEFTOVER"));

    smoke_proofs.insert("private_input".into(), json!(private_input_proven));
    smoke_proofs.insert("colours_valid".into(), json!(colours_valid));
    smoke_proofs.insert("geometry_valid".into(), json!(geometry_valid));
    smoke_proofs.insert("cleanup_clean".into(), json!(cleanup_clean));
    let seq0_confirmed = smoke_proofs.get("seq0_confirmed") == Some(&json!(true));
    let all_smoke_passed = private_input_proven
        && colours_valid
        && geometry_valid
        && cleanup_clean
        && seq0_confirmed
        && errors.is_empty();
    smoke_proofs.insert("all_passed".into(), json!(all_smoke_passed));

    report.insert("smoke_proofs".into(), Value::Object(smoke_proofs));
    report.insert("page_report".into(), page_report_val);
    report.insert("cleanup_log".into(), json!(cleanup));
    report.insert("errors".into(), json!(errors));

    if let Some(rep) = eval_report {
        let samples_json: Vec<Value> = rep
            .samples
            .iter()
            .map(|s| {
                json!({
                    "index": s.index,
                    "upper_ns": s.upper_ns,
                    "lower_ns": s.lower_ns,
                    "captures": s.captures,
                })
            })
            .collect();
        report.insert("samples".into(), json!(samples_json));
        report.insert("p95_upper_ns".into(), json!(rep.p95_upper_ns));
        report.insert("p99_upper_ns".into(), json!(rep.p99_upper_ns));
        report.insert("p95_lower_ns".into(), json!(rep.p95_lower_ns));
        report.insert("p99_lower_ns".into(), json!(rep.p99_lower_ns));
        let verdict_status = match &rep.verdict {
            crate::latency::record::Verdict::Met => json!({ "status": "Met" }),
            crate::latency::record::Verdict::NotMet => json!({ "status": "NotMet" }),
            crate::latency::record::Verdict::Insufficient(r) => {
                json!({ "status": "Insufficient", "reasons": r })
            }
            crate::latency::record::Verdict::Invalid(r) => {
                json!({ "status": "Invalid", "reasons": r })
            }
        };
        report.insert("verdict".into(), verdict_status);
    }

    let report_val = Value::Object(report);
    let _ = std::fs::write(
        env.out.join("run-report.json"),
        serde_json::to_vec_pretty(&report_val).unwrap(),
    );
    report_val
}
