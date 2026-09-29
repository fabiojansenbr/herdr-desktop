//! Spec 007 — native Ctrl+Shift+V of the composed terminal (seam: the future the real
//! `surface_paste_clipboard` command awaits, `TerminalActions::dispatch_paste`, polled by a
//! single-threaded executor written here; no GUI, no engine, no default session: hosts are fakes
//! behind the one hub of the window, the clipboard is a recorder behind `NativeEffects`).
//!
//! WebKitGTK 2.52.6 emits no ClipboardEvent for Ctrl+Shift+V, so the desktop reads the local
//! clipboard in Rust and sends one `Paste` through the same ordered, bounded input lane as typing.
//! What the seam must show:
//! - the clipboard is read at the paste's turn in the lane, off the calling thread, once, and the
//!   text reaches the host once as `Paste`, literally; input called after it waits for it;
//! - identity (endpoint/session/generation/boot/pane) is checked before the read, and the
//!   attachment epoch captured at the call fences both the read and the send (hide, detach,
//!   switch away and back, disconnect);
//! - empty clipboard sends nothing; read failure and oversize are explicit, never replayed;
//! - the paste is bounded like any batch (turn + reserved bytes at the call, the actual text
//!   accounted before the send); the receipt carries no clipboard content.
//!
//! Fixture values differ on purpose: boots `boot-local-7` / `boot-local-8`, session
//! `hd007i-local`, panes `w1:p1` / `w1:p2`, clipboard corpus with newline, accents, CJK and emoji.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};

use herdr_client::protocol::wire::{
    CellData, ClientPaneInputEvent, ClientShellSnapshot, CursorState, FrameData, PaneSurfaceFrame,
    PaneSurfacePane, SurfaceRect,
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
use herdr_desktop::bridge::terminal_actions::{NativeEffects, PasteReceipt, TerminalActions};
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
                ClientPaneInputEvent::Paste(text) => format!("paste:{text}"),
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
    connections
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
// Clipboard recorder and paste helpers
// =======================================================================================

/// Literal multiline corpus: newline, accents, CJK and emoji (byte length != char count).
const CORPUS: &str = "linha 1 ação\n你好 🙂 fim\n";

/// Local clipboard behind `NativeEffects`: answers in order (the last one repeats), counts reads
/// and can hold a read until released.
struct Clipboard {
    answers: Mutex<Vec<Result<String, RuntimeError>>>,
    reads: AtomicUsize,
    hold: Hold,
}

impl Clipboard {
    fn with(answer: Result<String, RuntimeError>) -> Arc<Self> {
        Arc::new(Self {
            answers: Mutex::new(vec![answer]),
            reads: AtomicUsize::new(0),
            hold: Hold::default(),
        })
    }
    fn text(text: &str) -> Arc<Self> {
        Self::with(Ok(text.to_owned()))
    }
    fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }
    fn next(&self, answer: Result<String, RuntimeError>) {
        *self.answers.lock().unwrap() = vec![answer];
    }
}

impl NativeEffects for Clipboard {
    fn write_clipboard(&self, _text: &str) -> Result<(), RuntimeError> {
        Err(not_in_fake())
    }
    fn open_link(&self, _uri: &str) -> Result<(), RuntimeError> {
        Err(not_in_fake())
    }
    fn read_clipboard(&self) -> Result<String, RuntimeError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.hold.pass()?;
        self.answers.lock().unwrap()[0].clone()
    }
}

type PasteTask = Task<Result<PasteReceipt, RuntimeError>>;

fn paste(actions: &TerminalActions, expected: &SurfaceIdentityDto) -> PasteTask {
    task(actions.dispatch_paste(expected.clone()))
}

fn paste_code(result: &Result<PasteReceipt, RuntimeError>) -> String {
    match result {
        Ok(receipt) => format!("ok:{}:{}", receipt.sent, receipt.pasted_bytes),
        Err(error) => error.code.clone(),
    }
}

fn composed(w: &Window, clipboard: &Arc<Clipboard>) -> TerminalActions {
    TerminalActions::composed(w.surface.clone(), clipboard.clone())
}

// =======================================================================================
// Tests
// =======================================================================================

/// Would catch: the clipboard read before the paste's turn (or on the calling thread), a key typed
/// after Ctrl+Shift+V overtaking a slow clipboard read, the text sent twice or as `TextCommit`,
/// a receipt that leaks the clipboard content, or earlier input dropped/reordered by the paste.
#[test]
fn a_native_paste_reads_once_at_its_turn_and_input_called_after_it_waits_for_it() {
    let exec = Executor::new();
    let (w, local) = live_local(Arc::new(Sink::default()));
    let expected = w.expected(LOCAL);
    let clipboard = Clipboard::text(CORPUS);
    let actions = composed(&w, &clipboard);

    local.hold.arm();
    let mut before = w.send(&expected, "antes");
    exec.assert_pending(&mut before, "held earlier write");
    local.hold.wait_entered(1);
    clipboard.hold.arm();
    let mut pasted = paste(&actions, &expected);
    exec.assert_pending(&mut pasted, "paste behind a held write");
    let mut after = w.send(&expected, "depois");
    exec.assert_pending(&mut after, "key after the paste");
    assert_eq!(
        clipboard.reads(),
        0,
        "the clipboard is read only at the paste's turn"
    );

    local.hold.release();
    assert_eq!(code(exec.wait(&mut before, "earlier write")), "ok");
    exec.assert_pending(&mut pasted, "paste reading at its turn");
    clipboard.hold.wait_entered(1);
    exec.assert_pending(&mut pasted, "paste while the clipboard read is slow");
    exec.assert_pending(&mut after, "key while the paste reads the clipboard");
    assert_eq!(wire(&local), ["antes"], "nothing overtook the slow paste");

    clipboard.hold.release();
    let receipt = exec.wait(&mut pasted, "paste").expect("paste sent");
    assert_eq!(code(exec.wait(&mut after, "key after paste")), "ok");
    assert_eq!(
        wire(&local),
        [
            "antes".to_owned(),
            format!("paste:{CORPUS}"),
            "depois".to_owned()
        ]
    );
    assert_eq!(clipboard.reads(), 1);
    assert_eq!(
        (receipt.pane_id.as_str(), receipt.sent, receipt.pasted_bytes),
        (PANE, true, CORPUS.len())
    );
    let json = serde_json::to_string(&receipt).unwrap();
    assert!(
        !json.contains("linha") && !json.contains("你好"),
        "receipt leaks content: {json}"
    );
}

/// Would catch: a paste issued for another session/generation/boot/pane/host read from the
/// clipboard (or sent to the current connection), a standalone action seam fabricating a paste,
/// or a paste accepted with no attached channel.
#[test]
fn a_paste_for_any_other_identity_or_without_the_composed_window_never_reads_the_clipboard() {
    let exec = Executor::new();
    let (w, local) = live_local(Arc::new(Sink::default()));
    let expected = w.expected(LOCAL);
    let clipboard = Clipboard::text(CORPUS);
    let actions = composed(&w, &clipboard);

    type Mutation = (&'static str, fn(&mut SurfaceIdentityDto));
    let mutations: [Mutation; 5] = [
        ("session", |e| e.session = "hd007i-outra".into()),
        ("generation", |e| e.connection_generation += 1),
        ("boot", |e| e.boot_id = "boot-local-8".into()),
        ("pane", |e| e.pane_id = "w1:p2".into()),
        ("endpoint", |e| e.endpoint = "ssh-nao-selecionado".into()),
    ];
    for (what, mutate) in mutations {
        let mut stale = expected.clone();
        mutate(&mut stale);
        let mut t = paste(&actions, &stale);
        let result = exec.wait(&mut t, what);
        assert!(
            result.is_err(),
            "{what}: refused, got {}",
            paste_code(&result)
        );
    }
    assert_eq!(
        clipboard.reads(),
        0,
        "no clipboard effect for a stale identity"
    );
    assert!(wire(&local).is_empty());

    let standalone = TerminalActions::new(w.surface.selection().clone(), clipboard.clone());
    let mut t = paste(&standalone, &expected);
    assert_eq!(
        paste_code(&exec.wait(&mut t, "standalone")),
        "native_paste_unavailable"
    );

    w.surface.detach();
    let mut t = paste(&actions, &expected);
    assert_eq!(paste_code(&exec.ready(&mut t, "detached")), "not_attached");
    assert_eq!(clipboard.reads(), 0);
    assert!(wire(&local).is_empty());
}

/// Would catch: a paste issued before hide / switch away and back / disconnect still reading or
/// sending afterwards (the epoch not captured at the call, or checked only before the read).
#[test]
fn hide_switch_back_or_disconnect_after_the_gesture_cancels_the_read_or_the_send() {
    let exec = Executor::new();

    // Hidden while one paste reads (no host lock held) and another waits for its turn: the
    // reading one is not sent, the waiting one never reads.
    let (w, local) = live_local(Arc::new(Sink::default()));
    let expected = w.expected(LOCAL);
    let clipboard = Clipboard::text(CORPUS);
    let actions = composed(&w, &clipboard);
    clipboard.hold.arm();
    let mut reading = paste(&actions, &expected);
    exec.assert_pending(&mut reading, "slow read");
    clipboard.hold.wait_entered(1);
    let mut waiting = paste(&actions, &expected);
    exec.assert_pending(&mut waiting, "paste waiting for its turn");
    w.surface.set_interest(false).unwrap();
    clipboard.hold.release();
    assert_eq!(
        paste_code(&exec.wait(&mut reading, "hidden during read")),
        "input_cancelled"
    );
    assert_eq!(
        paste_code(&exec.wait(&mut waiting, "hidden before turn")),
        "input_cancelled"
    );
    assert_eq!(clipboard.reads(), 1, "the waiting paste never read");
    assert!(wire(&local).is_empty());

    // Channel released and attached again to the same host and identity during the read (the
    // window leaving the surface and coming back): read once, never sent, although every
    // identity field and the input gate are the same again; a paste issued afterwards goes.
    let (w, local) = live_local(Arc::new(Sink::default()));
    let expected = w.expected(LOCAL);
    let clipboard = Clipboard::text(CORPUS);
    let actions = composed(&w, &clipboard);
    clipboard.hold.arm();
    let mut pasted = paste(&actions, &expected);
    exec.assert_pending(&mut pasted, "slow read");
    clipboard.hold.wait_entered(1);
    w.surface.detach();
    w.surface
        .attach(geometry(), Arc::new(Sink::default()))
        .unwrap();
    assert_eq!(
        w.expected(LOCAL),
        expected,
        "premise: identical identity after coming back"
    );
    assert_eq!(
        w.hub().link(LOCAL).unwrap().input_block,
        None,
        "premise: input open again"
    );
    clipboard.hold.release();
    assert_eq!(
        paste_code(&exec.wait(&mut pasted, "switched back")),
        "input_cancelled"
    );
    assert_eq!(clipboard.reads(), 1);
    assert!(wire(&local).is_empty());
    // A paste issued after coming back pastes normally.
    let mut again = paste(&actions, &expected);
    assert_eq!(
        paste_code(&exec.wait(&mut again, "new attach")),
        format!("ok:true:{}", CORPUS.len())
    );
    assert_eq!(wire(&local), [format!("paste:{CORPUS}")]);

    // Disconnected during the read: never sent.
    let (w, local) = live_local(Arc::new(Sink::default()));
    let expected = w.expected(LOCAL);
    let clipboard = Clipboard::text(CORPUS);
    let actions = composed(&w, &clipboard);
    clipboard.hold.arm();
    let mut pasted = paste(&actions, &expected);
    exec.assert_pending(&mut pasted, "slow read");
    clipboard.hold.wait_entered(1);
    w.hub().apply_event(
        LOCAL,
        local.token,
        GatewayEvent::Shutdown(None),
        Instant::now(),
    );
    clipboard.hold.release();
    let result = exec.wait(&mut pasted, "disconnected");
    assert!(result.is_err(), "disconnected: got {}", paste_code(&result));
    assert!(wire(&local).is_empty());
}

/// Would catch: an empty clipboard sending an empty paste, a read failure or an oversize text
/// sent/truncated/retried or reported as success, an oversize bound looser than the input
/// limits (e.g. the 4 MiB copy limit), or a refused paste stalling the lane.
#[test]
fn empty_failed_or_oversize_clipboard_sends_nothing_and_the_lane_moves_on() {
    let exec = Executor::new();
    let (w, local) = live_local(Arc::new(Sink::default()));
    let expected = w.expected(LOCAL);
    let clipboard = Clipboard::text("");
    let actions = composed(&w, &clipboard);

    let mut t = paste(&actions, &expected);
    assert_eq!(paste_code(&exec.wait(&mut t, "empty")), "ok:false:0");

    clipboard.next(Err(RuntimeError::new(
        "fake_clipboard_hd007p",
        "no clipboard owner",
    )));
    let mut t = paste(&actions, &expected);
    assert_eq!(
        paste_code(&exec.wait(&mut t, "read failure")),
        "fake_clipboard_hd007p"
    );

    clipboard.next(Ok("x".repeat(MAX_PENDING_INPUT_BYTES)));
    let mut t = paste(&actions, &expected);
    assert_eq!(
        paste_code(&exec.wait(&mut t, "oversize")),
        "clipboard_too_large"
    );

    let mut next = w.send(&expected, "depois");
    assert_eq!(code(exec.wait(&mut next, "next input")), "ok");
    assert_eq!(
        clipboard.reads(),
        3,
        "each paste read once, nothing retried"
    );
    assert_eq!(wire(&local), ["depois"]);
}

/// Would catch: a native paste escaping the lane's batch limit (unbounded intents), or its text
/// sent without being accounted against the pending byte budget of the input admitted after it.
#[test]
fn a_paste_is_bounded_at_the_call_and_its_text_is_accounted_before_the_send() {
    let exec = Executor::new();
    let (w, local) = live_local(Arc::new(Sink::default()));
    let expected = w.expected(LOCAL);
    let clipboard = Clipboard::text(CORPUS);
    let actions = composed(&w, &clipboard);

    local.hold.arm();
    let mut waiting: Vec<_> = (0..MAX_PENDING_INPUT_BATCHES)
        .map(|n| w.send(&expected, &n.to_string()))
        .collect();
    exec.assert_pending(&mut waiting[0], "held write");
    local.hold.wait_entered(1);
    let mut excess = paste(&actions, &expected);
    assert_eq!(
        paste_code(&exec.ready(&mut excess, "257th intent")),
        "input_overflow"
    );
    local.hold.release();
    for t in waiting.iter_mut() {
        assert_eq!(code(exec.wait(t, "waiting")), "ok");
    }
    assert_eq!(clipboard.reads(), 0);

    // The paste holds its turn with a small reservation; input admitted meanwhile keeps its
    // bytes, so a text that no longer fits the pending budget is refused, never sent.
    let half = MAX_PENDING_INPUT_BYTES / 2;
    clipboard.next(Ok("c".repeat(half)));
    clipboard.hold.arm();
    let mut pasted = paste(&actions, &expected);
    exec.assert_pending(&mut pasted, "slow read");
    clipboard.hold.wait_entered(1);
    let mut big = w.send(&expected, &"b".repeat(half));
    exec.assert_pending(&mut big, "admitted behind the paste");
    clipboard.hold.release();
    assert_eq!(
        paste_code(&exec.wait(&mut pasted, "over budget")),
        "input_overflow"
    );
    assert_eq!(code(exec.wait(&mut big, "big")), "ok");
    let sent = wire(&local);
    assert_eq!(sent.len(), MAX_PENDING_INPUT_BATCHES + 1);
    assert!(sent.last().unwrap().starts_with('b'));
    assert!(!sent.iter().any(|t| t.starts_with("paste:")));
}
