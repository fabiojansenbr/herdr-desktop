//! The one terminal surface of the composed window (007): IPC `selection_*` / `surface_*` over
//! the shared [`SelectionState`] and the hub of `ConnectionsState`. No second Local client.
//!
//! - [`ComposedSurface`] binds at most one frame channel ([`FrameSink`]) to the selected host.
//!   Attaching replaces (and silences) the previous channel; detaching only releases it — the hub
//!   connection, the engine and its PTYs stay.
//! - The hub observer holds the surface weakly (no `Arc` cycle through the hub). A notice is judged
//!   by the connection it was produced on ([`NoticeOrigin`]), never relabelled with the host's
//!   current connection: a frame of a lost/renewed connection or another boot is dropped.
//!   Delivery to the channel is serialized under the binding lock, which is taken without any hub
//!   lock held; while holding it only lock-only hub reads run (no I/O).
//! - Input needs the expected endpoint/session/generation/boot/pane of the current connection and
//!   the engine's confirmed focused pane; refused input is never queued nor replayed.
//! - The `surface_input` command goes through [`ComposedSurface::dispatch_input`]: one ordered,
//!   bounded lane per endpoint off the calling thread; a batch still waiting for its turn is
//!   refused (never sent) once the channel, the selection or the connection changes.
//! - An absent Local session is a recoverable status: nothing is started or retried by itself
//!   (`session_start` stays the explicit action). Other hosts are (re)connected by the hub.
//! - `HERDR_DESKTOP_SURFACE_TRACE` (001) stays opt-in: nothing is computed when unset.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::task::{Context, Poll, Waker};

use herdr_client::bootstrap::session_available;
use herdr_client::protocol::endpoint::ENDPOINT_PROTOCOL_GENERATION;
use herdr_client::protocol::wire::key_modifiers;
use herdr_client::{
    LiveIdentity, RuntimeError, SessionName, SessionPaths, SurfaceGeometry, SurfaceState,
};
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;

use super::selection::SelectionState;
use crate::connections::hub::{
    HostKind, HostLink, HostNotice, HubObserver, InterestOutcome, NoticeOrigin,
};
use crate::connections::state::LinkPhase;
use crate::terminal::{
    full_event, input_events_for_pane, metadata_event, patch_events, FrameEvent, GeometryDto,
    InputDto, StatusDto,
};

/// Commands this module exposes (kept in sync with `lib.rs` by a test).
pub const COMMANDS: &[&str] = &[
    "selection_get",
    "selection_set",
    "surface_attach",
    "surface_input",
    "surface_resize",
    "surface_focus",
    "surface_status",
    "surface_detach",
    "surface_interest",
];

/// Receiver of the frame events of one attach (the IPC channel in the window).
pub trait FrameSink: Send + Sync {
    /// False when the receiver is gone.
    fn send(&self, event: FrameEvent) -> bool;
}

impl FrameSink for Channel<FrameEvent> {
    fn send(&self, event: FrameEvent) -> bool {
        Channel::send(self, event).is_ok()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SelectionDto {
    pub endpoint: Option<String>,
    pub kind: Option<HostKind>,
    pub label: Option<String>,
    pub session: Option<String>,
    pub online: bool,
    pub identity: Option<LiveIdentity>,
}

/// Identity the WebView expects its input to reach (from `FrameEvent::Identity` of this host).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceIdentityDto {
    pub endpoint: String,
    pub session: String,
    pub connection_generation: u64,
    pub boot_id: String,
    pub pane_id: String,
}

#[derive(Debug, Clone)]
pub struct SurfaceConfig {
    /// Engine config dir of the Local host (session sockets).
    pub local_config_dir: PathBuf,
    /// Local session; `None` = no Local host and nothing selected initially.
    pub local_session: Option<SessionName>,
    /// Zero-config: an absent Local session is started by the connection attempt itself
    /// (AC-016-01); false keeps the previous "recoverable, nothing started" status.
    pub local_auto_start: bool,
    /// Opt-in JSON trace of the surface state (`HERDR_DESKTOP_SURFACE_TRACE`).
    pub surface_trace: Option<PathBuf>,
}

struct Binding {
    id: u64,
    endpoint: String,
    sink: Arc<dyn FrameSink>,
    /// Newest connection generation this binding has seen; older notices are dropped.
    seen_generation: u64,
    /// Identity and confirmed focused pane last announced to the channel.
    announced: Option<(LiveIdentity, Option<String>)>,
    /// A full surface of the announced connection was delivered (patches need one).
    has_full: bool,
}

struct Shared {
    selection: SelectionState,
    config: SurfaceConfig,
    binding: Mutex<Option<Binding>>,
    next_binding: AtomicU64,
    /// Last error of the surface itself (e.g. an absent Local session on attach).
    last_error: Mutex<Option<RuntimeError>>,
    /// Trace only: shell pid of (identity, pane), looked up once.
    trace_pid: Mutex<Option<(LiveIdentity, String, Option<u32>)>>,
    /// Bumped by select/attach/detach: input accepted under an older epoch is not sent anymore.
    input_epoch: AtomicU64,
    /// One ordered input lane per endpoint (created on first use, bounded by the hosts).
    input_lanes: Mutex<BTreeMap<String, Arc<InputLane>>>,
}

/// Attachment a terminal action was issued under: the channel binding of its endpoint and the
/// input epoch (bumped by select/attach/detach/hide) read at the call. Internal to the backend;
/// the WebView never sees or supplies it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionTicket {
    endpoint: String,
    binding_id: u64,
    epoch: u64,
}

fn action_cancelled(endpoint: &str) -> RuntimeError {
    RuntimeError::new(
        "action_cancelled",
        "the surface changed before this action's turn; nothing was sent",
    )
    .with_endpoint(endpoint)
}

/// Selected surface of the window.
#[derive(Clone)]
pub struct ComposedSurface {
    shared: Arc<Shared>,
}

struct SurfaceObserver {
    shared: Weak<Shared>,
}

impl HubObserver for SurfaceObserver {
    fn notice(&self, origin: &NoticeOrigin, notice: &HostNotice) {
        if let Some(shared) = self.shared.upgrade() {
            // The hosted agents follow the same notices (topology/focus/geometry confirmed by
            // the server); the hub keeps one observer, so this one fans them out.
            shared.selection.forward_notice(origin, notice);
            shared.on_notice(origin, notice);
        }
    }
}

fn state_event(state: &str, reason: Option<String>, error: Option<RuntimeError>) -> FrameEvent {
    FrameEvent::State {
        state: state.to_owned(),
        reason,
        error,
    }
}

fn snake<T: Serialize>(value: T) -> Option<String> {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
}

/// Why the host has no usable surface now (InputBlock names, snake_case).
fn link_reason(link: &HostLink) -> Option<String> {
    match link.phase {
        LinkPhase::Online => link.input_block.and_then(snake),
        LinkPhase::Offline => Some("host_offline".into()),
        LinkPhase::Connecting => Some("host_connecting".into()),
        LinkPhase::Reconnecting => Some("host_reconnecting".into()),
        LinkPhase::Attention => Some("host_needs_attention".into()),
    }
}

fn surface_label(link: &HostLink) -> &'static str {
    match link.phase {
        LinkPhase::Online => match link.surface {
            Some(SurfaceState::Live) => "live",
            Some(SurfaceState::Stale(_)) => "stale",
            _ => "connecting",
        },
        LinkPhase::Connecting => "connecting",
        LinkPhase::Offline | LinkPhase::Reconnecting | LinkPhase::Attention => "disconnected",
    }
}

fn no_selection() -> RuntimeError {
    RuntimeError::new("no_selection", "no host is selected")
}

fn not_attached(endpoint: &str) -> RuntimeError {
    RuntimeError::new(
        "not_attached",
        "this host's surface is not attached; nothing was sent",
    )
    .with_endpoint(endpoint)
}

impl Shared {
    fn hub(&self) -> &crate::connections::hub::HostHub {
        self.selection.connections().hub()
    }

    fn local_session_available(&self) -> bool {
        self.config.local_session.as_ref().is_some_and(|session| {
            session_available(&SessionPaths::for_session(
                &self.config.local_config_dir,
                session,
            ))
        })
    }

    /// Sends the identity (and confirmed focused pane) of `identity` unless already announced.
    /// Returns false when `identity` is not the host's current connection.
    fn announce(&self, binding: &mut Binding, identity: &LiveIdentity, link: &HostLink) -> bool {
        if link.identity.as_ref() != Some(identity) {
            return false;
        }
        let pane = link.focused.as_ref().map(|(pane, _)| pane.clone());
        let next = (identity.clone(), pane.clone());
        if binding.announced.as_ref() != Some(&next) {
            if binding.announced.as_ref().map(|(i, _)| i) != Some(identity) {
                binding.has_full = false;
            }
            binding.sink.send(FrameEvent::Identity {
                boot_id: identity.boot_id.clone(),
                generation: ENDPOINT_PROTOCOL_GENERATION,
                connection_generation: identity.connection_generation,
                // The hub does not keep the welcome's server version: unknown, not synthesized.
                server_version: String::new(),
                pane_id: pane,
            });
            binding.announced = Some(next);
        }
        true
    }

    fn on_notice(&self, origin: &NoticeOrigin, notice: &HostNotice) {
        let mut resend = None;
        {
            let mut guard = self.binding.lock().expect("surface binding");
            let Some(binding) = guard.as_mut() else {
                return;
            };
            if binding.endpoint != origin.endpoint
                || self.selection.selected().as_deref() != Some(origin.endpoint.as_str())
                || origin.connection_generation < binding.seen_generation
            {
                return;
            }
            let hub = self.hub();
            match notice {
                HostNotice::Connected { generation } => {
                    binding.seen_generation = *generation;
                    binding.announced = None;
                    binding.has_full = false;
                    binding.sink.send(state_event("connecting", None, None));
                    // The connection was opened with the configured geometry: ask for the
                    // window's (one full surface), outside the binding lock.
                    resend = hub.geometry(&origin.endpoint);
                }
                HostNotice::Snapshot => {
                    let (Some(identity), Ok(link)) =
                        (origin.identity.as_ref(), hub.link(&origin.endpoint))
                    else {
                        return;
                    };
                    self.announce(binding, identity, &link);
                }
                HostNotice::Full(frame) => {
                    let (Some(identity), Ok(link)) =
                        (origin.identity.as_ref(), hub.link(&origin.endpoint))
                    else {
                        return;
                    };
                    if frame.boot_id != identity.boot_id || !self.announce(binding, identity, &link)
                    {
                        return;
                    }
                    binding.sink.send(full_event(frame));
                    binding.sink.send(metadata_event(frame));
                    binding.has_full = true;
                    binding
                        .sink
                        .send(state_event(surface_label(&link), link_reason(&link), None));
                }
                HostNotice::Patch(patch) => {
                    let Some(identity) = origin.identity.as_ref() else {
                        return;
                    };
                    let current = binding.announced.as_ref().map(|(i, _)| i) == Some(identity)
                        && binding.has_full
                        && patch.boot_id == identity.boot_id
                        && hub.check_identity(identity).is_ok();
                    if !current {
                        return;
                    }
                    let (event, meta) = patch_events(patch);
                    binding.sink.send(event);
                    if let Some(meta) = meta {
                        binding.sink.send(meta);
                    }
                }
                HostNotice::Stale(_) => {
                    let current = origin
                        .identity
                        .as_ref()
                        .is_some_and(|identity| hub.check_identity(identity).is_ok());
                    if current {
                        binding
                            .sink
                            .send(state_event("stale", Some("surface_stale".into()), None));
                    }
                }
                HostNotice::Lost(error)
                | HostNotice::Failed(error)
                | HostNotice::Invalidated(error) => {
                    binding.announced = None;
                    binding.has_full = false;
                    let link = hub.link(&origin.endpoint).ok();
                    // Never "live": the connection this channel showed is gone.
                    let state = match link.as_ref().map(surface_label) {
                        Some("connecting") => "connecting",
                        _ => "disconnected",
                    };
                    let reason = link.as_ref().and_then(link_reason);
                    binding
                        .sink
                        .send(state_event(state, reason, Some(error.clone())));
                }
            }
        }
        if let Some(geometry) = resend {
            let _ = self.hub().resize(&origin.endpoint, geometry);
        }
        self.trace();
    }

    fn status(&self, probe_session: bool) -> StatusDto {
        let last_error = self.last_error.lock().expect("surface error").clone();
        let Some(endpoint) = self.selection.selected() else {
            return StatusDto {
                session: None,
                session_available: false,
                connected: false,
                state: "empty".into(),
                reason: None,
                generation: None,
                connection_generation: 0,
                boot_id: None,
                server_version: None,
                pane_id: None,
                last_error,
            };
        };
        let link = match self.hub().link(&endpoint) {
            Ok(link) => link,
            Err(error) => {
                return StatusDto {
                    session: None,
                    session_available: false,
                    connected: false,
                    state: "disconnected".into(),
                    reason: Some("host_offline".into()),
                    generation: None,
                    connection_generation: 0,
                    boot_id: None,
                    server_version: None,
                    pane_id: None,
                    last_error: Some(error),
                }
            }
        };
        let online = link.phase == LinkPhase::Online;
        let session_available = match link.kind {
            HostKind::Local if probe_session => online || self.local_session_available(),
            _ => online,
        };
        StatusDto {
            session: Some(link.session.clone()),
            session_available,
            connected: online,
            state: surface_label(&link).into(),
            reason: link_reason(&link),
            generation: link.identity.as_ref().map(|_| ENDPOINT_PROTOCOL_GENERATION),
            connection_generation: link.generation,
            boot_id: link.identity.as_ref().map(|i| i.boot_id.clone()),
            server_version: None,
            pane_id: link.focused.as_ref().map(|(pane, _)| pane.clone()),
            last_error: if online {
                None
            } else {
                link.error.clone().or(last_error)
            },
        }
    }

    /// Opt-in trace (same fields as the 001 terminal trace). Never runs with the binding lock.
    fn trace(&self) {
        let Some(path) = self.config.surface_trace.as_ref() else {
            return;
        };
        let status = self.status(false);
        let endpoint = self.selection.selected();
        let attached = self.binding.lock().expect("surface binding").is_some();
        let link = endpoint.as_deref().and_then(|e| self.hub().link(e).ok());
        let identity = link.as_ref().and_then(|l| l.identity.clone());
        let pane = link.as_ref().and_then(|l| l.focused.clone());
        let shell_pid = match (identity.as_ref(), pane.as_ref()) {
            (Some(identity), Some((pane, _))) => {
                let mut cached = self.trace_pid.lock().expect("trace pid");
                match cached.as_ref() {
                    Some((i, p, pid)) if i == identity && p == pane => *pid,
                    _ => {
                        let pid = self
                            .hub()
                            .run_api(
                                identity,
                                "pane.process_info",
                                serde_json::json!({ "pane_id": pane }),
                                false,
                            )
                            .ok()
                            .and_then(|v| v.pointer("/process_info/shell_pid")?.as_u64())
                            .and_then(|pid| u32::try_from(pid).ok());
                        *cached = Some((identity.clone(), pane.clone(), pid));
                        pid
                    }
                }
            }
            _ => None,
        };
        let frame = identity.as_ref().and_then(|i| self.hub().surface_for(i));
        let probe = endpoint.as_deref().and_then(|e| self.hub().store_probe(e));
        let trace = serde_json::json!({
            "endpoint": endpoint,
            "state": status.state,
            "reason": status.reason,
            "connected": status.connected,
            "input_enabled": attached
                && link.as_ref().is_some_and(|l| l.focused.is_some() && l.input_block.is_none()),
            "last_error": status.last_error,
            "revision": link.as_ref().and_then(|l| l.revision).unwrap_or(0),
            "boot_id": status.boot_id,
            "generation": status.generation,
            "connection_generation": status.connection_generation,
            "pane_id": status.pane_id,
            "shell_pid": shell_pid,
            "width": frame.as_ref().map(|f| f.frame.width),
            "height": frame.as_ref().map(|f| f.frame.height),
            "rows": probe.as_ref().map(|(rows, _)| rows.clone()).unwrap_or_default(),
            "stats": probe.map(|(_, stats)| stats),
        });
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, serde_json::to_vec(&trace).unwrap_or_default()).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

impl Shared {
    fn cancel_pending_input(&self) {
        self.input_epoch.fetch_add(1, Ordering::AcqRel);
    }

    /// The attached channel and the selection are `expected`'s endpoint.
    fn input_target(&self, expected: &SurfaceIdentityDto) -> Result<(), RuntimeError> {
        let endpoint = expected.endpoint.as_str();
        let attached = self
            .binding
            .lock()
            .expect("surface binding")
            .as_ref()
            .is_some_and(|b| b.endpoint == endpoint);
        if !attached {
            return Err(not_attached(endpoint));
        }
        if self.selection.selected().as_deref() != Some(endpoint) {
            return Err(RuntimeError::new(
                "selection_changed",
                "the selected host changed; nothing was sent",
            )
            .with_endpoint(endpoint));
        }
        Ok(())
    }

    /// Identity, confirmed focus, gate, conversion against the current pane, the epoch (when
    /// dispatched) and the send happen under one host lock.
    fn send_input(
        &self,
        expected: &SurfaceIdentityDto,
        events: &[InputDto],
        epoch: Option<u64>,
    ) -> Result<(), RuntimeError> {
        let wanted = LiveIdentity {
            endpoint: expected.endpoint.clone(),
            session: expected.session.clone(),
            connection_generation: expected.connection_generation,
            boot_id: expected.boot_id.clone(),
        };
        self.hub()
            .send_focused_input(&wanted, &expected.pane_id, |pane| {
                if epoch.is_some_and(|e| self.input_epoch.load(Ordering::Acquire) != e) {
                    return Err(input_cancelled(&expected.endpoint));
                }
                input_events_for_pane(pane, events)
            })
    }

    /// One clipboard image to the confirmed focused pane under one host lock, with the same
    /// identity/focus/gate checks as [`Self::send_input`]; nothing is re-encoded here and nothing
    /// is replayed on refusal.
    fn send_clipboard_image(
        &self,
        expected: &SurfaceIdentityDto,
        extension: &str,
        data: &[u8],
        epoch: Option<u64>,
    ) -> Result<(), RuntimeError> {
        let wanted = LiveIdentity {
            endpoint: expected.endpoint.clone(),
            session: expected.session.clone(),
            connection_generation: expected.connection_generation,
            boot_id: expected.boot_id.clone(),
        };
        self.hub().send_focused_clipboard_image(
            &wanted,
            &expected.pane_id,
            extension,
            data.to_vec(),
            || {
                if epoch.is_some_and(|e| self.input_epoch.load(Ordering::Acquire) != e) {
                    Err(input_cancelled(&expected.endpoint))
                } else {
                    Ok(())
                }
            },
        )
    }

    /// Connection, confirmed focus of `expected.pane_id` and input gate, checked without sending
    /// (before a native effect such as a clipboard read). The send repeats them under the host lock.
    fn input_ready(&self, expected: &SurfaceIdentityDto) -> Result<(), RuntimeError> {
        let endpoint = expected.endpoint.as_str();
        let hub = self.hub();
        hub.check_identity(&LiveIdentity {
            endpoint: expected.endpoint.clone(),
            session: expected.session.clone(),
            connection_generation: expected.connection_generation,
            boot_id: expected.boot_id.clone(),
        })?;
        let link = hub.link(endpoint)?;
        if link.focused.as_ref().map(|(pane, _)| pane.as_str()) != Some(expected.pane_id.as_str()) {
            return Err(RuntimeError::new(
                "pane_not_focused",
                "the pane is not the focus confirmed by the server; nothing was sent",
            )
            .with_endpoint(endpoint));
        }
        if link.input_block.is_some() {
            return Err(RuntimeError::new(
                "input_blocked",
                "this pane's input is blocked right now; nothing was sent",
            )
            .retryable()
            .with_endpoint(endpoint));
        }
        Ok(())
    }

    /// True for the Local host: a local session never bridges clipboard images (the bridge exists
    /// for remote clients, `../herdr/src/client/clipboard_images.rs`); the app reads the local
    /// clipboard itself after the Ctrl+V key. SSH hosts keep the bridged `ClipboardImage`.
    fn host_is_local(&self, expected: &SurfaceIdentityDto) -> Result<bool, RuntimeError> {
        Ok(self.hub().link(&expected.endpoint)?.kind == HostKind::Local)
    }

    /// Checks at the call and reserves a turn plus budget in the endpoint's lane.
    fn admit_input(
        &self,
        expected: &SurfaceIdentityDto,
        events: &[InputDto],
    ) -> Result<(InputTicket, u64), RuntimeError> {
        self.admit_bytes(expected, input_payload_bytes(events))
    }

    /// [`Self::admit_input`] for `bytes` of payload known (or reserved) at the call.
    fn admit_bytes(
        &self,
        expected: &SurfaceIdentityDto,
        bytes: usize,
    ) -> Result<(InputTicket, u64), RuntimeError> {
        let endpoint = expected.endpoint.as_str();
        if bytes > MAX_PENDING_INPUT_BYTES {
            return Err(RuntimeError::new(
                "input_too_large",
                "the input batch exceeds the limit; nothing was sent",
            )
            .with_endpoint(endpoint));
        }
        let epoch = self.input_epoch.load(Ordering::Acquire);
        self.input_target(expected)?;
        let lane = self
            .input_lanes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(endpoint.to_owned())
            .or_default()
            .clone();
        let ticket = InputTicket::take(&lane, bytes).ok_or_else(|| input_overflow(endpoint))?;
        Ok((ticket, epoch))
    }
}

fn input_overflow(endpoint: &str) -> RuntimeError {
    RuntimeError::new(
        "input_overflow",
        "too much input pending for this host; this batch was not sent",
    )
    .retryable()
    .with_endpoint(endpoint)
}

fn input_cancelled(endpoint: &str) -> RuntimeError {
    RuntimeError::new(
        "input_cancelled",
        "the surface changed before this input's turn; nothing was sent",
    )
    .with_endpoint(endpoint)
}

// ---------------------------------------------------------------------------
// Input lanes
// ---------------------------------------------------------------------------

/// Batches admitted per endpoint and not answered yet (the one being sent included).
pub const MAX_PENDING_INPUT_BATCHES: usize = 256;
/// Payload bytes admitted per endpoint and not answered yet (the one being sent included).
pub const MAX_PENDING_INPUT_BYTES: usize = 1024 * 1024;
/// Accounted per event on top of its strings (fixed fields of the IPC JSON).
const INPUT_EVENT_OVERHEAD_BYTES: usize = 64;
/// Payload a native paste reserves at the call, before its clipboard text is known (one event).
pub const NATIVE_PASTE_RESERVED_BYTES: usize = INPUT_EVENT_OVERHEAD_BYTES;
/// Largest clipboard image bridged to the engine, mirroring `MAX_CLIPBOARD_IMAGE_PAYLOAD` of the
/// reference protocol (16 MiB). A larger image is refused before any byte reaches the connection.
pub const MAX_CLIPBOARD_IMAGE_PAYLOAD: usize = 16 * 1024 * 1024;

/// Payload read once at the paste's turn: text continues through the ordered input lane; an image
/// goes out as one `ClipboardImage` client message on the same connection, exactly as the TUI —
/// except on a Local host, where the r3 rule forwards the Ctrl+V key instead (the local app reads
/// the image itself, as the TUI's local session does).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardPayload {
    Text(String),
    Image { extension: String, data: Vec<u8> },
}

/// What a completed clipboard paste sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PasteKind {
    Text,
    Image,
    /// The Ctrl+V key was sent once (Local host with an image on the clipboard): the app reads the
    /// local clipboard itself, exactly like the TUI in a local session; no bytes were bridged.
    ForwardKey,
    /// Nothing was sent (empty clipboard or empty image).
    Empty,
}

/// Result of one clipboard paste: what it was and how many bytes went out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PasteSent {
    pub kind: PasteKind,
    pub bytes: usize,
}

/// Strings of the batch plus a fixed overhead per event (the DTO is only deserialized here).
fn input_payload_bytes(events: &[InputDto]) -> usize {
    events
        .iter()
        .map(|event| {
            INPUT_EVENT_OVERHEAD_BYTES
                + match event {
                    InputDto::Text { text } | InputDto::Paste { text } => text.len(),
                    InputDto::Key { code, .. } => code.len(),
                    InputDto::Mouse { action, button, .. } => {
                        action.len() + button.as_ref().map_or(0, String::len)
                    }
                }
        })
        .fold(0usize, usize::saturating_add)
}

/// Entry-ordered turns of one endpoint plus its pending budget. A ticket is issued at the call;
/// its turn comes when every earlier ticket was dropped (answered, or its future dropped).
#[derive(Default)]
struct InputLane {
    state: Mutex<LaneState>,
}

#[derive(Default)]
struct LaneState {
    issued: u64,
    next: u64,
    finished: BTreeSet<u64>,
    waiting: BTreeMap<u64, Waker>,
    batches: usize,
    bytes: usize,
}

impl InputLane {
    fn state(&self) -> MutexGuard<'_, LaneState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

struct InputTicket {
    lane: Arc<InputLane>,
    number: u64,
    bytes: usize,
}

impl InputTicket {
    fn take(lane: &Arc<InputLane>, bytes: usize) -> Option<Self> {
        let mut state = lane.state();
        if state.batches >= MAX_PENDING_INPUT_BATCHES
            || state.bytes.saturating_add(bytes) > MAX_PENDING_INPUT_BYTES
        {
            return None;
        }
        state.batches += 1;
        state.bytes += bytes;
        let number = state.issued;
        state.issued += 1;
        Some(Self {
            lane: lane.clone(),
            number,
            bytes,
        })
    }

    /// Grows this ticket's accounted payload to `bytes`; false (unchanged) when the endpoint's
    /// pending budget, with everything admitted meanwhile, has no room for it.
    fn grow(&mut self, bytes: usize) -> bool {
        let mut state = self.lane.state();
        let extra = bytes.saturating_sub(self.bytes);
        if state.bytes.saturating_add(extra) > MAX_PENDING_INPUT_BYTES {
            return false;
        }
        state.bytes += extra;
        self.bytes += extra;
        true
    }

    fn poll_turn(&self, cx: &mut Context<'_>) -> Poll<()> {
        let mut state = self.lane.state();
        if state.next == self.number {
            return Poll::Ready(());
        }
        state.waiting.insert(self.number, cx.waker().clone());
        Poll::Pending
    }
}

impl Drop for InputTicket {
    fn drop(&mut self) {
        let mut state = self.lane.state();
        state.batches -= 1;
        state.bytes -= self.bytes;
        state.waiting.remove(&self.number);
        state.finished.insert(self.number);
        loop {
            let next = state.next;
            if !state.finished.remove(&next) {
                break;
            }
            state.next += 1;
        }
        let next = state.next;
        let waker = state.waiting.remove(&next);
        drop(state);
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

impl ComposedSurface {
    /// Installs the surface observer on the hub (weakly held) and selects the Local host when a
    /// named Local session is configured and registered. Nothing is connected or started.
    pub fn new(selection: SelectionState, config: SurfaceConfig) -> Self {
        let local = config.local_session.is_some();
        let surface = Self {
            shared: Arc::new(Shared {
                selection,
                config,
                binding: Mutex::new(None),
                next_binding: AtomicU64::new(1),
                last_error: Mutex::new(None),
                trace_pid: Mutex::new(None),
                input_epoch: AtomicU64::new(0),
                input_lanes: Mutex::new(BTreeMap::new()),
            }),
        };
        let hub = surface.shared.hub();
        hub.set_observer(surface.observer());
        if local && hub.spec(herdr_client::LOCAL_ENDPOINT).is_ok() {
            let _ = surface
                .shared
                .selection
                .select(herdr_client::LOCAL_ENDPOINT);
        }
        surface
    }

    /// A new hub observer of this surface (weak: dropping the surface silences it).
    pub fn observer(&self) -> Arc<dyn HubObserver> {
        Arc::new(SurfaceObserver {
            shared: Arc::downgrade(&self.shared),
        })
    }

    pub fn selection(&self) -> &SelectionState {
        &self.shared.selection
    }

    pub fn selection_dto(&self) -> SelectionDto {
        let Some(endpoint) = self.shared.selection.selected() else {
            return SelectionDto {
                endpoint: None,
                kind: None,
                label: None,
                session: None,
                online: false,
                identity: None,
            };
        };
        let link = self.shared.hub().link(&endpoint).ok();
        SelectionDto {
            kind: link.as_ref().map(|l| l.kind),
            label: link.as_ref().map(|l| l.label.clone()),
            session: link.as_ref().map(|l| l.session.clone()),
            online: link.as_ref().is_some_and(|l| l.phase == LinkPhase::Online),
            identity: link.and_then(|l| l.identity),
            endpoint: Some(endpoint),
        }
    }

    /// Selects `endpoint` (unknown → `endpoint_unknown`, nothing changes). The attached channel
    /// is released first: it never receives frames of the newly selected host.
    pub fn select(&self, endpoint: &str) -> Result<SelectionDto, RuntimeError> {
        self.shared.hub().spec(endpoint)?;
        self.shared.cancel_pending_input();
        self.shared.binding.lock().expect("surface binding").take();
        *self.shared.last_error.lock().expect("surface error") = None;
        self.shared.selection.select(endpoint)?;
        Ok(self.selection_dto())
    }

    /// Binds `sink` to the selected host (replacing any previous channel), keeps `geometry` in the
    /// hub (sent now when connected, re-sent on each new connection) and replays the committed
    /// surface of the current connection. An offline host is connected by the hub, except an
    /// absent Local session: that is a recoverable `disconnected` status, nothing is started.
    pub fn attach(
        &self,
        geometry: SurfaceGeometry,
        sink: Arc<dyn FrameSink>,
    ) -> Result<StatusDto, RuntimeError> {
        let shared = &self.shared;
        let endpoint = shared.selection.selected().ok_or_else(no_selection)?;
        shared.cancel_pending_input();
        let id = shared.next_binding.fetch_add(1, Ordering::AcqRel);
        *shared.binding.lock().expect("surface binding") = Some(Binding {
            id,
            endpoint: endpoint.clone(),
            sink,
            seen_generation: 0,
            announced: None,
            has_full: false,
        });
        *shared.last_error.lock().expect("surface error") = None;
        let hub = shared.hub();
        // Stored for recovery and new connections; a failed write shows up as a lost connection.
        let _ = hub.resize(&endpoint, geometry);
        let link = hub.link(&endpoint)?;
        if !matches!(link.phase, LinkPhase::Online | LinkPhase::Connecting) {
            // Zero-config: the connector owns the detached start and its 10 s wait, so the
            // attempt below runs even with the socket absent. Without `auto_start` an absent
            // Local session stays the recoverable status (nothing is started here).
            if link.kind == HostKind::Local
                && !shared.local_session_available()
                && !shared.config.local_auto_start
            {
                let error = RuntimeError::new(
                    "server_unavailable",
                    "the local Herdr session is not running; use Start session",
                )
                .retryable()
                .with_endpoint(endpoint.as_str());
                *shared.last_error.lock().expect("surface error") = Some(error.clone());
                if let Some(binding) = shared.binding.lock().expect("surface binding").as_ref() {
                    if binding.id == id {
                        binding.sink.send(state_event(
                            "disconnected",
                            Some("host_offline".into()),
                            Some(error),
                        ));
                    }
                }
                shared.trace();
                return Ok(shared.status(false));
            }
            shared.selection.connections().connect(&endpoint)?;
        }
        {
            let mut guard = shared.binding.lock().expect("surface binding");
            if let Some(binding) = guard.as_mut().filter(|b| b.id == id) {
                let link = hub.link(&endpoint)?;
                binding.seen_generation = link.generation;
                let frame = link.identity.as_ref().and_then(|identity| {
                    shared
                        .announce(binding, identity, &link)
                        .then(|| hub.surface_for(identity))
                        .flatten()
                });
                if let Some(frame) = frame {
                    binding.sink.send(full_event(&frame));
                    binding.sink.send(metadata_event(&frame));
                    binding.has_full = true;
                }
                binding.sink.send(state_event(
                    surface_label(&link),
                    link_reason(&link),
                    link.error
                        .clone()
                        .filter(|_| link.phase != LinkPhase::Online),
                ));
            }
        }
        shared.trace();
        Ok(shared.status(false))
    }

    /// Sends `events` once to the confirmed focused pane of the current connection, only when
    /// `expected` still names it. Nothing is queued or replayed on refusal.
    pub fn input(
        &self,
        expected: &SurfaceIdentityDto,
        events: &[InputDto],
    ) -> Result<(), RuntimeError> {
        self.shared.input_target(expected)?;
        self.shared.send_input(expected, events, None)
    }

    /// Entry of the `surface_input` command: the checks, the limits and the turn in the
    /// endpoint's lane are taken now (at the call); the returned future waits for the turn
    /// without a thread and then sends once on the blocking pool, answering with that send's
    /// result. Refusals at the call (`not_attached`, `selection_changed`, `input_too_large`,
    /// `input_overflow`) create no work. A batch whose turn comes after select/attach/detach is
    /// answered `input_cancelled` unsent; the check is repeated under the host lock of the send,
    /// with identity, confirmed focus and gate. Nothing is retried or replayed.
    pub fn dispatch_input(
        &self,
        expected: SurfaceIdentityDto,
        events: Vec<InputDto>,
    ) -> impl Future<Output = Result<(), RuntimeError>> + Send + 'static {
        let admitted = self.shared.admit_input(&expected, &events);
        let shared = self.shared.clone();
        async move {
            let (ticket, epoch) = admitted?;
            std::future::poll_fn(|cx| ticket.poll_turn(cx)).await;
            let job = tauri::async_runtime::spawn_blocking(move || {
                let _ticket = ticket;
                if shared.input_epoch.load(Ordering::Acquire) != epoch {
                    return Err(input_cancelled(&expected.endpoint));
                }
                shared.send_input(&expected, &events, Some(epoch))
            });
            job.await.unwrap_or_else(|_| {
                Err(RuntimeError::new(
                    "input_interrupted",
                    "the input send ended without a confirmed result; nothing will be repeated",
                ))
            })
        }
    }

    /// Native paste (Ctrl+Shift+V) through the same lane as [`Self::dispatch_input`]: at the call,
    /// the checks and a turn with [`NATIVE_PASTE_RESERVED_BYTES`] are taken and the epoch is
    /// captured. At its turn, on the blocking pool: epoch, attachment, connection, confirmed focus
    /// and gate are checked before `read` runs (once); afterwards the epoch again, the text is
    /// bounded (`clipboard_too_large`) and accounted in the lane (`input_overflow`), then sent once
    /// as one `Paste` under the same host-lock checks as typing. Answers the pasted bytes (0: the
    /// clipboard was empty, nothing sent). The clipboard text never leaves the backend and nothing
    /// is retried.
    pub fn dispatch_paste_with(
        &self,
        expected: SurfaceIdentityDto,
        read: impl FnOnce() -> Result<String, RuntimeError> + Send + 'static,
    ) -> impl Future<Output = Result<usize, RuntimeError>> + Send + 'static {
        let admitted = self
            .shared
            .admit_bytes(&expected, NATIVE_PASTE_RESERVED_BYTES);
        let shared = self.shared.clone();
        async move {
            let (ticket, epoch) = admitted?;
            std::future::poll_fn(|cx| ticket.poll_turn(cx)).await;
            let job = tauri::async_runtime::spawn_blocking(move || {
                let mut ticket = ticket;
                let endpoint = expected.endpoint.as_str();
                let current = |shared: &Shared| shared.input_epoch.load(Ordering::Acquire) == epoch;
                if !current(&shared) {
                    return Err(input_cancelled(endpoint));
                }
                shared.input_target(&expected)?;
                shared.input_ready(&expected)?;
                let text = read().map_err(|e| e.with_endpoint(endpoint))?;
                if !current(&shared) {
                    return Err(input_cancelled(endpoint));
                }
                if text.is_empty() {
                    return Ok(0);
                }
                let events = [InputDto::Paste { text }];
                let bytes = input_payload_bytes(&events);
                if bytes > MAX_PENDING_INPUT_BYTES {
                    return Err(RuntimeError::new(
                        "clipboard_too_large",
                        "the clipboard text exceeds the input limit; nothing was sent",
                    )
                    .with_endpoint(endpoint));
                }
                if !ticket.grow(bytes) {
                    return Err(input_overflow(endpoint));
                }
                shared.send_input(&expected, &events, Some(epoch))?;
                Ok(bytes - INPUT_EVENT_OVERHEAD_BYTES)
            });
            job.await.unwrap_or_else(|_| {
                Err(RuntimeError::new(
                    "input_interrupted",
                    "the input send ended without a confirmed result; nothing will be repeated",
                ))
            })
        }
    }

    /// Native paste with an image: [`Self::dispatch_paste_with`] on the same ordered lane, with
    /// `read` answering a [`ClipboardPayload`] once at its turn. Text follows the exact text path;
    /// a Local host with an image answers `PasteKind::ForwardKey` after one Ctrl+V key on the same
    /// lane (r3: the app reads the image itself, as in a local TUI session); an SSH image is
    /// bounded by [`MAX_CLIPBOARD_IMAGE_PAYLOAD`] (`image_too_large`) and sent once as one
    /// `ClipboardImage` on the same connection under the same identity/focus/gate checks. The
    /// image bytes are not charged to the JSON input budget (they do not travel as input events),
    /// so the lane turn and the protocol frame bound the write. Nothing is retried or replayed.
    pub fn dispatch_clipboard_with(
        &self,
        expected: SurfaceIdentityDto,
        read: impl FnOnce() -> Result<ClipboardPayload, RuntimeError> + Send + 'static,
    ) -> impl Future<Output = Result<PasteSent, RuntimeError>> + Send + 'static {
        let admitted = self
            .shared
            .admit_bytes(&expected, NATIVE_PASTE_RESERVED_BYTES);
        let shared = self.shared.clone();
        async move {
            let (ticket, epoch) = admitted?;
            std::future::poll_fn(|cx| ticket.poll_turn(cx)).await;
            let job = tauri::async_runtime::spawn_blocking(move || {
                let mut ticket = ticket;
                let endpoint = expected.endpoint.as_str();
                let current = |shared: &Shared| shared.input_epoch.load(Ordering::Acquire) == epoch;
                if !current(&shared) {
                    return Err(input_cancelled(endpoint));
                }
                shared.input_target(&expected)?;
                shared.input_ready(&expected)?;
                let payload = read().map_err(|e| e.with_endpoint(endpoint))?;
                if !current(&shared) {
                    return Err(input_cancelled(endpoint));
                }
                match payload {
                    ClipboardPayload::Text(text) => {
                        if text.is_empty() {
                            return Ok(PasteSent {
                                kind: PasteKind::Empty,
                                bytes: 0,
                            });
                        }
                        let events = [InputDto::Paste { text }];
                        let bytes = input_payload_bytes(&events);
                        if bytes > MAX_PENDING_INPUT_BYTES {
                            return Err(RuntimeError::new(
                                "clipboard_too_large",
                                "the clipboard text exceeds the input limit; nothing was sent",
                            )
                            .with_endpoint(endpoint));
                        }
                        if !ticket.grow(bytes) {
                            return Err(input_overflow(endpoint));
                        }
                        shared.send_input(&expected, &events, Some(epoch))?;
                        Ok(PasteSent {
                            kind: PasteKind::Text,
                            bytes: bytes - INPUT_EVENT_OVERHEAD_BYTES,
                        })
                    }
                    ClipboardPayload::Image { extension, data } => {
                        if data.is_empty() {
                            return Ok(PasteSent {
                                kind: PasteKind::Empty,
                                bytes: 0,
                            });
                        }
                        // Spec 028 r3 (corrected rule): in a Local session the desktop does not
                        // bridge the image — the TUI only bridges it for remote clients
                        // (`../herdr/src/client/clipboard_images.rs`); the app running locally
                        // reads the image itself from the clipboard after receiving Ctrl+V. One
                        // normal key event on the same lane and host-lock checks as typing;
                        // nothing else is sent, and the protocol size limit does not apply (the
                        // bytes never cross the connection).
                        if shared.host_is_local(&expected)? {
                            let events = [InputDto::Key {
                                code: "Char".into(),
                                modifiers: key_modifiers::CONTROL,
                                ch: Some('v'),
                            }];
                            let bytes = input_payload_bytes(&events);
                            if !ticket.grow(bytes) {
                                return Err(input_overflow(endpoint));
                            }
                            shared.send_input(&expected, &events, Some(epoch))?;
                            return Ok(PasteSent {
                                kind: PasteKind::ForwardKey,
                                bytes: 0,
                            });
                        }
                        if data.len() > MAX_CLIPBOARD_IMAGE_PAYLOAD {
                            return Err(RuntimeError::new(
                                "image_too_large",
                                format!(
                                    "the clipboard image has {} bytes and exceeds the protocol limit of {} bytes; nothing was sent",
                                    data.len(),
                                    MAX_CLIPBOARD_IMAGE_PAYLOAD
                                ),
                            )
                            .with_endpoint(endpoint));
                        }
                        // The lane turn is held; the image bytes are not input payload.
                        shared.send_clipboard_image(&expected, &extension, &data, Some(epoch))?;
                        Ok(PasteSent {
                            kind: PasteKind::Image,
                            bytes: data.len(),
                        })
                    }
                }
            });
            job.await.unwrap_or_else(|_| {
                Err(RuntimeError::new(
                    "input_interrupted",
                    "the paste send ended without a confirmed result; nothing will be repeated",
                ))
            })
        }
    }

    pub fn resize(&self, geometry: SurfaceGeometry) -> Result<(), RuntimeError> {
        let Some(endpoint) = self.shared.selection.selected() else {
            return Ok(());
        };
        self.shared.hub().resize(&endpoint, geometry)
    }

    /// Focus report for the current connection of the selected host (none while offline).
    pub fn focus(&self, focused: bool) -> Result<(), RuntimeError> {
        let Some(endpoint) = self.shared.selection.selected() else {
            return Ok(());
        };
        match self.shared.hub().link(&endpoint)?.identity {
            Some(identity) => self.shared.hub().set_focus(&identity, focused),
            None => Ok(()),
        }
    }

    pub fn status(&self) -> StatusDto {
        self.shared.status(true)
    }

    /// Window interest in the selected host's surface (terminal hidden by the document or by
    /// the files layer). Hiding cancels pending input before anything waits on the network; the
    /// host, its agents and the channel binding stay. Showing reopens presentation and input only
    /// after a coherent full surface (see `HostHub::set_surface_interest`). Without a selection
    /// nothing happens.
    pub fn set_interest(&self, active: bool) -> Result<Option<InterestOutcome>, RuntimeError> {
        let Some(endpoint) = self.shared.selection.selected() else {
            return Ok(None);
        };
        if !active {
            self.shared.cancel_pending_input();
        }
        self.shared
            .hub()
            .set_surface_interest(&endpoint, active)
            .map(Some)
    }

    /// Ticket of a terminal action issued now for `expected`: the epoch is read before the
    /// selection and the attached binding of that endpoint are checked, so any later
    /// select/attach/detach/hide invalidates it. Refused at the call: `selection_changed`,
    /// `not_attached`.
    pub fn action_ticket(
        &self,
        expected: &SurfaceIdentityDto,
    ) -> Result<ActionTicket, RuntimeError> {
        let shared = &self.shared;
        let endpoint = expected.endpoint.as_str();
        let epoch = shared.input_epoch.load(Ordering::Acquire);
        if shared.selection.selected().as_deref() != Some(endpoint) {
            return Err(RuntimeError::new(
                "selection_changed",
                "the selected host changed; the action was cancelled",
            )
            .with_endpoint(endpoint));
        }
        let binding_id = shared
            .binding
            .lock()
            .expect("surface binding")
            .as_ref()
            .filter(|b| b.endpoint == endpoint)
            .map(|b| b.id)
            .ok_or_else(|| not_attached(endpoint))?;
        Ok(ActionTicket {
            endpoint: endpoint.to_owned(),
            binding_id,
            epoch,
        })
    }

    /// `action_cancelled` unless `ticket`'s epoch, binding and selection are still current. No
    /// hub lock and no I/O: safe inside a lane admission.
    pub fn check_action_ticket(&self, ticket: &ActionTicket) -> Result<(), RuntimeError> {
        let shared = &self.shared;
        let endpoint = ticket.endpoint.as_str();
        let current = shared.input_epoch.load(Ordering::Acquire) == ticket.epoch
            && shared.selection.selected().as_deref() == Some(endpoint)
            && shared
                .binding
                .lock()
                .expect("surface binding")
                .as_ref()
                .is_some_and(|b| b.id == ticket.binding_id && b.endpoint == endpoint);
        if current {
            Ok(())
        } else {
            Err(action_cancelled(endpoint))
        }
    }

    /// Releases the channel only; hub connection, engine and PTYs stay.
    pub fn detach(&self) -> StatusDto {
        self.shared.cancel_pending_input();
        self.shared.binding.lock().expect("surface binding").take();
        self.shared.status(false)
    }
}

// ---------------------------------------------------------------------------
// Commands (all off the GUI thread)
// ---------------------------------------------------------------------------

async fn blocking<T: Send + 'static>(
    run: impl FnOnce() -> Result<T, RuntimeError> + Send + 'static,
) -> Result<T, RuntimeError> {
    tauri::async_runtime::spawn_blocking(run)
        .await
        .map_err(|_| RuntimeError::new("surface_failed", "internal surface failure"))?
}

#[tauri::command]
pub async fn selection_get(
    state: tauri::State<'_, ComposedSurface>,
) -> Result<SelectionDto, RuntimeError> {
    let surface = state.inner().clone();
    blocking(move || Ok(surface.selection_dto())).await
}

#[tauri::command]
pub async fn selection_set(
    state: tauri::State<'_, ComposedSurface>,
    endpoint: String,
) -> Result<SelectionDto, RuntimeError> {
    let surface = state.inner().clone();
    blocking(move || surface.select(&endpoint)).await
}

#[tauri::command]
pub async fn surface_attach(
    state: tauri::State<'_, ComposedSurface>,
    geometry: GeometryDto,
    on_event: Channel<FrameEvent>,
) -> Result<StatusDto, RuntimeError> {
    let surface = state.inner().clone();
    blocking(move || surface.attach(geometry.into(), Arc::new(on_event))).await
}

/// Off the GUI thread through the endpoint's ordered, bounded lane ([`ComposedSurface::dispatch_input`]).
/// The lane orders by entry into this function; the WebView keeps one `surface_input` in flight
/// at a time (`src/shell/controller.ts`), so entry order is its call order.
#[tauri::command]
pub async fn surface_input(
    state: tauri::State<'_, ComposedSurface>,
    expected: SurfaceIdentityDto,
    events: Vec<InputDto>,
) -> Result<(), RuntimeError> {
    let dispatched = state.dispatch_input(expected, events);
    dispatched.await
}

#[tauri::command]
pub async fn surface_resize(
    state: tauri::State<'_, ComposedSurface>,
    geometry: GeometryDto,
) -> Result<(), RuntimeError> {
    let surface = state.inner().clone();
    blocking(move || surface.resize(geometry.into())).await
}

#[tauri::command]
pub async fn surface_focus(
    state: tauri::State<'_, ComposedSurface>,
    focused: bool,
) -> Result<(), RuntimeError> {
    let surface = state.inner().clone();
    blocking(move || surface.focus(focused)).await
}

#[tauri::command]
pub async fn surface_status(
    state: tauri::State<'_, ComposedSurface>,
) -> Result<StatusDto, RuntimeError> {
    let surface = state.inner().clone();
    blocking(move || Ok(surface.status())).await
}

#[tauri::command]
pub async fn surface_interest(
    state: tauri::State<'_, ComposedSurface>,
    active: bool,
) -> Result<Option<InterestOutcome>, RuntimeError> {
    let surface = state.inner().clone();
    blocking(move || surface.set_interest(active)).await
}

#[tauri::command]
pub async fn surface_detach(
    state: tauri::State<'_, ComposedSurface>,
) -> Result<StatusDto, RuntimeError> {
    let surface = state.inner().clone();
    blocking(move || Ok(surface.detach())).await
}
