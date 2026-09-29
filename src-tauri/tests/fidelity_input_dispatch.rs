//! Spec 007 — ordered input of the composed surface off the GUI thread (seam: the future the real
//! `surface_input` command awaits, `ComposedSurface::dispatch_input`, polled by a single-threaded
//! executor written here; no GUI, no engine, no default session: hosts are fakes behind the one
//! hub of the window).
//!
//! What the seam must show, deterministically (holds/unpolled futures, never wall-time claims):
//! - a batch whose host write blocks leaves the calling thread free (`Pending`), and batches
//!   that merely wait for their turn hold no host lock: frames keep reaching the channel;
//! - batches reach the host in call order, even when a later one is polled first, and one
//!   refused/failed batch never blocks the next nor is retried;
//! - the pending work of one host is bounded (batches and bytes, in-flight included) and an
//!   excess is refused at the call, before any work exists;
//! - a batch waiting for its turn is refused, never sent, once the channel is detached or
//!   replaced, the selection changes or the host reconnects to another boot; the waiting of one
//!   host does not hold back the input of the host selected next.
//!
//! Fixture values differ on purpose: boots `boot-local-7` / `boot-ssh-3` / `boot-ssh-9`,
//! sessions `hd007i-local` / `hd007i-remote`, both hosts expose pane `w1:p1`, error code
//! `fake_write_failed_hd007i`.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};

use herdr_client::protocol::wire::{
    CellData, ClientPaneInputEvent, ClientShellSnapshot, CursorState, FrameData, PaneSurfaceFrame,
    PaneSurfacePane, PaneSurfacePatch, PaneSurfacePatchRow, SurfaceRect,
};
use herdr_client::{
    ConnectOptions, GatewayEvent, LiveIdentity, Negotiated, QualifiedTarget, RuntimeError,
    RuntimeGateway, SessionName, SurfaceGeometry,
};
use herdr_desktop::bridge::composition::{
    ComposedSurface, FrameSink, SurfaceConfig, SurfaceIdentityDto, MAX_PENDING_INPUT_BATCHES,
    MAX_PENDING_INPUT_BYTES,
};
use herdr_desktop::bridge::selection::SelectionState;
use herdr_desktop::bridge::ssh::{ProcessOutput, SshChild, SshRunner};
use herdr_desktop::connections::commands::{ConnectionsConfig, ConnectionsState};
use herdr_desktop::connections::hub::{ApiLane, ConnectTicket, Connected, HostHub};
use herdr_desktop::connections::profiles::SshProfileDraft;
use herdr_desktop::connections::ssh_options::OpenSshCommand;
use herdr_desktop::terminal::{FrameEvent, InputDto};
use serde_json::{json, Value};

const LOCAL: &str = "local";
const PANE: &str = "w1:p1";
/// Upper bound for anything that must eventually happen (a deadline, not a performance claim).
const WITHIN: Duration = Duration::from_secs(10);
/// A held write gives up after this: an implementation that sends inline on the calling thread
/// returns `hold_timeout` instead of hanging, so the `Pending` assertions fail.
const HOLD_LIMIT: Duration = Duration::from_secs(3);

// =======================================================================================
// Single-threaded executor
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

type Task<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Polls futures only on the test thread (the "GUI thread" of this seam).
struct Executor {
    signal: Arc<Signal>,
    waker: Waker,
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
            "{what}: the calling thread must be handed back while the batch waits"
        );
    }

    fn ready<T>(&self, task: &mut Task<T>, what: &str) -> T {
        match self.poll(task) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("{what}: expected an answer at the call"),
        }
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

fn task<T>(future: impl Future<Output = T> + Send + 'static) -> Task<T> {
    Box::pin(future)
}

// =======================================================================================
// Fakes
// =======================================================================================

/// Host writes block while armed until released (at most `HOLD_LIMIT`).
#[derive(Default)]
struct Hold {
    state: Mutex<(bool, usize)>,
    cv: Condvar,
}

impl Hold {
    fn arm(&self) {
        *self.state.lock().unwrap() = (true, 0);
    }
    fn release(&self) {
        self.state.lock().unwrap().0 = false;
        self.cv.notify_all();
    }
    fn pass(&self) -> Result<(), RuntimeError> {
        let mut state = self.state.lock().unwrap();
        if !state.0 {
            return Ok(());
        }
        state.1 += 1;
        self.cv.notify_all();
        let deadline = Instant::now() + HOLD_LIMIT;
        while state.0 {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return Err(RuntimeError::new(
                    "hold_timeout",
                    "write was never released",
                ));
            };
            state = self.cv.wait_timeout(state, left).unwrap().0;
        }
        Ok(())
    }
    fn wait_entered(&self, count: usize) {
        let deadline = Instant::now() + WITHIN;
        let mut state = self.state.lock().unwrap();
        while state.1 < count {
            let left = deadline
                .checked_duration_since(Instant::now())
                .expect("the held write was reached");
            state = self.cv.wait_timeout(state, left).unwrap().0;
        }
    }
}

/// Texts each host received, in arrival order.
type Wire = Arc<Mutex<Vec<String>>>;

struct FakeGateway {
    endpoint: String,
    session: String,
    boot: String,
    wire: Wire,
    hold: Arc<Hold>,
}

fn not_in_fake() -> RuntimeError {
    RuntimeError::new("unsupported_in_fake", "not used by this seam")
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
        Err(not_in_fake())
    }
    fn take_events(&mut self) -> Option<std::sync::mpsc::Receiver<GatewayEvent>> {
        None
    }
    fn api_request(&self, _method: &str, _params: Value) -> Result<Value, RuntimeError> {
        Err(not_in_fake())
    }
    fn endpoint_request(&self, _method: &str, _params: Value) -> Result<Value, RuntimeError> {
        Err(not_in_fake())
    }
    fn send_input(
        &self,
        target: &QualifiedTarget,
        events: Vec<ClientPaneInputEvent>,
    ) -> Result<(), RuntimeError> {
        assert_eq!(target.pane_id, PANE);
        let text: String = events
            .iter()
            .map(|event| match event {
                ClientPaneInputEvent::TextCommit(text) => text.clone(),
                other => format!("{other:?}"),
            })
            .collect();
        self.wire.lock().unwrap().push(text.clone());
        self.hold.pass()?;
        if text == "falha" {
            return Err(RuntimeError::new(
                "fake_write_failed_hd007i",
                "write failed after reaching the host",
            ));
        }
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
    fn request(&self, _method: &str, _params: Value) -> Result<Value, RuntimeError> {
        Ok(json!({ "type": "ok" }))
    }
}

struct NoSsh;

impl SshRunner for NoSsh {
    fn output(
        &self,
        _command: &OpenSshCommand,
        _timeout: Duration,
        _stdin: Option<&[u8]>,
    ) -> std::io::Result<ProcessOutput> {
        Err(std::io::Error::other("no ssh in this seam"))
    }
    fn spawn(&self, _command: &OpenSshCommand) -> std::io::Result<Box<dyn SshChild>> {
        Err(std::io::Error::other("no ssh in this seam"))
    }
}

#[derive(Default)]
struct Sink {
    events: Mutex<Vec<Value>>,
}

impl FrameSink for Sink {
    fn send(&self, event: FrameEvent) -> bool {
        self.events
            .lock()
            .unwrap()
            .push(serde_json::to_value(&event).unwrap());
        true
    }
}

impl Sink {
    fn kinds(&self) -> Vec<String> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .map(|e| e["type"].as_str().unwrap().to_owned())
            .collect()
    }
}

// =======================================================================================
// Fixtures
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
    let rect = SurfaceRect {
        x: 0,
        y: 0,
        width: 12,
        height: 2,
    };
    PaneSurfaceFrame {
        boot_id: boot.into(),
        projection_revision: 1,
        surface_revision: revision,
        frame: FrameData {
            cells: text
                .chars()
                .chain(std::iter::repeat(' '))
                .take(24)
                .map(|c| CellData {
                    symbol: c.to_string(),
                    fg: 0,
                    bg: 0,
                    modifier: 0,
                    skip: false,
                    hyperlink: None,
                })
                .collect(),
            width: 12,
            height: 2,
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

fn patch(boot: &str, base: u64, revision: u64, row: &str) -> PaneSurfacePatch {
    let frame = full(boot, 1, row);
    PaneSurfacePatch {
        boot_id: boot.into(),
        projection_revision: 1,
        base_surface_revision: base,
        surface_revision: revision,
        rows: vec![PaneSurfacePatchRow {
            x: 0,
            y: 0,
            cells: frame.frame.cells[..12].to_vec(),
        }],
        panes: frame.panes,
        cursor: None,
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

struct Host {
    wire: Wire,
    hold: Arc<Hold>,
    token: u64,
}

fn install(hub: &HostHub, ticket: &ConnectTicket, boot: &str) -> Host {
    let now = Instant::now();
    let wire = Wire::default();
    let hold = Arc::new(Hold::default());
    let session = hub.spec(&ticket.endpoint).unwrap().session;
    hub.finish_connect(
        ticket,
        Ok(Connected {
            gateway: Box::new(FakeGateway {
                endpoint: ticket.endpoint.clone(),
                session,
                boot: boot.into(),
                wire: wire.clone(),
                hold: hold.clone(),
            }),
            api: Arc::new(FakeApi),
        }),
        now,
    );
    Host {
        wire,
        hold,
        token: ticket.token,
    }
}

fn feed(hub: &HostHub, endpoint: &str, host: &Host, boot: &str, revision: u64) {
    let now = Instant::now();
    hub.apply_event(
        endpoint,
        host.token,
        GatewayEvent::Snapshot(Box::new(snapshot(boot))),
        now,
    );
    hub.apply_event(
        endpoint,
        host.token,
        GatewayEvent::Surface(Box::new(full(boot, revision, "$"))),
        now,
    );
}

fn connect_host(hub: &HostHub, endpoint: &str, boot: &str) -> Host {
    let ticket = hub
        .request_connect(endpoint, Instant::now())
        .unwrap()
        .expect("a connect ticket");
    let host = install(hub, &ticket, boot);
    feed(hub, endpoint, &host, boot, 4);
    host
}

struct Window {
    _dirs: Vec<tempfile::TempDir>,
    connections: ConnectionsState,
    surface: ComposedSurface,
    ssh: String,
}

impl Window {
    fn hub(&self) -> &HostHub {
        self.connections.hub()
    }
    fn expected(&self, endpoint: &str) -> SurfaceIdentityDto {
        let live = self.hub().live_identity(endpoint).unwrap();
        SurfaceIdentityDto {
            endpoint: live.endpoint,
            session: live.session,
            connection_generation: live.connection_generation,
            boot_id: live.boot_id,
            pane_id: PANE.into(),
        }
    }
    fn send(&self, expected: &SurfaceIdentityDto, text: &str) -> Task<Result<(), RuntimeError>> {
        task(
            self.surface
                .dispatch_input(expected.clone(), text_batch(text)),
        )
    }
}

fn text_batch(text: &str) -> Vec<InputDto> {
    vec![serde_json::from_value(json!({ "kind": "text", "text": text })).unwrap()]
}

/// Local `hd007i-local` selected and one saved SSH profile (`hd007i-remote`); nothing connected.
fn window() -> Window {
    let prefs = tempfile::tempdir().unwrap();
    let herdr_config = tempfile::tempdir().unwrap();
    let herdr_state = tempfile::tempdir().unwrap();
    let session = SessionName::parse("hd007i-local").unwrap();
    let connections = ConnectionsState::with_runner(
        ConnectionsConfig {
            prefs_dir: prefs.path().to_path_buf(),
            herdr_config_dir: herdr_config.path().to_path_buf(),
            herdr_state_dir: herdr_state.path().to_path_buf(),
            local_session: Some(session.clone()),
            local_auto_start: false,
            herdr_bin: "herdr".into(),
            isolated_ssh: None,
            geometry: geometry(),
        },
        Arc::new(NoSsh),
    );
    let view = connections
        .save_profile(
            SshProfileDraft {
                id: None,
                label: "remoto".into(),
                target: "tester@127.0.0.1".into(),
                port: None,
                session: "hd007i-remote".into(),
                auth: None,
            },
            false,
        )
        .unwrap();
    let ssh = view.profiles[0].id.as_str().to_owned();
    let surface = ComposedSurface::new(
        SelectionState::new(connections.clone()),
        SurfaceConfig {
            local_config_dir: herdr_config.path().to_path_buf(),
            local_session: Some(session),
            local_auto_start: false,
            surface_trace: None,
        },
    );
    assert_eq!(
        surface.selection_dto().endpoint.as_deref(),
        Some(LOCAL),
        "premise"
    );
    Window {
        _dirs: vec![prefs, herdr_config, herdr_state],
        connections,
        surface,
        ssh,
    }
}

/// Local connected (`boot-local-7`), selected and attached to `sink`.
fn live_local(sink: Arc<Sink>) -> (Window, Host) {
    let w = window();
    let local = connect_host(w.hub(), LOCAL, "boot-local-7");
    let status = w.surface.attach(geometry(), sink).unwrap();
    assert_eq!(status.state, "live", "premise");
    (w, local)
}

fn code(result: Result<(), RuntimeError>) -> String {
    result.err().map(|e| e.code).unwrap_or_else(|| "ok".into())
}

fn wire(host: &Host) -> Vec<String> {
    host.wire.lock().unwrap().clone()
}

// =======================================================================================
// Tests
// =======================================================================================

/// Would catch: `surface_input` writing on the calling thread (a Local write under backpressure
/// freezes the GUI), batches that wait for their turn holding the host lock (frames stop), or
/// batches ordered by poll/spawn instead of by call.
#[test]
fn a_held_write_hands_the_calling_thread_back_and_waiting_batches_keep_call_order() {
    let exec = Executor::new();
    let sink = Arc::new(Sink::default());
    let (w, local) = live_local(sink.clone());
    let expected = w.expected(LOCAL);

    local.hold.arm();
    let mut first = w.send(&expected, "a");
    exec.assert_pending(&mut first, "held write");
    local.hold.wait_entered(1);
    let mut second = w.send(&expected, "b");
    exec.assert_pending(&mut second, "batch behind a held write");
    local.hold.release();
    assert_eq!(code(exec.wait(&mut first, "first")), "ok");
    assert_eq!(code(exec.wait(&mut second, "second")), "ok");
    assert_eq!(wire(&local), ["a", "b"]);

    // Call order wins over poll order: `third` is created first, `fourth` polled first.
    let mut third = w.send(&expected, "c");
    let mut fourth = w.send(&expected, "d");
    exec.assert_pending(&mut fourth, "a later batch waits for the earlier call");
    // Waiting batches hold no host lock: frames of the same host reach the channel meanwhile.
    let before = sink.kinds().len();
    w.hub().apply_event(
        LOCAL,
        local.token,
        GatewayEvent::Patch(Box::new(patch("boot-local-7", 4, 5, "ok"))),
        Instant::now(),
    );
    assert_eq!(sink.kinds()[before..], ["patch", "metadata"]);
    assert_eq!(w.surface.status().state, "live");
    assert_eq!(
        wire(&local),
        ["a", "b"],
        "nothing overtook the unpolled call"
    );
    assert_eq!(code(exec.wait(&mut third, "third")), "ok");
    assert_eq!(code(exec.wait(&mut fourth, "fourth")), "ok");
    assert_eq!(wire(&local), ["a", "b", "c", "d"]);
}

/// Would catch: a failed or refused batch stalling every later batch, a failure retried (the
/// failed text reaching the host twice), or results delivered to the wrong call.
#[test]
fn a_failed_or_refused_batch_answers_its_own_call_and_the_next_batches_still_go_once() {
    let exec = Executor::new();
    let (w, local) = live_local(Arc::new(Sink::default()));
    let expected = w.expected(LOCAL);
    let mut unfocused = expected.clone();
    unfocused.pane_id = "w1:p2".into();

    local.hold.arm();
    let mut first = w.send(&expected, "um");
    exec.assert_pending(&mut first, "held write");
    local.hold.wait_entered(1);
    let mut failing = w.send(&expected, "falha");
    let mut refused = task(w.surface.dispatch_input(unfocused, text_batch("nunca")));
    let mut last = w.send(&expected, "dois");
    exec.assert_pending(&mut last, "last waits for the others");
    local.hold.release();
    assert_eq!(code(exec.wait(&mut first, "first")), "ok");
    assert_eq!(
        code(exec.wait(&mut failing, "failing")),
        "fake_write_failed_hd007i"
    );
    assert_eq!(code(exec.wait(&mut refused, "refused")), "pane_not_focused");
    assert_eq!(code(exec.wait(&mut last, "last")), "ok");
    assert_eq!(wire(&local), ["um", "falha", "dois"]);
}

/// Would catch: unbounded pending input (an unbounded queue or one blocking-pool job per batch),
/// the in-flight batch not counted, an excess refused only after being queued (or sent), or
/// capacity never given back.
#[test]
fn pending_input_is_bounded_in_batches_and_bytes_and_an_excess_is_refused_at_the_call() {
    let exec = Executor::new();
    let (w, local) = live_local(Arc::new(Sink::default()));
    let expected = w.expected(LOCAL);
    assert_eq!(MAX_PENDING_INPUT_BATCHES, 256);
    assert_eq!(MAX_PENDING_INPUT_BYTES, 1024 * 1024);

    // A single batch above the byte limit never becomes work.
    let mut huge = w.send(&expected, &"x".repeat(MAX_PENDING_INPUT_BYTES + 1));
    assert_eq!(code(exec.ready(&mut huge, "huge batch")), "input_too_large");
    assert!(wire(&local).is_empty());

    local.hold.arm();
    let mut held = w.send(&expected, "0");
    exec.assert_pending(&mut held, "held write");
    local.hold.wait_entered(1);
    let mut waiting: Vec<_> = (1..MAX_PENDING_INPUT_BATCHES)
        .map(|n| {
            let mut t = w.send(&expected, &n.to_string());
            exec.assert_pending(&mut t, "within the limit");
            t
        })
        .collect();
    let mut excess = w.send(&expected, "excesso");
    assert_eq!(
        code(exec.ready(&mut excess, "257th batch")),
        "input_overflow"
    );
    local.hold.release();
    assert_eq!(code(exec.wait(&mut held, "held")), "ok");
    for (n, t) in waiting.iter_mut().enumerate() {
        assert_eq!(code(exec.wait(t, "waiting")), "ok", "batch {}", n + 1);
    }
    let sent = wire(&local);
    assert_eq!(sent.len(), MAX_PENDING_INPUT_BATCHES);
    assert_eq!(sent.last().map(String::as_str), Some("255"));
    assert!(!sent.iter().any(|t| t == "excesso"));

    // Bytes count the in-flight batch too, and capacity comes back after completion.
    let half = MAX_PENDING_INPUT_BYTES / 2;
    local.hold.arm();
    let mut big = w.send(&expected, &"a".repeat(half));
    exec.assert_pending(&mut big, "held big write");
    local.hold.wait_entered(1);
    let mut over = w.send(&expected, &"b".repeat(half));
    assert_eq!(
        code(exec.ready(&mut over, "bytes above the limit")),
        "input_overflow"
    );
    local.hold.release();
    assert_eq!(code(exec.wait(&mut big, "big")), "ok");
    let mut again = w.send(&expected, &"c".repeat(half));
    assert_eq!(code(exec.wait(&mut again, "capacity back")), "ok");
    let sent = wire(&local);
    assert_eq!(sent.len(), MAX_PENDING_INPUT_BATCHES + 2);
    assert!(!sent.iter().any(|t| t.starts_with('b')));
}

/// Would catch: batches accepted for one channel sent after it was detached or replaced (a
/// "retry" re-attach), after another host was selected, or after the host came back with another
/// boot; the in-flight write interrupted or repeated by the invalidation; or the waiting input of
/// one host holding back the host selected next.
#[test]
fn waiting_batches_are_refused_unsent_when_channel_selection_or_boot_change() {
    let exec = Executor::new();

    // Detach while one write is in flight and two batches wait.
    let (w, local) = live_local(Arc::new(Sink::default()));
    let expected = w.expected(LOCAL);
    local.hold.arm();
    let mut flying = w.send(&expected, "voando");
    exec.assert_pending(&mut flying, "held write");
    local.hold.wait_entered(1);
    let mut after_a = w.send(&expected, "depois-a");
    let mut after_b = w.send(&expected, "depois-b");
    exec.assert_pending(&mut after_a, "waiting");
    // `detach` answers with a status read under the host lock the in-flight write holds (hub
    // design, bounded by that one write): run it aside and wait until the channel is gone.
    let detacher = {
        let surface = w.surface.clone();
        std::thread::spawn(move || surface.detach())
    };
    let deadline = Instant::now() + WITHIN;
    loop {
        let mut probe = w.send(&expected, "sonda");
        if let Poll::Ready(result) = exec.poll(&mut probe) {
            assert_eq!(code(result), "not_attached", "probe answered at the call");
            break;
        }
        drop(probe);
        assert!(Instant::now() < deadline, "detach released the channel");
        std::thread::yield_now();
    }
    local.hold.release();
    detacher.join().unwrap();
    assert_eq!(code(exec.wait(&mut flying, "in flight")), "ok");
    assert_eq!(
        code(exec.wait(&mut after_a, "after detach")),
        "input_cancelled"
    );
    assert_eq!(
        code(exec.wait(&mut after_b, "after detach")),
        "input_cancelled"
    );
    assert_eq!(wire(&local), ["voando"], "in-flight once, waiting never");
    // Nothing is queued for a channel that is not attached.
    let mut detached = w.send(&expected, "solto");
    assert_eq!(code(exec.ready(&mut detached, "detached")), "not_attached");

    // Re-attach (retry) of the same host: waiting batches of the old channel are refused.
    w.surface
        .attach(geometry(), Arc::new(Sink::default()))
        .unwrap();
    let mut old_channel = w.send(&expected, "canal-antigo");
    w.surface
        .attach(geometry(), Arc::new(Sink::default()))
        .unwrap();
    assert_eq!(
        code(exec.wait(&mut old_channel, "replaced channel")),
        "input_cancelled"
    );
    assert_eq!(wire(&local), ["voando"]);

    // Selection change: the Local batch waiting for its turn is refused; the SSH input selected
    // next is not held back by it, even though that Local turn is still unfinished.
    let mut local_first = w.send(&expected, "local-1");
    let mut local_waiting = w.send(&expected, "local-2");
    exec.assert_pending(&mut local_waiting, "waiting behind an unpolled call");
    w.surface.select(&w.ssh).unwrap();
    let ssh = connect_host(w.hub(), &w.ssh, "boot-ssh-3");
    w.surface
        .attach(geometry(), Arc::new(Sink::default()))
        .unwrap();
    let remote = w.expected(&w.ssh);
    assert_ne!(remote.boot_id, expected.boot_id, "premise");
    let mut on_ssh = w.send(&remote, "remoto");
    assert_eq!(
        code(exec.wait(&mut on_ssh, "input of the newly selected host")),
        "ok"
    );
    assert_eq!(
        code(exec.wait(&mut local_first, "old host")),
        "input_cancelled"
    );
    assert_eq!(
        code(exec.wait(&mut local_waiting, "old host")),
        "input_cancelled"
    );
    assert_eq!(wire(&local), ["voando"]);
    assert_eq!(wire(&ssh), ["remoto"]);

    // Reconnection to another boot while a batch waits: never sent to the new connection.
    // `earlier` is never polled before the reconnection, so `stale` really waits for its turn.
    let mut earlier = w.send(&remote, "boot-antigo-1");
    let mut stale = w.send(&remote, "boot-antigo-2");
    exec.assert_pending(&mut stale, "created before the reconnection");
    let now = Instant::now();
    w.hub().cancel(&w.ssh).unwrap();
    let ticket = w
        .hub()
        .request_connect(&w.ssh, now)
        .unwrap()
        .expect("reconnect ticket");
    let renewed = install(w.hub(), &ticket, "boot-ssh-9");
    feed(w.hub(), &w.ssh, &renewed, "boot-ssh-9", 1);
    assert_eq!(
        w.hub().live_identity(&w.ssh).unwrap().boot_id,
        "boot-ssh-9",
        "premise"
    );
    for (t, what) in [(&mut earlier, "earlier"), (&mut stale, "stale")] {
        let refused = code(exec.wait(t, what));
        assert!(
            refused.starts_with("target_"),
            "{what}: stale identity refused, got {refused}"
        );
    }
    assert_eq!(wire(&ssh), ["remoto"]);
    assert!(wire(&renewed).is_empty(), "never sent to the new boot");
}
