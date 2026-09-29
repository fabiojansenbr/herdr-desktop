//! Spec 007 — responsiveness of the connections IPC commands. Seam: the entry points the real
//! `#[tauri::command]` wrappers await (`ConnectionsState::<command>`), polled by a
//! single-threaded executor written here that plays the IPC calling (GUI) thread. No GUI, no
//! engine, no default session: hosts are fakes on the one hub of `ConnectionsState`, and the
//! profile store lives in temporary directories.
//!
//! What must hold, shown with barriers that the wrappers really cross (a host write held in
//! `RuntimeGateway::send_input`, the TUI catalog read held on a FIFO, the hub revision wait):
//! - a command whose socket write, file read or lock waits returns `Pending` to the calling
//!   thread; another host's input and gateway events keep progressing meanwhile;
//! - commands share no lane: a long `connections_watch` never delays `connection_cancel`, and a
//!   held host write never delays another host's `connection_send_text`;
//! - results and errors are exactly the synchronous methods' ones, delivered once; input is
//!   written once and never replayed after the connection is gone.
//!
//! Ordering of conflicting calls (connect/cancel, profile save/import, text per host) belongs to
//! the frontend bridge, which sends them one at a time (`src/connections/bridge.test.ts`).
//!
//! Fixture values differ on purpose: hosts `ssh-hd007c-a` / `ssh-hd007c-b` / `ssh-hd007c-c`,
//! boots `boot-hd007c-a4` / `boot-hd007c-b9`, texts `alpha-hd007c` / `beta-hd007c`, TUI profile
//! `tui-hd007c` (`importado-hd007c`).

use std::future::Future;
use std::io::Write;
use std::pin::Pin;
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};

use herdr_client::protocol::wire::{
    CellData, ClientKeyCode, ClientPaneInputEvent, ClientShellSnapshot, CursorState, FrameData,
    PaneSurfaceFrame, PaneSurfacePane, SurfaceRect,
};
use herdr_client::{
    ConnectOptions, GatewayEvent, LiveIdentity, Negotiated, QualifiedTarget, RuntimeError,
    RuntimeGateway, SurfaceGeometry,
};
use herdr_desktop::connections::commands::{ConnectionsConfig, ConnectionsState};
use herdr_desktop::connections::hub::{ApiLane, Connected, HostKind, HostSpec};
use herdr_desktop::connections::profiles::{tui_catalog_path, SshProfileDraft};
use serde::Serialize;
use serde_json::Value;

/// Upper bound for anything that must eventually happen (a deadline, not a performance claim).
const WITHIN: Duration = Duration::from_secs(10);
/// How long a held call waits for its release before giving up. An implementation that runs a
/// command inline on the calling thread returns after this instead of hanging the test, so the
/// `Pending` assertions fail instead of deadlocking.
const HOLD_LIMIT: Duration = Duration::from_secs(3);

const PANE: &str = "w1:p1";
const HOST_A: &str = "ssh-hd007c-a";
const HOST_B: &str = "ssh-hd007c-b";
const HOST_C: &str = "ssh-hd007c-c";
const BOOT_A: &str = "boot-hd007c-a4";
const BOOT_B: &str = "boot-hd007c-b9";

// =======================================================================================
// Single-threaded executor (the IPC calling thread)
// =======================================================================================

#[derive(Default)]
struct Signal {
    wakes: Mutex<u64>,
    cv: Condvar,
}

impl Wake for Signal {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        *self.wakes.lock().unwrap() += 1;
        self.cv.notify_all();
    }
}

struct Executor {
    signal: Arc<Signal>,
    waker: Waker,
}

type Task<T> = Pin<Box<dyn Future<Output = T> + Send>>;

fn task<T>(future: impl Future<Output = T> + Send + 'static) -> Task<T> {
    Box::pin(future)
}

impl Executor {
    fn new() -> Self {
        let signal = Arc::new(Signal::default());
        Self {
            waker: Waker::from(signal.clone()),
            signal,
        }
    }

    fn poll<T>(&self, task: &mut Task<T>) -> Poll<T> {
        task.as_mut().poll(&mut Context::from_waker(&self.waker))
    }

    fn assert_pending<T>(&self, task: &mut Task<T>, what: &str) {
        assert!(
            self.poll(task).is_pending(),
            "{what}: the command must hand the calling thread back while its work waits"
        );
    }

    /// Polls until ready, sleeping only on wakes.
    fn wait<T>(&self, task: &mut Task<T>, what: &str) -> T {
        let deadline = Instant::now() + WITHIN;
        loop {
            let seen = *self.signal.wakes.lock().unwrap();
            if let Poll::Ready(value) = self.poll(task) {
                return value;
            }
            let mut wakes = self.signal.wakes.lock().unwrap();
            while *wakes == seen {
                let left = deadline
                    .checked_duration_since(Instant::now())
                    .unwrap_or_else(|| panic!("{what}: never completed"));
                wakes = self.signal.cv.wait_timeout(wakes, left).unwrap().0;
            }
        }
    }
}

// =======================================================================================
// Barriers
// =======================================================================================

/// A host write that blocks while armed until the test releases it.
#[derive(Default)]
struct Hold {
    state: Mutex<HoldState>,
    cv: Condvar,
}

#[derive(Default)]
struct HoldState {
    armed: bool,
    entered: usize,
    released: bool,
}

impl Hold {
    fn arm(&self) {
        let mut state = self.state.lock().unwrap();
        state.armed = true;
        state.released = false;
    }

    fn pass(&self) -> Result<(), RuntimeError> {
        let mut state = self.state.lock().unwrap();
        if !state.armed {
            return Ok(());
        }
        state.armed = false;
        state.entered += 1;
        self.cv.notify_all();
        let deadline = Instant::now() + HOLD_LIMIT;
        while !state.released {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return Err(RuntimeError::new("hold_timeout", "hold never released"));
            };
            state = self.cv.wait_timeout(state, left).unwrap().0;
        }
        Ok(())
    }

    fn wait_entered(&self) {
        let deadline = Instant::now() + WITHIN;
        let mut state = self.state.lock().unwrap();
        while state.entered == 0 {
            let left = deadline
                .checked_duration_since(Instant::now())
                .expect("the held host write was never reached");
            state = self.cv.wait_timeout(state, left).unwrap().0;
        }
    }

    fn release(&self) {
        self.state.lock().unwrap().released = true;
        self.cv.notify_all();
    }
}

/// The TUI catalog as a FIFO: `import` blocks inside its file read (holding the profile store)
/// until the test writes it. A writer thread opens the FIFO, which returns only once the reader
/// opened it, and reports that; then it writes the test's body. If the test gives no body within
/// [`HOLD_LIMIT`] (a command run inline on the calling thread), it writes an empty catalog so the
/// test fails on its assertions instead of hanging.
struct CatalogFifo {
    shared: Arc<(Mutex<FifoState>, Condvar)>,
}

#[derive(Default)]
struct FifoState {
    reader_opened: bool,
    body: Option<String>,
}

impl CatalogFifo {
    fn create(state_dir: &std::path::Path) -> Self {
        let path = tui_catalog_path(state_dir);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let status = std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .expect("mkfifo available");
        assert!(status.success(), "mkfifo failed");
        let shared: Arc<(Mutex<FifoState>, Condvar)> = Arc::default();
        let writer = shared.clone();
        std::thread::spawn(move || {
            let mut fifo = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            let (state, cv) = &*writer;
            let mut guard = state.lock().unwrap();
            guard.reader_opened = true;
            cv.notify_all();
            let (mut guard, _) = cv
                .wait_timeout_while(guard, HOLD_LIMIT, |s| s.body.is_none())
                .unwrap();
            let body = guard
                .body
                .get_or_insert_with(|| "{\"ssh\":[]}".to_owned())
                .clone();
            drop(guard);
            let _ = fifo.write_all(body.as_bytes());
        });
        Self { shared }
    }

    /// Returns once the import opened the catalog for reading (inside the store lock).
    fn wait_reader(&self) {
        let (state, cv) = &*self.shared;
        let (guard, _) = cv
            .wait_timeout_while(state.lock().unwrap(), WITHIN, |s| !s.reader_opened)
            .unwrap();
        assert!(guard.reader_opened, "the import never opened the catalog");
    }

    fn write(&self, body: &str) {
        let (state, cv) = &*self.shared;
        let mut guard = state.lock().unwrap();
        assert!(guard.body.is_none(), "the held read was already answered");
        guard.body = Some(body.to_owned());
        cv.notify_all();
    }
}

// =======================================================================================
// Fake hosts
// =======================================================================================

fn geometry() -> SurfaceGeometry {
    SurfaceGeometry {
        cols: 12,
        rows: 2,
        cell_width_px: 9,
        cell_height_px: 18,
    }
}

fn full(boot: &str, revision: u64, text: &str) -> PaneSurfaceFrame {
    let (width, height) = (12u16, 2u16);
    let rect = SurfaceRect {
        x: 0,
        y: 0,
        width,
        height,
    };
    PaneSurfaceFrame {
        boot_id: boot.into(),
        projection_revision: 1,
        surface_revision: revision,
        frame: FrameData {
            cells: text
                .chars()
                .chain(std::iter::repeat(' '))
                .take(usize::from(width * height))
                .map(|c| CellData {
                    symbol: c.to_string(),
                    fg: 0,
                    bg: 0,
                    modifier: 0,
                    skip: false,
                    hyperlink: None,
                })
                .collect(),
            width,
            height,
            cursor: Some(CursorState {
                x: 0,
                y: 0,
                visible: true,
                shape: 0,
            }),
            hyperlinks: vec![],
            graphics: vec![],
        },
        panes: vec![PaneSurfacePane {
            pane_id: PANE.into(),
            content_revision: 1,
            rect,
            inner_rect: rect,
            scrollbar_rect: None,
            scroll: None,
            focused: true,
            mouse_reporting: false,
            sgr_pixel_mouse: false,
            alternate_screen_active: false,
            pixel_width: 108,
            pixel_height: 36,
        }],
        splits: vec![],
        popup: None,
        graphics: Default::default(),
    }
}

fn snapshot(boot: &str) -> ClientShellSnapshot {
    let mut snap: ClientShellSnapshot = serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../tests/fixtures/endpoint-snapshot-v1.json"
        ))
        .unwrap(),
    )
    .unwrap();
    snap.boot_id = boot.into();
    assert_eq!(snap.panes[0].pane_id, PANE, "fixture premise");
    snap
}

type Inputs = Arc<Mutex<Vec<(String, Vec<ClientPaneInputEvent>)>>>;

struct FakeGateway {
    endpoint: String,
    session: String,
    boot: String,
    inputs: Inputs,
    hold: Arc<Hold>,
}

impl RuntimeGateway for FakeGateway {
    fn endpoint(&self) -> &str {
        &self.endpoint
    }
    fn identity(&self) -> Option<LiveIdentity> {
        Some(LiveIdentity {
            endpoint: self.endpoint.clone(),
            session: self.session.clone(),
            connection_generation: 1,
            boot_id: self.boot.clone(),
        })
    }
    fn connect(&mut self, _options: ConnectOptions) -> Result<Negotiated, RuntimeError> {
        Err(RuntimeError::new("unsupported_in_fake", "not used"))
    }
    fn take_events(&mut self) -> Option<std::sync::mpsc::Receiver<GatewayEvent>> {
        None
    }
    fn api_request(&self, _method: &str, _params: Value) -> Result<Value, RuntimeError> {
        Err(RuntimeError::new("unsupported_in_fake", "not used"))
    }
    fn endpoint_request(&self, _method: &str, _params: Value) -> Result<Value, RuntimeError> {
        Err(RuntimeError::new("unsupported_in_fake", "not used"))
    }
    /// The socket write of the host: may be held by the test; recorded only once it went out.
    fn send_input(
        &self,
        target: &QualifiedTarget,
        events: Vec<ClientPaneInputEvent>,
    ) -> Result<(), RuntimeError> {
        self.hold.pass()?;
        self.inputs
            .lock()
            .unwrap()
            .push((target.pane_id.clone(), events));
        Ok(())
    }
    fn resize(&self, _geometry: SurfaceGeometry) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn set_focus(&self, _focused: bool) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn detach(&mut self) {}
    fn is_connected(&self) -> bool {
        true
    }
}

struct FakeApi;

impl ApiLane for FakeApi {
    fn request(&self, method: &str, _params: Value) -> Result<Value, RuntimeError> {
        Err(RuntimeError::new("unsupported_in_fake", "not used").with_endpoint(method))
    }
}

struct Host {
    inputs: Inputs,
    hold: Arc<Hold>,
    token: u64,
    target: QualifiedTarget,
}

fn add_host(state: &ConnectionsState, endpoint: &str, session: &str) {
    state
        .hub()
        .add_host(HostSpec {
            endpoint: endpoint.into(),
            label: format!("rótulo {endpoint}"),
            kind: HostKind::Ssh,
            session: session.into(),
            target: Some(format!("tester@{endpoint}")),
            visible: true,
        })
        .unwrap();
}

/// Registers `endpoint` and connects it with a fake gateway, snapshot and full frame.
fn online_host(state: &ConnectionsState, endpoint: &str, session: &str, boot: &str) -> Host {
    add_host(state, endpoint, session);
    let hub = state.hub();
    let now = Instant::now();
    let inputs: Inputs = Arc::default();
    let hold = Arc::new(Hold::default());
    let ticket = hub.request_connect(endpoint, now).unwrap().expect("ticket");
    hub.finish_connect(
        &ticket,
        Ok(Connected {
            gateway: Box::new(FakeGateway {
                endpoint: endpoint.into(),
                session: session.into(),
                boot: boot.into(),
                inputs: inputs.clone(),
                hold: hold.clone(),
            }),
            api: Arc::new(FakeApi),
        }),
        now,
    );
    hub.apply_event(
        endpoint,
        ticket.token,
        GatewayEvent::Snapshot(Box::new(snapshot(boot))),
        now,
    );
    hub.apply_event(
        endpoint,
        ticket.token,
        GatewayEvent::Surface(Box::new(full(boot, 1, "pronto"))),
        now,
    );
    let target = hub.input_gate(endpoint, PANE).expect("input enabled");
    Host {
        inputs,
        hold,
        token: ticket.token,
        target,
    }
}

struct Fixture {
    state: ConnectionsState,
    state_dir: std::path::PathBuf,
    _dirs: Vec<tempfile::TempDir>,
}

/// No local host (nothing touches a real session); profile store in temporary directories.
fn fixture() -> Fixture {
    let prefs = tempfile::tempdir().unwrap();
    let herdr_config = tempfile::tempdir().unwrap();
    let herdr_state = tempfile::tempdir().unwrap();
    let state = ConnectionsState::new(ConnectionsConfig {
        prefs_dir: prefs.path().to_path_buf(),
        herdr_config_dir: herdr_config.path().to_path_buf(),
        herdr_state_dir: herdr_state.path().to_path_buf(),
        local_session: None,
        local_auto_start: false,
        herdr_bin: "herdr".into(),
        isolated_ssh: None,
        geometry: geometry(),
    });
    Fixture {
        state,
        state_dir: herdr_state.path().to_path_buf(),
        _dirs: vec![prefs, herdr_config, herdr_state],
    }
}

fn text(value: &str) -> ClientPaneInputEvent {
    ClientPaneInputEvent::TextCommit(value.into())
}

fn enter() -> ClientPaneInputEvent {
    ClientPaneInputEvent::key_press(ClientKeyCode::Enter, 0)
}

fn json_of<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap()
}

fn draft(label: &str, target: &str, session: &str) -> SshProfileDraft {
    SshProfileDraft {
        id: None,
        label: label.into(),
        target: target.into(),
        port: None,
        session: session.into(),
        auth: None,
    }
}

// =======================================================================================
// Contracts
// =======================================================================================

/// Would catch: `connection_send_text` writing on the calling thread (the GUI froze for as long
/// as the host socket), or every connections command sharing one lane (host B's text waited
/// for host A's stuck write). Also that the held text is written once and answered once.
#[test]
fn held_host_write_hands_the_thread_back_while_another_host_and_events_progress() {
    let fx = fixture();
    let exec = Executor::new();
    let a = online_host(&fx.state, HOST_A, "hd007c-sess-a", BOOT_A);
    let b = online_host(&fx.state, HOST_B, "hd007c-sess-b", BOOT_B);

    a.hold.arm();
    let mut send_a = task(fx.state.connection_send_text(
        a.target.clone(),
        "alpha-hd007c".into(),
        true,
    ));
    exec.assert_pending(&mut send_a, "text to a host whose write is held");
    a.hold.wait_entered();
    exec.assert_pending(&mut send_a, "text still held in the socket write");

    let mut send_b = task(fx.state.connection_send_text(
        b.target.clone(),
        "beta-hd007c".into(),
        false,
    ));
    let result_b = exec.wait(&mut send_b, "text to the other host");
    assert_eq!(result_b, Ok(()));
    assert_eq!(
        *b.inputs.lock().unwrap(),
        vec![(PANE.to_owned(), vec![text("beta-hd007c")])]
    );

    // Gateway events of the other host are applied while host A's write is held.
    fx.state.hub().apply_event(
        HOST_B,
        b.token,
        GatewayEvent::Surface(Box::new(full(BOOT_B, 2, "beta-ok"))),
        Instant::now(),
    );
    let (frame, _) = fx.state.hub().surface(HOST_B).expect("surface of B");
    assert_eq!(frame.surface_revision, 2);

    exec.assert_pending(&mut send_a, "host A still held after B progressed");
    assert!(a.inputs.lock().unwrap().is_empty(), "nothing written yet");

    a.hold.release();
    let result_a = exec.wait(&mut send_a, "released text");
    assert_eq!(result_a, Ok(()));
    assert_eq!(
        *a.inputs.lock().unwrap(),
        vec![(PANE.to_owned(), vec![text("alpha-hd007c"), enter()])],
        "written exactly once, text then Enter"
    );
}

/// Would catch: a lane shared by `connections_watch` and the mutations (cancel waited for the
/// watch timeout, and the watch answered the old revision instead of the cancel).
#[test]
fn cancel_never_waits_for_a_long_watch() {
    let fx = fixture();
    let exec = Executor::new();
    add_host(&fx.state, HOST_C, "hd007c-sess-c");
    let before = fx.state.hub().revision();

    let mut watch = task(fx.state.connections_watch(before));
    exec.assert_pending(&mut watch, "watch waiting for a newer revision");

    let mut cancel = task(fx.state.connection_cancel(HOST_C.into()));
    let cancelled = exec
        .wait(&mut cancel, "cancel during a watch")
        .expect("cancel view");
    assert!(
        cancelled.hub.revision > before,
        "cancel bumps the revision ({} > {before})",
        cancelled.hub.revision
    );
    let host = cancelled
        .hub
        .hosts
        .iter()
        .find(|h| h.endpoint == HOST_C)
        .expect("host C in the view");
    assert!(host.cancelled, "the view answered by cancel shows it");

    let watched = exec.wait(&mut watch, "watch").expect("watch view");
    assert_eq!(
        watched.hub.revision, cancelled.hub.revision,
        "the watch woke up on the cancel instead of timing out on the old revision"
    );
}

/// Would catch: list/save/connect/cancel/import touching the profile store or file system on the
/// calling thread, or a lane shared with input (another host's text waited for the import).
#[cfg(unix)]
#[test]
fn held_catalog_read_keeps_store_commands_off_the_thread_and_input_flowing() {
    let fx = fixture();
    let exec = Executor::new();
    let b = online_host(&fx.state, HOST_B, "hd007c-sess-b", BOOT_B);
    add_host(&fx.state, HOST_C, "hd007c-sess-c");
    let fifo = CatalogFifo::create(&fx.state_dir);

    let mut import = task(fx.state.connection_profiles_import());
    exec.assert_pending(&mut import, "import reading the held catalog");
    fifo.wait_reader();
    exec.assert_pending(&mut import, "import held inside the catalog read");

    // The import holds the profile store while it reads: every command that answers a view
    // waits for it, off the calling thread.
    let mut list = task(fx.state.connections_list());
    exec.assert_pending(&mut list, "list waiting for the store");
    let mut save = task(fx.state.connection_profile_save(
        draft("salvo-hd007c", "saver@10.0.0.7", "hd007c-salvo"),
        false,
    ));
    exec.assert_pending(&mut save, "save waiting for the store");
    let mut connect = task(fx.state.connection_connect(HOST_C.into()));
    exec.assert_pending(&mut connect, "connect waiting for the store");

    // Input and events of a connected host do not touch the store.
    let mut send_b = task(fx.state.connection_send_text(
        b.target.clone(),
        "beta-hd007c".into(),
        true,
    ));
    assert_eq!(exec.wait(&mut send_b, "text while importing"), Ok(()));
    assert_eq!(
        *b.inputs.lock().unwrap(),
        vec![(PANE.to_owned(), vec![text("beta-hd007c"), enter()])]
    );
    fx.state.hub().apply_event(
        HOST_B,
        b.token,
        GatewayEvent::Surface(Box::new(full(BOOT_B, 3, "beta-3"))),
        Instant::now(),
    );
    assert_eq!(
        fx.state.hub().surface(HOST_B).unwrap().0.surface_revision,
        3
    );
    exec.assert_pending(&mut import, "import still held");

    fifo.write(
        r#"{"ssh":[{"id":"tui-hd007c","label":"importado-hd007c","target":"tui@10.0.0.9","session":"hd007c-tui"}]}"#,
    );
    let imported = exec
        .wait(&mut import, "released import")
        .expect("import view");
    assert_eq!(
        imported.report.imported,
        vec!["importado-hd007c".to_owned()]
    );
    assert!(imported
        .view
        .profiles
        .iter()
        .any(|p| p.label == "importado-hd007c" && p.session.as_str() == "hd007c-tui"));

    let saved = exec.wait(&mut save, "save").expect("save view");
    assert!(saved.profiles.iter().any(|p| p.label == "salvo-hd007c"));
    let connected = exec.wait(&mut connect, "connect").expect("connect view");
    assert!(connected.hub.hosts.iter().any(|h| h.endpoint == HOST_C));
    let listed = exec.wait(&mut list, "list").expect("list view");
    assert!(listed
        .profiles
        .iter()
        .any(|p| p.label == "importado-hd007c"));

    let mut cancel = task(fx.state.connection_cancel(HOST_C.into()));
    let cancelled = exec.wait(&mut cancel, "cancel").expect("cancel view");
    assert!(cancelled
        .hub
        .hosts
        .iter()
        .any(|h| h.endpoint == HOST_C && h.cancelled));
}

/// Would catch: wrappers that rename/rewrap errors or change success payloads, and input that is
/// replayed or delivered to a connection that is gone.
#[test]
fn results_and_errors_are_the_synchronous_ones_and_input_is_never_replayed() {
    let fx = fixture();
    let exec = Executor::new();
    let a = online_host(&fx.state, HOST_A, "hd007c-sess-a", BOOT_A);

    // Success payloads.
    let mut list = task(fx.state.connections_list());
    let listed = exec.wait(&mut list, "list").expect("list");
    assert_eq!(json_of(&listed), json_of(&fx.state.view()));

    let mut save = task(fx.state.connection_profile_save(
        draft("perfil-hd007c", "perfil@10.0.0.5", "hd007c-perfil"),
        false,
    ));
    let saved = exec.wait(&mut save, "save").expect("save");
    assert_eq!(json_of(&saved), json_of(&fx.state.view()));
    assert!(saved.profiles.iter().any(|p| p.label == "perfil-hd007c"));

    let mut import = task(fx.state.connection_profiles_import());
    let imported = exec
        .wait(&mut import, "import without catalog")
        .expect("import");
    assert_eq!(
        json_of(&imported),
        json_of(&fx.state.import_profiles().unwrap()),
        "import without a catalog answers the same report and view"
    );

    // Errors: exactly the synchronous ones.
    let mut connect = task(fx.state.connection_connect("ssh-hd007c-nenhum".into()));
    assert_eq!(
        exec.wait(&mut connect, "connect unknown").err(),
        Some(fx.state.connect("ssh-hd007c-nenhum").unwrap_err())
    );
    let mut cancel = task(fx.state.connection_cancel("ssh-hd007c-nenhum".into()));
    let cancel_error = exec.wait(&mut cancel, "cancel unknown").unwrap_err();
    assert_eq!(cancel_error.code, "endpoint_unknown");
    assert_eq!(cancel_error.endpoint.as_deref(), Some("ssh-hd007c-nenhum"));

    let mut bad = draft("ruim", "nao é alvo; rm -rf", "hd007c-ruim");
    bad.port = Some(0);
    let mut save_bad = task(fx.state.connection_profile_save(bad.clone(), false));
    assert_eq!(
        exec.wait(&mut save_bad, "invalid draft").err(),
        Some(fx.state.save_profile(bad, false).unwrap_err())
    );

    let mut workspaces = task(fx.state.connection_workspaces("ssh-hd007c-nenhum".into()));
    assert_eq!(
        exec.wait(&mut workspaces, "workspaces unknown").err(),
        Some(fx.state.workspaces("ssh-hd007c-nenhum").unwrap_err())
    );

    let mut stale = a.target.clone();
    stale.boot_id = "boot-hd007c-antigo".into();
    let mut send_stale = task(fx.state.connection_send_text(
        stale.clone(),
        "nunca-hd007c".into(),
        true,
    ));
    let stale_error = exec.wait(&mut send_stale, "stale target").unwrap_err();
    assert_eq!(
        Err(stale_error.clone()),
        fx.state.send_text(&stale, "nunca-hd007c", true)
    );
    assert_ne!(stale_error.code, "", "a scoped error code");

    // Input goes out once, and a target of a cancelled connection sends nothing.
    let mut send = task(fx.state.connection_send_text(
        a.target.clone(),
        "alpha-hd007c".into(),
        false,
    ));
    assert_eq!(exec.wait(&mut send, "text"), Ok(()));
    let mut cancel_a = task(fx.state.connection_cancel(HOST_A.into()));
    exec.wait(&mut cancel_a, "cancel A").expect("cancel A");
    let mut after = task(fx.state.connection_send_text(
        a.target.clone(),
        "alpha-hd007c".into(),
        false,
    ));
    let after_error = exec.wait(&mut after, "text after cancel").unwrap_err();
    assert_eq!(after_error.code, "host_offline");
    assert_eq!(after_error.endpoint.as_deref(), Some(HOST_A));
    assert_eq!(
        *a.inputs.lock().unwrap(),
        vec![(PANE.to_owned(), vec![text("alpha-hd007c")])],
        "one write, no replay"
    );
}
