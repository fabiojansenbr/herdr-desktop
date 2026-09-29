//! Hosts (Local and SSH) side by side: routing by endpoint, per-host link state, input gating
//! by reconciled snapshot/surface, and an action ledger that never replays unknown results.
//!
//! Each host has its own lock; the map lock is held only to find the host. JSON API actions
//! run outside the host lock on their own lane, so a slow action never delays input or
//! frames of the same host, and nothing on one host waits for another.

use std::collections::{BTreeMap, VecDeque};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Condvar, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant};

use herdr_client::protocol::wire::{
    ClientPaneInputEvent, ClientShellAgent, ClientShellSnapshot, PaneSurfaceFrame, PaneSurfacePatch,
};
use herdr_client::{
    ApplyOutcome, FrameStore, GatewayEvent, LiveIdentity, QualifiedTarget, RuntimeError,
    RuntimeGateway, StaleReason, SurfaceGeometry, SurfaceState,
};
use serde::Serialize;
use serde_json::Value;

use super::failure::{AttentionReason, ConnectFailure};
use super::remote_binary::RemoteBinary;
use super::state::{LinkMachine, LinkPhase};

/// Endpoint command lane of one connection, cloned out of the host so a request waiting on
/// the reader never holds the host lock.
pub trait EndpointLane: Send + Sync {
    /// One announced command for the server boot `boot_id`, correlated with its reply.
    fn request(&self, boot_id: &str, method: &str, params: Value) -> Result<Value, RuntimeError>;
    /// Like `request`, but `admit` runs once this requester holds the lane's turn (after the
    /// request ahead of it finished) and before the write; a refusal writes nothing. Not atomic
    /// with the write: the lane may still acquire its own state/writer after admitting.
    /// A lane that cannot run the admission refuses without writing: guarded callers are never
    /// sent unguarded.
    fn request_admitted(
        &self,
        _boot_id: &str,
        method: &str,
        _params: Value,
        _admit: &dyn Fn() -> Result<(), RuntimeError>,
    ) -> Result<Value, RuntimeError> {
        Err(RuntimeError::new(
            "endpoint_guard_unsupported",
            format!("this connection does not confirm {method} at send time; nothing was sent"),
        ))
    }
    fn methods(&self) -> Vec<String>;
}

impl EndpointLane for herdr_client::local::LocalEndpointLane {
    fn request(&self, boot_id: &str, method: &str, params: Value) -> Result<Value, RuntimeError> {
        herdr_client::local::LocalEndpointLane::request(self, boot_id, method, params)
    }

    fn request_admitted(
        &self,
        boot_id: &str,
        method: &str,
        params: Value,
        admit: &dyn Fn() -> Result<(), RuntimeError>,
    ) -> Result<Value, RuntimeError> {
        herdr_client::local::LocalEndpointLane::request_admitted(
            self, boot_id, method, params, admit,
        )
    }

    fn methods(&self) -> Vec<String> {
        herdr_client::local::LocalEndpointLane::methods(self).to_vec()
    }
}

/// Errors after which an action may or may not have run on the server.
fn result_unknown(error: &RuntimeError) -> bool {
    matches!(
        error.code.as_str(),
        "timeout"
            | "connection_lost"
            | "empty_response"
            | "server_unavailable"
            | "ssh_failed"
            | "ssh_unreachable"
            | "ssh_terminated"
            | "endpoint_boot_changed"
            | "response_too_large"
    )
}

/// What the renderer of the window may need to know about one host, delivered after the host
/// lock is released.
#[derive(Debug, Clone)]
pub enum HostNotice {
    Connected {
        generation: u64,
    },
    Snapshot,
    Full(Box<PaneSurfaceFrame>),
    Patch(Box<PaneSurfacePatch>),
    Stale(StaleReason),
    Lost(RuntimeError),
    /// The current connection attempt failed (the host stays offline or retries later).
    Failed(RuntimeError),
    /// The window dropped the connection of the origin (cancel, or a renegotiation of its surface
    /// mode); nothing of that connection is current any more.
    Invalidated(RuntimeError),
}

/// Connection a notice belongs to, captured under the host lock together with the notice. A
/// notice is delivered after the lock is released, when the host may already run a newer
/// connection; the observer must judge it by this origin, never by the host's current identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoticeOrigin {
    pub endpoint: String,
    /// Hub-owned connection generation the notice was produced on.
    pub connection_generation: u64,
    /// Identity of that connection when known (none before its first snapshot).
    pub identity: Option<LiveIdentity>,
}

pub trait HubObserver: Send + Sync {
    fn notice(&self, origin: &NoticeOrigin, notice: &HostNotice);
}

/// Link and confirmed focus of one host, read under one lock (no I/O).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostLink {
    pub kind: HostKind,
    pub label: String,
    pub session: String,
    pub visible: bool,
    pub phase: LinkPhase,
    pub generation: u64,
    /// Identity of the online connection.
    pub identity: Option<LiveIdentity>,
    pub error: Option<RuntimeError>,
    /// Focused (pane, workspace) of the snapshot of the current connection and boot.
    pub focused: Option<(String, String)>,
    /// Surface state while the committed surface belongs to the current connection.
    pub surface: Option<SurfaceState>,
    pub revision: Option<u64>,
    /// Why input to the focused pane is refused now (`None` = allowed or no focused pane).
    pub input_block: Option<InputBlock>,
}

/// JSON API lane of one host (local socket client or one SSH process per request).
pub trait ApiLane: Send + Sync {
    fn request(&self, method: &str, params: Value) -> Result<Value, RuntimeError>;
}

impl ApiLane for herdr_client::api::ApiClient {
    fn request(&self, method: &str, params: Value) -> Result<Value, RuntimeError> {
        herdr_client::api::ApiClient::request(self, method, params)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostKind {
    Local,
    Ssh,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostSpec {
    /// "local" or the SSH profile id.
    pub endpoint: String,
    pub label: String,
    pub kind: HostKind,
    pub session: String,
    pub target: Option<String>,
    /// Visible hosts request a surface; hidden hosts stay metadata-only.
    pub visible: bool,
}

/// A successful connection attempt.
pub struct Connected {
    pub gateway: Box<dyn RuntimeGateway>,
    pub api: Arc<dyn ApiLane>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectTicket {
    pub endpoint: String,
    pub token: u64,
    pub surface_active: bool,
}

/// What one visibility change of a host did (spec 035).
#[derive(Debug)]
pub enum VisibilityOutcome {
    /// The host was already in the requested mode: nothing was sent.
    Unchanged,
    /// The live connection (or the next one) toggled its surface lease in place, exactly like the
    /// TUI: no bridge was closed and no attempt was started.
    Interest(InterestOutcome),
    /// Old behavior for a connection that did not announce `surface_interest`: the connection was
    /// dropped and the returned ticket must be executed to renegotiate the surface mode.
    Renegotiate(ConnectTicket),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InputBlock {
    HostOffline,
    HostConnecting,
    HostReconnecting,
    HostNeedsAttention,
    NoSnapshot,
    PaneNotInSnapshot,
    SurfaceHidden,
    NoSurface,
    SurfaceStale,
    SurfaceFromPreviousBoot,
    SurfaceFromPreviousConnection,
}

impl InputBlock {
    fn error(self, endpoint: &str) -> RuntimeError {
        let (code, message) = match self {
            Self::HostOffline => (
                "host_offline",
                "the host is disconnected; input is unavailable",
            ),
            Self::HostConnecting => ("host_connecting", "connecting; input is unavailable"),
            Self::HostReconnecting => ("host_reconnecting", "reconnecting; input is unavailable"),
            Self::HostNeedsAttention => (
                "host_needs_attention",
                "the host needs attention; input is unavailable",
            ),
            _ => (
                "input_blocked",
                "the pane has not been reconciled with the server yet",
            ),
        };
        let error = RuntimeError::new(code, message).with_endpoint(endpoint);
        if matches!(self, Self::HostNeedsAttention) {
            error
        } else {
            error.retryable()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionOutcome {
    Pending,
    Succeeded,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActionRecord {
    pub id: u64,
    pub method: String,
    pub generation: u64,
    pub outcome: ActionOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RuntimeError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PaneDto {
    pub pane_id: String,
    pub workspace_id: String,
    pub focused: bool,
    pub input_enabled: bool,
    pub input_block: Option<InputBlock>,
    pub target: Option<QualifiedTarget>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub foreground_cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_status: Option<String>,
}

/// One tab of a host, as the engine reports it (spec 039, AC-039-01).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostTabDto {
    pub tab_id: String,
    pub workspace_id: String,
    pub number: usize,
    pub label: String,
    pub custom_label: bool,
    pub focused: bool,
    pub zoomed: bool,
    pub pane_count: usize,
    pub agent_status: String,
}

/// One agent of a host, as the engine reports it (spec 039, AC-039-01).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostAgentDto {
    pub pane_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    pub name: Option<String>,
    pub display_agent: Option<String>,
    pub agent: Option<String>,
    pub title: Option<String>,
    pub terminal_title: Option<String>,
    pub terminal_title_stripped: Option<String>,
    pub agent_status: String,
    pub focused: bool,
}

/// One workspace of a host, as the engine reports it (spec 025, AC-025-01): the same list the
/// TUI shows. `cwd` and `branch` come from the workspace's focused pane (foreground cwd) with
/// the active tab as fallback; `cwd` is the root the desktop groups by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostWorkspaceDto {
    pub workspace_id: String,
    pub number: usize,
    pub label: String,
    pub focused: bool,
    pub tab_count: usize,
    pub pane_count: usize,
    pub active_tab_id: String,
    /// Engine status, snake_case (`working`, `blocked`, `idle`, `done`, `unknown`).
    pub agent_status: String,
    pub cwd: Option<String>,
    /// Every cwd the engine reported for the workspace's panes (cwd and foreground cwd). Spec
    /// 025 AC-025-05: a `cd` inside a pane never hides the workspace behind a closed saved
    /// project. A workspace without panes matches through the primary `cwd`.
    pub cwds: Vec<String>,
    pub branch: Option<String>,
}

/// Projection of the engine's client-shell snapshot into the workspace rows of one host.
pub fn workspace_views(snapshot: &ClientShellSnapshot) -> Vec<HostWorkspaceDto> {
    let mut views: Vec<HostWorkspaceDto> = snapshot
        .workspaces
        .iter()
        .map(|workspace| {
            let focused = snapshot
                .panes
                .iter()
                .find(|pane| pane.workspace_id == workspace.workspace_id && pane.focused);
            let pane_cwd = |pane: &herdr_client::protocol::wire::ClientShellPane| {
                pane.foreground_cwd.clone().or_else(|| pane.cwd.clone())
            };
            let cwd = focused
                .into_iter()
                .chain(snapshot.panes.iter().filter(|pane| {
                    pane.workspace_id == workspace.workspace_id
                        && pane.tab_id == workspace.active_tab_id
                }))
                .chain(
                    snapshot
                        .panes
                        .iter()
                        .filter(|pane| pane.workspace_id == workspace.workspace_id),
                )
                .find_map(pane_cwd)
                .or_else(|| {
                    (!workspace.new_workspace_cwd.is_empty())
                        .then(|| workspace.new_workspace_cwd.clone())
                });
            let mut cwds: Vec<String> = Vec::new();
            for pane in snapshot
                .panes
                .iter()
                .filter(|pane| pane.workspace_id == workspace.workspace_id)
            {
                for root in [pane.foreground_cwd.clone(), pane.cwd.clone()]
                    .into_iter()
                    .flatten()
                {
                    if !cwds.contains(&root) {
                        cwds.push(root);
                    }
                }
            }
            // `new_workspace_cwd` is never added on its own: without runtime cwds it can resolve
            // to the follow policy (e.g. HOME) and would falsely match an unrelated saved root.
            // A workspace with no panes is still matched through the primary `cwd`.
            if let Some(root) = cwd.as_ref() {
                if !cwds.contains(root) {
                    cwds.insert(0, root.clone());
                }
            }
            HostWorkspaceDto {
                workspace_id: workspace.workspace_id.clone(),
                number: workspace.number,
                label: workspace.label.clone(),
                focused: workspace.focused,
                tab_count: snapshot
                    .tabs
                    .iter()
                    .filter(|tab| tab.workspace_id == workspace.workspace_id)
                    .count(),
                pane_count: snapshot
                    .panes
                    .iter()
                    .filter(|pane| pane.workspace_id == workspace.workspace_id)
                    .count(),
                active_tab_id: workspace.active_tab_id.clone(),
                agent_status: serde_json::to_value(workspace.agent_status)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .unwrap_or_else(|| "unknown".into()),
                cwd,
                cwds,
                branch: workspace.branch.clone(),
            }
        })
        .collect();
    views.sort_by_key(|workspace| workspace.number);
    views
}

/// Projection of the engine's client-shell snapshot into the tab rows of one host (spec 039).
pub fn tab_views(snapshot: &ClientShellSnapshot) -> Vec<HostTabDto> {
    snapshot
        .tabs
        .iter()
        .map(|tab| {
            let focused = tab.focused || snapshot.focused_tab_id.as_deref() == Some(&tab.tab_id);
            let pane_count = snapshot
                .panes
                .iter()
                .filter(|pane| pane.tab_id == tab.tab_id)
                .count();
            let agent_status = serde_json::to_value(tab.agent_status)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_else(|| "unknown".into());
            HostTabDto {
                tab_id: tab.tab_id.clone(),
                workspace_id: tab.workspace_id.clone(),
                number: tab.number,
                label: tab.label.clone(),
                custom_label: tab.custom_label,
                focused,
                zoomed: tab.zoomed,
                pane_count,
                agent_status,
            }
        })
        .collect()
}

/// Projection of the engine's client-shell snapshot into the agent list of one host (spec 039).
pub fn agent_views(snapshot: &ClientShellSnapshot) -> Vec<HostAgentDto> {
    snapshot
        .agents
        .iter()
        .map(|agent| {
            let agent_status = serde_json::to_value(agent.agent_status)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_else(|| "unknown".into());
            HostAgentDto {
                pane_id: agent.pane_id.clone(),
                workspace_id: agent.workspace_id.clone(),
                tab_id: agent.tab_id.clone(),
                name: agent.name.clone(),
                display_agent: agent.display_agent.clone(),
                agent: agent.agent.clone(),
                title: agent.title.clone(),
                terminal_title: agent.terminal_title.clone(),
                terminal_title_stripped: agent.terminal_title_stripped.clone(),
                agent_status,
                focused: agent.focused,
            }
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostDto {
    pub endpoint: String,
    pub label: String,
    pub kind: HostKind,
    pub session: String,
    pub target: Option<String>,
    pub visible: bool,
    pub phase: LinkPhase,
    pub phase_label: String,
    pub attempt: u32,
    pub retry_in_ms: Option<u64>,
    pub cancelled: bool,
    pub attention: Option<AttentionReason>,
    pub guidance: Option<String>,
    pub connection_error: Option<RuntimeError>,
    pub action_error: Option<RuntimeError>,
    pub generation: Option<u64>,
    pub boot_id: Option<String>,
    /// Git branch of the workspace of this connection's focused tab, as the engine reported it in
    /// its client shell snapshot (spec 010 status bar); `None` when unknown. Never inferred.
    pub branch: Option<String>,
    /// Server version of the installed connection's negotiated welcome (spec 010 status bar);
    /// `None` without a connection.
    pub server_version: Option<String>,
    /// Remote herdr binary chosen by discovery for the installed connection (spec 029); the
    /// tooltip of the host shows `herdr <versão> · <caminho>`. `None` for Local or offline.
    pub herdr_binary: Option<RemoteBinary>,
    /// Handshake latency in ms measured during connect (spec 011); `None` when unknown or offline.
    pub latency_ms: Option<u64>,
    /// Remote context shown while not online (cache, input disabled).
    pub cached: bool,
    pub api: bool,
    pub surface: Option<SurfaceState>,
    pub screen: Vec<String>,
    pub tabs: Vec<HostTabDto>,
    pub panes: Vec<PaneDto>,
    pub agents: Vec<HostAgentDto>,
    /// The engine's own workspaces (spec 025); cached while offline, empty without a snapshot.
    pub workspaces: Vec<HostWorkspaceDto>,
    pub actions: Vec<ActionRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HubSnapshot {
    pub revision: u64,
    pub hosts: Vec<HostDto>,
}

const MAX_ACTIONS: usize = 16;
const MAX_SCREEN_ROWS: usize = 60;

#[derive(Clone)]
struct SnapshotPaneMeta {
    pane_id: String,
    workspace_id: String,
    tab_id: String,
    focused: bool,
    cwd: Option<String>,
    foreground_cwd: Option<String>,
    title: Option<String>,
    terminal_title: Option<String>,
    agent: Option<String>,
    agent_status: Option<String>,
}

struct SnapshotView {
    boot_id: String,
    revision: u64,
    panes: Vec<SnapshotPaneMeta>,
    /// The engine's own workspace list, projected for the WebView (spec 025). Order by `number`.
    workspaces: Vec<HostWorkspaceDto>,
    tabs: Vec<HostTabDto>,
    tab_ids: Vec<String>,
    agents: Vec<HostAgentDto>,
    /// Tab focused by THIS connection (engine `tab.focus` moves only its source connection).
    focused_tab_id: Option<String>,
    /// Branch of the focused tab's workspace (engine `ClientShellWorkspace.branch`).
    branch: Option<String>,
}

/// Explicit target of one endpoint command, taken from the method's own target field. The hub
/// never substitutes the active pane, tab or workspace for a missing target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndpointScope {
    Pane {
        pane_id: String,
        workspace_id: Option<String>,
    },
    Tab(String),
    Workspace(String),
}

struct HostInner {
    spec: HostSpec,
    link: LinkMachine,
    gateway: Option<Box<dyn RuntimeGateway>>,
    api: Option<Arc<dyn ApiLane>>,
    /// Endpoint command lane of the installed connection (cloned out to wait on the reader).
    endpoint_lane: Option<Arc<dyn EndpointLane>>,
    /// Last geometry requested by the window; re-sent to ask for one full surface.
    geometry: Option<SurfaceGeometry>,
    /// Token of the connection currently installed.
    token: Option<u64>,
    /// Hub-owned connection generation, strictly increasing per host.
    generation: u64,
    snapshot: Option<SnapshotView>,
    store: FrameStore,
    surface_generation: Option<u64>,
    actions: VecDeque<ActionRecord>,
    next_action: u64,
    action_error: Option<RuntimeError>,
    /// Client-side interest of the window in this host's surface (survives reconnection).
    interest: bool,
    /// Capabilities announced by the installed connection (see `install_capabilities`).
    capabilities: Vec<String>,
    /// Server version negotiated by the installed connection (see `install_server_version`).
    server_version: Option<String>,
    /// Remote binary chosen by discovery for the installed connection (see `install_remote_binary`).
    remote_binary: Option<RemoteBinary>,
    /// Handshake latency in ms measured during connect (see `install_latency`).
    latency_ms: Option<u64>,
    pub api_supported: bool,
    /// Bumped by every interest change: acknowledgements of older changes are ignored.
    interest_seq: u64,
    /// Reactivation in progress: presentation and input stay closed until it completes.
    activation: Option<Activation>,
}

/// Evidence gathered while a shown surface waits to be coherent again.
struct Activation {
    seq: u64,
    generation: u64,
    /// Projection floor acknowledged by the server (`Some(0)` for the fallback without the
    /// optimization); `None` while the acknowledgement is pending.
    floor: Option<u64>,
    /// Newest full surface received during the activation, not yet committed.
    candidate: Option<PaneSurfaceFrame>,
    /// A patch was discarded after the candidate: the committed surface must be refreshed.
    patched: bool,
}

/// Endpoint method toggling the client-shell surface lease (endpoint channel only).
pub const SURFACE_INTEREST_METHOD: &str = "client_shell.surface.set";
pub const SURFACE_INTEREST_CAPABILITY: &str = "surface_interest";
pub const PRESENTATION_EFFECTS_FENCE_CAPABILITY: &str = "presentation_effects_fence";

/// Result of one interest change of a host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InterestOutcome {
    pub endpoint: String,
    pub active: bool,
    /// `client_shell.surface.set` was sent (and answered) on the connection.
    pub sent: bool,
    /// The connection announced the method and both capabilities.
    pub supported: bool,
    /// Projection floor acknowledged for an activation.
    pub floor: Option<u64>,
}

impl HostInner {
    /// Frames may be committed, fanned out and accepted as input targets.
    fn presenting(&self) -> bool {
        self.interest && self.activation.is_none()
    }

    fn supports_interest(&self) -> bool {
        let has = |c: &str| self.capabilities.iter().any(|x| x == c);
        has(SURFACE_INTEREST_CAPABILITY)
            && has(PRESENTATION_EFFECTS_FENCE_CAPABILITY)
            && self
                .endpoint_lane
                .as_ref()
                .is_some_and(|lane| lane.methods().iter().any(|m| m == SURFACE_INTEREST_METHOD))
    }

    /// Commits the activation candidate once acknowledgement floor, snapshot of the same boot
    /// and projection, and current geometry all agree. Returns the notices to deliver.
    fn try_complete_activation(&mut self) -> Vec<HostNotice> {
        let Some(activation) = self.activation.as_ref() else {
            return Vec::new();
        };
        let (Some(floor), Some(candidate), Some(snapshot), Some(live)) = (
            activation.floor,
            activation.candidate.as_ref(),
            self.snapshot.as_ref(),
            self.live_identity(),
        ) else {
            return Vec::new();
        };
        let geometry_ok = self
            .geometry
            .is_none_or(|g| candidate.frame.width == g.cols && candidate.frame.height == g.rows);
        if activation.generation != self.generation
            || candidate.boot_id != live.boot_id
            || snapshot.boot_id != live.boot_id
            || candidate.projection_revision < floor
            || snapshot.revision != candidate.projection_revision
            || !geometry_ok
        {
            return Vec::new();
        }
        let activation = self.activation.take().expect("checked");
        let candidate = activation.candidate.expect("checked");
        self.store.set_interest(true);
        let mut notices = Vec::new();
        match self.store.apply_full(candidate) {
            Ok(_) => {
                self.surface_generation = Some(self.generation);
                if activation.patched {
                    self.store.mark_stale(StaleReason::RevisionGap);
                }
                if let Some(frame) = self.store.surface() {
                    notices.push(HostNotice::Full(Box::new(frame.clone())));
                }
                if activation.patched {
                    notices.push(HostNotice::Stale(StaleReason::RevisionGap));
                }
            }
            Err(reason) => notices.push(HostNotice::Stale(reason)),
        }
        notices
    }

    fn origin(&self) -> NoticeOrigin {
        NoticeOrigin {
            endpoint: self.spec.endpoint.clone(),
            connection_generation: self.generation,
            identity: self.live_identity(),
        }
    }

    fn live_identity(&self) -> Option<LiveIdentity> {
        let gateway = self.gateway.as_ref()?;
        let mut live = gateway.identity()?;
        live.connection_generation = self.generation;
        Some(live)
    }

    fn gate(&self, pane_id: &str) -> Result<QualifiedTarget, InputBlock> {
        match self.link.phase() {
            LinkPhase::Online => {}
            LinkPhase::Offline => return Err(InputBlock::HostOffline),
            LinkPhase::Connecting => return Err(InputBlock::HostConnecting),
            LinkPhase::Reconnecting => return Err(InputBlock::HostReconnecting),
            LinkPhase::Attention => return Err(InputBlock::HostNeedsAttention),
        }
        let snapshot = self.snapshot.as_ref().ok_or(InputBlock::NoSnapshot)?;
        let live = self.live_identity().ok_or(InputBlock::NoSnapshot)?;
        if live.boot_id != snapshot.boot_id {
            return Err(InputBlock::NoSnapshot);
        }
        let pane = snapshot
            .panes
            .iter()
            .find(|p| p.pane_id == pane_id)
            .ok_or(InputBlock::PaneNotInSnapshot)?;
        if !self.spec.visible || !self.presenting() {
            return Err(InputBlock::SurfaceHidden);
        }
        let surface = self.store.surface().ok_or(InputBlock::NoSurface)?;
        if self.surface_generation != Some(self.generation) {
            return Err(InputBlock::SurfaceFromPreviousConnection);
        }
        if surface.boot_id != snapshot.boot_id {
            return Err(InputBlock::SurfaceFromPreviousBoot);
        }
        if !self.store.input_allowed() {
            return Err(InputBlock::SurfaceStale);
        }
        Ok(QualifiedTarget::new(
            &live,
            Some(pane.workspace_id.clone()),
            pane_id.to_owned(),
        ))
    }

    fn drop_connection(&mut self) {
        if let Some(mut gateway) = self.gateway.take() {
            gateway.detach();
        }
        self.api = None;
        self.endpoint_lane = None;
        self.capabilities.clear();
        self.server_version = None;
        self.remote_binary = None;
        self.latency_ms = None;
        self.activation = None;
        self.token = None;
        if self.store.surface().is_some() {
            self.store.mark_stale(StaleReason::Disconnected);
        }
        for record in self.actions.iter_mut() {
            if record.outcome == ActionOutcome::Pending {
                record.outcome = ActionOutcome::Unknown;
            }
        }
    }

    fn record(&mut self, method: &str) -> u64 {
        let id = self.next_action;
        self.next_action += 1;
        self.actions.push_front(ActionRecord {
            id,
            method: method.to_owned(),
            generation: self.generation,
            outcome: ActionOutcome::Pending,
            error: None,
        });
        self.actions.truncate(MAX_ACTIONS);
        id
    }

    fn dto(&self, now: Instant) -> HostDto {
        let phase = self.link.phase();
        let panes = self
            .snapshot
            .as_ref()
            .map(|s| {
                s.panes
                    .iter()
                    .map(|p| {
                        let gate = self.gate(&p.pane_id);
                        PaneDto {
                            pane_id: p.pane_id.clone(),
                            workspace_id: p.workspace_id.clone(),
                            tab_id: Some(p.tab_id.clone()),
                            cwd: p.cwd.clone(),
                            foreground_cwd: p.foreground_cwd.clone(),
                            title: p.title.clone(),
                            terminal_title: p.terminal_title.clone(),
                            agent: p.agent.clone(),
                            agent_status: p.agent_status.clone(),
                            focused: p.focused,
                            input_enabled: gate.is_ok(),
                            input_block: gate.as_ref().err().copied(),
                            target: gate.ok(),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let screen = if self.spec.visible {
            let mut rows = self.store.text_rows();
            rows.truncate(MAX_SCREEN_ROWS);
            rows.into_iter().map(|r| r.trim_end().to_owned()).collect()
        } else {
            Vec::new()
        };
        HostDto {
            endpoint: self.spec.endpoint.clone(),
            label: self.spec.label.clone(),
            kind: self.spec.kind,
            session: self.spec.session.clone(),
            target: self.spec.target.clone(),
            visible: self.spec.visible,
            phase,
            phase_label: phase.label().to_owned(),
            attempt: self.link.attempt(),
            retry_in_ms: self
                .link
                .retry_at()
                .map(|at| at.saturating_duration_since(now).as_millis() as u64),
            cancelled: self.link.cancelled(),
            attention: self.link.attention(),
            guidance: self.link.attention().map(|r| r.guidance().to_owned()),
            connection_error: if phase == LinkPhase::Online {
                None
            } else {
                self.link.error().cloned()
            },
            action_error: self.action_error.clone(),
            generation: (self.generation > 0).then_some(self.generation),
            boot_id: self.snapshot.as_ref().map(|s| s.boot_id.clone()),
            branch: self.snapshot.as_ref().and_then(|s| s.branch.clone()),
            server_version: self.server_version.clone(),
            herdr_binary: self.remote_binary.clone(),
            latency_ms: (phase == LinkPhase::Online)
                .then_some(self.latency_ms)
                .flatten(),
            cached: phase != LinkPhase::Online && self.store.surface().is_some(),
            api: match self.spec.kind {
                HostKind::Local => true,
                HostKind::Ssh => self.api_supported,
            },
            surface: self.store.surface().map(|_| self.store.state()),
            screen,
            tabs: self
                .snapshot
                .as_ref()
                .map(|s| s.tabs.clone())
                .unwrap_or_default(),
            panes,
            agents: self
                .snapshot
                .as_ref()
                .map(|s| s.agents.clone())
                .unwrap_or_default(),
            workspaces: self
                .snapshot
                .as_ref()
                .map(|snapshot| snapshot.workspaces.clone())
                .unwrap_or_default(),
            actions: self.actions.iter().cloned().collect(),
        }
    }
}

struct HostSlot {
    inner: Mutex<HostInner>,
    /// Serializes interest changes of the host (decision and request in wire order). Never
    /// taken by the event reader.
    interest_turn: Mutex<()>,
}

/// All hosts of the window.
pub struct HostHub {
    order: RwLock<Vec<String>>,
    hosts: RwLock<BTreeMap<String, Arc<HostSlot>>>,
    revision: Mutex<u64>,
    changed: Condvar,
    observer: RwLock<Option<Arc<dyn HubObserver>>>,
}

impl Default for HostHub {
    fn default() -> Self {
        Self::new()
    }
}

fn unknown_endpoint(endpoint: &str) -> RuntimeError {
    RuntimeError::new("endpoint_unknown", "unknown endpoint; no action was sent")
        .with_endpoint(endpoint)
}

impl HostHub {
    pub fn new() -> Self {
        Self {
            order: RwLock::new(Vec::new()),
            hosts: RwLock::new(BTreeMap::new()),
            revision: Mutex::new(1),
            changed: Condvar::new(),
            observer: RwLock::new(None),
        }
    }

    fn slot(&self, endpoint: &str) -> Result<Arc<HostSlot>, RuntimeError> {
        self.hosts
            .read()
            .expect("hosts lock")
            .get(endpoint)
            .cloned()
            .ok_or_else(|| unknown_endpoint(endpoint))
    }

    fn bump(&self) {
        let mut revision = self.revision.lock().expect("revision lock");
        *revision += 1;
        self.changed.notify_all();
    }

    pub fn revision(&self) -> u64 {
        *self.revision.lock().expect("revision lock")
    }

    /// Blocks until the revision differs from `since` or `timeout` elapses.
    pub fn wait_changed(&self, since: u64, timeout: Duration) -> u64 {
        let guard = self.revision.lock().expect("revision lock");
        let (guard, _) = self
            .changed
            .wait_timeout_while(guard, timeout, |r| *r == since)
            .expect("revision lock");
        *guard
    }

    pub fn add_host(&self, spec: HostSpec) -> Result<(), RuntimeError> {
        let mut hosts = self.hosts.write().expect("hosts lock");
        if hosts.contains_key(&spec.endpoint) {
            return Err(RuntimeError::new(
                "endpoint_duplicate",
                "a host with this identifier already exists",
            )
            .with_endpoint(spec.endpoint));
        }
        let endpoint = spec.endpoint.clone();
        hosts.insert(
            endpoint.clone(),
            Arc::new(HostSlot {
                inner: Mutex::new(HostInner {
                    spec,
                    link: LinkMachine::new(),
                    gateway: None,
                    api: None,
                    endpoint_lane: None,
                    geometry: None,
                    token: None,
                    generation: 0,
                    snapshot: None,
                    store: FrameStore::new(),
                    surface_generation: None,
                    actions: VecDeque::new(),
                    next_action: 1,
                    action_error: None,
                    interest: true,
                    capabilities: Vec::new(),
                    server_version: None,
                    remote_binary: None,
                    latency_ms: None,
                    api_supported: true,
                    interest_seq: 0,
                    activation: None,
                }),
                interest_turn: Mutex::new(()),
            }),
        );
        self.order.write().expect("order lock").push(endpoint);
        drop(hosts);
        self.bump();
        Ok(())
    }

    /// Replaces label/target/session of a host that is not connected.
    pub fn update_host(&self, spec: HostSpec) -> Result<(), RuntimeError> {
        let slot = self.slot(&spec.endpoint)?;
        let mut inner = slot.inner.lock().expect("host lock");
        if inner.gateway.is_some() || inner.link.in_flight().is_some() {
            return Err(RuntimeError::new(
                "profile_in_use",
                "cancel the connection before changing the profile",
            )
            .with_endpoint(spec.endpoint));
        }
        inner.spec = spec;
        drop(inner);
        self.bump();
        Ok(())
    }

    /// Whether the host has a connection or an attempt in flight.
    pub fn is_busy(&self, endpoint: &str) -> bool {
        self.slot(endpoint).is_ok_and(|slot| {
            let inner = slot.inner.lock().expect("host lock");
            inner.gateway.is_some() || inner.link.in_flight().is_some()
        })
    }

    /// Shows (surface) or hides (metadata-only) a host (spec 035).
    ///
    /// An online connection that announced `surface_interest` only changes its lease in place,
    /// like the TUI: the bridge, its generation, the snapshot and the agents stay, and the
    /// requested interest is sent on that connection. Every other host keeps the old behavior:
    /// an online connection is dropped and the returned ticket must be executed (renegotiation);
    /// an offline host just records the mode for its next connection (the interest is sent once
    /// it is online).
    pub fn set_visible(
        &self,
        endpoint: &str,
        visible: bool,
        now: Instant,
    ) -> Result<VisibilityOutcome, RuntimeError> {
        let slot = self.slot(endpoint)?;
        let mut invalidated = None;
        let mut renegotiate = None;
        {
            let mut inner = slot.inner.lock().expect("host lock");
            if inner.spec.visible == visible {
                return Ok(VisibilityOutcome::Unchanged);
            }
            inner.spec.visible = visible;
            if inner.gateway.is_some() && !inner.supports_interest() {
                invalidated = Some(inner.origin());
                inner.drop_connection();
                renegotiate = inner.link.renegotiate(now).map(|token| ConnectTicket {
                    endpoint: endpoint.to_owned(),
                    token,
                    surface_active: visible,
                });
            }
        }
        self.bump();
        if let Some(origin) = invalidated {
            self.deliver(&origin, vec![renegotiating(endpoint)]);
        }
        if let Some(ticket) = renegotiate {
            return Ok(VisibilityOutcome::Renegotiate(ticket));
        }
        self.set_surface_interest(endpoint, visible)
            .map(VisibilityOutcome::Interest)
    }

    pub fn spec(&self, endpoint: &str) -> Result<HostSpec, RuntimeError> {
        Ok(self
            .slot(endpoint)?
            .inner
            .lock()
            .expect("host lock")
            .spec
            .clone())
    }

    /// Observer of applied frames and link notices; always invoked without any hub lock.
    pub fn set_observer(&self, observer: Arc<dyn HubObserver>) {
        *self.observer.write().expect("observer lock") = Some(observer);
    }

    fn observer(&self) -> Option<Arc<dyn HubObserver>> {
        self.observer.read().expect("observer lock").clone()
    }

    fn deliver(&self, origin: &NoticeOrigin, notices: Vec<HostNotice>) {
        if notices.is_empty() {
            return;
        }
        if let Some(observer) = self.observer() {
            for notice in &notices {
                observer.notice(origin, notice);
            }
        }
    }

    /// Installs the endpoint command lane of the connection identified by `token`. A lane of a
    /// superseded or dropped connection is refused.
    pub fn install_endpoint_lane(
        &self,
        endpoint: &str,
        token: u64,
        lane: Arc<dyn EndpointLane>,
    ) -> bool {
        let Ok(slot) = self.slot(endpoint) else {
            return false;
        };
        let mut inner = slot.inner.lock().expect("host lock");
        if inner.token != Some(token) || inner.gateway.is_none() {
            return false;
        }
        inner.endpoint_lane = Some(lane);
        true
    }

    /// Records the capabilities negotiated by the connection identified by `token`. Refused for a
    /// superseded or dropped connection.
    pub fn install_capabilities(
        &self,
        endpoint: &str,
        token: u64,
        capabilities: Vec<String>,
    ) -> bool {
        let Ok(slot) = self.slot(endpoint) else {
            return false;
        };
        let mut inner = slot.inner.lock().expect("host lock");
        if inner.token != Some(token) || inner.gateway.is_none() {
            return false;
        }
        inner.capabilities = capabilities;
        true
    }

    /// Records the server version negotiated by the connection identified by `token`. Refused for
    /// a superseded or dropped connection.
    pub fn install_server_version(&self, endpoint: &str, token: u64, version: String) -> bool {
        let Ok(slot) = self.slot(endpoint) else {
            return false;
        };
        let mut inner = slot.inner.lock().expect("host lock");
        if inner.token != Some(token) || inner.gateway.is_none() {
            return false;
        }
        inner.server_version = Some(version);
        true
    }

    /// Records the remote binary chosen by discovery for the connection identified by `token`
    /// (spec 029). Refused for a superseded or dropped connection.
    pub fn install_remote_binary(&self, endpoint: &str, token: u64, binary: RemoteBinary) -> bool {
        let Ok(slot) = self.slot(endpoint) else {
            return false;
        };
        let mut inner = slot.inner.lock().expect("host lock");
        if inner.token != Some(token) || inner.gateway.is_none() {
            return false;
        }
        inner.remote_binary = Some(binary);
        true
    }

    /// Records the handshake latency in ms measured by the connection identified by `token`. Refused
    /// for a superseded or dropped connection.
    pub fn install_latency(&self, endpoint: &str, token: u64, latency_ms: u64) -> bool {
        let Ok(slot) = self.slot(endpoint) else {
            return false;
        };
        let mut inner = slot.inner.lock().expect("host lock");
        if inner.token != Some(token) || inner.gateway.is_none() {
            return false;
        }
        inner.latency_ms = Some(latency_ms);
        true
    }

    pub fn install_api_supported(&self, endpoint: &str, token: u64, supported: bool) -> bool {
        let Ok(slot) = self.slot(endpoint) else {
            return false;
        };
        let mut inner = slot.inner.lock().expect("host lock");
        if inner.token != Some(token) || inner.gateway.is_none() {
            return false;
        }
        inner.api_supported = supported;
        true
    }

    pub fn set_api_supported(&self, endpoint: &str, supported: bool) -> bool {
        let Ok(slot) = self.slot(endpoint) else {
            return false;
        };
        let mut inner = slot.inner.lock().expect("host lock");
        inner.api_supported = supported;
        true
    }

    /// Changes the window's interest in the host's surface.
    ///
    /// Hiding closes input and stops committing/fanning out Full/Patch before any network
    /// wait, keeps the connection, snapshot and agents, and sends `client_shell.surface.set
    /// {active:false}` once when the connection announced the optimization. Showing re-sends the
    /// current geometry, sends `{active:true}` (same condition) and keeps presentation and input
    /// closed until the acknowledged floor, a snapshot of the same boot/projection and a full
    /// surface of the current geometry agree. Repeating the current interest sends nothing.
    /// Interest changes of one host are serialized (wire order); the request is awaited with no
    /// hub lock held. Errors are returned as they are and never retried.
    pub fn set_surface_interest(
        &self,
        endpoint: &str,
        active: bool,
    ) -> Result<InterestOutcome, RuntimeError> {
        let slot = self.slot(endpoint)?;
        let _turn = slot
            .interest_turn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut outcome = InterestOutcome {
            endpoint: endpoint.to_owned(),
            active,
            sent: false,
            supported: false,
            floor: None,
        };
        let (seq, request, geometry) = {
            let mut inner = slot.inner.lock().expect("host lock");
            outcome.supported = inner.supports_interest();
            if inner.interest == active && inner.activation.is_none() {
                return Ok(outcome);
            }
            inner.interest = active;
            inner.interest_seq += 1;
            let seq = inner.interest_seq;
            inner.store.set_interest(false);
            inner.activation = active.then(|| Activation {
                seq,
                generation: inner.generation,
                floor: (!outcome.supported).then_some(0),
                candidate: None,
                patched: false,
            });
            let request = match (
                outcome.supported,
                inner.live_identity(),
                inner.endpoint_lane.clone(),
            ) {
                (true, Some(live), Some(lane)) => Some((live, lane)),
                _ => None,
            };
            (seq, request, inner.geometry)
        };
        self.bump();
        if active {
            if let Some(geometry) = geometry {
                // A failed write means the transport is gone; its Disconnected event follows.
                let _ = self.resize(endpoint, geometry);
            }
        }
        let Some((live, lane)) = request else {
            return Ok(outcome);
        };
        let reply = lane
            .request(
                &live.boot_id,
                SURFACE_INTEREST_METHOD,
                serde_json::json!({ "active": active }),
            )
            .map_err(|error| error.with_endpoint(endpoint));
        outcome.sent = true;
        let floor = reply.and_then(|value| interest_floor(endpoint, &value, active))?;
        if !active {
            return Ok(outcome);
        }
        outcome.floor = Some(floor);
        let mut inner = slot.inner.lock().expect("host lock");
        let current = inner.live_identity().is_some_and(|now| now == live);
        let notices = match inner.activation.as_mut() {
            Some(activation)
                if current
                    && activation.seq == seq
                    && activation.generation == live.connection_generation =>
            {
                activation.floor = Some(floor);
                inner.try_complete_activation()
            }
            _ => {
                return Err(RuntimeError::new(
                    "surface_interest_superseded",
                    "the connection or the interest changed before the confirmation; nothing was reopened",
                )
                .with_endpoint(endpoint));
            }
        };
        let origin = inner.origin();
        drop(inner);
        self.bump();
        self.deliver(&origin, notices);
        Ok(outcome)
    }

    /// After a new connection `token` installed its lane and capabilities: a hidden window
    /// tells that connection (negotiated with an active surface) once that it is not interested.
    pub fn restore_surface_interest(
        &self,
        endpoint: &str,
        token: u64,
    ) -> Result<Option<InterestOutcome>, RuntimeError> {
        let slot = self.slot(endpoint)?;
        let _turn = slot
            .interest_turn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (live, lane) = {
            let inner = slot.inner.lock().expect("host lock");
            if inner.token != Some(token) || inner.interest || !inner.supports_interest() {
                return Ok(None);
            }
            match (inner.live_identity(), inner.endpoint_lane.clone()) {
                (Some(live), Some(lane)) => (live, lane),
                _ => return Ok(None),
            }
        };
        let reply = lane
            .request(
                &live.boot_id,
                SURFACE_INTEREST_METHOD,
                serde_json::json!({ "active": false }),
            )
            .map_err(|error| error.with_endpoint(endpoint))?;
        interest_floor(endpoint, &reply, false)?;
        Ok(Some(InterestOutcome {
            endpoint: endpoint.to_owned(),
            active: false,
            sent: true,
            supported: true,
            floor: None,
        }))
    }

    /// Records the window geometry of the host and sends it to the installed connection. The
    /// stored geometry is what a recovery re-sends to request one full surface.
    pub fn resize(&self, endpoint: &str, geometry: SurfaceGeometry) -> Result<(), RuntimeError> {
        let slot = self.slot(endpoint)?;
        let mut inner = slot.inner.lock().expect("host lock");
        inner.geometry = Some(geometry);
        match inner.gateway.as_ref() {
            Some(gateway) => gateway.resize(geometry),
            None => Ok(()),
        }
    }

    /// Geometry last requested for the host (used for the hello of a new connection).
    pub fn geometry(&self, endpoint: &str) -> Option<SurfaceGeometry> {
        let slot = self.slot(endpoint).ok()?;
        let inner = slot.inner.lock().expect("host lock");
        inner.geometry
    }

    /// Committed surface of a visible host and its state; hidden hosts keep none.
    pub fn surface(&self, endpoint: &str) -> Option<(PaneSurfaceFrame, SurfaceState)> {
        let slot = self.slot(endpoint).ok()?;
        let inner = slot.inner.lock().expect("host lock");
        if !inner.spec.visible || !inner.presenting() {
            return None;
        }
        let frame = inner.store.surface()?.clone();
        Some((frame, inner.store.state()))
    }

    /// Explicit connection (or retry after configuring).
    pub fn request_connect(
        &self,
        endpoint: &str,
        now: Instant,
    ) -> Result<Option<ConnectTicket>, RuntimeError> {
        let slot = self.slot(endpoint)?;
        let mut inner = slot.inner.lock().expect("host lock");
        let ticket = inner.link.request(now).map(|token| ConnectTicket {
            endpoint: endpoint.to_owned(),
            token,
            surface_active: inner.spec.visible,
        });
        drop(inner);
        if ticket.is_some() {
            self.bump();
        }
        Ok(ticket)
    }

    /// Automatic retries that are due.
    pub fn due_connects(&self, now: Instant) -> Vec<ConnectTicket> {
        let slots: Vec<Arc<HostSlot>> = self
            .hosts
            .read()
            .expect("hosts lock")
            .values()
            .cloned()
            .collect();
        let mut tickets = Vec::new();
        for slot in slots {
            let mut inner = slot.inner.lock().expect("host lock");
            if let Some(token) = inner.link.start_retry(now) {
                tickets.push(ConnectTicket {
                    endpoint: inner.spec.endpoint.clone(),
                    token,
                    surface_active: inner.spec.visible,
                });
            }
        }
        if !tickets.is_empty() {
            self.bump();
        }
        tickets
    }

    /// Installs the result of an attempt. Returns the event stream of the new connection;
    /// results of cancelled/superseded attempts are detached and dropped.
    pub fn finish_connect(
        &self,
        ticket: &ConnectTicket,
        result: Result<Connected, ConnectFailure>,
        now: Instant,
    ) -> Option<Receiver<GatewayEvent>> {
        let Ok(slot) = self.slot(&ticket.endpoint) else {
            if let Ok(mut connected) = result {
                connected.gateway.detach();
            }
            return None;
        };
        let mut inner = slot.inner.lock().expect("host lock");
        let mut notices = Vec::new();
        let events = match result {
            Ok(mut connected) => {
                if !inner.link.succeeded(ticket.token, now) {
                    connected.gateway.detach();
                    return None;
                }
                inner.drop_connection();
                inner.generation += 1;
                inner.snapshot = None;
                // The connection negotiated its own surface mode; a hidden window keeps the
                // renderer closed and re-sends its interest once capabilities are installed.
                let interest = inner.interest;
                inner.store.set_interest(interest);
                inner.token = Some(ticket.token);
                let events = connected.gateway.take_events();
                inner.gateway = Some(connected.gateway);
                inner.api = Some(connected.api);
                notices.push(HostNotice::Connected {
                    generation: inner.generation,
                });
                events
            }
            Err(failure) => {
                if !inner.link.failed(ticket.token, &failure, now) {
                    return None;
                }
                if let Some(error) = inner.link.error().cloned() {
                    notices.push(HostNotice::Failed(error));
                }
                None
            }
        };
        let origin = inner.origin();
        drop(inner);
        self.bump();
        self.deliver(&origin, notices);
        events
    }

    /// Applies one gateway event of the connection identified by `token`.
    pub fn apply_event(&self, endpoint: &str, token: u64, event: GatewayEvent, now: Instant) {
        let Ok(slot) = self.slot(endpoint) else {
            return;
        };
        let mut inner = slot.inner.lock().expect("host lock");
        if inner.token != Some(token) {
            return;
        }
        let observed = self.observer().is_some();
        // Origin before the event: a loss drops the connection it belongs to.
        let before = inner.origin();
        let mut notices = Vec::new();
        let changed = match event {
            GatewayEvent::Snapshot(snapshot) => {
                inner.snapshot = Some(snapshot_view(&snapshot));
                notices.push(HostNotice::Snapshot);
                notices.extend(inner.try_complete_activation());
                true
            }
            GatewayEvent::Surface(frame) => {
                if !inner.spec.visible || !inner.interest {
                    false
                } else if let Some(activation) = inner.activation.as_mut() {
                    let newer = activation.candidate.as_ref().is_none_or(|current| {
                        frame.projection_revision > current.projection_revision
                            || (frame.projection_revision == current.projection_revision
                                && frame.surface_revision >= current.surface_revision)
                    });
                    if newer {
                        activation.candidate = Some(*frame);
                        activation.patched = false;
                    }
                    notices.extend(inner.try_complete_activation());
                    true
                } else {
                    match inner.store.apply_full(*frame) {
                        Ok(_) => {
                            inner.surface_generation = Some(inner.generation);
                            if observed {
                                if let Some(frame) = inner.store.surface() {
                                    notices.push(HostNotice::Full(Box::new(frame.clone())));
                                }
                            }
                        }
                        Err(reason) => notices.push(HostNotice::Stale(reason)),
                    }
                    true
                }
            }
            GatewayEvent::Patch(patch) => {
                if !inner.spec.visible || !inner.interest {
                    false
                } else if let Some(activation) = inner.activation.as_mut() {
                    activation.patched |= activation.candidate.is_some();
                    false
                } else {
                    // A rejected patch leaves the surface stale until a full frame arrives;
                    // only applied patches reach the renderer.
                    let copy = observed.then(|| patch.clone());
                    match inner.store.apply_patch(*patch) {
                        ApplyOutcome::Applied => notices.extend(copy.map(HostNotice::Patch)),
                        ApplyOutcome::Rejected(reason) => notices.push(HostNotice::Stale(reason)),
                    }
                    true
                }
            }
            GatewayEvent::QueueOverflow { .. } => {
                inner.store.mark_stale(StaleReason::QueueOverflow);
                if inner.presenting() {
                    notices.push(HostNotice::Stale(StaleReason::QueueOverflow));
                } else if let Some(activation) = inner.activation.as_mut() {
                    // Frames were lost: the candidate may be behind the server.
                    activation.patched |= activation.candidate.is_some();
                }
                true
            }
            GatewayEvent::Disconnected(error) => {
                let error = error.with_endpoint(endpoint);
                inner.drop_connection();
                inner.link.lost(error.clone(), now);
                notices.push(HostNotice::Lost(error));
                true
            }
            GatewayEvent::Shutdown(_) => {
                let error = RuntimeError::new("server_shutdown", "the Herdr server was shut down")
                    .retryable()
                    .with_endpoint(endpoint);
                inner.drop_connection();
                inner.link.lost(error.clone(), now);
                notices.push(HostNotice::Lost(error));
                true
            }
            _ => false,
        };
        // One full surface per stale episode: re-send the stored geometry on the live
        // connection. Refused input is never queued, so nothing is replayed afterwards.
        if inner.spec.visible
            && inner.presenting()
            && inner.gateway.is_some()
            && inner.geometry.is_some()
            && inner.store.take_recovery_request()
        {
            let geometry = inner.geometry.expect("checked");
            // A failed write means the transport is gone: its Disconnected event follows and
            // the next connection starts with a full surface.
            let _ = inner.gateway.as_ref().expect("checked").resize(geometry);
        }
        let origin = if inner.gateway.is_some() {
            inner.origin()
        } else {
            before
        };
        drop(inner);
        if changed {
            self.bump();
        }
        self.deliver(&origin, notices);
    }

    /// After `finish_connect` of `ticket`: when the host's visibility changed while the attempt
    /// was in flight, the installed connection carries the old surface mode. It is dropped and
    /// renegotiated with the current one (ticket returned for the caller to run), so an older
    /// selection transition can never leave a hidden host with a surface or the selected host
    /// metadata-only. Nothing happens for any other connection.
    pub fn settle_visibility(&self, ticket: &ConnectTicket) -> Option<ConnectTicket> {
        let slot = self.slot(&ticket.endpoint).ok()?;
        let mut inner = slot.inner.lock().expect("host lock");
        if inner.token != Some(ticket.token)
            || inner.gateway.is_none()
            || inner.spec.visible == ticket.surface_active
        {
            return None;
        }
        let origin = inner.origin();
        inner.drop_connection();
        let renegotiated = inner
            .link
            .renegotiate(Instant::now())
            .map(|token| ConnectTicket {
                endpoint: ticket.endpoint.clone(),
                token,
                surface_active: inner.spec.visible,
            });
        drop(inner);
        self.bump();
        self.deliver(&origin, vec![renegotiating(&ticket.endpoint)]);
        renegotiated
    }

    /// Cancels attempts and retries of one host and detaches its client connection. The
    /// cached screen stays; the remote server and its sessions are not touched.
    pub fn cancel(&self, endpoint: &str) -> Result<(), RuntimeError> {
        let slot = self.slot(endpoint)?;
        let mut inner = slot.inner.lock().expect("host lock");
        let active = inner.gateway.is_some() || inner.link.in_flight().is_some();
        let origin = inner.origin();
        inner.link.cancel();
        inner.drop_connection();
        drop(inner);
        self.bump();
        if active {
            let error = RuntimeError::new(
                "connection_cancelled",
                "the connection was cancelled; the server and its sessions keep running",
            )
            .with_endpoint(endpoint);
            self.deliver(&origin, vec![HostNotice::Invalidated(error)]);
        }
        Ok(())
    }

    /// Explicit disconnect of one host (spec 029, AC-029-03): cancels the attempts and retries,
    /// detaches only the desktop connection (the remote server, its sessions and processes keep
    /// running), drops the snapshot so the host's workspaces leave the tree, and keeps the last
    /// screen as cache. Unlike an unexpected loss, nothing is retried on its own.
    pub fn disconnect(&self, endpoint: &str) -> Result<(), RuntimeError> {
        let slot = self.slot(endpoint)?;
        let mut inner = slot.inner.lock().expect("host lock");
        let active = inner.gateway.is_some() || inner.link.in_flight().is_some();
        let origin = inner.origin();
        inner.link.cancel();
        inner.drop_connection();
        inner.snapshot = None;
        drop(inner);
        self.bump();
        if active {
            let error = RuntimeError::new(
                "connection_closed",
                "disconnected from this host; the server and its sessions keep running",
            )
            .with_endpoint(endpoint);
            self.deliver(&origin, vec![HostNotice::Invalidated(error)]);
        }
        Ok(())
    }

    /// Removes one host from the hub after its saved profile is gone (spec 029, AC-029-03). The
    /// connection is detached; a late result of an attempt in flight finds no host and is
    /// detached by `finish_connect`.
    pub fn remove_host(&self, endpoint: &str) -> Result<(), RuntimeError> {
        let slot = self.slot(endpoint)?;
        {
            let mut inner = slot.inner.lock().expect("host lock");
            inner.link.cancel();
            inner.drop_connection();
            inner.snapshot = None;
        }
        self.hosts.write().expect("hosts lock").remove(endpoint);
        self.order
            .write()
            .expect("order lock")
            .retain(|candidate| candidate != endpoint);
        self.bump();
        Ok(())
    }

    /// Input for the engine's confirmed focused pane of the connection `expected`. Identity, focus
    /// (snapshot of the same boot), the input gate of that pane, the conversion of `events` against
    /// the pane of the committed surface and the send all happen under one host lock, so a
    /// snapshot moving the focus can never interleave. Nothing is queued on refusal. Explicit-pane
    /// callers keep [`HostHub::send_input`].
    pub fn send_focused_input(
        &self,
        expected: &LiveIdentity,
        pane_id: &str,
        convert: impl FnOnce(
            Option<&herdr_client::protocol::wire::PaneSurfacePane>,
        ) -> Result<Vec<ClientPaneInputEvent>, RuntimeError>,
    ) -> Result<(), RuntimeError> {
        let endpoint = expected.endpoint.as_str();
        let slot = self.slot(endpoint)?;
        let inner = slot.inner.lock().expect("host lock");
        current_identity(&inner, expected)?;
        let focused = inner
            .snapshot
            .as_ref()
            .filter(|snapshot| snapshot.boot_id == expected.boot_id)
            .and_then(|snapshot| snapshot.panes.iter().find(|p| p.focused))
            .is_some_and(|p| p.pane_id == pane_id);
        if !focused {
            return Err(RuntimeError::new(
                "pane_not_focused",
                "the pane is not the focus confirmed by the server; nothing was sent",
            )
            .with_endpoint(endpoint));
        }
        let mut target = inner.gate(pane_id).map_err(|block| block.error(endpoint))?;
        let pane = inner
            .store
            .surface()
            .filter(|frame| frame.boot_id == expected.boot_id)
            .and_then(|frame| frame.panes.iter().find(|p| p.pane_id == pane_id));
        let events = convert(pane)?;
        let gateway = inner.gateway.as_ref().expect("gate implies gateway");
        target.connection_generation = gateway
            .identity()
            .map(|i| i.connection_generation)
            .unwrap_or_default();
        gateway.send_input(&target, events)
    }

    /// Target for input to `pane_id`, or why input is blocked.
    pub fn input_gate(&self, endpoint: &str, pane_id: &str) -> Result<QualifiedTarget, InputBlock> {
        let slot = self.slot(endpoint).map_err(|_| InputBlock::HostOffline)?;
        let inner = slot.inner.lock().expect("host lock");
        inner.gate(pane_id)
    }

    /// Routes input by the target's endpoint only. Nothing is queued for later delivery.
    pub fn send_input(
        &self,
        target: &QualifiedTarget,
        events: Vec<ClientPaneInputEvent>,
    ) -> Result<(), RuntimeError> {
        let slot = self.slot(&target.endpoint)?;
        let inner = slot.inner.lock().expect("host lock");
        let current = inner
            .gate(&target.pane_id)
            .map_err(|block| block.error(&target.endpoint))?;
        let live = inner
            .live_identity()
            .ok_or_else(|| InputBlock::NoSnapshot.error(&target.endpoint))?;
        target.validate(&live)?;
        let gateway = inner.gateway.as_ref().expect("gate implies gateway");
        let mut inner_target = current;
        inner_target.connection_generation = gateway
            .identity()
            .map(|i| i.connection_generation)
            .unwrap_or_default();
        gateway.send_input(&inner_target, events)
    }

    /// One clipboard image to the engine's confirmed focused pane of the connection `expected`.
    /// Same checks as [`HostHub::send_focused_input`] (identity, confirmed focus of the snapshot,
    /// input gate), plus the caller `admit`, all under one host lock; the image then goes out as
    /// one `ClipboardImage` client message, exactly as the TUI bridges local images. Nothing is
    /// queued or replayed on refusal.
    pub fn send_focused_clipboard_image(
        &self,
        expected: &LiveIdentity,
        pane_id: &str,
        extension: &str,
        data: Vec<u8>,
        admit: impl FnOnce() -> Result<(), RuntimeError>,
    ) -> Result<(), RuntimeError> {
        let endpoint = expected.endpoint.as_str();
        let slot = self.slot(endpoint)?;
        let inner = slot.inner.lock().expect("host lock");
        current_identity(&inner, expected)?;
        let focused = inner
            .snapshot
            .as_ref()
            .filter(|snapshot| snapshot.boot_id == expected.boot_id)
            .and_then(|snapshot| snapshot.panes.iter().find(|p| p.focused))
            .is_some_and(|p| p.pane_id == pane_id);
        if !focused {
            return Err(RuntimeError::new(
                "pane_not_focused",
                "the pane is not the focus confirmed by the server; nothing was sent",
            )
            .with_endpoint(endpoint));
        }
        let mut target = inner.gate(pane_id).map_err(|block| block.error(endpoint))?;
        admit()?;
        let gateway = inner.gateway.as_ref().expect("gate implies gateway");
        target.connection_generation = gateway
            .identity()
            .map(|i| i.connection_generation)
            .unwrap_or_default();
        gateway.send_clipboard_image(&target, extension, data)
    }

    /// One clipboard image to an explicit pane target; the pane, the connection and the input
    /// gate are validated under the host lock. Nothing is queued for later delivery.
    pub fn send_clipboard_image(
        &self,
        target: &QualifiedTarget,
        extension: &str,
        data: Vec<u8>,
    ) -> Result<(), RuntimeError> {
        let slot = self.slot(&target.endpoint)?;
        let inner = slot.inner.lock().expect("host lock");
        let current = inner
            .gate(&target.pane_id)
            .map_err(|block| block.error(&target.endpoint))?;
        let live = inner
            .live_identity()
            .ok_or_else(|| InputBlock::NoSnapshot.error(&target.endpoint))?;
        target.validate(&live)?;
        let gateway = inner.gateway.as_ref().expect("gate implies gateway");
        let mut inner_target = current;
        inner_target.connection_generation = gateway
            .identity()
            .map(|i| i.connection_generation)
            .unwrap_or_default();
        gateway.send_clipboard_image(&inner_target, extension, data)
    }

    /// Runs one JSON API action on the host's own lane, outside the host lock. A transport
    /// loss or timeout makes the result unknown; it is recorded and never re-sent.
    pub fn run_action(
        &self,
        endpoint: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, RuntimeError> {
        let slot = self.slot(endpoint)?;
        let (api, generation, boot, id) = {
            let mut inner = slot.inner.lock().expect("host lock");
            online(&inner, endpoint)?;
            let api = inner.api.clone().expect("online implies api lane");
            let generation = inner.generation;
            let boot = inner.live_identity().map(|live| live.boot_id);
            let id = inner.record(method);
            (api, generation, boot, id)
        };
        self.bump();
        let result = api.request(method, params);
        self.finish_action(
            &slot,
            endpoint,
            method,
            (generation, boot.as_deref()),
            id,
            result,
        )
    }

    /// Runs one announced endpoint command for `target`. Endpoint, session, generation, boot,
    /// workspace and pane are validated against the host under its lock, and nothing is sent
    /// when any of them no longer matches; the reply is awaited on the connection's own lane
    /// with no hub lock held. A result made unknown by the transport is never re-sent.
    pub fn run_endpoint(
        &self,
        target: &QualifiedTarget,
        method: &str,
        params: Value,
    ) -> Result<Value, RuntimeError> {
        let live = LiveIdentity {
            endpoint: target.endpoint.clone(),
            session: target.session.clone(),
            connection_generation: target.connection_generation,
            boot_id: target.boot_id.clone(),
        };
        let scope = EndpointScope::Pane {
            pane_id: target.pane_id.clone(),
            workspace_id: target.workspace_id.clone(),
        };
        self.run_endpoint_scoped(&live, &scope, method, params)
    }

    /// Runs one announced endpoint command for the connection identified by `expected`, whose
    /// explicit target `scope` must exist in that connection's snapshot. Validation happens under
    /// the host lock and nothing is sent on any mismatch; the reply is awaited on the lane with
    /// no hub lock held and an unknown result is recorded and never re-sent.
    pub fn run_endpoint_scoped(
        &self,
        expected: &LiveIdentity,
        scope: &EndpointScope,
        method: &str,
        params: Value,
    ) -> Result<Value, RuntimeError> {
        let endpoint = expected.endpoint.as_str();
        let slot = self.slot(endpoint)?;
        let (lane, generation, id) = {
            let mut inner = slot.inner.lock().expect("host lock");
            let lane = scoped_lane(&inner, expected, scope, method)?;
            let generation = inner.generation;
            let id = inner.record(method);
            (lane, generation, id)
        };
        self.bump();
        let result = lane.request(&expected.boot_id, method, params);
        self.finish_action(
            &slot,
            endpoint,
            method,
            (generation, Some(&expected.boot_id)),
            id,
            result,
        )
    }

    /// [`Self::run_endpoint_scoped`] with a caller admission at the lane's turn: the host checks
    /// run now under the host lock (nothing waits on a refusal); then, once this request holds
    /// the connection's turn and before the lane writes, `admit` runs (no hub lock held) and the
    /// same host checks are repeated under the host lock, where the action is recorded. A refusal
    /// at either point sends and records nothing. The reply is awaited with no hub lock held and
    /// an unknown result is never re-sent. Guarantee: at the admission point, the facts the client
    /// had observed (selection, attachment, connection, confirmed focus, live surface) matched.
    /// Not guaranteed: that they still hold when the lane acquires its state/writer and writes, nor
    /// anything about the server's focus once the command is sent.
    pub fn run_endpoint_scoped_admitted(
        &self,
        expected: &LiveIdentity,
        scope: &EndpointScope,
        method: &str,
        params: Value,
        admit: &dyn Fn() -> Result<(), RuntimeError>,
    ) -> Result<Value, RuntimeError> {
        let endpoint = expected.endpoint.as_str();
        let slot = self.slot(endpoint)?;
        let lane = {
            let inner = slot.inner.lock().expect("host lock");
            scoped_lane(&inner, expected, scope, method)?
        };
        let admitted: Mutex<Option<(u64, u64)>> = Mutex::new(None);
        let result = lane.request_admitted(&expected.boot_id, method, params, &|| {
            admit()?;
            let mut inner = slot.inner.lock().expect("host lock");
            let current = scoped_lane(&inner, expected, scope, method)?;
            if !Arc::ptr_eq(&current, &lane) {
                return Err(RuntimeError::new(
                    "connection_changed",
                    "this host's connection changed; nothing was sent",
                )
                .with_endpoint(endpoint));
            }
            let generation = inner.generation;
            let id = inner.record(method);
            *admitted.lock().unwrap_or_else(PoisonError::into_inner) = Some((generation, id));
            Ok(())
        });
        let admitted = *admitted.lock().unwrap_or_else(PoisonError::into_inner);
        let Some((generation, id)) = admitted else {
            // Refused before the write (admission, lane checks or a lane without admission).
            return result.map_err(|error| error.with_endpoint(endpoint));
        };
        self.bump();
        self.finish_action(
            &slot,
            endpoint,
            method,
            (generation, Some(&expected.boot_id)),
            id,
            result,
        )
    }

    /// Live identity of an online host (hub-owned connection generation).
    pub fn live_identity(&self, endpoint: &str) -> Result<LiveIdentity, RuntimeError> {
        let slot = self.slot(endpoint)?;
        let inner = slot.inner.lock().expect("host lock");
        online(&inner, endpoint)?;
        inner.live_identity().ok_or_else(|| {
            RuntimeError::new("boot_unknown", "the server identity is not established yet")
                .retryable()
                .with_endpoint(endpoint)
        })
    }

    /// Fails unless the host is online on exactly the connection and boot of `expected`.
    pub fn check_identity(&self, expected: &LiveIdentity) -> Result<(), RuntimeError> {
        let slot = self.slot(&expected.endpoint)?;
        let inner = slot.inner.lock().expect("host lock");
        current_identity(&inner, expected)
    }

    /// Endpoint methods announced to the installed connection.
    pub fn endpoint_methods(&self, endpoint: &str) -> Result<Vec<String>, RuntimeError> {
        let slot = self.slot(endpoint)?;
        let inner = slot.inner.lock().expect("host lock");
        online(&inner, endpoint)?;
        inner
            .endpoint_lane
            .as_ref()
            .map(|lane| lane.methods())
            .ok_or_else(|| {
                RuntimeError::new(
                    "endpoint_unavailable",
                    "endpoint commands are unavailable on this connection",
                )
                .retryable()
                .with_endpoint(endpoint)
            })
    }

    /// JSON API request for the connection identified by `expected`, on the host's API lane with
    /// no hub lock held. A `record`ed call (one with consequences) goes to the action ledger and
    /// becomes unknown on transport loss, never re-sent; an unrecorded read returns its error.
    pub fn run_api(
        &self,
        expected: &LiveIdentity,
        method: &str,
        params: Value,
        record: bool,
    ) -> Result<Value, RuntimeError> {
        let endpoint = expected.endpoint.as_str();
        let slot = self.slot(endpoint)?;
        let (api, generation, id) = {
            let mut inner = slot.inner.lock().expect("host lock");
            current_identity(&inner, expected)?;
            let api = inner.api.clone().expect("online implies api lane");
            let id = record.then(|| inner.record(method));
            (api, inner.generation, id)
        };
        let Some(id) = id else {
            let result = api.request(method, params);
            if let Err(e) = &result {
                if e.code == "remote_api_unsupported" {
                    let mut inner = slot.inner.lock().expect("host lock");
                    inner.api_supported = false;
                }
            }
            // A reply of a connection or boot that is no longer current is not handed out.
            current_identity(&slot.inner.lock().expect("host lock"), expected)?;
            return result.map_err(|error| error.with_endpoint(endpoint));
        };
        self.bump();
        let result = api.request(method, params);
        if let Err(e) = &result {
            if e.code == "remote_api_unsupported" {
                let mut inner = slot.inner.lock().expect("host lock");
                inner.api_supported = false;
            }
        }
        self.finish_action(
            &slot,
            endpoint,
            method,
            (generation, Some(&expected.boot_id)),
            id,
            result,
        )
    }

    /// Surface committed by the connection of `expected` while the host is visible; a frame of
    /// another connection or boot is never returned for it.
    pub fn surface_for(&self, expected: &LiveIdentity) -> Option<PaneSurfaceFrame> {
        let slot = self.slot(&expected.endpoint).ok()?;
        let inner = slot.inner.lock().expect("host lock");
        current_identity(&inner, expected).ok()?;
        if !inner.spec.visible
            || !inner.presenting()
            || inner.surface_generation != Some(inner.generation)
        {
            return None;
        }
        inner
            .store
            .surface()
            .filter(|frame| frame.boot_id == expected.boot_id)
            .cloned()
    }

    /// Tab focused by the connection of `expected`, confirmed only when the stored client-shell
    /// snapshot and the committed full surface of that same connection (endpoint, generation,
    /// boot) carry `projection_revision` and the surface is live. `None`: nothing confirmed for that
    /// revision (another connection or boot, hidden or not presenting, resync pending, or a
    /// surface not committed yet);
    /// `Some(None)`: the snapshot confirms no focused tab. Never derived from the JSON API.
    pub fn confirmed_tab_focus(
        &self,
        expected: &LiveIdentity,
        projection_revision: u64,
    ) -> Option<Option<String>> {
        let slot = self.slot(&expected.endpoint).ok()?;
        let inner = slot.inner.lock().expect("host lock");
        current_identity(&inner, expected).ok()?;
        if !inner.spec.visible
            || !inner.presenting()
            || inner.surface_generation != Some(inner.generation)
            || inner.store.state() != SurfaceState::Live
        {
            return None;
        }
        let frame = inner.store.surface()?;
        let snapshot = inner.snapshot.as_ref()?;
        (frame.boot_id == expected.boot_id
            && snapshot.boot_id == expected.boot_id
            && frame.projection_revision == projection_revision
            && snapshot.revision == projection_revision)
            .then(|| snapshot.focused_tab_id.clone())
    }

    /// Focus report for the connection of `expected`.
    pub fn set_focus(&self, expected: &LiveIdentity, focused: bool) -> Result<(), RuntimeError> {
        let slot = self.slot(&expected.endpoint)?;
        let inner = slot.inner.lock().expect("host lock");
        current_identity(&inner, expected)?;
        inner
            .gateway
            .as_ref()
            .expect("online implies gateway")
            .set_focus(focused)
    }

    /// Records the outcome of a sent action. Unknown results stay unknown and are reported
    /// as such, never retried. A reply that arrives after the host lost, renewed or rebooted the
    /// connection it was sent on (`sent_on` = generation and boot) is unknown even when it
    /// reports success.
    fn finish_action(
        &self,
        slot: &HostSlot,
        endpoint: &str,
        method: &str,
        sent_on: (u64, Option<&str>),
        id: u64,
        result: Result<Value, RuntimeError>,
    ) -> Result<Value, RuntimeError> {
        let mut inner = slot.inner.lock().expect("host lock");
        let (generation, boot) = sent_on;
        let still_same = inner.generation == generation
            && inner.link.phase() == LinkPhase::Online
            && boot.is_none_or(|boot| inner.live_identity().is_some_and(|l| l.boot_id == boot));
        let (outcome, returned) = match result {
            Ok(value) if still_same => (ActionOutcome::Succeeded, Ok(value)),
            Err(error) if !result_unknown(&error) && still_same => {
                (ActionOutcome::Failed, Err(error.with_endpoint(endpoint)))
            }
            _ => {
                let unknown = RuntimeError::new(
                    "result_unknown",
                    format!(
                        "the result of {method} is unknown; the action will not be retried automatically"
                    ),
                )
                .with_endpoint(endpoint);
                (ActionOutcome::Unknown, Err(unknown))
            }
        };
        if let Some(record) = inner.actions.iter_mut().find(|r| r.id == id) {
            // A record already marked unknown by a disconnect stays unknown.
            if record.outcome == ActionOutcome::Pending {
                record.outcome = outcome;
            }
            record.error = returned.as_ref().err().cloned();
        }
        match &returned {
            Err(error) => inner.action_error = Some(error.clone()),
            Ok(_) => inner.action_error = None,
        }
        drop(inner);
        self.bump();
        returned
    }

    /// Link, identity and confirmed focus of one host under one lock, without I/O.
    pub fn link(&self, endpoint: &str) -> Result<HostLink, RuntimeError> {
        let slot = self.slot(endpoint)?;
        let inner = slot.inner.lock().expect("host lock");
        let phase = inner.link.phase();
        let identity = (phase == LinkPhase::Online)
            .then(|| inner.live_identity())
            .flatten();
        let focused = identity.as_ref().and_then(|live| {
            let snapshot = inner.snapshot.as_ref()?;
            if snapshot.boot_id != live.boot_id {
                return None;
            }
            snapshot
                .panes
                .iter()
                .find(|p| p.focused)
                .map(|p| (p.pane_id.clone(), p.workspace_id.clone()))
        });
        let current_surface = inner.store.surface().is_some()
            && inner.presenting()
            && inner.surface_generation == Some(inner.generation);
        let input_block = focused
            .as_ref()
            .and_then(|(pane, _)| inner.gate(pane).err());
        Ok(HostLink {
            kind: inner.spec.kind,
            label: inner.spec.label.clone(),
            session: inner.spec.session.clone(),
            visible: inner.spec.visible,
            phase,
            generation: inner.generation,
            identity,
            error: inner.link.error().cloned(),
            focused,
            surface: current_surface.then(|| inner.store.state()),
            revision: current_surface.then(|| inner.store.revision()).flatten(),
            input_block,
        })
    }

    /// Text rows and counters of a host's frame store (opt-in surface trace only).
    pub fn store_probe(
        &self,
        endpoint: &str,
    ) -> Option<(Vec<String>, herdr_client::frame_store::FrameStats)> {
        let slot = self.slot(endpoint).ok()?;
        let inner = slot.inner.lock().expect("host lock");
        Some((inner.store.text_rows(), inner.store.stats()))
    }

    /// Returns the workspace projection delivered by the visual client-shell snapshot. Older
    /// remote Herdr versions do not expose `remote-api-bridge`, but the snapshot already carries
    /// the same read-only workspace facts. Wait only while this exact host connection is online;
    /// cancellation turns into `host_offline` and never returns cached data as if it were live.
    pub fn snapshot_workspaces(
        &self,
        endpoint: &str,
        timeout: Duration,
    ) -> Result<Vec<HostWorkspaceDto>, RuntimeError> {
        let deadline = Instant::now() + timeout;
        loop {
            let slot = self.slot(endpoint)?;
            {
                let inner = slot.inner.lock().expect("host lock");
                online(&inner, endpoint)?;
                if let Some(snapshot) = inner.snapshot.as_ref() {
                    return Ok(snapshot.workspaces.clone());
                }
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(
                    RuntimeError::new("timeout", "no answer while reading workspaces")
                        .retryable()
                        .with_endpoint(endpoint),
                );
            }
            let revision = self.revision();
            self.wait_changed(revision, remaining);
        }
    }

    pub fn snapshot(&self, now: Instant) -> HubSnapshot {
        let order = self.order.read().expect("order lock").clone();
        let hosts = self.hosts.read().expect("hosts lock").clone();
        let revision = self.revision();
        HubSnapshot {
            revision,
            hosts: order
                .iter()
                .filter_map(|endpoint| hosts.get(endpoint))
                .map(|slot| slot.inner.lock().expect("host lock").dto(now))
                .collect(),
        }
    }
}

fn renegotiating(endpoint: &str) -> HostNotice {
    HostNotice::Invalidated(
        RuntimeError::new(
            "surface_renegotiating",
            "the connection was renegotiated to another surface mode",
        )
        .retryable()
        .with_endpoint(endpoint),
    )
}

/// The installed endpoint lane of `expected`'s connection when the host is online on it, the lane
/// announces `method` and the explicit target `scope` exists in that connection's snapshot.
fn scoped_lane(
    inner: &HostInner,
    expected: &LiveIdentity,
    scope: &EndpointScope,
    method: &str,
) -> Result<Arc<dyn EndpointLane>, RuntimeError> {
    let endpoint = expected.endpoint.as_str();
    online(inner, endpoint)?;
    let lane = inner.endpoint_lane.clone().ok_or_else(|| {
        RuntimeError::new(
            "endpoint_unavailable",
            "endpoint commands are unavailable on this connection",
        )
        .retryable()
        .with_endpoint(endpoint)
    })?;
    if !lane.methods().iter().any(|m| m == method) {
        return Err(RuntimeError::new(
            "unsupported_method",
            format!("the {method} method is not available on this machine"),
        )
        .with_endpoint(endpoint));
    }
    let live = inner
        .live_identity()
        .ok_or_else(|| InputBlock::NoSnapshot.error(endpoint))?;
    same_identity(expected, &live)?;
    let snapshot = inner
        .snapshot
        .as_ref()
        .filter(|snapshot| snapshot.boot_id == live.boot_id)
        .ok_or_else(|| InputBlock::NoSnapshot.error(endpoint))?;
    let missing = match scope {
        EndpointScope::Pane {
            pane_id,
            workspace_id,
        } => (!snapshot.panes.iter().any(|p| {
            p.pane_id == *pane_id && workspace_id.as_ref().is_none_or(|w| w == &p.workspace_id)
        }))
        .then_some((
            "pane_not_in_snapshot",
            "the pane no longer exists on this host; select the target again",
        )),
        EndpointScope::Tab(tab) => (!snapshot.tab_ids.contains(tab)).then_some((
            "tab_not_in_snapshot",
            "the tab no longer exists on this host; select the target again",
        )),
        EndpointScope::Workspace(workspace) => (!snapshot
            .workspaces
            .iter()
            .any(|candidate| candidate.workspace_id == *workspace))
        .then_some((
            "workspace_not_in_snapshot",
            "the workspace no longer exists on this host; select the target again",
        )),
    };
    if let Some((code, message)) = missing {
        return Err(RuntimeError::new(code, message).with_endpoint(endpoint));
    }
    Ok(lane)
}

fn same_identity(expected: &LiveIdentity, live: &LiveIdentity) -> Result<(), RuntimeError> {
    QualifiedTarget::new(expected, None, String::new()).validate(live)
}

fn current_identity(inner: &HostInner, expected: &LiveIdentity) -> Result<(), RuntimeError> {
    let endpoint = expected.endpoint.as_str();
    online(inner, endpoint)?;
    let live = inner
        .live_identity()
        .ok_or_else(|| InputBlock::NoSnapshot.error(endpoint))?;
    same_identity(expected, &live)
}

fn online(inner: &HostInner, endpoint: &str) -> Result<(), RuntimeError> {
    match inner.link.phase() {
        LinkPhase::Online => Ok(()),
        LinkPhase::Offline => Err(InputBlock::HostOffline.error(endpoint)),
        LinkPhase::Connecting => Err(InputBlock::HostConnecting.error(endpoint)),
        LinkPhase::Reconnecting => Err(InputBlock::HostReconnecting.error(endpoint)),
        LinkPhase::Attention => Err(InputBlock::HostNeedsAttention.error(endpoint)),
    }
}

fn snapshot_view(snapshot: &ClientShellSnapshot) -> SnapshotView {
    let agent_map: BTreeMap<&str, &ClientShellAgent> = snapshot
        .agents
        .iter()
        .map(|a| (a.pane_id.as_str(), a))
        .collect();
    let panes = snapshot
        .panes
        .iter()
        .map(|p| {
            let agent = agent_map.get(p.pane_id.as_str()).copied();
            let agent_name = agent.and_then(|a| {
                a.name
                    .clone()
                    .or_else(|| a.display_agent.clone())
                    .or_else(|| a.agent.clone())
            });
            let agent_status = agent.and_then(|a| {
                serde_json::to_value(a.agent_status)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
            });
            SnapshotPaneMeta {
                pane_id: p.pane_id.clone(),
                workspace_id: p.workspace_id.clone(),
                tab_id: p.tab_id.clone(),
                focused: p.focused,
                cwd: p.cwd.clone(),
                foreground_cwd: p.foreground_cwd.clone(),
                title: agent
                    .and_then(|a| a.title.clone())
                    .or_else(|| p.label.clone()),
                terminal_title: agent.and_then(|a| a.terminal_title.clone()),
                agent: agent_name,
                agent_status,
            }
        })
        .collect();
    let tabs = tab_views(snapshot);
    let tab_ids = snapshot.tabs.iter().map(|t| t.tab_id.clone()).collect();
    let agents = agent_views(snapshot);
    SnapshotView {
        boot_id: snapshot.boot_id.clone(),
        revision: snapshot.revision,
        panes,
        workspaces: workspace_views(snapshot),
        tabs,
        tab_ids,
        agents,
        focused_tab_id: snapshot.focused_tab_id.clone(),
        branch: snapshot
            .focused_tab_id
            .as_ref()
            .and_then(|tab| snapshot.tabs.iter().find(|t| &t.tab_id == tab))
            .and_then(|tab| {
                snapshot
                    .workspaces
                    .iter()
                    .find(|w| w.workspace_id == tab.workspace_id)
            })
            .and_then(|w| w.branch.clone()),
    }
}

/// Projection floor of a `client_shell_surface_set` acknowledgement for `active`.
fn interest_floor(endpoint: &str, reply: &Value, active: bool) -> Result<u64, RuntimeError> {
    let valid = reply.get("type").and_then(Value::as_str) == Some("client_shell_surface_set")
        && reply.get("active").and_then(Value::as_bool) == Some(active);
    match reply.get("projection_revision").and_then(Value::as_u64) {
        Some(floor) if valid => Ok(floor),
        _ => Err(RuntimeError::new(
            "surface_interest_invalid",
            "invalid surface interest confirmation",
        )
        .with_endpoint(endpoint)),
    }
}
