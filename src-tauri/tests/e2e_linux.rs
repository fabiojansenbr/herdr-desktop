//! The single native Linux E2E of spec 001 (AC-001-01, latency sample for AC-001-02 and
//! the recoverable half of AC-001-03: connection lost after attach and GUI opened without a
//! server — `disconnected`, retryable error, input disabled, process alive ≥ 2 s, no engine
//! or session socket created by the GUI).
//!
//! Requires a disposable session created by `scripts/e2e-session.sh start` and these
//! variables (the gate sets them; missing variables FAIL the test, they never skip it):
//!   HERDR_DESKTOP_E2E_SESSION   name of the disposable session (hd001-*)
//!   HERDR_DESKTOP_E2E_PANE      root pane id of that session (e.g. w1:p1)
//!   HERDR_DESKTOP_E2E_APP_BIN   built herdr-desktop executable for the window phase
//!   HERDR_DESKTOP_E2E_REPORT    directory where latency/geometry evidence is written
//!
//! Marked `#[ignore]` because the default suite must not depend on a server; the gate runs
//! it explicitly with `--run-ignored ignored-only`.

#![cfg(target_os = "linux")]

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use herdr_client::bootstrap::FATAL_LINE;
use herdr_client::local::GatewayEvents;
use herdr_client::protocol::wire::{ClientKeyCode, ClientPaneInputEvent};
use herdr_client::{
    ApplyOutcome, ConnectOptions, FrameStore, GatewayEvent, LocalGateway, QualifiedTarget,
    RuntimeGateway, SessionName, SessionPaths, SurfaceGeometry,
};

const STEP_TIMEOUT: Duration = Duration::from_secs(20);

fn required(name: &str) -> String {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("{name} must be set by the gate (scripts/check-spec.mjs)"))
}

struct Harness {
    gateway: LocalGateway,
    events: GatewayEvents,
    store: FrameStore,
    target: QualifiedTarget,
    geometry: SurfaceGeometry,
}

impl Harness {
    fn connect(
        config_dir: &std::path::Path,
        session: &SessionName,
        pane_id: &str,
        geometry: SurfaceGeometry,
    ) -> Self {
        let mut gateway = LocalGateway::new(config_dir, session.clone());
        let negotiated = gateway
            .connect(ConnectOptions {
                geometry,
                surface_active: true,
            })
            .expect("generation-1 negotiation against the disposable session");
        assert_eq!(negotiated.generation, 1, "endpoint generation must be 1");
        assert!(negotiated.supports_method("client_shell.surface.set"));
        assert!(negotiated
            .capabilities
            .iter()
            .any(|c| c == "surface_interest"));
        let events = gateway.take_event_stream().expect("event stream");
        let mut harness = Harness {
            gateway,
            events,
            store: FrameStore::new(),
            target: QualifiedTarget {
                endpoint: "local".into(),
                session: session.as_str().into(),
                connection_generation: negotiated.identity.connection_generation,
                boot_id: String::new(),
                workspace_id: None,
                pane_id: pane_id.to_owned(),
            },
            geometry,
        };
        // First full surface establishes boot id and geometry.
        harness.pump_until(|h| h.store.surface().is_some(), "first full surface");
        let identity = harness
            .gateway
            .identity()
            .expect("boot id known after snapshot/surface");
        harness.target = QualifiedTarget::new(&identity, None, pane_id);
        harness.gateway.set_focus(true).unwrap();
        harness
    }

    /// Drains events until `done` returns true or the step timeout elapses (panics).
    fn pump_until(&mut self, done: impl Fn(&Harness) -> bool, what: &str) {
        let deadline = Instant::now() + STEP_TIMEOUT;
        loop {
            if done(self) {
                return;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "timed out waiting for {what}; rows={:?}",
                self.store.text_rows()
            );
            match self
                .events
                .recv_timeout(remaining.min(Duration::from_millis(500)))
            {
                Ok(event) => self.apply(event),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("gateway reader ended while waiting for {what}")
                }
            }
        }
    }

    fn apply(&mut self, event: GatewayEvent) {
        match event {
            GatewayEvent::Surface(surface) => {
                self.store
                    .apply_full(*surface)
                    .expect("consistent full surface");
            }
            GatewayEvent::Patch(patch) => {
                if let ApplyOutcome::Rejected(reason) = self.store.apply_patch(*patch) {
                    // Recovery without input replay: ask for a full surface with the same geometry.
                    eprintln!("patch rejected ({reason:?}); requesting full surface");
                    self.gateway.resize(self.geometry).unwrap();
                }
            }
            GatewayEvent::QueueOverflow { .. } => {
                self.gateway.resize(self.geometry).unwrap();
            }
            GatewayEvent::Disconnected(error) => panic!("disconnected: {error}"),
            GatewayEvent::Shutdown(reason) => panic!("server shutdown: {reason:?}"),
            _ => {}
        }
    }

    /// Pumps until no frame arrived for `quiet` (prompt fully redrawn, no pending output).
    fn pump_until_quiet(&mut self, quiet: Duration) {
        let deadline = Instant::now() + STEP_TIMEOUT;
        let mut last_change = Instant::now();
        let mut last_revision = self.store.revision();
        loop {
            if last_change.elapsed() >= quiet {
                return;
            }
            assert!(Instant::now() < deadline, "surface never became quiet");
            match self.events.recv_timeout(quiet) {
                Ok(event) => {
                    self.apply(event);
                    if self.store.revision() != last_revision {
                        last_revision = self.store.revision();
                        last_change = Instant::now();
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("gateway reader ended while waiting for a quiet surface")
                }
            }
        }
    }

    fn screen_contains(&self, needle: &str) -> bool {
        self.store
            .text_rows()
            .iter()
            .any(|row| row.contains(needle))
    }

    fn screen_count(&self, needle: &str) -> usize {
        self.store
            .text_rows()
            .iter()
            .map(|row| row.matches(needle).count())
            .sum()
    }

    fn type_line(&mut self, text: &str) {
        assert!(self.store.input_allowed(), "input requires a live surface");
        self.gateway
            .send_input(
                &self.target,
                vec![
                    ClientPaneInputEvent::TextCommit(text.to_owned()),
                    ClientPaneInputEvent::key_press(ClientKeyCode::Enter, 0),
                ],
            )
            .unwrap();
    }

    fn shell_pid(&self) -> u32 {
        self.gateway
            .api()
            .pane_shell_pid(&self.target.pane_id)
            .expect("pane.process_info over the JSON API")
            .expect("shell pid reported")
    }
}

fn percentile(sorted_ms: &[f64], p: f64) -> f64 {
    if sorted_ms.is_empty() {
        return 0.0;
    }
    let rank = ((p / 100.0) * (sorted_ms.len() as f64 - 1.0)).round() as usize;
    sorted_ms[rank.min(sorted_ms.len() - 1)]
}

/// `(pid, comm)` of every live descendant of `root`, read from /proc. A child started with
/// `setsid` keeps `root` as its parent, so an engine spawned by the GUI would show up here.
fn process_tree(root: u32) -> Vec<(u32, String)> {
    let mut procs: Vec<(u32, u32, String)> = Vec::new();
    for entry in std::fs::read_dir("/proc").unwrap().flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        // "<pid> (<comm>) <state> <ppid> ..." — comm may contain spaces; split at the last ')'.
        let (Some(open), Some(close)) = (stat.find('('), stat.rfind(')')) else {
            continue;
        };
        let comm = stat[open + 1..close].to_owned();
        let ppid = stat[close + 1..]
            .split_whitespace()
            .nth(1)
            .and_then(|p| p.parse().ok())
            .unwrap_or(0);
        procs.push((pid, ppid, comm));
    }
    let mut tree = Vec::new();
    let mut frontier = vec![root];
    while let Some(parent) = frontier.pop() {
        for (pid, ppid, comm) in &procs {
            if *ppid == parent {
                tree.push((*pid, comm.clone()));
                frontier.push(*pid);
            }
        }
    }
    tree
}

/// A spawned window, terminated on drop (SIGTERM = user closing the app) so a failing step
/// never leaves a GUI process behind.
struct Window(Option<std::process::Child>);

impl Window {
    fn spawn(command: &mut Command, what: &str) -> Self {
        Self(Some(
            command
                .spawn()
                .unwrap_or_else(|e| panic!("launch {what}: {e}")),
        ))
    }

    fn child(&mut self) -> &mut std::process::Child {
        self.0.as_mut().expect("window still owned")
    }

    fn id(&self) -> u32 {
        self.0.as_ref().expect("window still owned").id()
    }

    /// Closes the window and collects its exit status and stderr.
    fn close(mut self) -> std::process::Output {
        let child = self.0.take().expect("window still owned");
        let _ = Command::new("kill")
            .arg("-TERM")
            .arg(child.id().to_string())
            .status();
        child.wait_with_output().unwrap()
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            if child.try_wait().ok().flatten().is_none() {
                let _ = Command::new("kill")
                    .arg("-TERM")
                    .arg(child.id().to_string())
                    .status();
            }
            let _ = child.wait();
        }
    }
}

/// The exact session name this test owns for the "no server" step. Nothing may create it;
/// if a regression (or a mutant) starts an engine for it, only this name is stopped and
/// deleted on the way out — also when the step panics — so no other session is touched.
struct OwnedSessionName {
    name: String,
    herdr_bin: String,
}

impl OwnedSessionName {
    fn engine(&self, args: &[&str]) -> std::io::Result<std::process::Output> {
        Command::new(&self.herdr_bin)
            .env_remove("HERDR_SOCKET_PATH")
            .env_remove("HERDR_CLIENT_SOCKET_PATH")
            .env_remove("HERDR_SESSION")
            .args(args)
            .output()
    }

    fn listed(&self) -> bool {
        self.engine(&["session", "list"])
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .any(|l| l.split_whitespace().next() == Some(self.name.as_str()))
            })
            .unwrap_or(false)
    }
}

impl Drop for OwnedSessionName {
    fn drop(&mut self) {
        if !self.listed() {
            eprintln!(
                "owned session {}: absent after the step (nothing to clean)",
                self.name
            );
            return;
        }
        eprintln!(
            "owned session {}: present after the step; stopping and deleting exactly this name",
            self.name
        );
        let _ = self.engine(&["session", "stop", &self.name]);
        let _ = self.engine(&["session", "delete", &self.name]);
    }
}

/// The window process must still be running `at_least` after `since` (AC-001-03: a
/// connection failure is not a fatal bootstrap failure).
fn assert_alive_for(app: &mut Window, since: Instant, at_least: Duration, what: &str) {
    while since.elapsed() < at_least + Duration::from_millis(200) {
        if let Some(status) = app.child().try_wait().unwrap() {
            panic!(
                "process exited ({status}) {:?} after {what}; it must stay alive at least {at_least:?}",
                since.elapsed()
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The GUI never bootstraps an engine on its own: no `herdr` process under the window, no
/// session socket, nobody listening, and the session not running for the engine CLI.
fn assert_no_engine_bootstrap(
    app_pid: u32,
    paths: &SessionPaths,
    herdr_bin: &str,
    session: &str,
    when: &str,
) -> Vec<(u32, String)> {
    let tree = process_tree(app_pid);
    let engines: Vec<&(u32, String)> = tree.iter().filter(|(_, comm)| comm == "herdr").collect();
    assert!(
        engines.is_empty(),
        "GUI started an engine {when}: {engines:?}"
    );
    assert!(
        !paths.client_socket.exists() && !paths.api_socket.exists(),
        "session socket present {when}: {:?}",
        paths.data_dir
    );
    assert!(
        !herdr_client::bootstrap::session_available(paths),
        "something accepts connections for the session {when}"
    );
    let list = Command::new(herdr_bin)
        .env_remove("HERDR_SOCKET_PATH")
        .env_remove("HERDR_CLIENT_SOCKET_PATH")
        .env_remove("HERDR_SESSION")
        .args(["session", "list"])
        .output()
        .unwrap();
    let running = String::from_utf8_lossy(&list.stdout).lines().any(|line| {
        let mut fields = line.split_whitespace();
        fields.next() == Some(session) && fields.next() == Some("running")
    });
    assert!(!running, "session {session} is running {when}");
    tree
}

#[test]
#[ignore = "needs the disposable session created by scripts/e2e-session.sh; run by just check-spec 001"]
fn e2e_linux_terminal_flow() {
    let session_name = required("HERDR_DESKTOP_E2E_SESSION");
    let pane_id = required("HERDR_DESKTOP_E2E_PANE");
    let app_bin = PathBuf::from(required("HERDR_DESKTOP_E2E_APP_BIN"));
    let report_dir = PathBuf::from(required("HERDR_DESKTOP_E2E_REPORT"));
    std::fs::create_dir_all(&report_dir).unwrap();
    assert!(
        session_name.starts_with("hd001-"),
        "never run against a non-disposable session"
    );
    let session = SessionName::parse(&session_name).unwrap();
    let config_dir = herdr_client::session::herdr_config_dir(&|k| std::env::var(k).ok());

    let mut log = std::fs::File::create(report_dir.join("e2e-linux.log")).unwrap();
    let mut note = |line: String| {
        eprintln!("{line}");
        writeln!(log, "{line}").unwrap();
    };

    // --- 1. negotiate generation 1 for an 80x24 surface ---------------------------------
    let geometry = SurfaceGeometry {
        cols: 80,
        rows: 24,
        cell_width_px: 9,
        cell_height_px: 18,
    };
    let mut h = Harness::connect(&config_dir, &session, &pane_id, geometry);
    let first = h.store.surface().unwrap();
    assert_eq!(
        (first.frame.width, first.frame.height),
        (80, 24),
        "surface follows the negotiated 80x24"
    );
    let pane = first
        .panes
        .iter()
        .find(|p| p.pane_id == pane_id)
        .expect("target pane on the surface");
    let inner = pane.inner_rect;
    note(format!(
        "negotiated generation=1 boot_id={} surface=80x24 pane={} inner_rect={}x{}@{},{}",
        h.target.boot_id, pane_id, inner.width, inner.height, inner.x, inner.y
    ));
    let pid_before = h.shell_pid();
    note(format!("shell pid before: {pid_before}"));

    // --- 2. printf GUI_OK shows on the surface -------------------------------------------
    assert!(
        !h.screen_contains("GUI_OK"),
        "fixture premise: marker absent before typing"
    );
    h.type_line(r#"printf "GUI_%s\n" OK"#);
    h.pump_until(|h| h.screen_contains("GUI_OK"), "GUI_OK on the surface");
    note(format!(
        "GUI_OK visible at revision {}",
        h.store.revision().unwrap()
    ));

    // --- 3. multi-line paste arrives exactly once ---------------------------------------
    let marker_a = "PASTE_$((40+2))_A";
    let marker_b = "PASTE_$((40+2))_B";
    let paste = format!("echo {marker_a}\necho {marker_b}\n");
    assert_eq!(h.screen_count("PASTE_42_"), 0);
    h.gateway
        .send_input(
            &h.target,
            vec![
                ClientPaneInputEvent::Paste(paste),
                ClientPaneInputEvent::key_press(ClientKeyCode::Enter, 0),
            ],
        )
        .unwrap();
    h.pump_until(
        |h| h.screen_contains("PASTE_42_B"),
        "second pasted line executed",
    );
    // Let any duplicate delivery surface before counting.
    let settle = Instant::now();
    h.pump_until(
        |_| settle.elapsed() > Duration::from_millis(800),
        "paste settle",
    );
    assert_eq!(
        h.screen_count("PASTE_42_A"),
        1,
        "rows={:?}",
        h.store.text_rows()
    );
    assert_eq!(
        h.screen_count("PASTE_42_B"),
        1,
        "rows={:?}",
        h.store.text_rows()
    );
    note("multi-line paste executed once per line".into());

    // --- 4. resize produces the negotiated geometry --------------------------------------
    let resized = SurfaceGeometry {
        cols: 100,
        rows: 30,
        ..geometry
    };
    h.geometry = resized;
    h.gateway.resize(resized).unwrap();
    h.pump_until(
        |h| {
            h.store
                .surface()
                .is_some_and(|s| s.frame.width == 100 && s.frame.height == 30)
        },
        "full surface at 100x30",
    );
    let pane = h
        .store
        .surface()
        .unwrap()
        .panes
        .iter()
        .find(|p| p.pane_id == pane_id)
        .unwrap()
        .clone();
    assert!(pane.inner_rect.width > inner.width && pane.inner_rect.height > inner.height);
    h.type_line("stty size");
    let expected_stty = format!("{} {}", pane.inner_rect.height, pane.inner_rect.width);
    h.pump_until(
        |h| h.screen_contains(&expected_stty),
        &format!("stty size == {expected_stty}"),
    );
    note(format!(
        "resize: surface=100x30 pane inner={}x{} stty='{}'",
        pane.inner_rect.width, pane.inner_rect.height, expected_stty
    ));

    // --- 5. input→frame comprometido latency sample (AC-001-02 report) -------------------
    // Measured in the harness: TextCommit on the socket → first frame/patch whose committed
    // grid echoes it. No WebView/canvas paint is included; this is not input→paint.
    let mut samples_ms = Vec::new();
    // Each sample types on an idle prompt: wait for the surface to settle first so the
    // marker is not echoed as typeahead before the prompt redraw.
    h.pump_until_quiet(Duration::from_millis(400));
    for i in 0..40u32 {
        let marker = format!("L{i:02}");
        let before = h.store.revision().unwrap();
        let started = Instant::now();
        h.gateway
            .send_input(
                &h.target,
                vec![ClientPaneInputEvent::TextCommit(marker.clone())],
            )
            .unwrap();
        h.pump_until(
            |h| h.store.revision().unwrap() > before && h.screen_contains(&marker),
            "echo of typed marker",
        );
        samples_ms.push(started.elapsed().as_secs_f64() * 1000.0);
        h.gateway
            .send_input(
                &h.target,
                vec![ClientPaneInputEvent::key_press(ClientKeyCode::Backspace, 0); 3],
            )
            .unwrap();
        h.pump_until(|h| !h.screen_contains(&marker), "marker erased");
        h.pump_until_quiet(Duration::from_millis(150));
    }
    samples_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let latency = serde_json::json!({
        "label": "input→frame comprometido",
        "method": "TextCommit(3 chars) sent on the endpoint socket → first PaneSurfacePatch/PaneSurface whose committed grid (harness FrameStore) contains the echoed text; measured in-process with Instant, 40 samples, bash prompt, disposable session, debug build of the gateway; excludes WebView/canvas paint (not input→paint)",
        "samples": samples_ms.len(),
        "p50_ms": percentile(&samples_ms, 50.0),
        "p95_ms": percentile(&samples_ms, 95.0),
        "p99_ms": percentile(&samples_ms, 99.0),
        "max_ms": samples_ms.last().copied().unwrap_or(0.0),
        "frame_stats": h.store.stats(),
    });
    std::fs::write(
        report_dir.join("latency-gateway.json"),
        serde_json::to_string_pretty(&latency).unwrap(),
    )
    .unwrap();
    note(format!(
        "latency input→frame comprometido p50={:.1}ms p95={:.1}ms p99={:.1}ms",
        latency["p50_ms"], latency["p95_ms"], latency["p99_ms"]
    ));

    // --- 6. detach / reconnect preserves the shell pid -----------------------------------
    h.gateway.detach();
    assert!(!h.gateway.is_connected());
    std::thread::sleep(Duration::from_millis(300));
    let mut h = Harness::connect(&config_dir, &session, &pane_id, geometry);
    let pid_after = h.shell_pid();
    assert_eq!(
        pid_before, pid_after,
        "shell pid must survive detach/reattach"
    );
    assert!(
        h.screen_contains("GUI_OK") || h.screen_contains("stty"),
        "scrollback context preserved: {:?}",
        h.store.text_rows()
    );
    note(format!(
        "gateway detach/reattach: shell pid {pid_after} preserved"
    ));

    // --- 7. real window: open, external printf shows in the app's surface, close, reopen ---
    let trace = report_dir.join("window-surface-trace.json");
    let _ = std::fs::remove_file(&trace);
    let spawn_app = |trace: &PathBuf| {
        Window::spawn(
            Command::new(&app_bin)
                .env("HERDR_DESKTOP_SESSION", session.as_str())
                // Spec 067 (AC-067-04): the window opens in Portuguese, so the selectors of
                // this harness do not depend on the host's language.
                .env("HERDR_DESKTOP_LOCALE", "pt")
                .env("HERDR_DESKTOP_SURFACE_TRACE", trace)
                .env_remove("HERDR_SOCKET_PATH")
                .env_remove("HERDR_CLIENT_SOCKET_PATH")
                .env_remove("HERDR_SESSION")
                .env_remove("HERDR_PANE_ID")
                .stdout(Stdio::null())
                .stderr(Stdio::piped()),
            "herdr-desktop window",
        )
    };
    let read_trace = |trace: &PathBuf| -> Option<serde_json::Value> {
        std::fs::read_to_string(trace)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
    };
    let wait_trace = |trace: &PathBuf,
                      pred: &dyn Fn(&serde_json::Value) -> bool,
                      what: &str|
     -> serde_json::Value {
        let deadline = Instant::now() + Duration::from_secs(40);
        loop {
            if let Some(value) = read_trace(trace) {
                if pred(&value) {
                    return value;
                }
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}: {:?}",
                read_trace(trace)
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    };
    // Same wait, but an exiting window fails immediately (a dead app never reaches the state).
    let wait_trace_alive = |app: &mut Window,
                            trace: &PathBuf,
                            pred: &dyn Fn(&serde_json::Value) -> bool,
                            what: &str|
     -> serde_json::Value {
        let deadline = Instant::now() + Duration::from_secs(40);
        loop {
            if let Some(value) = read_trace(trace) {
                if pred(&value) {
                    return value;
                }
            }
            if let Some(status) = app.child().try_wait().unwrap() {
                panic!(
                    "window exited ({status}) while waiting for {what}: {:?}",
                    read_trace(trace)
                );
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}: {:?}",
                read_trace(trace)
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    };
    let herdr_bin = std::env::var("HERDR_DESKTOP_HERDR_BIN").unwrap_or_else(|_| "herdr".into());

    // Drop our own gateway so the window is the only client painting this pane.
    h.gateway.detach();
    let app = spawn_app(&trace);
    let connected = wait_trace(
        &trace,
        &|v| v["state"] == "live" && v["revision"].as_u64().unwrap_or(0) > 0,
        "window connected",
    );
    note(format!(
        "window connected: generation={} boot={} surface={}x{}",
        connected["generation"], connected["boot_id"], connected["width"], connected["height"]
    ));
    let external = Command::new(&herdr_bin)
        .env_remove("HERDR_SOCKET_PATH")
        .env_remove("HERDR_CLIENT_SOCKET_PATH")
        .env_remove("HERDR_PANE_ID")
        .env("HERDR_SESSION", session.as_str())
        .args(["pane", "run", &pane_id, r#"printf "GUI_%s_WINDOW\n" OK"#])
        .output()
        .unwrap();
    assert!(
        external.status.success(),
        "{}",
        String::from_utf8_lossy(&external.stderr)
    );
    let shown = wait_trace(
        &trace,
        &|v| {
            v["rows"].as_array().is_some_and(|rows| {
                rows.iter()
                    .any(|r| r.as_str().unwrap_or("").contains("GUI_OK_WINDOW"))
            })
        },
        "GUI_OK_WINDOW painted by the app",
    );
    note(format!(
        "window shows GUI_OK_WINDOW at revision {}",
        shown["revision"]
    ));
    let pid_window_before: u64 = shown["shell_pid"]
        .as_u64()
        .expect("app reports shell pid via API");
    assert_eq!(pid_window_before, u64::from(pid_before));

    // Close the window (SIGTERM = user closing the app); engine and shell must survive.
    let exit = app.close().status;
    note(format!("window closed with status {exit}"));
    std::thread::sleep(Duration::from_millis(500));
    let _ = std::fs::remove_file(&trace);
    let mut app = spawn_app(&trace);
    let reopened = wait_trace(
        &trace,
        &|v| v["state"] == "live" && v["shell_pid"].as_u64().is_some(),
        "window reopened",
    );
    assert_eq!(
        reopened["shell_pid"].as_u64().unwrap(),
        u64::from(pid_before),
        "shell pid preserved across close/reopen"
    );
    assert!(
        reopened["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r.as_str().unwrap_or("").contains("GUI_OK_WINDOW")),
        "previous output still on screen after reopen"
    );
    note(format!(
        "window reopened: shell pid {} preserved, GUI_OK_WINDOW still visible",
        reopened["shell_pid"]
    ));
    let no_path =
        |error: &serde_json::Value| !error["message"].as_str().unwrap_or("/").contains('/');

    // --- 8. connection lost after attach (AC-001-03): recoverable, alive, no restart -----
    // Only the disposable engine this run created is stopped. The attached window must
    // survive, expose `disconnected` with a retryable error, block input and never restart
    // the engine on its own.
    let paths = SessionPaths::for_session(&config_dir, &session);
    assert!(herdr_client::bootstrap::session_available(&paths));
    let stopped = Command::new(&herdr_bin)
        .env_remove("HERDR_SOCKET_PATH")
        .env_remove("HERDR_CLIENT_SOCKET_PATH")
        .env_remove("HERDR_SESSION")
        .env_remove("HERDR_PANE_ID")
        .args(["session", "stop", session.as_str()])
        .output()
        .unwrap();
    assert!(
        stopped.status.success(),
        "{}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    let stopped_at = Instant::now();
    let lost = wait_trace_alive(
        &mut app,
        &trace,
        &|v| v["state"] == "disconnected",
        "window reporting the lost connection",
    );
    let lost_observed_at = Instant::now();
    assert!(
        matches!(
            lost["last_error"]["code"].as_str(),
            Some("connection_lost") | Some("server_shutdown")
        ),
        "{lost}"
    );
    assert_eq!(lost["last_error"]["retryable"], true, "{lost}");
    assert_eq!(lost["input_enabled"], false, "{lost}");
    assert_eq!(lost["connected"], false, "{lost}");
    assert!(no_path(&lost["last_error"]), "{lost}");
    assert_alive_for(
        &mut app,
        lost_observed_at,
        Duration::from_secs(2),
        "observing the lost connection",
    );
    let tree = assert_no_engine_bootstrap(
        app.id(),
        &paths,
        &herdr_bin,
        session.as_str(),
        "after the connection loss",
    );
    note(format!(
        "connection lost: state={} reason={} error={} retryable={} input_enabled={} observed {} ms after session stop; alive {:.1}s later; children={:?}; no engine/socket restarted",
        lost["state"],
        lost["reason"],
        lost["last_error"]["code"],
        lost["last_error"]["retryable"],
        lost["input_enabled"],
        (lost_observed_at - stopped_at).as_millis(),
        lost_observed_at.elapsed().as_secs_f64(),
        tree
    ));
    let closed = app.close();
    let stderr = String::from_utf8_lossy(&closed.stderr);
    assert!(
        !stderr.contains(FATAL_LINE) && !stderr.contains("panicked"),
        "connection loss must not be reported as fatal: {stderr}"
    );
    note(format!(
        "window closed after the loss with status {} (no fatal line on stderr)",
        closed.status
    ));

    // --- 9. GUI opened without a server (AC-001-03): recoverable, alive, no bootstrap ----
    let absent = SessionName::parse(&format!("hd001-absent-{}", std::process::id())).unwrap();
    let absent_paths = SessionPaths::for_session(&config_dir, &absent);
    let owned = OwnedSessionName {
        name: absent.as_str().to_owned(),
        herdr_bin: herdr_bin.clone(),
    };
    note(format!(
        "absent session name: {absent} (owned by this test; must not exist before or after)"
    ));
    assert!(
        !owned.listed() && !absent_paths.data_dir.exists(),
        "fixture premise: no session or directory for {absent}"
    );
    let trace_absent = report_dir.join("window-absent-trace.json");
    let _ = std::fs::remove_file(&trace_absent);
    let mut app = Window::spawn(
        Command::new(&app_bin)
            .env("HERDR_DESKTOP_SESSION", absent.as_str())
            .env("HERDR_DESKTOP_LOCALE", "pt")
            .env("HERDR_DESKTOP_SURFACE_TRACE", &trace_absent)
            .env("HERDR_DESKTOP_HERDR_BIN", &herdr_bin)
            .env_remove("HERDR_SOCKET_PATH")
            .env_remove("HERDR_CLIENT_SOCKET_PATH")
            .env_remove("HERDR_SESSION")
            .env_remove("HERDR_PANE_ID")
            .stdout(Stdio::null())
            .stderr(Stdio::piped()),
        "herdr-desktop window without a server",
    );
    let observed = wait_trace_alive(
        &mut app,
        &trace_absent,
        &|v| v["state"] == "disconnected",
        "window reporting the absent server",
    );
    let observed_at = Instant::now();
    assert_eq!(
        observed["last_error"]["code"], "server_unavailable",
        "{observed}"
    );
    assert_eq!(observed["last_error"]["retryable"], true, "{observed}");
    assert_eq!(observed["reason"], "server_unavailable", "{observed}");
    assert_eq!(observed["input_enabled"], false, "{observed}");
    assert_eq!(observed["connected"], false, "{observed}");
    assert!(
        observed["shell_pid"].is_null() && observed["boot_id"].is_null(),
        "{observed}"
    );
    assert!(no_path(&observed["last_error"]), "{observed}");
    assert_alive_for(
        &mut app,
        observed_at,
        Duration::from_secs(2),
        "observing the absent server",
    );
    let tree = assert_no_engine_bootstrap(
        app.id(),
        &absent_paths,
        &herdr_bin,
        absent.as_str(),
        "without a server",
    );
    assert!(
        !absent_paths.data_dir.exists(),
        "session directory created by the GUI without a server"
    );
    note(format!(
        "absent server ({}): state={} error={} retryable={} input_enabled={}; alive {:.1}s after observing; children={:?}; no engine, session dir or socket created",
        absent,
        observed["state"],
        observed["last_error"]["code"],
        observed["last_error"]["retryable"],
        observed["input_enabled"],
        observed_at.elapsed().as_secs_f64(),
        tree
    ));
    let closed = app.close();
    let stderr = String::from_utf8_lossy(&closed.stderr);
    assert!(
        !stderr.contains(FATAL_LINE) && !stderr.contains("panicked"),
        "absent server must not be reported as fatal: {stderr}"
    );
    note(format!(
        "window without a server closed with status {} (no fatal line on stderr)",
        closed.status
    ));

    std::fs::write(
        report_dir.join("e2e-linux-summary.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "session": session.as_str(),
            "pane_id": pane_id,
            "boot_id": h.target.boot_id,
            "shell_pid": pid_before,
            "surface_initial": "80x24",
            "surface_resized": "100x30",
            "pane_inner_after_resize": format!("{}x{}", pane.inner_rect.width, pane.inner_rect.height),
            "latency": latency,
            "connection_loss": {
                "state": lost["state"],
                "error": lost["last_error"],
                "input_enabled": lost["input_enabled"],
                "alive_after_observation_s": 2,
                "close_status": closed.status.to_string(),
            },
            "absent_server": {
                "session": absent.as_str(),
                "state": observed["state"],
                "error": observed["last_error"],
                "input_enabled": observed["input_enabled"],
                "alive_after_observation_s": 2,
            },
        }))
        .unwrap(),
    )
    .unwrap();
}
