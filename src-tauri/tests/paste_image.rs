//! Spec 028 AC-028-04 (r3 corrected rule) — image paste: a Local host never bridges the image
//! (the TUI only bridges it for remote clients, `../herdr/src/client/clipboard_images.rs`), so an
//! image on the clipboard answers one Ctrl+V key and the app reads the image itself; a remote
//! (SSH) host gets one `ClientMessage::ClipboardImage` for the confirmed focused pane, exactly as
//! the TUI bridges it. Text keeps the existing `Paste` path. Seam: the future
//! `surface_paste_clipboard` awaits, polled by a single-threaded executor; no GUI, no engine, no
//! default session: the hosts are fakes behind the one hub of the window and the clipboard is a
//! recorder behind `NativeEffects`.
//!
//! Fixture values differ on purpose: boot `boot-local-9`/`boot-ssh-7`, sessions `hd028i-local`/
//! `hd028i-remote`, panes `w1:p1`/`w1:p2`, image corpus with the PNG signature.
//!
//! What the seam must show:
//! - a Local image read once at the paste's turn is not bridged: one Ctrl+V key goes to the
//!   focused pane, no `ClipboardImage` and no `Paste`; the protocol size limit does not apply;
//! - a remote (SSH) image is sent once as `ClipboardImage { target: Pane(pane), extension, data }`,
//!   byte for byte, bounded by the protocol limit (`image_too_large`), and no `Paste` is sent;
//! - no image: the text path is unchanged (one `Paste`);
//! - empty clipboard answers `empty`/`sent: false`; a stale identity never reads the clipboard.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};

use herdr_client::protocol::wire::{
    key_modifiers, CellData, ClientKeyCode, ClientPaneInputEvent, ClientShellSnapshot, CursorState,
    FrameData, PaneSurfaceFrame, PaneSurfacePane, SurfaceRect,
};
use herdr_client::{
    ConnectOptions, GatewayEvent, LiveIdentity, Negotiated, QualifiedTarget, RuntimeError,
    RuntimeGateway, SessionName, SurfaceGeometry,
};
use herdr_desktop::bridge::composition::{
    ComposedSurface, FrameSink, PasteKind, SurfaceConfig, SurfaceIdentityDto,
    MAX_CLIPBOARD_IMAGE_PAYLOAD,
};
use herdr_desktop::bridge::selection::SelectionState;
use herdr_desktop::bridge::ssh::{ProcessOutput, SshChild, SshRunner};
use herdr_desktop::bridge::terminal_actions::{
    ClipboardImagePayload, NativeEffects, PasteReceipt, TerminalActions,
};
use herdr_desktop::connections::commands::{ConnectionsConfig, ConnectionsState};
use herdr_desktop::connections::hub::{ApiLane, ConnectTicket, Connected, HostHub};
use herdr_desktop::connections::profiles::SshProfileDraft;
use herdr_desktop::connections::ssh_options::OpenSshCommand;
use herdr_desktop::terminal::FrameEvent;
use serde_json::{json, Value};

const LOCAL: &str = "local";
const PANE: &str = "w1:p1";
const WITHIN: Duration = Duration::from_secs(10);

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

    fn wait<T>(&self, task: &mut Task<T>, what: &str) -> T {
        let deadline = Instant::now() + WITHIN;
        loop {
            let seen = *self.signal.wakes.lock().unwrap();
            match task.as_mut().poll(&mut Context::from_waker(&self.waker)) {
                Poll::Ready(value) => return value,
                Poll::Pending => {}
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

/// One image the fake host received.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReceivedImage {
    pane_id: String,
    extension: String,
    data: Vec<u8>,
}

#[derive(Default)]
struct Wire {
    texts: Vec<String>,
    keys: Vec<(ClientKeyCode, u8)>,
    images: Vec<ReceivedImage>,
}

struct FakeGateway {
    endpoint: String,
    session: String,
    boot: String,
    wire: Arc<Mutex<Wire>>,
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
        assert_eq!(target.pane_id, PANE, "input reaches the focused pane");
        let mut wire = self.wire.lock().unwrap();
        for event in &events {
            match event {
                ClientPaneInputEvent::TextCommit(text) => wire.texts.push(text.clone()),
                ClientPaneInputEvent::Paste(text) => wire.texts.push(format!("paste:{text}")),
                ClientPaneInputEvent::Key {
                    code, modifiers, ..
                } => wire.keys.push((code.clone(), *modifiers)),
                other => wire.texts.push(format!("{other:?}")),
            }
        }
        Ok(())
    }
    fn send_clipboard_image(
        &self,
        target: &QualifiedTarget,
        extension: &str,
        data: Vec<u8>,
    ) -> Result<(), RuntimeError> {
        assert_eq!(target.pane_id, PANE, "the image reaches the focused pane");
        self.wire.lock().unwrap().images.push(ReceivedImage {
            pane_id: target.pane_id.clone(),
            extension: extension.to_owned(),
            data,
        });
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
    wire: Arc<Mutex<Wire>>,
    token: u64,
}

fn install(hub: &HostHub, ticket: &ConnectTicket, boot: &str) -> Host {
    let now = Instant::now();
    let wire = Arc::new(Mutex::new(Wire::default()));
    let session = hub.spec(&ticket.endpoint).unwrap().session;
    hub.finish_connect(
        ticket,
        Ok(Connected {
            gateway: Box::new(FakeGateway {
                endpoint: ticket.endpoint.clone(),
                session,
                boot: boot.into(),
                wire: wire.clone(),
            }),
            api: Arc::new(FakeApi),
        }),
        now,
    );
    Host {
        wire,
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
    /// Registered SSH profile ("remoto", session `hd028i-remote`): a remote client host.
    ssh_endpoint: String,
}

impl Window {
    fn hub(&self) -> &HostHub {
        self.connections.hub()
    }
    fn expected_for(&self, endpoint: &str) -> SurfaceIdentityDto {
        let live = self.hub().live_identity(endpoint).unwrap();
        SurfaceIdentityDto {
            endpoint: live.endpoint,
            session: live.session,
            connection_generation: live.connection_generation,
            boot_id: live.boot_id,
            pane_id: PANE.into(),
        }
    }
    fn expected(&self) -> SurfaceIdentityDto {
        self.expected_for(LOCAL)
    }
}

/// Local `hd028i-local` selected; nothing connected.
fn window() -> Window {
    let prefs = tempfile::tempdir().unwrap();
    let herdr_config = tempfile::tempdir().unwrap();
    let herdr_state = tempfile::tempdir().unwrap();
    let session = SessionName::parse("hd028i-local").unwrap();
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
                session: "hd028i-remote".into(),
                auth: None,
            },
            false,
        )
        .unwrap();
    let ssh_endpoint = connections
        .view()
        .profiles
        .first()
        .expect("the saved SSH profile")
        .id
        .as_str()
        .to_owned();
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
        ssh_endpoint,
    }
}

/// Local connected (`boot-local-9`), selected and attached to `sink`.
fn live_local(sink: Arc<Sink>) -> (Window, Host) {
    let w = window();
    let local = connect_host(w.hub(), LOCAL, "boot-local-9");
    let status = w.surface.attach(geometry(), sink).unwrap();
    assert_eq!(status.state, "live", "premise");
    (w, local)
}

/// SSH `hd028i-remote` selected (`select` before connecting: the window's initial Local selection
/// hides every other host, and a hide drops an existing connection), connected (`boot-ssh-7`) and
/// attached to `sink`.
fn live_ssh(sink: Arc<Sink>) -> (Window, Host) {
    let w = window();
    let endpoint = w.ssh_endpoint.clone();
    w.surface.select(&endpoint).unwrap();
    let host = connect_host(w.hub(), &endpoint, "boot-ssh-7");
    let status = w.surface.attach(geometry(), sink).unwrap();
    assert_eq!(status.state, "live", "premise");
    (w, host)
}

// =======================================================================================
// Clipboard recorder and paste helpers
// =======================================================================================

/// Valid PNG signature followed by the payload (the server stages bytes by extension).
fn png(payload: &[u8]) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.extend_from_slice(payload);
    bytes
}

/// Local clipboard behind `NativeEffects`: an optional image, text, and read counters.
#[derive(Default)]
struct Clipboard {
    image: Mutex<Option<ClipboardImagePayload>>,
    text: Mutex<String>,
    image_reads: AtomicUsize,
    text_reads: AtomicUsize,
}

impl Clipboard {
    fn with_image(bytes: Vec<u8>) -> Arc<Self> {
        Arc::new(Self {
            image: Mutex::new(Some(ClipboardImagePayload {
                bytes,
                extension: "png".into(),
            })),
            ..Self::default()
        })
    }
    fn with_text(text: &str) -> Arc<Self> {
        Arc::new(Self {
            text: Mutex::new(text.to_owned()),
            ..Self::default()
        })
    }
    fn image_reads(&self) -> usize {
        self.image_reads.load(Ordering::SeqCst)
    }
    fn text_reads(&self) -> usize {
        self.text_reads.load(Ordering::SeqCst)
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
        self.text_reads.fetch_add(1, Ordering::SeqCst);
        Ok(self.text.lock().unwrap().clone())
    }
    fn read_clipboard_image(&self) -> Result<Option<ClipboardImagePayload>, RuntimeError> {
        self.image_reads.fetch_add(1, Ordering::SeqCst);
        Ok(self.image.lock().unwrap().clone())
    }
}

fn composed(w: &Window, clipboard: &Arc<Clipboard>) -> TerminalActions {
    TerminalActions::composed(w.surface.clone(), clipboard.clone())
}

fn paste_code(result: &Result<PasteReceipt, RuntimeError>) -> String {
    match result {
        Ok(receipt) => format!(
            "ok:{:?}:{}:{}",
            receipt.kind, receipt.sent, receipt.pasted_bytes
        ),
        Err(error) => error.code.clone(),
    }
}

// =======================================================================================
// Tests
// =======================================================================================

/// Would catch: the Local session bridging a `ClipboardImage` (the TUI only bridges images for
/// remote clients, `../herdr/src/client/clipboard_images.rs:92-99`), the key sent more than once,
/// with the wrong code/modifier or alongside a `Paste`.
#[test]
fn a_local_image_paste_forwards_one_ctrl_v_key_without_bridging_the_image() {
    let exec = Executor::new();
    let (w, local) = live_local(Arc::new(Sink::default()));
    let expected = w.expected();
    let bytes = png(b"hd028-image-bytes");
    let clipboard = Clipboard::with_image(bytes.clone());
    let actions = composed(&w, &clipboard);

    let mut pasted = task(actions.dispatch_paste(expected.clone()));
    let receipt = exec.wait(&mut pasted, "local image paste").expect("sent");

    assert_eq!(receipt.kind, PasteKind::ForwardKey);
    assert_eq!(receipt.pane_id, PANE);
    assert!(receipt.sent);
    assert_eq!(receipt.pasted_bytes, 0);
    assert_eq!(clipboard.image_reads(), 1, "the image probe ran");
    assert_eq!(clipboard.text_reads(), 0, "image has priority over text");
    let wire = local.wire.lock().unwrap();
    assert!(
        wire.images.is_empty(),
        "nothing was bridged to the local engine"
    );
    assert_eq!(
        wire.keys,
        vec![(ClientKeyCode::Char('v'), key_modifiers::CONTROL)],
        "one Ctrl+V key for the local app to read the clipboard itself"
    );
    assert!(wire.texts.is_empty(), "no Paste was also sent");
    let json = serde_json::to_string(&receipt).unwrap();
    assert!(
        !json.contains("hd028-image-bytes"),
        "receipt leaks image bytes: {json}"
    );
}

/// Would catch: a remote (SSH) image not bridged (the TUI bridges it for remote clients), sent
/// to another pane, re-encoded on the way, or sent alongside a `Paste`.
#[test]
fn a_remote_image_is_sent_once_as_a_clipboard_image_for_the_focused_pane() {
    let exec = Executor::new();
    let (w, remote) = live_ssh(Arc::new(Sink::default()));
    let expected = w.expected_for(&w.ssh_endpoint);
    let bytes = png(b"hd028-remote-image");
    let clipboard = Clipboard::with_image(bytes.clone());
    let actions = composed(&w, &clipboard);

    let mut pasted = task(actions.dispatch_paste(expected));
    let receipt = exec.wait(&mut pasted, "remote image paste").expect("sent");

    assert_eq!(receipt.kind, PasteKind::Image);
    assert_eq!(receipt.pane_id, PANE);
    assert!(receipt.sent);
    assert_eq!(receipt.pasted_bytes, bytes.len());
    assert_eq!(clipboard.image_reads(), 1);
    assert_eq!(clipboard.text_reads(), 0, "image has priority over text");
    let wire = remote.wire.lock().unwrap();
    assert_eq!(
        wire.images,
        vec![ReceivedImage {
            pane_id: PANE.into(),
            extension: "png".into(),
            data: bytes,
        }],
        "one ClipboardImage, byte for byte, for the focused pane"
    );
    assert!(
        wire.keys.is_empty(),
        "no Ctrl+V was forwarded to the remote host"
    );
    assert!(wire.texts.is_empty(), "no Paste was also sent");
}

/// Would catch: the text path changed by the image probe (two reads, wrong kind, or no send).
#[test]
fn without_an_image_the_text_path_is_unchanged() {
    let exec = Executor::new();
    let (w, local) = live_local(Arc::new(Sink::default()));
    let expected = w.expected();
    let clipboard = Clipboard::with_text("colar ação");
    let actions = composed(&w, &clipboard);

    let mut pasted = task(actions.dispatch_paste(expected));
    let receipt = exec.wait(&mut pasted, "text paste").expect("sent");

    assert_eq!(receipt.kind, PasteKind::Text);
    assert!(receipt.sent);
    assert_eq!(receipt.pasted_bytes, "colar ação".len());
    assert_eq!(clipboard.image_reads(), 1, "the image probe ran");
    assert_eq!(clipboard.text_reads(), 1);
    let wire = local.wire.lock().unwrap();
    assert!(wire.images.is_empty());
    assert_eq!(wire.texts, ["paste:colar ação"]);
}

/// Would catch: an oversize image written to a remote connection or refused without its size.
#[test]
fn an_image_larger_than_the_protocol_limit_is_refused_with_the_size() {
    let exec = Executor::new();
    let (w, remote) = live_ssh(Arc::new(Sink::default()));
    let expected = w.expected_for(&w.ssh_endpoint);
    let oversized = vec![0u8; MAX_CLIPBOARD_IMAGE_PAYLOAD + 1];
    let clipboard = Clipboard::with_image(oversized);
    let actions = composed(&w, &clipboard);

    let mut pasted = task(actions.dispatch_paste(expected));
    let result = exec.wait(&mut pasted, "oversize image");
    assert_eq!(paste_code(&result), "image_too_large");
    let error = result.unwrap_err();
    assert!(
        error
            .message
            .contains(&(MAX_CLIPBOARD_IMAGE_PAYLOAD + 1).to_string())
            && error
                .message
                .contains(&MAX_CLIPBOARD_IMAGE_PAYLOAD.to_string()),
        "message carries the size and the limit: {}",
        error.message
    );
    let wire = remote.wire.lock().unwrap();
    assert!(wire.images.is_empty(), "nothing reached the connection");
    assert!(wire.texts.is_empty() && wire.keys.is_empty());
}

/// Would catch: the protocol size limit leaking into the Local rule (nothing crosses the
/// connection there: the app reads its own clipboard after the key).
#[test]
fn a_local_image_past_the_protocol_limit_still_forwards_the_key() {
    let exec = Executor::new();
    let (w, local) = live_local(Arc::new(Sink::default()));
    let expected = w.expected();
    let oversized = vec![0u8; MAX_CLIPBOARD_IMAGE_PAYLOAD + 1];
    let clipboard = Clipboard::with_image(oversized);
    let actions = composed(&w, &clipboard);

    let mut pasted = task(actions.dispatch_paste(expected));
    let receipt = exec
        .wait(&mut pasted, "local oversize image")
        .expect("sent");

    assert_eq!(receipt.kind, PasteKind::ForwardKey);
    assert_eq!(receipt.pasted_bytes, 0);
    let wire = local.wire.lock().unwrap();
    assert!(wire.images.is_empty());
    assert_eq!(
        wire.keys,
        vec![(ClientKeyCode::Char('v'), key_modifiers::CONTROL)]
    );
}

/// Would catch: an empty clipboard reported as sent, as an error, or with bytes.
#[test]
fn an_empty_clipboard_answers_empty_with_nothing_sent() {
    let exec = Executor::new();
    let (w, local) = live_local(Arc::new(Sink::default()));
    let expected = w.expected();
    let clipboard = Clipboard::with_text("");
    let actions = composed(&w, &clipboard);

    let mut pasted = task(actions.dispatch_paste(expected));
    let receipt = exec.wait(&mut pasted, "empty paste").expect("answered");
    assert_eq!(receipt.kind, PasteKind::Empty);
    assert!(!receipt.sent);
    assert_eq!(receipt.pasted_bytes, 0);
    let wire = local.wire.lock().unwrap();
    assert!(wire.images.is_empty() && wire.texts.is_empty());
}

/// Would catch: the clipboard read for an identity that no longer matches the connection.
#[test]
fn an_image_paste_for_a_stale_identity_never_reads_the_clipboard() {
    let exec = Executor::new();
    let (w, local) = live_local(Arc::new(Sink::default()));
    let mut stale = w.expected();
    stale.pane_id = "w1:p2".into();
    let clipboard = Clipboard::with_image(png(b"stale"));
    let actions = composed(&w, &clipboard);

    let mut pasted = task(actions.dispatch_paste(stale));
    let result = exec.wait(&mut pasted, "stale paste");
    assert!(result.is_err(), "refused, got {}", paste_code(&result));
    assert_eq!(clipboard.image_reads(), 0);
    assert_eq!(clipboard.text_reads(), 0);
    let wire = local.wire.lock().unwrap();
    assert!(wire.images.is_empty() && wire.texts.is_empty());
}

// =======================================================================================
// Linux image reader (spec 028 r2b): the TUI path (`wl-paste`/`xclip`), not arboard
// =======================================================================================

/// The r2b window test showed Ctrl+V reading text (arboard `get_text`) but not the image:
/// arboard reads only `image/png` on Linux and its Wayland path is unproven, while the TUI
/// (`../herdr/src/platform/linux.rs`) reads `wl-paste --type <mime>` per MIME and validates the
/// magic bytes. These tests pin the desktop to that same proven path: PNG/JPEG/WebP by
/// signature, Wayland before X11, TUI MIME order, bounded read, and the reader's output going out
/// as one `ClipboardImage` for the focused pane. No GUI, no engine, no real clipboard.
#[cfg(target_os = "linux")]
mod linux_image_reader {
    use super::*;
    use herdr_desktop::bridge::terminal_actions::{
        read_bounded, read_linux_clipboard_image, LinuxClipboardSockets,
    };
    use std::sync::Mutex;

    /// TUI MIME order (`../herdr/src/platform/linux.rs:682-707`): the first offer that validates
    /// wins; for each MIME the Wayland socket is tried before the X11 one.
    const TUI_MIME_ORDER: [&str; 6] = [
        "image/png",
        "image/jpeg",
        "image/jpg",
        "image/gif",
        "image/webp",
        "image/bmp",
    ];

    fn jpeg(payload: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xD8, 0xFF];
        bytes.extend_from_slice(payload);
        bytes
    }

    fn webp(payload: &[u8]) -> Vec<u8> {
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        bytes.extend_from_slice(b"WEBP");
        bytes.extend_from_slice(payload);
        bytes
    }

    /// What one scripted command answers for a `(program, args)` pair.
    type Answer = Box<dyn Fn(&str, &[&str]) -> Option<Vec<u8>> + Send + Sync>;

    /// Scripted command runner: records every `(program, args)` and answers the script.
    struct Commands {
        calls: Mutex<Vec<(String, Vec<String>)>>,
        answer: Answer,
    }

    impl Commands {
        fn new(answer: impl Fn(&str, &[&str]) -> Option<Vec<u8>> + Send + Sync + 'static) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                answer: Box::new(answer),
            }
        }
        fn run(&self, program: &str, args: &[&str]) -> Option<Vec<u8>> {
            self.calls.lock().unwrap().push((
                program.to_owned(),
                args.iter().map(|arg| (*arg).to_owned()).collect(),
            ));
            (self.answer)(program, args)
        }
        fn calls(&self) -> Vec<(String, Vec<String>)> {
            self.calls.lock().unwrap().clone()
        }
    }

    fn read(sockets: LinuxClipboardSockets, commands: &Commands) -> Option<ClipboardImagePayload> {
        read_linux_clipboard_image(sockets, |program, args| commands.run(program, args))
    }

    /// Would catch: reading X11 first (or any other program/MIME) when Wayland holds the offer, or
    /// returning re-encoded bytes instead of the offered ones.
    #[test]
    fn a_wayland_png_offer_is_read_with_wl_paste_and_returned_byte_for_byte() {
        let bytes = png(b"r2b-wayland-png");
        let commands = Commands::new({
            let bytes = bytes.clone();
            move |program, args| {
                (program == "wl-paste" && args == ["--type", "image/png"]).then(|| bytes.clone())
            }
        });
        let payload = read(
            LinuxClipboardSockets {
                wayland: true,
                x11: true,
            },
            &commands,
        )
        .expect("the png offer is an image");
        assert_eq!(payload.bytes, bytes);
        assert_eq!(payload.extension, "png");
        assert_eq!(
            commands.calls(),
            vec![(
                "wl-paste".to_owned(),
                vec!["--type".to_owned(), "image/png".to_owned()]
            )],
            "Wayland first, one MIME read, xclip never reached"
        );
    }

    /// Would catch: JPEG/WebP (AC-028-04) not accepted, or a different MIME order than the TUI.
    #[test]
    fn png_absent_reads_jpeg_and_webp_in_the_tui_order() {
        let jpeg_bytes = jpeg(b"r2b-jpeg");
        let commands = Commands::new({
            let jpeg_bytes = jpeg_bytes.clone();
            move |program, args| {
                (program == "wl-paste" && args == ["--type", "image/jpeg"])
                    .then(|| jpeg_bytes.clone())
            }
        });
        let payload = read(
            LinuxClipboardSockets {
                wayland: true,
                x11: false,
            },
            &commands,
        )
        .expect("the jpeg offer is an image");
        assert_eq!(payload.extension, "jpg");
        assert_eq!(payload.bytes, jpeg_bytes);
        assert_eq!(
            commands
                .calls()
                .iter()
                .map(|(_, args)| args[1].clone())
                .collect::<Vec<_>>(),
            TUI_MIME_ORDER[..2],
            "png first, then jpeg, exactly like the TUI"
        );

        let webp_bytes = webp(b"r2b-webp");
        let commands = Commands::new({
            let webp_bytes = webp_bytes.clone();
            move |program, args| {
                (program == "wl-paste" && args == ["--type", "image/webp"])
                    .then(|| webp_bytes.clone())
            }
        });
        let payload = read(
            LinuxClipboardSockets {
                wayland: true,
                x11: false,
            },
            &commands,
        )
        .expect("the webp offer is an image");
        assert_eq!(payload.extension, "webp");
        assert_eq!(payload.bytes, webp_bytes);
        assert_eq!(
            commands
                .calls()
                .iter()
                .map(|(_, args)| args[1].clone())
                .collect::<Vec<_>>(),
            TUI_MIME_ORDER[..5],
            "the whole TUI order up to webp"
        );
    }

    /// Would catch: a text/plain offer served for an image MIME being pasted as an image (the TUI
    /// validates the signature; a plain-text clipboard keeps the text path).
    #[test]
    fn text_served_for_an_image_mime_is_rejected() {
        let commands =
            Commands::new(|_program, _args| Some(b"texto puro no lugar da imagem".to_vec()));
        let payload = read(
            LinuxClipboardSockets {
                wayland: true,
                x11: true,
            },
            &commands,
        );
        assert!(payload.is_none(), "a text offer is not an image");
        assert_eq!(
            commands.calls().len(),
            TUI_MIME_ORDER.len() * 2,
            "every MIME was tried on both sockets before giving up"
        );
    }

    /// Would catch: X11 sessions (no Wayland socket) not reading images, or different xclip args.
    #[test]
    fn xclip_reads_the_same_mime_when_there_is_no_wayland_socket() {
        let bytes = png(b"r2b-xclip");
        let commands = Commands::new({
            let bytes = bytes.clone();
            move |program, args| {
                (program == "xclip" && args == ["-selection", "clipboard", "-t", "image/png", "-o"])
                    .then(|| bytes.clone())
            }
        });
        let payload = read(
            LinuxClipboardSockets {
                wayland: false,
                x11: true,
            },
            &commands,
        )
        .expect("the png offer is an image");
        assert_eq!(payload.bytes, bytes);
        assert_eq!(payload.extension, "png");
        assert_eq!(
            commands.calls(),
            vec![(
                "xclip".to_owned(),
                vec![
                    "-selection".to_owned(),
                    "clipboard".to_owned(),
                    "-t".to_owned(),
                    "image/png".to_owned(),
                    "-o".to_owned(),
                ]
            )]
        );
    }

    /// Would catch: spawning clipboard commands in a session with no clipboard socket.
    #[test]
    fn no_clipboard_socket_reads_nothing() {
        let commands = Commands::new(|_program, _args| panic!("no command may run"));
        assert!(read(LinuxClipboardSockets::default(), &commands).is_none());
        assert!(commands.calls().is_empty());
    }

    /// Would catch: buffering an unbounded clipboard (the TUI stops at
    /// `MAX_CLIPBOARD_IMAGE_PAYLOAD`) or treating an empty offer as an image.
    #[test]
    fn bounded_read_accepts_exactly_the_limit_and_refuses_more() {
        assert_eq!(read_bounded(&b"12345"[..], 5), Some(b"12345".to_vec()));
        assert_eq!(read_bounded(&b"123456"[..], 5), None, "one byte over");
        assert_eq!(read_bounded(&b""[..], 5), None, "empty offer");
    }

    /// `NativeEffects` whose image read is exactly the Linux reader (scripted commands); proves the
    /// reader's payload is what `dispatch_paste` sends as `ClipboardImage`.
    struct ReaderEffects {
        commands: Commands,
        sockets: LinuxClipboardSockets,
        text: String,
        image_reads: AtomicUsize,
        text_reads: AtomicUsize,
    }

    impl NativeEffects for ReaderEffects {
        fn write_clipboard(&self, _text: &str) -> Result<(), RuntimeError> {
            Err(not_in_fake())
        }
        fn open_link(&self, _uri: &str) -> Result<(), RuntimeError> {
            Err(not_in_fake())
        }
        fn read_clipboard(&self) -> Result<String, RuntimeError> {
            self.text_reads.fetch_add(1, Ordering::SeqCst);
            Ok(self.text.clone())
        }
        fn read_clipboard_image(&self) -> Result<Option<ClipboardImagePayload>, RuntimeError> {
            self.image_reads.fetch_add(1, Ordering::SeqCst);
            Ok(read(self.sockets, &self.commands))
        }
    }

    /// Would catch: the reader succeeding while the paste sends the text path (or another pane's
    /// target, or re-encoded bytes); the bridge is the remote (SSH) path (r3: a Local host
    /// forwards the key instead).
    #[test]
    fn the_reader_output_is_the_clipboard_image_sent_to_the_focused_pane() {
        let exec = Executor::new();
        let (w, remote) = live_ssh(Arc::new(Sink::default()));
        let bytes = jpeg(b"r2b-integration");
        let commands = Commands::new({
            let bytes = bytes.clone();
            move |program, args| {
                (program == "wl-paste" && args == ["--type", "image/jpeg"]).then(|| bytes.clone())
            }
        });
        let effects = Arc::new(ReaderEffects {
            commands,
            sockets: LinuxClipboardSockets {
                wayland: true,
                x11: false,
            },
            text: "texto de fallback".into(),
            image_reads: AtomicUsize::new(0),
            text_reads: AtomicUsize::new(0),
        });
        let actions = TerminalActions::composed(w.surface.clone(), effects.clone());

        let mut pasted = task(actions.dispatch_paste(w.expected_for(&w.ssh_endpoint)));
        let receipt = exec.wait(&mut pasted, "reader paste").expect("sent");

        assert_eq!(receipt.kind, PasteKind::Image);
        assert_eq!(receipt.pane_id, PANE);
        assert_eq!(receipt.pasted_bytes, bytes.len());
        assert_eq!(effects.image_reads.load(Ordering::SeqCst), 1);
        assert_eq!(effects.text_reads.load(Ordering::SeqCst), 0);
        let wire = remote.wire.lock().unwrap();
        assert_eq!(
            wire.images,
            vec![ReceivedImage {
                pane_id: PANE.into(),
                extension: "jpg".into(),
                data: bytes,
            }]
        );
        assert!(wire.texts.is_empty() && wire.keys.is_empty());
    }
}
