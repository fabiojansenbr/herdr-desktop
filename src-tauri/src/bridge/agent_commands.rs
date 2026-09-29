//! Agents, tabs and split panes (spec 004).
//!
//! - [`AgentsCore`]: gateway-agnostic rules. Agent kinds, agents and tabs come from the
//!   server's JSON API (`server.agent_manifests`, `agent.list`, `tab.list`); starting and
//!   prompting use `agent.start`/`agent.prompt` on the API of the qualified host. Splits,
//!   focus, split ratio and tabs use endpoint commands the generation-1 welcome announced.
//!   Geometry is the server's surface v1 topology (panes, splits and the confirmed focus);
//!   input is sent only to the pane the server confirmed focused. Every action validates
//!   endpoint, session, connection generation, boot and pane before any call, and a method
//!   the server does not offer disables only its own action. An unknown prompt outcome
//!   (timeout) is never re-sent without an explicit acknowledgement. Nothing here answers
//!   an agent's approval: opening a blocked agent only focuses its pane.
//! - Tauri commands (`COMMANDS`) over a [`LocalGateway`] with an active surface, a bridge
//!   thread for frames and an `events.subscribe` stream for agent/tab changes (no output
//!   polling). Registering them in the window belongs to spec 007; until then they are
//!   compiled and exercised by `src-tauri/tests/agents.rs`.
//! - Responsiveness (spec 007): every command is `async` and never runs on the IPC calling
//!   thread. [`AgentsState`] hands each call a turn in its command lane when it enters; the
//!   turn is awaited without holding a thread, then the blocking work (locks, network, the
//!   engine) runs on the async runtime's bounded blocking pool. Calls of one state therefore
//!   apply in entry order, one at a time: an older detach never undoes a newer attach, and
//!   input/actions keep their order. The synchronous methods stay as the harness seam.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::future::Future;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Waker};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use herdr_client::local::GatewayEvents;
use herdr_client::protocol::wire::{
    ClientPaneInputEvent, PaneSurfaceFrame, PaneSurfacePatch, PaneSurfaceSplitDirection,
};
use herdr_client::{
    ApplyOutcome, ConnectOptions, FrameStore, GatewayEvent, LiveIdentity, LocalGateway, Negotiated,
    QualifiedTarget, RuntimeError, RuntimeGateway, SessionName, StaleReason, SurfaceGeometry,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::ipc::Channel;
use tauri::State;

use crate::terminal::{GeometryDto, InputDto};

/// Commands this module exposes to the WebView (registered by the window composition, 007).
pub const COMMANDS: &[&str] = &[
    "agents_connect",
    "agents_overview",
    "agents_detach",
    "agent_start",
    "agent_autonomy_flags",
    "agent_prompt",
    "agent_open_attention",
    "pane_split",
    "pane_focus",
    "pane_set_split_ratio",
    "pane_input",
    "pane_rename",
    "pane_swap",
    "pane_input_set",
    "tab_create",
    "tab_focus",
    "tab_close",
    "tab_rename",
    "pane_zoom",
    "pane_close",
];

/// JSON API methods whose availability is discovered per server (the endpoint welcome does
/// not announce them).
const API_METHODS: &[&str] = &[
    "agent.list",
    "agent.get",
    // Read-only snapshot of one pane; used once per transition to waiting (spec 014), never
    // polled. Probed with an empty params object, which the engine rejects before reading.
    "agent.read",
    "agent.start",
    "agent.prompt",
    "server.agent_manifests",
    "tab.list",
    "pane.list",
    // Static screen snapshot for the home thumbnails (spec 012): one request per card and
    // visit, never a frame loop. Probed with empty params like the other methods.
    "pane.read",
];

/// Zoom modes the engine accepts (`PaneZoomParams::mode`).
const ZOOM_MODES: [&str; 3] = ["toggle", "on", "off"];

/// Split ratio range accepted by the engine (`Layout::set_ratio_at` clamps to it).
const MIN_RATIO: f32 = 0.1;
const MAX_RATIO: f32 = 0.9;
const MAX_AGENT_NAME_CHARS: usize = 64;
/// Longest manual pane name accepted by `pane.rename` from the menu (the TUI has no such limit;
/// this is an IPC bound, not an engine one).
const MAX_PANE_LABEL_CHARS: usize = 128;
const MAX_PROMPT_BYTES: usize = 64 * 1024;
/// Upper bound of one `pane.read` snapshot line count; the home asks for a handful.
const MAX_SNAPSHOT_LINES: u32 = 200;

/// Arguments that start each agent kind in its own autonomous mode ("yolo"), spec 076. The table
/// lives here, in the backend: the WebView only says on or off, so no generic argument list — and
/// no shell — can reach an agent through the IPC. The flags come from each binary's own `--help`
/// (`~/.claude/skills/herdr-delegate/SKILL.md`); a kind absent here simply has no auto mode, and
/// the same table serves a remote host, since the flag belongs to the agent binary, not the host.
const AUTONOMY_FLAGS: &[(&str, &[&str])] = &[
    ("claude", &["--dangerously-skip-permissions"]),
    ("agy", &["--dangerously-skip-permissions"]),
    ("codex", &["--dangerously-bypass-approvals-and-sandbox"]),
    ("gemini", &["--yolo"]),
    ("cursor", &["--yolo", "--trust"]),
    ("copilot", &["--allow-all"]),
    ("opencode", &["--auto"]),
    ("grok", &["--always-approve"]),
    ("omp", &["--auto-approve"]),
    ("pi", &["--approve"]),
    ("hermes", &["--yolo", "--accept-hooks"]),
];

/// Longest kind list one autonomy read may carry (an IPC bound, not an engine one).
pub const MAX_AUTONOMY_KINDS: usize = 64;

/// Flags that put `kind` in its autonomous mode; empty for a kind with no entry.
pub fn autonomy_flags(kind: &str) -> &'static [&'static str] {
    match AUTONOMY_FLAGS.iter().find(|(name, _)| *name == kind) {
        Some((_, flags)) => flags,
        None => &[],
    }
}

/// The table as the popup reads it: one entry per asked kind, in the order asked.
pub fn autonomy_flags_of(kinds: &[String]) -> Result<Vec<AgentAutonomyFlags>, RuntimeError> {
    if kinds.len() > MAX_AUTONOMY_KINDS {
        return Err(RuntimeError::new(
            "invalid_input",
            format!("at most {MAX_AUTONOMY_KINDS} agent kinds per query"),
        ));
    }
    Ok(kinds
        .iter()
        .map(|kind| AgentAutonomyFlags {
            kind: kind.clone(),
            flags: autonomy_flags(kind)
                .iter()
                .map(|f| (*f).to_owned())
                .collect(),
        })
        .collect())
}

// ---------------------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------------------

/// Agent state as published by the engine. Anything else (missing, renamed, new) is
/// `Unknown`: it is never shown as done.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Working,
    Blocked,
    Idle,
    Done,
    Unknown,
}

impl AgentStatus {
    pub fn from_wire(value: Option<&str>) -> Self {
        match value {
            Some("working") => Self::Working,
            Some("blocked") => Self::Blocked,
            Some("idle") => Self::Idle,
            Some("done") => Self::Done,
            _ => Self::Unknown,
        }
    }
}

/// Autonomy flags of one kind, read by the "New agent" popup so each row can name the flag it
/// would use (spec 076). It is a read of the table above; nothing here starts an agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentAutonomyFlags {
    pub kind: String,
    pub flags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentDto {
    pub pane_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    pub name: Option<String>,
    pub kind: Option<String>,
    pub status: AgentStatus,
    pub launch_pending: bool,
    pub ready: bool,
    pub focused: bool,
    /// Engine's `terminal_title_stripped` (the pane's own title); never composed here.
    pub terminal_title: Option<String>,
    /// Engine's `state_change_seq`: it changes when the engine publishes another state.
    pub state_change_seq: u64,
    /// Last line of the pane's detection snapshot, read once when the engine reported this
    /// agent waiting for the user (spec 014). `None` until such a transition is observed on
    /// this connection, and dropped as soon as the state changes again.
    pub detection_last_line: Option<String>,
}

impl AgentDto {
    fn from_wire(value: &Value) -> Option<Self> {
        let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
        let flag = |key: &str| value.get(key).and_then(Value::as_bool).unwrap_or(false);
        Some(Self {
            pane_id: text("pane_id")?,
            workspace_id: text("workspace_id").unwrap_or_default(),
            tab_id: text("tab_id").unwrap_or_default(),
            name: text("name"),
            kind: text("agent"),
            status: AgentStatus::from_wire(value.get("agent_status").and_then(Value::as_str)),
            launch_pending: flag("launch_pending"),
            ready: flag("interactive_ready"),
            focused: flag("focused"),
            terminal_title: text("terminal_title_stripped").filter(|t| !t.trim().is_empty()),
            state_change_seq: value
                .get("state_change_seq")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            detection_last_line: None,
        })
    }
}

/// Which actions this server offers. A missing method disables only its action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Capabilities {
    pub list_agents: bool,
    pub start_agent: bool,
    pub send_prompt: bool,
    pub open_attention: bool,
    pub split: bool,
    pub focus: bool,
    pub split_ratio: bool,
    pub input: bool,
    pub create_tab: bool,
    pub focus_tab: bool,
    /// `tab.close`: close one tab of the workspace.
    pub close_tab: bool,
    /// `tab.rename`: set the engine label of one tab.
    pub rename_tab: bool,
    /// `pane.zoom`: expand one pane of the tab (and restore it).
    pub zoom: bool,
    /// `pane.rename`: set (or clear) the manual name of one pane.
    pub rename_pane: bool,
    /// `pane.swap`: exchange two panes of the same tab.
    pub swap: bool,
    /// `pane.input.set`: choose whether right clicks go to the pane or the Herdr menu.
    pub input_set: bool,
    /// `pane.close`: close one pane.
    pub close_pane: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PaneBox {
    pub pane_id: String,
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    pub focused: bool,
    /// Working directory the engine reported for this pane (`pane.list`), when it reported one.
    /// The surface frame has no cwd: the center region shows this, never the project root.
    pub cwd: Option<String>,
    /// Manual name the engine reported for this pane (`pane.list`); absent when there is none.
    /// Drives `Limpar nome do pane` in the pane context menu.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SplitBox {
    /// Engine split path (false = first child, true = second child), as in the surface.
    pub path: Vec<bool>,
    /// "right" (side by side) or "down" (stacked).
    pub direction: String,
    pub pos: u16,
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    pub ratio: f32,
}

/// Active-tab topology copied from the committed surface v1 frame.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Topology {
    pub revision: u64,
    pub width: u16,
    pub height: u16,
    pub focused_pane_id: Option<String>,
    pub panes: Vec<PaneBox>,
    pub splits: Vec<SplitBox>,
}

impl Topology {
    fn from_surface(surface: &PaneSurfaceFrame) -> Self {
        let panes: Vec<PaneBox> = surface
            .panes
            .iter()
            .map(|p| PaneBox {
                pane_id: p.pane_id.clone(),
                x: p.rect.x,
                y: p.rect.y,
                width: p.rect.width,
                height: p.rect.height,
                focused: p.focused,
                cwd: None,
                label: None,
            })
            .collect();
        let splits = surface
            .splits
            .iter()
            .map(|s| {
                let (direction, start, span) = match s.direction {
                    PaneSurfaceSplitDirection::Horizontal => ("right", s.area.x, s.area.width),
                    PaneSurfaceSplitDirection::Vertical => ("down", s.area.y, s.area.height),
                };
                let ratio = if span == 0 {
                    0.5
                } else {
                    f32::from(s.pos.saturating_sub(start)) / f32::from(span)
                };
                SplitBox {
                    path: s.path.clone(),
                    direction: direction.into(),
                    pos: s.pos,
                    x: s.area.x,
                    y: s.area.y,
                    width: s.area.width,
                    height: s.area.height,
                    ratio,
                }
            })
            .collect();
        Self {
            revision: surface.surface_revision,
            width: surface.frame.width,
            height: surface.frame.height,
            focused_pane_id: panes.iter().find(|p| p.focused).map(|p| p.pane_id.clone()),
            panes,
            splits,
        }
    }

    fn same_shape(&self, other: &Topology) -> bool {
        self.width == other.width
            && self.height == other.height
            && self.panes == other.panes
            && self.splits == other.splits
    }
}

/// Tab focused by one connection, confirmed by its client-shell snapshot whose revision matches
/// the committed full surface of that connection (`HostHub::confirmed_tab_focus`). The engine's
/// `tab.focus` moves only its source connection, so the JSON API `tab.list[].focused` (global)
/// never replaces it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TabFocus {
    pub endpoint: String,
    pub session: String,
    pub connection_generation: u64,
    pub boot_id: String,
    /// Snapshot revision == projection revision of the committed full surface.
    pub revision: u64,
    pub tab_id: Option<String>,
}

impl TabFocus {
    fn confirmed(identity: &LiveIdentity, revision: u64, tab_id: Option<String>) -> Self {
        Self {
            endpoint: identity.endpoint.clone(),
            session: identity.session.clone(),
            connection_generation: identity.connection_generation,
            boot_id: identity.boot_id.clone(),
            revision,
            tab_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TabDto {
    pub tab_id: String,
    pub workspace_id: String,
    pub label: String,
    /// Engine `tab.list` number (fallback title when `label` is empty).
    pub number: u64,
    pub focused: bool,
    /// Panes of this tab as the engine counted them; the client only has metadata of the active tab.
    pub pane_count: usize,
    /// Aggregated agent state of the tab published by the engine (`working`, `blocked`, ...).
    pub agent_status: String,
}

/// Static screen snapshot of one pane (`pane.read`), handed to the home thumbnails (spec 012).
/// The text is shown as received; this module never interprets, repaints or polls it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PaneSnapshotDto {
    pub pane_id: String,
    pub text: String,
    pub revision: u64,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum PromptOutcome {
    /// The engine confirmed the submission.
    Sent { agent: AgentDto },
    /// The request may or may not have reached the agent (timeout, lost connection). It is
    /// not repeated; the pane must be checked before an explicit resend.
    Unknown { error: RuntimeError },
}

/// Result of one `pane.zoom` (spec 013; `zoomed` added by spec 028 so the menu can offer
/// `Desfazer zoom` from the engine's own answer).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PaneZoomReceipt {
    pub pane_id: String,
    /// The engine's own `zoom.zoomed` for this request; absent when its reply did not carry it
    /// (the menu then keeps its previous label).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zoomed: Option<bool>,
}

/// Pane the engine created for one `pane.split` (spec 075). The engine answers `PaneInfo` with
/// it; a server whose reply carries no pane leaves `pane_id` absent and nothing is guessed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PaneSplitReceipt {
    pub pane_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Overview {
    pub state: String,
    pub session: Option<String>,
    pub identity: Option<LiveIdentity>,
    pub server_version: Option<String>,
    pub capabilities: Option<Capabilities>,
    pub kinds: Vec<String>,
    pub agents: Vec<AgentDto>,
    pub tabs: Vec<TabDto>,
    pub topology: Option<Topology>,
    /// Hosted only: tab focus confirmed by the attached connection (see [`TabFocus`]).
    pub tab_focus: Option<TabFocus>,
    pub error: Option<RuntimeError>,
}

/// Events pushed to the WebView channel. Topology is sent only when it changes.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentsEvent {
    Identity {
        identity: LiveIdentity,
    },
    Topology {
        topology: Topology,
    },
    Agents {
        agents: Vec<AgentDto>,
    },
    Tabs {
        tabs: Vec<TabDto>,
    },
    TabFocus {
        focus: TabFocus,
    },
    /// One engine lifecycle event (tab/workspace/pane created, closed, renamed, focused),
    /// forwarded so the window reconciles once per tick (spec 027). It carries no list: the
    /// reconciliation is the `agents_overview { reconcile: true }` call.
    Structure {
        events: Vec<String>,
    },
    State {
        state: String,
        error: Option<RuntimeError>,
    },
}

/// Engine event names (wire `snake_case`, dotted spellings accepted) whose arrival changes the
/// window's tab list: every `tab.*` plus the workspace/pane lifecycle events (spec 027). An
/// output or agent-status event is never structural: it must not cause a tab reconciliation.
pub fn structural_event(event: &str) -> bool {
    let name = event.replace('.', "_");
    name.starts_with("tab_")
        || matches!(
            name.as_str(),
            "workspace_created"
                | "workspace_closed"
                | "workspace_renamed"
                | "pane_created"
                | "pane_closed"
        )
}

// ---------------------------------------------------------------------------------------
// Core
// ---------------------------------------------------------------------------------------

fn unsupported(gateway: &dyn RuntimeGateway, method: &str) -> RuntimeError {
    RuntimeError::new(
        "unsupported_method",
        format!("the server does not offer {method}; the action is unavailable"),
    )
    .with_endpoint(gateway.endpoint().to_owned())
}

fn is_unknown_method(error: &RuntimeError) -> bool {
    error.code.starts_with("invalid_request") && error.message.contains("unknown variant")
}

/// Errors after which a prompt may or may not have been delivered.
fn outcome_unknown(error: &RuntimeError) -> bool {
    matches!(
        error.code.as_str(),
        "timeout" | "connection_lost" | "empty_response" | "protocol_error"
    )
}

/// Gateway-agnostic state of one connection: discovered methods, kinds, agents, tabs and the
/// committed surface.
#[derive(Debug)]
pub struct AgentsCore {
    endpoint_methods: BTreeSet<String>,
    api_methods: BTreeMap<String, bool>,
    kinds: Vec<String>,
    agents: Vec<AgentDto>,
    tabs: Vec<TabDto>,
    store: FrameStore,
    /// pane_id → cwd reported by `pane.list` (the surface frame has none).
    pane_cwds: BTreeMap<String, String>,
    /// pane_id → manual name reported by `pane.list` (only panes that have one).
    pane_labels: BTreeMap<String, String>,
    /// Refusal of the last tab reconciliation (never swallowed: the window is told).
    tabs_error: Option<RuntimeError>,
    topology: Option<Topology>,
    /// Panes the engine reported creating (`pane.split`) that the committed surface has not
    /// caught up with yet: the window may start an agent in the pane it just created without
    /// waiting for a frame (spec 075). An id is dropped as soon as a surface lists it.
    created_panes: BTreeSet<String>,
    unknown_prompts: HashSet<(String, String, String)>,
    /// pane_id -> the state change already accounted for and the line read for it (spec 014).
    detection: BTreeMap<String, DetectionSeen>,
}

/// One observed state change of one pane: the engine state and sequence it belongs to, and the
/// last line of the detection snapshot read for it (`None` when the state does not wait for the
/// user, the server offers no read, or the read failed or came back empty). The state is part of
/// the key so a server that does not move `state_change_seq` is still followed.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DetectionSeen {
    seq: u64,
    status: AgentStatus,
    line: Option<String>,
}

/// Last non-empty line of a snapshot, with trailing blanks removed and nothing interpreted.
fn last_snapshot_line(text: &str) -> Option<String> {
    text.lines()
        .rev()
        .map(str::trim_end)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
}

impl AgentsCore {
    /// `endpoint_methods`: the methods announced by the generation-1 welcome.
    pub fn new(endpoint_methods: &[String]) -> Self {
        Self {
            endpoint_methods: endpoint_methods.iter().cloned().collect(),
            api_methods: BTreeMap::new(),
            kinds: Vec::new(),
            agents: Vec::new(),
            tabs: Vec::new(),
            store: FrameStore::new(),
            pane_cwds: BTreeMap::new(),
            pane_labels: BTreeMap::new(),
            tabs_error: None,
            topology: None,
            created_panes: BTreeSet::new(),
            unknown_prompts: HashSet::new(),
            detection: BTreeMap::new(),
        }
    }

    /// Discovers which agent/tab API methods exist and loads kinds, agents and tabs. Read-only
    /// methods are called for real; `agent.get`/`agent.start`/`agent.prompt` are probed with an
    /// empty params object that the engine rejects before executing anything (all have
    /// required fields), so an unknown-method error can be told apart from a present method.
    ///
    /// A remote server without the JSON API bridge (`remote_api_unsupported`) disables every
    /// API action at once, without probing the remaining methods; endpoint actions (split,
    /// focus, tabs) stay governed by the welcome. Any other failure fails the discovery.
    pub fn discover(&mut self, gateway: &dyn RuntimeGateway) -> Result<(), RuntimeError> {
        let mut bridge_missing = false;
        for method in API_METHODS {
            if bridge_missing {
                self.api_methods.insert((*method).to_owned(), false);
                continue;
            }
            let present = match gateway.api_request(method, json!({})) {
                Ok(result) => {
                    self.absorb(method, &result);
                    true
                }
                Err(error) if is_unknown_method(&error) => false,
                Err(error) if error.code == "remote_api_unsupported" => {
                    bridge_missing = true;
                    false
                }
                Err(error) if error.code.starts_with("invalid_request") => true,
                Err(error) => return Err(error),
            };
            self.api_methods.insert((*method).to_owned(), present);
        }
        // This connection observed no transition yet: every current state counts as already
        // seen, so discovery never reads a pane (spec 004 contract) and an agent that was
        // already waiting shows no line until it changes state again.
        self.detection = self
            .agents
            .iter()
            .map(|a| {
                (
                    a.pane_id.clone(),
                    DetectionSeen {
                        seq: a.state_change_seq,
                        status: a.status,
                        line: None,
                    },
                )
            })
            .collect();
        Ok(())
    }

    fn absorb(&mut self, method: &str, result: &Value) {
        match method {
            "server.agent_manifests" => {
                let mut kinds = Vec::new();
                for manifest in result["manifests"].as_array().into_iter().flatten() {
                    if let Some(kind) = manifest["agent"].as_str() {
                        if !kinds.iter().any(|k| k == kind) {
                            kinds.push(kind.to_owned());
                        }
                    }
                }
                self.kinds = kinds;
            }
            "agent.list" => {
                self.agents = result["agents"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(AgentDto::from_wire)
                    .collect();
            }
            "tab.list" => {
                self.tabs = result["tabs"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|t| {
                        Some(TabDto {
                            tab_id: t["tab_id"].as_str()?.to_owned(),
                            workspace_id: t["workspace_id"].as_str().unwrap_or_default().into(),
                            label: t["label"].as_str().unwrap_or_default().into(),
                            number: t["number"].as_u64().unwrap_or(0),
                            focused: t["focused"].as_bool().unwrap_or(false),
                            pane_count: t["pane_count"].as_u64().unwrap_or(0) as usize,
                            agent_status: t["agent_status"].as_str().unwrap_or("unknown").into(),
                        })
                    })
                    .collect();
            }
            "pane.list" => {
                self.pane_cwds = result["panes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|p| {
                        Some((
                            p["pane_id"].as_str()?.to_owned(),
                            p["cwd"].as_str()?.to_owned(),
                        ))
                    })
                    .collect();
                self.pane_labels = result["panes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|p| {
                        Some((
                            p["pane_id"].as_str()?.to_owned(),
                            p["label"].as_str()?.to_owned(),
                        ))
                    })
                    .collect();
            }
            _ => {}
        }
    }

    fn api(&self, method: &str) -> bool {
        self.api_methods.get(method).copied().unwrap_or(false)
    }

    fn announced(&self, method: &str) -> bool {
        self.endpoint_methods.contains(method)
    }

    pub fn capabilities(&self) -> Capabilities {
        Capabilities {
            list_agents: self.api("agent.list"),
            start_agent: self.api("agent.start") && !self.kinds.is_empty(),
            send_prompt: self.api("agent.prompt"),
            open_attention: self.announced("pane.focus"),
            split: self.announced("pane.split"),
            focus: self.announced("pane.focus"),
            split_ratio: self.announced("layout.set_split_ratio"),
            input: true,
            create_tab: self.announced("tab.create"),
            focus_tab: self.announced("tab.focus"),
            close_tab: self.announced("tab.close"),
            rename_tab: self.announced("tab.rename"),
            zoom: self.announced("pane.zoom"),
            rename_pane: self.announced("pane.rename"),
            swap: self.announced("pane.swap"),
            input_set: self.announced("pane.input.set"),
            close_pane: self.announced("pane.close"),
        }
    }

    pub fn kinds(&self) -> &[String] {
        &self.kinds
    }

    pub fn agents(&self) -> &[AgentDto] {
        &self.agents
    }

    pub fn tabs(&self) -> &[TabDto] {
        &self.tabs
    }

    pub fn topology(&self) -> Option<Topology> {
        self.topology.clone().map(|t| self.with_paths(t))
    }

    /// Copies the cwd the engine reported for each pane (`pane.list`) into the surface topology.
    fn with_paths(&self, mut topology: Topology) -> Topology {
        apply_paths(&mut topology, &self.pane_cwds);
        apply_labels(&mut topology, &self.pane_labels);
        topology
    }

    pub fn surface_state(&self) -> herdr_client::SurfaceState {
        self.store.state()
    }

    /// Panes whose agent status the event stream must follow.
    pub fn watched_panes(&self) -> Vec<String> {
        let mut panes: BTreeSet<String> = self.agents.iter().map(|a| a.pane_id.clone()).collect();
        if let Some(topology) = &self.topology {
            panes.extend(topology.panes.iter().map(|p| p.pane_id.clone()));
        }
        panes.into_iter().collect()
    }

    /// Commits a full surface. Returns the new topology only when it differs from the last.
    pub fn apply_surface(
        &mut self,
        frame: PaneSurfaceFrame,
    ) -> Result<Option<Topology>, StaleReason> {
        let boot_changed = self.store.apply_full(frame)?;
        if boot_changed {
            // Unknown outcomes belonged to the previous boot's agents.
            self.unknown_prompts.clear();
            // So did the panes a split of the previous boot created.
            self.created_panes.clear();
        }
        let surface = self.store.surface().expect("committed");
        // The committed surface carries the paths already read from the engine, so a frame never
        // reaches the window with the panes but without their cwd.
        let next = self.with_paths(Topology::from_surface(surface));
        let changed = self
            .topology
            .as_ref()
            .is_none_or(|previous| !previous.same_shape(&next));
        // A pane this surface lists is known from the topology alone now.
        self.created_panes
            .retain(|pane| !next.panes.iter().any(|p| &p.pane_id == pane));
        self.topology = Some(next.clone());
        Ok(changed.then_some(next))
    }

    pub fn apply_patch(&mut self, patch: PaneSurfacePatch) -> ApplyOutcome {
        self.store.apply_patch(patch)
    }

    /// Marks the surface stale (disconnection or queue overflow): input is refused until a
    /// full frame arrives.
    pub fn mark_stale(&mut self) {
        self.store.mark_stale(StaleReason::Disconnected);
    }

    fn check_target(
        &self,
        gateway: &dyn RuntimeGateway,
        target: &QualifiedTarget,
    ) -> Result<(), RuntimeError> {
        let live = gateway.identity().ok_or_else(|| {
            RuntimeError::new("boot_unknown", "the server identity is not established yet")
                .retryable()
                .with_endpoint(gateway.endpoint().to_owned())
        })?;
        target.validate(&live)
    }

    fn pane_in_topology(
        &self,
        gateway: &dyn RuntimeGateway,
        pane_id: &str,
    ) -> Result<(), RuntimeError> {
        let known = self
            .topology
            .as_ref()
            .is_some_and(|t| t.panes.iter().any(|p| p.pane_id == pane_id))
            // A pane the engine answered creating, before its surface frame arrives (spec 075).
            || self.created_panes.contains(pane_id);
        if known {
            Ok(())
        } else {
            Err(RuntimeError::new(
                "target_pane_missing",
                format!("pane {pane_id} is not in the tab the server confirmed"),
            )
            .with_endpoint(gateway.endpoint().to_owned()))
        }
    }

    fn agent_for(
        &self,
        gateway: &dyn RuntimeGateway,
        pane_id: &str,
    ) -> Result<&AgentDto, RuntimeError> {
        self.agents
            .iter()
            .find(|a| a.pane_id == pane_id)
            .ok_or_else(|| {
                RuntimeError::new(
                    "agent_not_found",
                    format!("no known agent in pane {pane_id}"),
                )
                .with_endpoint(gateway.endpoint().to_owned())
            })
    }

    fn upsert_agent(&mut self, agent: AgentDto) {
        match self.agents.iter_mut().find(|a| a.pane_id == agent.pane_id) {
            Some(existing) => *existing = agent,
            None => self.agents.push(agent),
        }
    }

    pub fn refresh_agents(
        &mut self,
        gateway: &dyn RuntimeGateway,
    ) -> Result<Vec<AgentDto>, RuntimeError> {
        if !self.api("agent.list") {
            return Err(unsupported(gateway, "agent.list"));
        }
        let result = gateway.api_request("agent.list", json!({}))?;
        self.absorb("agent.list", &result);
        self.read_new_attention(gateway);
        Ok(self.agents.clone())
    }

    /// One static `pane.read` snapshot for the home thumbnails (spec 012): exactly one request
    /// per call, for a pane the engine listed as an agent of the qualified host, and only when
    /// the server offers the method. A refusal sends nothing and is never retried here.
    pub fn read_pane(
        &self,
        gateway: &dyn RuntimeGateway,
        target: &QualifiedTarget,
        pane_id: &str,
        lines: u32,
    ) -> Result<PaneSnapshotDto, RuntimeError> {
        self.check_target(gateway, target)?;
        self.agent_for(gateway, pane_id)?;
        if !self.api("pane.read") {
            return Err(unsupported(gateway, "pane.read"));
        }
        let result = gateway.api_request(
            "pane.read",
            json!({
                "pane_id": pane_id,
                "source": "visible",
                "lines": lines.clamp(1, MAX_SNAPSHOT_LINES),
            }),
        )?;
        let read = &result["read"];
        Ok(PaneSnapshotDto {
            pane_id: read["pane_id"].as_str().unwrap_or(pane_id).to_owned(),
            text: read["text"].as_str().unwrap_or_default().to_owned(),
            revision: read["revision"].as_u64().unwrap_or(0),
            truncated: read["truncated"].as_bool().unwrap_or(false),
        })
    }

    /// One `agent.read` of the detection snapshot per state change into "waiting for the user"
    /// (spec 014): a state already accounted for is never read again, no other state is read,
    /// and a server without the method (or a failed read) simply leaves the line absent.
    fn read_new_attention(&mut self, gateway: &dyn RuntimeGateway) {
        let live: BTreeSet<String> = self.agents.iter().map(|a| a.pane_id.clone()).collect();
        self.detection.retain(|pane, _| live.contains(pane));
        let pending: Vec<(String, u64)> = self
            .agents
            .iter()
            .filter(|a| a.status == AgentStatus::Blocked)
            .filter(|a| {
                self.detection
                    .get(&a.pane_id)
                    .is_none_or(|seen| seen.seq != a.state_change_seq || seen.status != a.status)
            })
            .map(|a| (a.pane_id.clone(), a.state_change_seq))
            .collect();
        for (pane, seq) in pending {
            let line = if self.api("agent.read") {
                gateway
                    .api_request(
                        "agent.read",
                        json!({ "target": pane, "source": "detection", "lines": 1 }),
                    )
                    .ok()
                    .and_then(|result| {
                        last_snapshot_line(result["read"]["text"].as_str().unwrap_or_default())
                    })
            } else {
                None
            };
            self.detection.insert(
                pane,
                DetectionSeen {
                    seq,
                    status: AgentStatus::Blocked,
                    line,
                },
            );
        }
        self.attach_detection();
    }

    /// Copies each pane's read line onto its agent, only while the agent still reports the
    /// state change that line was read for.
    fn attach_detection(&mut self) {
        for agent in &mut self.agents {
            agent.detection_last_line = self
                .detection
                .get(&agent.pane_id)
                .filter(|seen| seen.seq == agent.state_change_seq && seen.status == agent.status)
                .and_then(|seen| seen.line.clone());
        }
    }

    pub fn refresh_tabs(
        &mut self,
        gateway: &dyn RuntimeGateway,
    ) -> Result<Vec<TabDto>, RuntimeError> {
        if !self.api("tab.list") {
            return Err(unsupported(gateway, "tab.list"));
        }
        let result = gateway.api_request("tab.list", json!({}))?;
        self.absorb("tab.list", &result);
        Ok(self.tabs.clone())
    }

    /// Working directories of the panes (`pane.list`), the only source of each pane's path.
    /// Returns true when they changed since the last read.
    pub fn refresh_panes(&mut self, gateway: &dyn RuntimeGateway) -> Result<bool, RuntimeError> {
        if !self.api("pane.list") {
            return Err(unsupported(gateway, "pane.list"));
        }
        let before = (self.pane_cwds.clone(), self.pane_labels.clone());
        let result = gateway.api_request("pane.list", json!({}))?;
        self.absorb("pane.list", &result);
        Ok((self.pane_cwds.clone(), self.pane_labels.clone()) != before)
    }

    /// Reads the tab list again after an action the engine accepted, so a created tab (or a new
    /// pane count) reaches the window even when no engine event announces it. A refusal is kept
    /// for the caller to publish, never discarded.
    fn reconcile_tabs(&mut self, gateway: &dyn RuntimeGateway) {
        if !self.api("tab.list") {
            return;
        }
        match self.refresh_tabs(gateway) {
            Ok(_) => self.tabs_error = None,
            Err(error) => self.tabs_error = Some(error),
        }
    }

    /// Takes the refusal of the last tab reconciliation (reported once).
    pub fn take_tabs_error(&mut self) -> Option<RuntimeError> {
        self.tabs_error.take()
    }

    /// The paths the core last read from the engine.
    pub fn pane_paths(&self) -> BTreeMap<String, String> {
        self.pane_cwds.clone()
    }

    /// The manual pane names the core last read from the engine (`pane.list`).
    pub fn pane_labels(&self) -> BTreeMap<String, String> {
        self.pane_labels.clone()
    }

    /// Agents whose managed launch the engine has not settled yet.
    pub fn launching_panes(&self) -> Vec<String> {
        self.agents
            .iter()
            .filter(|a| a.launch_pending)
            .map(|a| a.pane_id.clone())
            .collect()
    }

    /// The engine settles a managed launch (pending → ready) when `agent.get` reconciles the
    /// target; no event announces it. Only launching agents are reconciled.
    pub fn reconcile_launch(
        &mut self,
        gateway: &dyn RuntimeGateway,
        pane_id: &str,
    ) -> Result<AgentDto, RuntimeError> {
        if !self.api("agent.get") {
            return Err(unsupported(gateway, "agent.get"));
        }
        let launching = self
            .agents
            .iter()
            .any(|a| a.pane_id == pane_id && a.launch_pending);
        if !launching {
            return Err(RuntimeError::new(
                "agent_not_launching",
                format!("the agent of pane {pane_id} is not launching"),
            )
            .with_endpoint(gateway.endpoint().to_owned()));
        }
        let result = gateway.api_request("agent.get", json!({ "target": pane_id }))?;
        let agent = AgentDto::from_wire(&result["agent"]).ok_or_else(|| {
            RuntimeError::new("protocol_error", "agent.get responded without an agent")
                .with_endpoint(gateway.endpoint().to_owned())
        })?;
        self.upsert_agent(agent.clone());
        Ok(agent)
    }

    /// `agent.start` on the target pane with a kind the server announced, in the kind's manual
    /// mode (no autonomy flags) — the form of every caller that does not offer the switch.
    pub fn start_agent(
        &mut self,
        gateway: &dyn RuntimeGateway,
        target: &QualifiedTarget,
        kind: &str,
        name: &str,
    ) -> Result<AgentDto, RuntimeError> {
        self.start_agent_with(gateway, target, kind, name, false)
    }

    /// `agent.start` on the target pane, optionally with the kind's own autonomy flags (spec 076).
    /// `autonomous` is the whole choice the WebView can make: the arguments come from the table
    /// here, and a kind with no entry starts exactly as it would without the switch.
    pub fn start_agent_with(
        &mut self,
        gateway: &dyn RuntimeGateway,
        target: &QualifiedTarget,
        kind: &str,
        name: &str,
        autonomous: bool,
    ) -> Result<AgentDto, RuntimeError> {
        if !self.api("agent.start") {
            return Err(unsupported(gateway, "agent.start"));
        }
        self.check_target(gateway, target)?;
        let name = name.trim();
        if name.is_empty() || name.chars().count() > MAX_AGENT_NAME_CHARS {
            return Err(RuntimeError::new(
                "invalid_agent_name",
                "enter an agent name (up to 64 characters)",
            )
            .with_endpoint(target.endpoint.clone()));
        }
        if !self.kinds.iter().any(|k| k == kind) {
            return Err(RuntimeError::new(
                "agent_kind_unavailable",
                format!("the server does not announce the {kind} agent kind"),
            )
            .with_endpoint(target.endpoint.clone()));
        }
        self.pane_in_topology(gateway, &target.pane_id)?;
        let mut params = json!({ "name": name, "kind": kind, "pane_id": target.pane_id });
        // Only an autonomous start carries arguments, and only the ones this table names: a kind
        // with no entry sends no `args` at all, like the engine's own default launch.
        let flags = if autonomous {
            autonomy_flags(kind)
        } else {
            &[][..]
        };
        if !flags.is_empty() {
            params["args"] = json!(flags);
        }
        let result = gateway.api_request("agent.start", params)?;
        let agent = AgentDto::from_wire(&result["agent"]).ok_or_else(|| {
            RuntimeError::new("protocol_error", "agent.start responded without an agent")
                .with_endpoint(target.endpoint.clone())
        })?;
        self.upsert_agent(agent.clone());
        Ok(agent)
    }

    /// `agent.prompt` with the text exactly as typed. Sent once per call; after an unknown
    /// outcome the same pane refuses until `resend_after_unknown` is set by the user.
    pub fn send_prompt(
        &mut self,
        gateway: &dyn RuntimeGateway,
        target: &QualifiedTarget,
        text: &str,
        resend_after_unknown: bool,
    ) -> Result<PromptOutcome, RuntimeError> {
        if !self.api("agent.prompt") {
            return Err(unsupported(gateway, "agent.prompt"));
        }
        self.check_target(gateway, target)?;
        if text.trim().is_empty() || text.len() > MAX_PROMPT_BYTES {
            return Err(RuntimeError::new(
                "empty_prompt",
                "write the prompt before sending (up to 64 KiB)",
            )
            .with_endpoint(target.endpoint.clone()));
        }
        let agent = self.agent_for(gateway, &target.pane_id)?;
        if agent.status == AgentStatus::Blocked {
            return Err(RuntimeError::new(
                "agent_blocked",
                "the agent is waiting for an answer in the terminal; open the pane to reply",
            )
            .with_endpoint(target.endpoint.clone()));
        }
        let key = (
            target.endpoint.clone(),
            target.session.clone(),
            target.pane_id.clone(),
        );
        if self.unknown_prompts.contains(&key) && !resend_after_unknown {
            return Err(RuntimeError::new(
                "prompt_outcome_unknown",
                "the previous send had no confirmed result; check the pane before sending again",
            )
            .with_endpoint(target.endpoint.clone()));
        }
        match gateway.api_request(
            "agent.prompt",
            json!({ "target": target.pane_id, "text": text }),
        ) {
            Ok(result) => {
                self.unknown_prompts.remove(&key);
                let agent = AgentDto::from_wire(&result["agent"]).ok_or_else(|| {
                    RuntimeError::new("protocol_error", "agent.prompt responded without an agent")
                        .with_endpoint(target.endpoint.clone())
                })?;
                self.upsert_agent(agent.clone());
                Ok(PromptOutcome::Sent { agent })
            }
            Err(error) if outcome_unknown(&error) => {
                self.unknown_prompts.insert(key);
                Ok(PromptOutcome::Unknown { error })
            }
            Err(error) => {
                self.unknown_prompts.remove(&key);
                Err(error)
            }
        }
    }

    /// User action on an agent that needs attention: focus its pane. Never sends input.
    pub fn open_attention(
        &mut self,
        gateway: &dyn RuntimeGateway,
        target: &QualifiedTarget,
    ) -> Result<(), RuntimeError> {
        if !self.announced("pane.focus") {
            return Err(unsupported(gateway, "pane.focus"));
        }
        self.check_target(gateway, target)?;
        self.agent_for(gateway, &target.pane_id)?;
        gateway
            .endpoint_request("pane.focus", json!({ "pane_id": target.pane_id }))
            .map(|_| ())
    }

    /// `pane.split` of the target pane, answering the pane the engine created so the window can
    /// act on it at once (spec 075): its own topology only learns of it on the next frame.
    pub fn split(
        &mut self,
        gateway: &dyn RuntimeGateway,
        target: &QualifiedTarget,
        direction: &str,
    ) -> Result<PaneSplitReceipt, RuntimeError> {
        if !self.announced("pane.split") {
            return Err(unsupported(gateway, "pane.split"));
        }
        self.check_target(gateway, target)?;
        if direction != "right" && direction != "down" {
            return Err(RuntimeError::new(
                "invalid_split_direction",
                "the split direction must be right or down",
            )
            .with_endpoint(target.endpoint.clone()));
        }
        self.pane_in_topology(gateway, &target.pane_id)?;
        let result = gateway.endpoint_request(
            "pane.split",
            json!({ "target_pane_id": target.pane_id, "direction": direction, "focus": false }),
        )?;
        // The engine's `PaneInfo`: the created pane becomes a valid target right away, so an
        // `agent.start` on it is not refused while the surface frame is still on its way.
        let pane_id = result
            .get("pane")
            .and_then(|pane| pane.get("pane_id"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        if let Some(pane_id) = pane_id.clone() {
            self.created_panes.insert(pane_id);
        }
        // The new pane changes the tab's pane count; the window must not depend on an event for it.
        self.reconcile_tabs(gateway);
        Ok(PaneSplitReceipt { pane_id })
    }

    pub fn focus_pane(
        &mut self,
        gateway: &dyn RuntimeGateway,
        target: &QualifiedTarget,
    ) -> Result<(), RuntimeError> {
        if !self.announced("pane.focus") {
            return Err(unsupported(gateway, "pane.focus"));
        }
        self.check_target(gateway, target)?;
        self.pane_in_topology(gateway, &target.pane_id)?;
        gateway
            .endpoint_request("pane.focus", json!({ "pane_id": target.pane_id }))
            .map(|_| ())
    }

    pub fn close_pane(
        &mut self,
        gateway: &dyn RuntimeGateway,
        target: &QualifiedTarget,
    ) -> Result<(), RuntimeError> {
        if !self.announced("pane.close") {
            return Err(unsupported(gateway, "pane.close"));
        }
        self.check_target(gateway, target)?;
        self.pane_in_topology(gateway, &target.pane_id)?;
        match gateway.endpoint_request("pane.close", json!({ "pane_id": target.pane_id })) {
            Ok(_) => {
                self.reconcile_tabs(gateway);
                Ok(())
            }
            Err(error) if is_unknown_method(&error) => Err(unsupported(gateway, "pane.close")),
            Err(error) => Err(error),
        }
    }

    /// Expand (or restore) one pane of the active tab in the engine. One request per user action;
    /// a refused target or mode sends nothing. The engine's `zoom.zoomed` travels back so the
    /// context menu can offer `Desfazer zoom` without reading anything else.
    pub fn zoom_pane(
        &mut self,
        gateway: &dyn RuntimeGateway,
        target: &QualifiedTarget,
        mode: &str,
    ) -> Result<PaneZoomReceipt, RuntimeError> {
        if !self.announced("pane.zoom") {
            return Err(unsupported(gateway, "pane.zoom"));
        }
        self.check_target(gateway, target)?;
        if !ZOOM_MODES.contains(&mode) {
            return Err(RuntimeError::new(
                "invalid_zoom_mode",
                "the zoom mode must be toggle, on or off",
            )
            .with_endpoint(target.endpoint.clone()));
        }
        self.pane_in_topology(gateway, &target.pane_id)?;
        let result = gateway.endpoint_request(
            "pane.zoom",
            json!({ "pane_id": target.pane_id, "mode": mode }),
        )?;
        Ok(PaneZoomReceipt {
            pane_id: target.pane_id.clone(),
            zoomed: result
                .get("zoom")
                .and_then(|zoom| zoom.get("zoomed"))
                .and_then(Value::as_bool),
        })
    }

    /// Sets (or clears, with `None`) the manual name of one pane the topology lists
    /// (`pane.rename`). The manual names read from `pane.list` are refreshed afterwards so the
    /// menu's `Limpar nome do pane` follows the engine; a failed refresh does not fail the rename.
    pub fn rename_pane(
        &mut self,
        gateway: &dyn RuntimeGateway,
        target: &QualifiedTarget,
        pane_id: &str,
        label: Option<&str>,
    ) -> Result<(), RuntimeError> {
        if !self.announced("pane.rename") {
            return Err(unsupported(gateway, "pane.rename"));
        }
        self.check_target(gateway, target)?;
        self.pane_in_topology(gateway, pane_id)?;
        let label = label
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .map(str::to_owned);
        if label
            .as_deref()
            .is_some_and(|label| label.chars().count() > MAX_PANE_LABEL_CHARS)
        {
            return Err(RuntimeError::new(
                "invalid_pane_label",
                format!("the pane name exceeds {MAX_PANE_LABEL_CHARS} characters"),
            )
            .with_endpoint(target.endpoint.clone()));
        }
        let params = match &label {
            Some(label) => json!({ "pane_id": pane_id, "label": label }),
            None => json!({ "pane_id": pane_id }),
        };
        gateway.endpoint_request("pane.rename", params)?;
        let _ = self.refresh_panes(gateway);
        Ok(())
    }

    /// Exchanges the pane `pane_id` (the one right-clicked) with the confirmed focused pane
    /// `target.pane_id` (`pane.swap`), then focuses the source again, as the TUI does. Both panes
    /// must be in the topology this connection confirmed; a no-op swap is refused here.
    pub fn swap_pane(
        &mut self,
        gateway: &dyn RuntimeGateway,
        target: &QualifiedTarget,
        pane_id: &str,
    ) -> Result<(), RuntimeError> {
        if !self.announced("pane.swap") {
            return Err(unsupported(gateway, "pane.swap"));
        }
        self.check_target(gateway, target)?;
        self.pane_in_topology(gateway, &target.pane_id)?;
        self.pane_in_topology(gateway, pane_id)?;
        if target.pane_id == pane_id {
            return Err(RuntimeError::new(
                "swap_same_pane",
                "the given pane is already the focused one; nothing was sent",
            )
            .with_endpoint(target.endpoint.clone()));
        }
        gateway.endpoint_request(
            "pane.swap",
            json!({ "source_pane_id": target.pane_id, "target_pane_id": pane_id }),
        )?;
        gateway.endpoint_request("pane.focus", json!({ "pane_id": target.pane_id }))?;
        let _ = self.refresh_panes(gateway);
        Ok(())
    }

    /// Chooses whether right clicks on `pane_id` go to the pane (`pane.input.set`, right_click
    /// `pane`) or open the Herdr menu (`herdr`), exactly as the TUI context menu toggles it.
    pub fn set_pane_right_click(
        &mut self,
        gateway: &dyn RuntimeGateway,
        target: &QualifiedTarget,
        pane_id: &str,
        passthrough: bool,
    ) -> Result<(), RuntimeError> {
        if !self.announced("pane.input.set") {
            return Err(unsupported(gateway, "pane.input.set"));
        }
        self.check_target(gateway, target)?;
        self.pane_in_topology(gateway, pane_id)?;
        gateway
            .endpoint_request(
                "pane.input.set",
                json!({
                    "pane_id": pane_id,
                    "right_click": if passthrough { "pane" } else { "herdr" },
                }),
            )
            .map(|_| ())
    }

    pub fn set_split_ratio(
        &mut self,
        gateway: &dyn RuntimeGateway,
        target: &QualifiedTarget,
        path: &[bool],
        ratio: f32,
    ) -> Result<(), RuntimeError> {
        if !self.announced("layout.set_split_ratio") {
            return Err(unsupported(gateway, "layout.set_split_ratio"));
        }
        self.check_target(gateway, target)?;
        if !ratio.is_finite() || !(MIN_RATIO..=MAX_RATIO).contains(&ratio) {
            return Err(RuntimeError::new(
                "invalid_ratio",
                "the split ratio must be between 0.1 and 0.9",
            )
            .with_endpoint(target.endpoint.clone()));
        }
        self.pane_in_topology(gateway, &target.pane_id)?;
        let known = self
            .topology
            .as_ref()
            .is_some_and(|t| t.splits.iter().any(|s| s.path == path));
        if !known {
            return Err(RuntimeError::new(
                "split_not_found",
                "the given split does not exist in the server's topology",
            )
            .with_endpoint(target.endpoint.clone()));
        }
        gateway
            .endpoint_request(
                "layout.set_split_ratio",
                json!({ "pane_id": target.pane_id, "path": path, "ratio": ratio }),
            )
            .map(|_| ())
    }

    /// Input to the pane the server confirmed focused on a live surface; nothing is queued
    /// or replayed when refused.
    pub fn send_input(
        &self,
        gateway: &dyn RuntimeGateway,
        target: &QualifiedTarget,
        events: Vec<ClientPaneInputEvent>,
    ) -> Result<(), RuntimeError> {
        self.check_target(gateway, target)?;
        if !self.store.input_allowed() {
            return Err(RuntimeError::new(
                "surface_stale",
                "stale surface: input is blocked until it resynchronizes",
            )
            .retryable()
            .with_endpoint(target.endpoint.clone()));
        }
        let confirmed = self
            .topology
            .as_ref()
            .and_then(|t| t.focused_pane_id.as_deref());
        if confirmed != Some(target.pane_id.as_str()) {
            return Err(RuntimeError::new(
                "pane_not_confirmed",
                "the focus on this pane has not been confirmed by the server yet; the input was dropped",
            )
            .retryable()
            .with_endpoint(target.endpoint.clone()));
        }
        gateway.send_input(target, events)
    }

    pub fn create_tab(
        &mut self,
        gateway: &dyn RuntimeGateway,
        target: &QualifiedTarget,
    ) -> Result<(), RuntimeError> {
        if !self.announced("tab.create") {
            return Err(unsupported(gateway, "tab.create"));
        }
        self.check_target(gateway, target)?;
        self.pane_in_topology(gateway, &target.pane_id)?;
        // Public pane ids are `{workspace_id}:p{n}`: the workspace comes from the validated
        // pane; a different workspace claimed by the WebView is refused.
        let workspace = target
            .pane_id
            .split_once(':')
            .map(|(workspace, _)| workspace)
            .filter(|workspace| !workspace.is_empty())
            .ok_or_else(|| {
                RuntimeError::new(
                    "target_pane_missing",
                    format!("pane {} does not identify a workspace", target.pane_id),
                )
                .with_endpoint(target.endpoint.clone())
            })?;
        if target
            .workspace_id
            .as_deref()
            .is_some_and(|claimed| claimed != workspace)
        {
            return Err(RuntimeError::new(
                "target_workspace_mismatch",
                format!(
                    "pane {} does not belong to the given workspace; the action was cancelled",
                    target.pane_id
                ),
            )
            .with_endpoint(target.endpoint.clone()));
        }
        gateway.endpoint_request(
            "tab.create",
            json!({ "workspace_id": workspace, "focus": true }),
        )?;
        // The created tab reaches the window from the engine's own list, not from an event.
        self.reconcile_tabs(gateway);
        Ok(())
    }

    /// Reads the tab list again when the connection confirmed another tab in focus: the list the
    /// window shows must contain the tab it is focused on, event or no event.
    pub fn reconcile_tabs_for_focus(
        &mut self,
        gateway: &dyn RuntimeGateway,
        tab_id: Option<&str>,
    ) -> Vec<TabDto> {
        let known = tab_id.is_some_and(|id| self.tabs.iter().any(|t| t.tab_id == id));
        if !known {
            self.reconcile_tabs(gateway);
        }
        self.tabs.clone()
    }

    pub fn focus_tab(
        &mut self,
        gateway: &dyn RuntimeGateway,
        target: &QualifiedTarget,
        tab_id: &str,
    ) -> Result<(), RuntimeError> {
        if !self.announced("tab.focus") {
            return Err(unsupported(gateway, "tab.focus"));
        }
        self.check_target(gateway, target)?;
        self.pane_in_topology(gateway, &target.pane_id)?;
        if self.api("tab.list")
            && !self.tabs.is_empty()
            && !self.tabs.iter().any(|t| t.tab_id == tab_id)
        {
            return Err(RuntimeError::new(
                "tab_not_found",
                format!("tab {tab_id} does not exist on this server"),
            )
            .with_endpoint(target.endpoint.clone()));
        }
        gateway
            .endpoint_request("tab.focus", json!({ "tab_id": tab_id }))
            .map(|_| ())
    }

    pub fn close_tab(
        &mut self,
        gateway: &dyn RuntimeGateway,
        target: &QualifiedTarget,
        tab_id: &str,
    ) -> Result<(), RuntimeError> {
        if !self.announced("tab.close") {
            return Err(unsupported(gateway, "tab.close"));
        }
        self.check_target(gateway, target)?;
        self.pane_in_topology(gateway, &target.pane_id)?;
        if self.api("tab.list")
            && !self.tabs.is_empty()
            && !self.tabs.iter().any(|t| t.tab_id == tab_id)
        {
            return Err(RuntimeError::new(
                "tab_not_found",
                format!("tab {tab_id} does not exist on this server"),
            )
            .with_endpoint(target.endpoint.clone()));
        }
        match gateway.endpoint_request("tab.close", json!({ "tab_id": tab_id })) {
            Ok(_) => {
                self.reconcile_tabs(gateway);
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    pub fn rename_tab(
        &mut self,
        gateway: &dyn RuntimeGateway,
        target: &QualifiedTarget,
        tab_id: &str,
        label: &str,
    ) -> Result<(), RuntimeError> {
        if !self.announced("tab.rename") {
            return Err(unsupported(gateway, "tab.rename"));
        }
        self.check_target(gateway, target)?;
        self.pane_in_topology(gateway, &target.pane_id)?;
        if self.api("tab.list")
            && !self.tabs.is_empty()
            && !self.tabs.iter().any(|t| t.tab_id == tab_id)
        {
            return Err(RuntimeError::new(
                "tab_not_found",
                format!("tab {tab_id} does not exist on this server"),
            )
            .with_endpoint(target.endpoint.clone()));
        }
        let label = label.trim();
        if label.is_empty() {
            return Err(
                RuntimeError::new("invalid_input", "the tab name cannot be empty")
                    .with_endpoint(target.endpoint.clone()),
            );
        }
        match gateway.endpoint_request("tab.rename", json!({ "tab_id": tab_id, "label": label })) {
            Ok(_) => {
                self.reconcile_tabs(gateway);
                Ok(())
            }
            Err(error) => Err(error),
        }
    }
}

/// `events.subscribe` params: agent status per watched pane plus structural pane/tab events.
/// Output events are never subscribed (no pane output polling from the desktop).
pub fn event_subscriptions(pane_ids: &[String]) -> Value {
    let mut subscriptions = vec![
        json!({ "type": "pane.agent_detected" }),
        json!({ "type": "pane.updated" }),
        json!({ "type": "pane.created" }),
        json!({ "type": "pane.closed" }),
        json!({ "type": "pane.exited" }),
        json!({ "type": "tab.created" }),
        json!({ "type": "tab.focused" }),
        json!({ "type": "tab.closed" }),
        json!({ "type": "tab.renamed" }),
        // Workspace lifecycle (spec 027): the focused workspace and the tree must follow a
        // workspace created, renamed or closed in another client, not only a tab event.
        json!({ "type": "workspace.created" }),
        json!({ "type": "workspace.renamed" }),
        json!({ "type": "workspace.closed" }),
    ];
    for pane in pane_ids {
        subscriptions.push(json!({ "type": "pane.agent_status_changed", "pane_id": pane }));
    }
    json!({ "subscriptions": subscriptions })
}

// ---------------------------------------------------------------------------------------
// Tauri state
// ---------------------------------------------------------------------------------------

/// What the host reports about the committed surface of one of its connections (composed window).
/// Delivered by the host's frame thread without any host lock held.
pub enum SurfaceSignal<'a> {
    /// A full surface of the connection `identity` was committed.
    Full {
        identity: &'a LiveIdentity,
        frame: &'a PaneSurfaceFrame,
        /// Tab focus of that connection confirmed for `frame.projection_revision`, if any.
        confirmed_tab: Option<Option<String>>,
    },
    /// The connection `identity` ended (lost, failed or dropped by the window).
    Ended {
        identity: &'a LiveIdentity,
        error: &'a RuntimeError,
    },
    /// Connection `generation` of `endpoint` started: every older connection of it ended.
    Replaced { endpoint: &'a str, generation: u64 },
}

/// Receiver of [`SurfaceSignal`]s. It must not block nor call back into the host.
pub type SurfaceListener = Arc<dyn Fn(SurfaceSignal<'_>) + Send + Sync>;

/// Host selected by the composed window (007). The shared connection hub owns the connection;
/// agents only borrow a gateway view of it per action and never connect or detach it.
///
/// Implementations live outside this module (it is compiled in isolation by the 004 tests).
pub trait AgentsHost: Send + Sync {
    /// Endpoint currently selected ("local" or the SSH profile id).
    fn endpoint(&self) -> String;
    /// Gateway view bound to the selected host's current connection (identity, JSON API lane,
    /// endpoint commands, input). Fails while the host is not online.
    fn gateway(&self) -> Result<Box<dyn RuntimeGateway>, RuntimeError>;
    /// Endpoint methods announced by the welcome of the selected host's connection.
    fn endpoint_methods(&self) -> Result<Vec<String>, RuntimeError>;
    /// Surface committed by the host for the selected connection, if visible.
    fn surface(&self) -> Option<PaneSurfaceFrame>;
    /// Byte stream for one `events.subscribe` request on the JSON API of the connection
    /// `attached` (captured by the caller before it started the watcher). It must never open or
    /// write on another host or connection, even when the selection changed meanwhile or pane
    /// ids match; reads must time out periodically so a stopped watcher ends.
    fn event_stream(&self, attached: &LiveIdentity) -> Result<Box<dyn EventStream>, RuntimeError>;
    /// Installs the one listener of surface changes of this host's connections (`None` removes
    /// it). Hosts without notifications ignore it: the topology is then only refreshed by
    /// actions, as before.
    fn listen_surface(&self, _listener: Option<SurfaceListener>) {}
    /// Tab focus of the connection `identity` confirmed for `projection_revision` (snapshot and
    /// committed full surface of that connection). Hosts without it confirm nothing.
    fn confirmed_tab_focus(
        &self,
        _identity: &LiveIdentity,
        _projection_revision: u64,
    ) -> Option<Option<String>> {
        None
    }
}

/// Copies the engine's paths into a topology: only `cwd` may change. The pane set, their order and
/// the confirmed focus belong to the surface, so a path refresh never republishes another focus
/// (spec 013, gate r3: a topology with a foreign focus sent actions to a pane the engine had not).
/// Returns true when some path changed.
pub fn apply_paths(topology: &mut Topology, paths: &BTreeMap<String, String>) -> bool {
    let mut changed = false;
    for pane in &mut topology.panes {
        let next = paths.get(&pane.pane_id).cloned();
        if pane.cwd != next {
            pane.cwd = next;
            changed = true;
        }
    }
    changed
}

/// Copies the engine's manual pane names into a topology: only `label` may change. A pane the
/// engine reports without a name loses the previous one (a cleared name disappears).
pub fn apply_labels(topology: &mut Topology, labels: &BTreeMap<String, String>) -> bool {
    let mut changed = false;
    for pane in &mut topology.panes {
        let next = labels.get(&pane.pane_id).cloned();
        if pane.label != next {
            pane.label = next;
            changed = true;
        }
    }
    changed
}

/// pane_id → cwd published by the engine (`pane.list`), shared by the core's refreshes, the
/// hosted surface feed and the event watcher: every topology that reaches the WebView carries the
/// same paths, whichever thread published it.
pub type PanePaths = Arc<Mutex<BTreeMap<String, String>>>;

/// pane_id → manual name published by the engine (`pane.list`), shared like [`PanePaths`].
pub type PaneLabels = Arc<Mutex<BTreeMap<String, String>>>;

/// Topology published to the channel of the current hosted attach, outside the core lock: the
/// host's frame thread updates it without waiting on actions that may wait on that thread.
#[derive(Default)]
struct TopologyFeed {
    /// Connection the agents are attached to; `None` once it ended or before an attach.
    attached: Option<LiveIdentity>,
    channel: Option<Channel<AgentsEvent>>,
    /// Last topology the channel knows (attach capture or last event).
    published: Option<Topology>,
    /// Last tab focus confirmed by the attached connection (attach capture or last event).
    tab_focus: Option<TabFocus>,
    /// Paths of the panes, shared with the core (`None` before the first attach).
    pane_paths: Option<PanePaths>,
    /// Manual pane names, shared with the core (`None` before the first attach).
    pane_labels: Option<PaneLabels>,
}

impl TopologyFeed {
    fn clear(&mut self) {
        *self = Self::default();
    }

    /// Republishes the topology the channel already has with the paths and manual names read from
    /// the engine. Only `cwd` and `label` may change here: the pane set and the confirmed focus
    /// come from the surface.
    fn republish_paths(&mut self) -> Option<AgentsEvent> {
        let paths = self.pane_paths.as_ref()?;
        let paths = paths.lock().unwrap_or_else(PoisonError::into_inner).clone();
        let labels = self
            .pane_labels
            .as_ref()
            .map(|labels| {
                labels
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone()
            })
            .unwrap_or_default();
        let mut topology = self.published.clone()?;
        let paths_changed = apply_paths(&mut topology, &paths);
        let labels_changed = apply_labels(&mut topology, &labels);
        if !paths_changed && !labels_changed {
            return None;
        }
        self.published = Some(topology.clone());
        Some(AgentsEvent::Topology { topology })
    }

    /// Publishes only layout/focus/geometry changes of the attached connection, once each.
    fn signal(&mut self, signal: SurfaceSignal<'_>) {
        let Some(attached) = self.attached.as_ref() else {
            return;
        };
        let ended = match signal {
            SurfaceSignal::Full {
                identity,
                frame,
                confirmed_tab,
            } => {
                if identity != attached || frame.boot_id != attached.boot_id {
                    return;
                }
                if let Some(tab_id) = confirmed_tab {
                    let changed = self
                        .tab_focus
                        .as_ref()
                        .is_none_or(|current| current.tab_id != tab_id);
                    let focus = TabFocus::confirmed(attached, frame.projection_revision, tab_id);
                    self.tab_focus = Some(focus.clone());
                    if changed {
                        if let Some(channel) = self.channel.as_ref() {
                            let _ = channel.send(AgentsEvent::TabFocus { focus });
                        }
                    }
                }
                let mut next = Topology::from_surface(frame);
                if let Some(paths) = self.pane_paths.as_ref() {
                    let paths = paths.lock().unwrap_or_else(PoisonError::into_inner);
                    for pane in &mut next.panes {
                        pane.cwd = paths.get(&pane.pane_id).cloned();
                    }
                }
                if let Some(labels) = self.pane_labels.as_ref() {
                    let labels = labels.lock().unwrap_or_else(PoisonError::into_inner);
                    for pane in &mut next.panes {
                        pane.label = labels.get(&pane.pane_id).cloned();
                    }
                }
                if self
                    .published
                    .as_ref()
                    .is_some_and(|published| published.same_shape(&next))
                {
                    return;
                }
                self.published = Some(next.clone());
                if let Some(channel) = self.channel.as_ref() {
                    let _ = channel.send(AgentsEvent::Topology { topology: next });
                }
                return;
            }
            SurfaceSignal::Ended { identity, error } if identity == attached => error.clone(),
            SurfaceSignal::Replaced {
                endpoint,
                generation,
            } if endpoint == attached.endpoint && generation > attached.connection_generation => {
                RuntimeError::new(
                    "connection_replaced",
                    "the agents' connection was replaced; attach to the host again",
                )
                .retryable()
                .with_endpoint(endpoint)
            }
            _ => return,
        };
        // The confirmation of that connection is gone; the next attach starts a new one.
        if let Some(channel) = self.channel.take() {
            let _ = channel.send(AgentsEvent::State {
                state: "disconnected".into(),
                error: Some(ended),
            });
        }
        self.clear();
    }
}

struct Watcher {
    panes: Vec<String>,
    stop: Arc<AtomicBool>,
}

struct Inner {
    config_dir: PathBuf,
    session: Option<SessionName>,
    gateway: Option<LocalGateway>,
    negotiated: Option<Negotiated>,
    core: Option<AgentsCore>,
    geometry: Option<SurfaceGeometry>,
    channel: Option<Channel<AgentsEvent>>,
    bridge: Option<JoinHandle<()>>,
    watcher: Option<Watcher>,
    /// Panes with a running launch reconciler, per connection.
    reconciling: HashSet<String>,
    last_identity: Option<LiveIdentity>,
    last_error: Option<RuntimeError>,
    /// Composed window: the selected host, and the endpoint the core was attached to.
    host: Option<Arc<dyn AgentsHost>>,
    hosted_endpoint: Option<String>,
    /// Paths of the panes, shared with the hosted surface feed.
    pane_paths: PanePaths,
    /// Manual pane names, shared with the hosted surface feed.
    pane_labels: PaneLabels,
    /// The hosted surface feed: the only publisher of pane sets and focus in the composed window.
    feed: Arc<Mutex<TopologyFeed>>,
}

/// Managed state. Clones share the same service (inner state and command lane).
#[derive(Clone)]
pub struct AgentsState {
    inner: Arc<Mutex<Inner>>,
    lane: Arc<CommandLane>,
    /// Hosted only: topology pushed by the host's surface notifications.
    feed: Arc<Mutex<TopologyFeed>>,
    /// Paths of the panes read from `pane.list`; the same map the core and the feed use.
    pane_paths: PanePaths,
    /// Manual pane names read from `pane.list`; the same map the core and the feed use.
    pane_labels: PaneLabels,
}

impl AgentsState {
    pub fn new(config_dir: PathBuf, session: Option<SessionName>) -> Self {
        let paths: PanePaths = Arc::default();
        let labels: PaneLabels = Arc::default();
        let feed: Arc<Mutex<TopologyFeed>> = Arc::default();
        Self {
            inner: Arc::new(Mutex::new(Inner {
                config_dir,
                session,
                gateway: None,
                negotiated: None,
                core: None,
                geometry: None,
                channel: None,
                bridge: None,
                watcher: None,
                reconciling: HashSet::new(),
                last_identity: None,
                last_error: None,
                host: None,
                hosted_endpoint: None,
                pane_paths: paths.clone(),
                pane_labels: labels.clone(),
                feed: feed.clone(),
            })),
            lane: Arc::new(CommandLane::default()),
            feed,
            pane_paths: paths,
            pane_labels: labels,
        }
    }

    /// Agents of the composed window, bound to the selected host of the shared hub.
    pub fn hosted(host: Arc<dyn AgentsHost>) -> Self {
        let state = Self::new(PathBuf::new(), None);
        let feed = Arc::downgrade(&state.feed);
        host.listen_surface(Some(Arc::new(move |signal| {
            if let Some(feed) = feed.upgrade() {
                feed.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .signal(signal);
            }
        })));
        state.inner.lock().expect("agents state").host = Some(host);
        state
    }

    /// Attaches the core to the host selected now: discovery and actions go to that host
    /// only. A later selection change makes actions fail with `selection_changed` until the
    /// window attaches again; nothing is retargeted silently.
    pub fn attach_hosted(
        &self,
        channel: Option<Channel<AgentsEvent>>,
    ) -> Result<Overview, RuntimeError> {
        self.detach();
        let shared = self.inner.clone();
        let mut guard = shared.lock().expect("agents state");
        let inner = &mut *guard;
        let Some(host) = inner.host.clone() else {
            return Err(RuntimeError::new(
                "no_host",
                "no host is selected for agents",
            ));
        };
        let endpoint = host.endpoint();
        let attached = (|| {
            let gateway = host.gateway()?;
            if gateway.endpoint() != endpoint {
                return Err(selection_changed(&endpoint));
            }
            let identity = gateway.identity().ok_or_else(|| {
                RuntimeError::new("boot_unknown", "the server identity is not established yet")
                    .retryable()
                    .with_endpoint(endpoint.clone())
            })?;
            let mut core = AgentsCore::new(&host.endpoint_methods()?);
            {
                // Feed first, then the capture, under the feed lock: a full surface committed
                // after the capture is published, one committed before it is the same shape.
                let mut feed = self.feed.lock().unwrap_or_else(PoisonError::into_inner);
                if let Some(frame) = host.surface() {
                    feed.tab_focus = host
                        .confirmed_tab_focus(&identity, frame.projection_revision)
                        .map(|tab| TabFocus::confirmed(&identity, frame.projection_revision, tab));
                    let _ = core.apply_surface(frame);
                }
                feed.attached = Some(identity.clone());
                feed.channel = channel.clone();
                feed.published = core.topology();
            }
            core.discover(&*gateway)?;
            if core.api("tab.list") {
                let _ = core.refresh_tabs(&*gateway);
            }
            {
                let paths = self.pane_paths.clone();
                *paths.lock().unwrap_or_else(PoisonError::into_inner) = core.pane_paths();
                let labels = self.pane_labels.clone();
                *labels.lock().unwrap_or_else(PoisonError::into_inner) = core.pane_labels();
                let mut feed = self.feed.lock().unwrap_or_else(PoisonError::into_inner);
                feed.pane_paths = Some(paths);
                feed.pane_labels = Some(labels);
                feed.published = core.topology();
            }
            // Revalidated after the I/O (feed lock not held during it): when the host reported
            // this connection ended meanwhile, the channel was already told `disconnected`, and
            // the attach must not confirm it again.
            let current = self
                .feed
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .attached
                .as_ref()
                == Some(&identity);
            if !current {
                return Err(RuntimeError::new(
                    "connection_lost",
                    "the agents' connection ended during the attach; attach to the host again",
                )
                .retryable()
                .with_endpoint(endpoint.clone()));
            }
            Ok((identity, core))
        })();
        match attached {
            Err(error) => {
                self.feed
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clear();
                inner.last_error = Some(error.clone());
                Err(error)
            }
            Ok((identity, core)) => {
                inner.hosted_endpoint = Some(endpoint);
                inner.last_identity = Some(identity);
                inner.core = Some(core);
                inner.channel = channel;
                inner.last_error = None;
                ensure_watcher(inner, &shared);
                ensure_launch_reconcilers(inner, &shared);
                let mut overview = overview_of(inner);
                overview.tab_focus = self.confirmed_tab_focus();
                Ok(overview)
            }
        }
    }

    /// The window's view of the attached connection. With `reconcile` the engine's tab list is
    /// read once before answering (spec 027: the window asks after coalesced lifecycle events and
    /// after a refused `tab.focus`); without it the pre-027 rule stays: a confirmed tab the list
    /// does not have is read again. A host that is not attached is never read.
    pub fn overview(&self, reconcile: bool) -> Overview {
        let mut inner = self.inner.lock().expect("agents state");
        let focus = inner
            .host
            .is_some()
            .then(|| self.confirmed_tab_focus())
            .flatten();
        if reconcile {
            // Exactly one `tab.list` per request; a refusal is kept in the core and the window
            // keeps the list it has.
            let _ = run_live(&mut inner, |gateway, core| {
                core.reconcile_tabs(gateway);
                Ok(())
            });
        } else if let Some(tab_id) = focus.as_ref().and_then(|f| f.tab_id.clone()) {
            let _ = run_live(&mut inner, |gateway, core| {
                core.reconcile_tabs_for_focus(gateway, Some(tab_id.as_str()));
                Ok(())
            });
        }
        let mut overview = overview_of(&inner);
        if inner.host.is_some() {
            overview.tab_focus = focus;
        }
        overview
    }

    /// Tab focus confirmed by the attached connection (feed lock only; taken after the core
    /// lock, never while waiting on I/O).
    fn confirmed_tab_focus(&self) -> Option<TabFocus> {
        let feed = self.feed.lock().unwrap_or_else(PoisonError::into_inner);
        feed.attached.as_ref()?;
        feed.tab_focus.clone()
    }

    /// Same as [`Self::with_core`] for actions that change the tabs (create, split): the list the
    /// core reconciled is published on the channel, and a refused reconciliation is published as
    /// an error instead of being discarded.
    pub fn with_core_publishing_tabs<T>(
        &self,
        action: impl FnOnce(&dyn RuntimeGateway, &mut AgentsCore) -> Result<T, RuntimeError>,
    ) -> Result<T, RuntimeError> {
        let mut guard = self.inner.lock().expect("agents state");
        let result = run_live(&mut guard, action);
        let inner = &mut *guard;
        let published = inner
            .core
            .as_mut()
            .map(|core| (core.take_tabs_error(), core.tabs().to_vec()));
        if let Some((error, tabs)) = published {
            match error {
                Some(error) => emit(
                    inner,
                    AgentsEvent::State {
                        state: "tabs_unavailable".into(),
                        error: Some(error),
                    },
                ),
                None => emit(inner, AgentsEvent::Tabs { tabs }),
            }
        }
        result
    }

    /// Runs `action` with the live gateway (local or the attached host) and the core; the
    /// error stays on the returned action.
    pub fn with_core<T>(
        &self,
        action: impl FnOnce(&dyn RuntimeGateway, &mut AgentsCore) -> Result<T, RuntimeError>,
    ) -> Result<T, RuntimeError> {
        let mut guard = self.inner.lock().expect("agents state");
        run_live(&mut guard, action)
    }

    /// Same as [`Self::with_core`] for actions that change pane metadata (rename, swap): the cwds,
    /// manual names and topology the channel already has are republished; this never carries a
    /// new pane set or another focus.
    pub fn with_core_publishing_panes<T>(
        &self,
        action: impl FnOnce(&dyn RuntimeGateway, &mut AgentsCore) -> Result<T, RuntimeError>,
    ) -> Result<T, RuntimeError> {
        let mut guard = self.inner.lock().expect("agents state");
        let result = run_live(&mut guard, action);
        let inner = &mut *guard;
        let paths = inner.core.as_ref().map(AgentsCore::pane_paths);
        if let Some(paths) = paths {
            *inner
                .pane_paths
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = paths;
        }
        let labels = inner.core.as_ref().map(AgentsCore::pane_labels);
        if let Some(labels) = labels {
            *inner
                .pane_labels
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = labels;
        }
        let event = inner
            .feed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .republish_paths();
        if let Some(event) = event {
            emit(inner, event);
        }
        result
    }

    /// Detaches this client (engine, panes and agents keep running).
    pub fn detach(&self) {
        self.feed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        let bridge = {
            let mut inner = self.inner.lock().expect("agents state");
            if let Some(watcher) = inner.watcher.take() {
                watcher.stop.store(true, Ordering::Release);
            }
            if let Some(mut gateway) = inner.gateway.take() {
                gateway.detach();
            }
            if let Some(core) = inner.core.as_mut() {
                core.mark_stale();
            }
            inner.negotiated = None;
            inner.hosted_endpoint = None;
            inner.channel = None;
            inner.reconciling.clear();
            inner.bridge.take()
        };
        if let Some(handle) = bridge {
            let _ = handle.join();
        }
    }
}

fn attached(inner: &Inner) -> bool {
    match inner.host {
        Some(_) => inner.hosted_endpoint.is_some(),
        None => inner.gateway.is_some(),
    }
}

fn state_name(inner: &Inner) -> String {
    if inner.session.is_none() && inner.host.is_none() {
        return "empty".into();
    }
    if !attached(inner) {
        return "disconnected".into();
    }
    match inner.core.as_ref().map(|c| c.surface_state()) {
        Some(herdr_client::SurfaceState::Live) => "live".into(),
        Some(herdr_client::SurfaceState::Stale(_)) => "stale".into(),
        _ => "connecting".into(),
    }
}

fn overview_of(inner: &Inner) -> Overview {
    let core = inner.core.as_ref();
    Overview {
        state: state_name(inner),
        session: match inner.host {
            Some(_) => inner
                .hosted_endpoint
                .as_ref()
                .and(inner.last_identity.as_ref())
                .map(|i| i.session.clone()),
            None => inner.session.as_ref().map(|s| s.as_str().to_owned()),
        },
        identity: match inner.host {
            Some(_) => inner
                .hosted_endpoint
                .as_ref()
                .and(inner.last_identity.clone()),
            None => inner.gateway.as_ref().and_then(|g| g.identity()),
        },
        server_version: inner.negotiated.as_ref().map(|n| n.server_version.clone()),
        capabilities: core.map(AgentsCore::capabilities),
        kinds: core.map(|c| c.kinds().to_vec()).unwrap_or_default(),
        agents: core.map(|c| c.agents().to_vec()).unwrap_or_default(),
        tabs: core.map(|c| c.tabs().to_vec()).unwrap_or_default(),
        topology: core.and_then(AgentsCore::topology),
        tab_focus: None,
        error: inner.last_error.clone(),
    }
}

fn not_connected() -> RuntimeError {
    RuntimeError::new("not_connected", "disconnected from the Herdr server").retryable()
}

fn selection_changed(attached: &str) -> RuntimeError {
    RuntimeError::new(
        "selection_changed",
        "the selected host changed; the agents must be attached to the new host",
    )
    .with_endpoint(attached.to_owned())
}

/// Commits the host's surface into the core when it is not the one the core already holds.
fn sync_surface(core: &mut AgentsCore, frame: PaneSurfaceFrame) {
    let same = core.store.surface().is_some_and(|current| {
        current.boot_id == frame.boot_id && current.surface_revision == frame.surface_revision
    });
    if !same {
        let _ = core.apply_surface(frame);
    }
}

/// Runs `action` with the live gateway and core. Hosted: only on the host the core was
/// attached to, after committing its latest surface.
fn run_live<T>(
    inner: &mut Inner,
    action: impl FnOnce(&dyn RuntimeGateway, &mut AgentsCore) -> Result<T, RuntimeError>,
) -> Result<T, RuntimeError> {
    let Inner {
        gateway,
        core,
        host,
        hosted_endpoint,
        ..
    } = inner;
    let Some(core) = core.as_mut() else {
        return Err(not_connected());
    };
    match host {
        None => match gateway.as_ref() {
            Some(gateway) => action(gateway, core),
            None => Err(not_connected()),
        },
        Some(host) => {
            let Some(attached) = hosted_endpoint.as_deref() else {
                return Err(not_connected());
            };
            if host.endpoint() != attached {
                return Err(selection_changed(attached));
            }
            let gateway = host.gateway()?;
            if gateway.endpoint() != attached {
                return Err(selection_changed(attached));
            }
            if let Some(frame) = host.surface() {
                sync_surface(core, frame);
            }
            action(&*gateway, core)
        }
    }
}

/// Connection generation of the live gateway (reconcilers stop when it changes).
fn live_generation(inner: &Inner) -> Option<u64> {
    match &inner.host {
        None => inner.gateway.as_ref().map(|g| g.connection_generation()),
        Some(host) => {
            let attached = inner.hosted_endpoint.as_deref()?;
            let gateway = host.gateway().ok()?;
            (gateway.endpoint() == attached)
                .then(|| gateway.identity().map(|i| i.connection_generation))
                .flatten()
        }
    }
}

fn emit(inner: &Inner, event: AgentsEvent) {
    if let Some(channel) = inner.channel.as_ref() {
        let _ = channel.send(event);
    }
}

/// (Re)starts the event stream when the watched pane set changed.
fn ensure_watcher(inner: &mut Inner, shared: &Arc<Mutex<Inner>>) {
    if !attached(inner) {
        return;
    }
    let Some(core) = inner.core.as_ref() else {
        return;
    };
    let panes = core.watched_panes();
    if inner.watcher.as_ref().is_some_and(|w| w.panes == panes) {
        return;
    }
    if let Some(previous) = inner.watcher.take() {
        previous.stop.store(true, Ordering::Release);
    }
    let stop = Arc::new(AtomicBool::new(false));
    let (endpoint, open): (String, EventOpener) = match (&inner.host, &inner.gateway) {
        (Some(host), _) => {
            // The identity is the one the core was attached and discovered on (captured by
            // `attach_hosted`), passed from the caller's thread. No fresh identity is asked for:
            // neither the watcher thread nor this call may follow a selection change or a
            // reconnect of the same endpoint; the opener refuses that identity once it is stale.
            let Some(attached) = inner.hosted_endpoint.clone() else {
                return;
            };
            let Some(identity) = inner
                .last_identity
                .clone()
                .filter(|identity| identity.endpoint == attached)
            else {
                return;
            };
            let host = host.clone();
            (attached, Box::new(move || host.event_stream(&identity)))
        }
        (None, Some(gateway)) => {
            let socket = gateway.api().socket_path().to_path_buf();
            (
                herdr_client::LOCAL_ENDPOINT.to_owned(),
                Box::new(move || {
                    open_event_stream(&socket).map_err(|error| {
                        RuntimeError::from_io_kind(error.kind(), "Herdr events are unavailable")
                            .with_endpoint(herdr_client::LOCAL_ENDPOINT)
                    })
                }),
            )
        }
        (None, None) => return,
    };
    let params = event_subscriptions(&panes);
    let weak = Arc::downgrade(shared);
    let thread_stop = stop.clone();
    let spawned = std::thread::Builder::new()
        .name("herdr-desktop-agent-events".into())
        .spawn(move || watch_events(endpoint, open, params, thread_stop, weak));
    if spawned.is_ok() {
        inner.watcher = Some(Watcher { panes, stop });
    }
}

/// Engine settle delay before a managed launch can become ready (`AGENT_START_SETTLE_DELAY`).
const LAUNCH_SETTLE: Duration = Duration::from_millis(3200);
const LAUNCH_RECHECK: Duration = Duration::from_secs(1);
/// Engine default launch timeout (30 s) plus margin: reconciliation never outlives it.
const LAUNCH_RECONCILE_LIMIT: Duration = Duration::from_secs(35);

/// Starts one bounded reconciler per launching agent: `agent.get` after the settle delay and
/// then once per second until the engine reports the launch settled or the launch timeout
/// passed. It reads agent metadata only (never pane output) and stops with the connection.
fn ensure_launch_reconcilers(inner: &mut Inner, shared: &Arc<Mutex<Inner>>) {
    let Some(core) = inner.core.as_ref() else {
        return;
    };
    for pane in core.launching_panes() {
        if !inner.reconciling.insert(pane.clone()) {
            continue;
        }
        let weak = Arc::downgrade(shared);
        let generation = live_generation(inner);
        let _ = std::thread::Builder::new()
            .name("herdr-desktop-agent-launch".into())
            .spawn(move || reconcile_launch_loop(weak, pane, generation));
    }
}

fn reconcile_launch_loop(
    shared: std::sync::Weak<Mutex<Inner>>,
    pane: String,
    generation: Option<u64>,
) {
    let started = Instant::now();
    std::thread::sleep(LAUNCH_SETTLE);
    loop {
        let Some(shared) = shared.upgrade() else {
            return;
        };
        {
            let Ok(mut guard) = shared.lock() else {
                return;
            };
            let inner = &mut *guard;
            let same_connection = live_generation(inner) == generation;
            if !same_connection || !attached(inner) || inner.core.is_none() {
                return;
            }
            let settled =
                match run_live(inner, |gateway, core| core.reconcile_launch(gateway, &pane)) {
                    Ok(agent) => !agent.launch_pending,
                    Err(error) if error.code == "selection_changed" => return,
                    Err(error) => {
                        error.code == "agent_not_launching" || error.code == "unsupported_method"
                    }
                };
            let agents = inner
                .core
                .as_ref()
                .map(|core| core.agents().to_vec())
                .unwrap_or_default();
            emit(inner, AgentsEvent::Agents { agents });
            if settled || started.elapsed() >= LAUNCH_RECONCILE_LIMIT {
                inner.reconciling.remove(&pane);
                return;
            }
        }
        std::thread::sleep(LAUNCH_RECHECK);
    }
}

static NEXT_SUBSCRIPTION: AtomicU64 = AtomicU64::new(1);

/// Byte stream of the `events.subscribe` connection.
pub trait EventStream: Read + Write + Send {}
impl<T: Read + Write + Send> EventStream for T {}

type EventOpener = Box<dyn FnOnce() -> Result<Box<dyn EventStream>, RuntimeError> + Send>;

#[cfg(unix)]
fn open_event_stream(socket: &std::path::Path) -> std::io::Result<Box<dyn EventStream>> {
    let stream = std::os::unix::net::UnixStream::connect(socket)?;
    // Periodic wake-up to observe the stop flag; partial lines are kept across timeouts.
    stream.set_read_timeout(Some(Duration::from_millis(250)))?;
    Ok(Box::new(stream))
}

#[cfg(not(unix))]
fn open_event_stream(socket: &std::path::Path) -> std::io::Result<Box<dyn EventStream>> {
    // Windows named pipe (unproven until 008): blocking reads, so a stopped watcher ends at
    // its next event or when the pipe closes.
    Ok(Box::new(herdr_client::local::connect_local_stream(socket)?))
}

fn watch_events(
    endpoint: String,
    open: EventOpener,
    params: Value,
    stop: Arc<AtomicBool>,
    shared: std::sync::Weak<Mutex<Inner>>,
) {
    let report = |error: RuntimeError| {
        if let Some(shared) = shared.upgrade() {
            if let Ok(inner) = shared.lock() {
                if !stop.load(Ordering::Acquire) {
                    emit(
                        &inner,
                        AgentsEvent::State {
                            state: "events_unavailable".into(),
                            error: Some(error),
                        },
                    );
                }
            }
        }
    };
    let mut stream = match open() {
        Ok(stream) => stream,
        Err(error) => {
            report(error);
            return;
        }
    };
    let id = format!(
        "desktop-agents:{}",
        NEXT_SUBSCRIPTION.fetch_add(1, Ordering::Relaxed)
    );
    let mut request =
        json!({ "id": id, "method": "events.subscribe", "params": params }).to_string();
    request.push('\n');
    if let Err(error) = stream.write_all(request.as_bytes()) {
        report(
            RuntimeError::from_io_kind(error.kind(), "Herdr events are unavailable")
                .with_endpoint(endpoint.clone()),
        );
        return;
    }
    let mut pending: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        if stop.load(Ordering::Acquire) {
            return;
        }
        let read = match stream.read(&mut chunk) {
            Ok(0) => {
                if !stop.load(Ordering::Acquire) {
                    report(
                        RuntimeError::new("connection_lost", "the event stream ended")
                            .retryable()
                            .with_endpoint(endpoint.clone()),
                    );
                }
                return;
            }
            Ok(n) => n,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) =>
            {
                continue
            }
            Err(error) => {
                report(
                    RuntimeError::from_io_kind(error.kind(), "the event stream ended")
                        .with_endpoint(endpoint.clone()),
                );
                return;
            }
        };
        pending.extend_from_slice(&chunk[..read]);
        // Bounded: a single event line larger than 1 MiB ends the stream.
        if pending.len() > 1024 * 1024 {
            report(RuntimeError::new(
                "event_too_large",
                "the Herdr event exceeds the limit",
            ));
            return;
        }
        // Every read tick is one batch: the lifecycle events it carried are forwarded coalesced
        // (one `structure` event), and the window answers with one reconciliation; the watcher
        // itself never reads `tab.list` here (spec 027).
        let mut agents_changed = false;
        let mut structure: BTreeSet<String> = BTreeSet::new();
        while let Some(end) = pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = pending.drain(..=end).collect();
            let Ok(value) = serde_json::from_slice::<Value>(&line) else {
                continue;
            };
            if let Some(error) = value.get("error") {
                report(RuntimeError::new(
                    error["code"].as_str().unwrap_or("events_error"),
                    error["message"]
                        .as_str()
                        .unwrap_or("the event stream failed"),
                ));
                return;
            }
            let Some(event) = value.get("event").and_then(Value::as_str) else {
                continue;
            };
            // Anything that is not a tab event still refreshes the agent list, as before.
            if !event.starts_with("tab") {
                agents_changed = true;
            }
            if structural_event(event) {
                structure.insert(event.to_owned());
            }
        }
        if !agents_changed && structure.is_empty() {
            continue;
        }
        let Some(shared) = shared.upgrade() else {
            return;
        };
        let Ok(mut guard) = shared.lock() else {
            return;
        };
        if stop.load(Ordering::Acquire) {
            return;
        }
        let inner = &mut *guard;
        if !attached(inner) || inner.core.is_none() {
            return;
        }
        let mut events = Vec::new();
        if agents_changed {
            match run_live(inner, |gateway, core| core.refresh_agents(gateway)) {
                Ok(agents) => events.push(AgentsEvent::Agents { agents }),
                Err(error) => events.push(AgentsEvent::State {
                    state: "agents_unavailable".into(),
                    error: Some(error),
                }),
            }
        }
        if !structure.is_empty() {
            events.push(AgentsEvent::Structure {
                events: structure.into_iter().collect(),
            });
        }
        // Panes created/closed/renamed change the working directories and manual names the frames
        // show. Only paths and labels are republished, on the topology the channel already has:
        // this event never carries another pane set or another focus (spec 013, gate r3).
        if let Ok(true) = run_live(inner, |gateway, core| core.refresh_panes(gateway)) {
            let paths = inner.core.as_ref().map(AgentsCore::pane_paths);
            if let Some(paths) = paths {
                *inner
                    .pane_paths
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = paths;
            }
            let labels = inner.core.as_ref().map(AgentsCore::pane_labels);
            if let Some(labels) = labels {
                *inner
                    .pane_labels
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = labels;
            }
            if let Some(event) = inner
                .feed
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .republish_paths()
            {
                events.push(event);
            }
        }
        for event in events {
            emit(inner, event);
        }
        ensure_watcher(inner, &shared);
        ensure_launch_reconcilers(inner, &shared);
    }
}

fn bridge_loop(shared: Arc<Mutex<Inner>>, events: GatewayEvents) {
    while let Ok(event) = events.recv() {
        let mut guard = shared.lock().expect("agents state");
        let inner = &mut *guard;
        if inner.gateway.is_none() {
            break;
        }
        match event {
            GatewayEvent::Snapshot(_) => {
                let identity = inner.gateway.as_ref().and_then(|g| g.identity());
                if identity.is_some() && identity != inner.last_identity {
                    inner.last_identity = identity.clone();
                    if let Some(identity) = identity {
                        emit(inner, AgentsEvent::Identity { identity });
                    }
                }
            }
            GatewayEvent::Surface(frame) => {
                let outcome = inner.core.as_mut().map(|c| c.apply_surface(*frame));
                match outcome {
                    Some(Ok(Some(topology))) => {
                        emit(inner, AgentsEvent::Topology { topology });
                        ensure_watcher(inner, &shared);
                    }
                    Some(Ok(None)) | None => {}
                    Some(Err(reason)) => emit(
                        inner,
                        AgentsEvent::State {
                            state: format!("stale:{reason:?}").to_lowercase(),
                            error: None,
                        },
                    ),
                }
            }
            GatewayEvent::Patch(patch) => {
                let outcome = inner.core.as_mut().map(|c| c.apply_patch(*patch));
                if let Some(ApplyOutcome::Rejected(reason)) = outcome {
                    emit(
                        inner,
                        AgentsEvent::State {
                            state: format!("stale:{reason:?}").to_lowercase(),
                            error: None,
                        },
                    );
                    request_full_surface(inner);
                }
            }
            GatewayEvent::QueueOverflow { .. } => {
                if let Some(core) = inner.core.as_mut() {
                    core.mark_stale();
                }
                request_full_surface(inner);
            }
            GatewayEvent::Shutdown(_) | GatewayEvent::Disconnected(_) => {
                let error = match event {
                    GatewayEvent::Disconnected(error) => error,
                    _ => RuntimeError::new("server_shutdown", "the server shut down").retryable(),
                };
                if let Some(core) = inner.core.as_mut() {
                    core.mark_stale();
                }
                if let Some(watcher) = inner.watcher.take() {
                    watcher.stop.store(true, Ordering::Release);
                }
                inner.last_error = Some(error.clone());
                emit(
                    inner,
                    AgentsEvent::State {
                        state: "disconnected".into(),
                        error: Some(error),
                    },
                );
                inner.gateway = None;
                inner.negotiated = None;
                break;
            }
            GatewayEvent::EndpointResponse { .. }
            | GatewayEvent::ShellError(_)
            | GatewayEvent::KeyboardReportAll(_)
            | GatewayEvent::MouseCapture { .. }
            | GatewayEvent::Unsupported { .. } => {}
        }
    }
}

/// Recovery = a full surface with the current geometry; input is never replayed.
fn request_full_surface(inner: &Inner) {
    if let (Some(gateway), Some(geometry)) = (inner.gateway.as_ref(), inner.geometry) {
        let _ = gateway.resize(geometry);
    }
}

// ---------------------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------------------

impl AgentsState {
    /// Body of `agents_connect` (blocking): hosted attach in the composed window, otherwise a
    /// Local connection owned by this state.
    fn connect_blocking(
        &self,
        geometry: GeometryDto,
        on_event: Channel<AgentsEvent>,
    ) -> Result<Overview, RuntimeError> {
        if self.inner.lock().expect("agents state").host.is_some() {
            // Composed window: the hub owns the connection and its geometry.
            return self.attach_hosted(Some(on_event));
        }
        self.detach();
        let shared = self.inner.clone();
        let mut guard = shared.lock().expect("agents state");
        let inner = &mut *guard;
        let Some(session) = inner.session.clone() else {
            return Err(RuntimeError::new(
                "no_session",
                "no session is configured for agents",
            ));
        };
        let geometry: SurfaceGeometry = geometry.into();
        let mut gateway = LocalGateway::new(&inner.config_dir, session);
        let connected = gateway
            .connect(ConnectOptions {
                geometry,
                surface_active: true,
            })
            .and_then(|negotiated| {
                let events = gateway
                    .take_event_stream()
                    .expect("fresh connection has an event stream");
                // Same as the terminal (001): an active, focused shell client is the foreground
                // client, so the server paints its surface.
                let _ = gateway.set_focus(true);
                let deadline = Instant::now() + Duration::from_secs(10);
                while gateway.identity().is_none() {
                    if Instant::now() >= deadline {
                        return Err(RuntimeError::new(
                            "boot_unknown",
                            "the server did not report its identity in time",
                        )
                        .retryable()
                        .with_endpoint(herdr_client::LOCAL_ENDPOINT));
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                let mut core = AgentsCore::new(&negotiated.methods);
                core.discover(&gateway)?;
                if core.api("tab.list") {
                    let _ = core.refresh_tabs(&gateway);
                }
                Ok((negotiated, events, core))
            });
        let (negotiated, events, core) = match connected {
            Ok(parts) => parts,
            Err(error) => {
                gateway.detach();
                inner.last_error = Some(error.clone());
                return Err(error);
            }
        };
        inner.last_identity = gateway.identity();
        inner.gateway = Some(gateway);
        inner.negotiated = Some(negotiated);
        inner.core = Some(core);
        inner.geometry = Some(geometry);
        inner.channel = Some(on_event);
        inner.last_error = None;
        let bridge_shared = shared.clone();
        inner.bridge = Some(
            std::thread::Builder::new()
                .name("herdr-desktop-agents-bridge".into())
                .spawn(move || bridge_loop(bridge_shared, events))
                .map_err(|e| RuntimeError::from_io_kind(e.kind(), "agents bridge thread"))?,
        );
        ensure_watcher(inner, &shared);
        ensure_launch_reconcilers(inner, &shared);
        Ok(overview_of(inner))
    }
}

/// IPC entry points awaited by the Tauri command wrappers below. Each one takes its turn
/// in the command lane when called (see [`CommandLane`]) and runs the blocking body on the
/// runtime's blocking pool; the returned future only waits.
impl AgentsState {
    pub fn agents_connect(
        &self,
        geometry: GeometryDto,
        on_event: Channel<AgentsEvent>,
    ) -> impl Future<Output = Result<Overview, RuntimeError>> + Send + 'static {
        self.in_lane(move |state| state.connect_blocking(geometry, on_event))
    }

    /// `reconcile` reads `tab.list` once before answering (spec 027).
    pub fn agents_overview(
        &self,
        reconcile: bool,
    ) -> impl Future<Output = Result<Overview, RuntimeError>> + Send + 'static {
        self.in_lane(move |state| Ok(state.overview(reconcile)))
    }

    pub fn agents_detach(
        &self,
    ) -> impl Future<Output = Result<Overview, RuntimeError>> + Send + 'static {
        self.in_lane(|state| {
            state.detach();
            Ok(state.overview(false))
        })
    }

    pub fn agent_start(
        &self,
        target: QualifiedTarget,
        kind: String,
        name: String,
        autonomous: bool,
    ) -> impl Future<Output = Result<AgentDto, RuntimeError>> + Send + 'static {
        self.in_lane(move |state| {
            let agent = state.with_core(|gateway, core| {
                core.start_agent_with(gateway, &target, &kind, &name, autonomous)
            })?;
            let shared = state.inner.clone();
            let mut guard = shared.lock().expect("agents state");
            ensure_watcher(&mut guard, &shared);
            ensure_launch_reconcilers(&mut guard, &shared);
            Ok(agent)
        })
    }

    pub fn agent_prompt(
        &self,
        target: QualifiedTarget,
        text: String,
        resend_after_unknown: bool,
    ) -> impl Future<Output = Result<PromptOutcome, RuntimeError>> + Send + 'static {
        self.in_lane(move |state| {
            state.with_core(|gateway, core| {
                core.send_prompt(gateway, &target, &text, resend_after_unknown)
            })
        })
    }

    pub fn agent_open_attention(
        &self,
        target: QualifiedTarget,
    ) -> impl Future<Output = Result<(), RuntimeError>> + Send + 'static {
        self.in_lane(move |state| {
            state.with_core(|gateway, core| core.open_attention(gateway, &target))
        })
    }

    pub fn pane_split(
        &self,
        target: QualifiedTarget,
        direction: String,
    ) -> impl Future<Output = Result<PaneSplitReceipt, RuntimeError>> + Send + 'static {
        self.in_lane(move |state| {
            state
                .with_core_publishing_tabs(|gateway, core| core.split(gateway, &target, &direction))
        })
    }

    pub fn pane_focus(
        &self,
        target: QualifiedTarget,
    ) -> impl Future<Output = Result<(), RuntimeError>> + Send + 'static {
        self.in_lane(move |state| {
            state.with_core(|gateway, core| core.focus_pane(gateway, &target))
        })
    }

    /// One static `pane.read` snapshot for a home thumbnail (spec 012); never polled by the
    /// client and never a second request when it fails.
    pub fn pane_read(
        &self,
        target: QualifiedTarget,
        pane_id: String,
        lines: u32,
    ) -> impl Future<Output = Result<PaneSnapshotDto, RuntimeError>> + Send + 'static {
        self.in_lane(move |state| {
            state.with_core(|gateway, core| core.read_pane(gateway, &target, &pane_id, lines))
        })
    }

    pub fn pane_set_split_ratio(
        &self,
        target: QualifiedTarget,
        path: Vec<bool>,
        ratio: f32,
    ) -> impl Future<Output = Result<(), RuntimeError>> + Send + 'static {
        self.in_lane(move |state| {
            state.with_core(|gateway, core| core.set_split_ratio(gateway, &target, &path, ratio))
        })
    }

    pub fn pane_input(
        &self,
        target: QualifiedTarget,
        events: Vec<InputDto>,
    ) -> impl Future<Output = Result<(), RuntimeError>> + Send + 'static {
        let events = events
            .iter()
            .map(InputDto::to_event)
            .collect::<Result<Vec<_>, _>>();
        self.in_lane(move |state| {
            let events = events?;
            state.with_core(|gateway, core| core.send_input(gateway, &target, events))
        })
    }

    pub fn tab_create(
        &self,
        target: QualifiedTarget,
    ) -> impl Future<Output = Result<(), RuntimeError>> + Send + 'static {
        self.in_lane(move |state| {
            state.with_core_publishing_tabs(|gateway, core| core.create_tab(gateway, &target))
        })
    }

    pub fn pane_zoom(
        &self,
        target: QualifiedTarget,
        mode: String,
    ) -> impl Future<Output = Result<PaneZoomReceipt, RuntimeError>> + Send + 'static {
        self.in_lane(move |state| {
            state.with_core(|gateway, core| core.zoom_pane(gateway, &target, &mode))
        })
    }

    pub fn pane_rename(
        &self,
        target: QualifiedTarget,
        pane_id: String,
        label: Option<String>,
    ) -> impl Future<Output = Result<(), RuntimeError>> + Send + 'static {
        self.in_lane(move |state| {
            state.with_core_publishing_panes(|gateway, core| {
                core.rename_pane(gateway, &target, &pane_id, label.as_deref())
            })
        })
    }

    pub fn pane_swap(
        &self,
        target: QualifiedTarget,
        pane_id: String,
    ) -> impl Future<Output = Result<(), RuntimeError>> + Send + 'static {
        self.in_lane(move |state| {
            state.with_core_publishing_panes(|gateway, core| {
                core.swap_pane(gateway, &target, &pane_id)
            })
        })
    }

    pub fn pane_input_set(
        &self,
        target: QualifiedTarget,
        pane_id: String,
        passthrough: bool,
    ) -> impl Future<Output = Result<(), RuntimeError>> + Send + 'static {
        self.in_lane(move |state| {
            state.with_core(|gateway, core| {
                core.set_pane_right_click(gateway, &target, &pane_id, passthrough)
            })
        })
    }

    pub fn pane_close(
        &self,
        target: QualifiedTarget,
    ) -> impl Future<Output = Result<(), RuntimeError>> + Send + 'static {
        self.in_lane(move |state| {
            state.with_core_publishing_tabs(|gateway, core| core.close_pane(gateway, &target))
        })
    }

    pub fn tab_focus(
        &self,
        target: QualifiedTarget,
        tab_id: String,
    ) -> impl Future<Output = Result<(), RuntimeError>> + Send + 'static {
        self.in_lane(move |state| {
            state.with_core(|gateway, core| core.focus_tab(gateway, &target, &tab_id))
        })
    }

    pub fn tab_close(
        &self,
        target: QualifiedTarget,
        tab_id: String,
    ) -> impl Future<Output = Result<(), RuntimeError>> + Send + 'static {
        self.in_lane(move |state| {
            state.with_core_publishing_tabs(|gateway, core| {
                core.close_tab(gateway, &target, &tab_id)
            })
        })
    }

    pub fn tab_rename(
        &self,
        target: QualifiedTarget,
        tab_id: String,
        label: String,
    ) -> impl Future<Output = Result<(), RuntimeError>> + Send + 'static {
        self.in_lane(move |state| {
            state.with_core_publishing_tabs(|gateway, core| {
                core.rename_tab(gateway, &target, &tab_id, &label)
            })
        })
    }

    fn in_lane<T: Send + 'static>(
        &self,
        work: impl FnOnce(&AgentsState) -> Result<T, RuntimeError> + Send + 'static,
    ) -> impl Future<Output = Result<T, RuntimeError>> + Send + 'static {
        run_in_lane(&self.lane, self.clone(), work)
    }
}

// ---------------------------------------------------------------------------------------
// Command lane (spec 007). Mirrored in `project_store.rs`: both modules are also compiled in
// isolation by their feature tests, so neither can import the other.
// ---------------------------------------------------------------------------------------

/// Entry-ordered execution of one service's IPC commands off the calling thread.
///
/// A call takes the next turn synchronously (a short lock, never the service lock). Its future
/// waits for the turn without occupying a thread, then runs the work on the async runtime's
/// bounded blocking pool (`tauri::async_runtime::spawn_blocking`) and releases the turn when
/// the work ends, panics included. A future dropped before its turn releases it unrun; one
/// dropped after the work started lets the work finish (the result is discarded, nothing is
/// retried). At most one command of the service runs at a time, as before on the GUI thread.
#[derive(Default)]
struct CommandLane {
    turns: Mutex<Turns>,
}

#[derive(Default)]
struct Turns {
    issued: u64,
    next: u64,
    finished: BTreeSet<u64>,
    waiting: BTreeMap<u64, Waker>,
}

impl CommandLane {
    fn turns(&self) -> MutexGuard<'_, Turns> {
        self.turns.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

struct Turn {
    lane: Arc<CommandLane>,
    number: u64,
}

impl Turn {
    fn take(lane: &Arc<CommandLane>) -> Self {
        let mut turns = lane.turns();
        let number = turns.issued;
        turns.issued += 1;
        Self {
            lane: lane.clone(),
            number,
        }
    }

    fn poll_ready(&self, cx: &mut Context<'_>) -> Poll<()> {
        let mut turns = self.lane.turns();
        if turns.next == self.number {
            return Poll::Ready(());
        }
        turns.waiting.insert(self.number, cx.waker().clone());
        Poll::Pending
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        let mut turns = self.lane.turns();
        turns.waiting.remove(&self.number);
        turns.finished.insert(self.number);
        loop {
            let next = turns.next;
            if !turns.finished.remove(&next) {
                break;
            }
            turns.next += 1;
        }
        let next = turns.next;
        let waker = turns.waiting.remove(&next);
        drop(turns);
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

fn run_in_lane<S: Send + 'static, T: Send + 'static>(
    lane: &Arc<CommandLane>,
    state: S,
    work: impl FnOnce(&S) -> Result<T, RuntimeError> + Send + 'static,
) -> impl Future<Output = Result<T, RuntimeError>> + Send + 'static {
    let turn = Turn::take(lane);
    async move {
        std::future::poll_fn(|cx| turn.poll_ready(cx)).await;
        let job = tauri::async_runtime::spawn_blocking(move || {
            let _turn = turn;
            work(&state)
        });
        job.await.unwrap_or_else(|_| Err(command_interrupted()))
    }
}

/// The blocking work ended without a result (it panicked or the runtime shut down): whether
/// the action reached the engine is unknown, and it is not repeated.
fn command_interrupted() -> RuntimeError {
    RuntimeError::new(
        "command_interrupted",
        "the action was interrupted without a confirmed result; check the state before repeating it",
    )
}

// ---------------------------------------------------------------------------------------
// Tauri commands: thin async wrappers (argument names and success payloads unchanged; the
// getters now answer `Result`, whose `Ok` serializes exactly as the former value).
// ---------------------------------------------------------------------------------------

#[tauri::command]
pub async fn agents_connect(
    state: State<'_, AgentsState>,
    geometry: GeometryDto,
    on_event: Channel<AgentsEvent>,
) -> Result<Overview, RuntimeError> {
    state.agents_connect(geometry, on_event).await
}

#[tauri::command]
pub async fn agents_overview(
    state: State<'_, AgentsState>,
    reconcile: Option<bool>,
) -> Result<Overview, RuntimeError> {
    state.agents_overview(reconcile.unwrap_or(false)).await
}

#[tauri::command]
pub async fn agents_detach(state: State<'_, AgentsState>) -> Result<Overview, RuntimeError> {
    state.agents_detach().await
}

#[tauri::command]
pub async fn agent_start(
    state: State<'_, AgentsState>,
    target: QualifiedTarget,
    kind: String,
    name: String,
    autonomous: Option<bool>,
) -> Result<AgentDto, RuntimeError> {
    state
        .agent_start(target, kind, name, autonomous.unwrap_or(false))
        .await
}

/// Autonomy flags of the asked kinds, so the popup can show the flag each row would use. It is a
/// pure read of the backend table: no process, no filesystem, no engine call.
#[tauri::command]
pub fn agent_autonomy_flags(kinds: Vec<String>) -> Result<Vec<AgentAutonomyFlags>, RuntimeError> {
    autonomy_flags_of(&kinds)
}

#[tauri::command]
pub async fn agent_prompt(
    state: State<'_, AgentsState>,
    target: QualifiedTarget,
    text: String,
    resend_after_unknown: bool,
) -> Result<PromptOutcome, RuntimeError> {
    state.agent_prompt(target, text, resend_after_unknown).await
}

#[tauri::command]
pub async fn agent_open_attention(
    state: State<'_, AgentsState>,
    target: QualifiedTarget,
) -> Result<(), RuntimeError> {
    state.agent_open_attention(target).await
}

#[tauri::command]
pub async fn pane_split(
    state: State<'_, AgentsState>,
    target: QualifiedTarget,
    direction: String,
) -> Result<PaneSplitReceipt, RuntimeError> {
    state.pane_split(target, direction).await
}

#[tauri::command]
pub async fn pane_focus(
    state: State<'_, AgentsState>,
    target: QualifiedTarget,
) -> Result<(), RuntimeError> {
    state.pane_focus(target).await
}

#[tauri::command]
pub async fn pane_set_split_ratio(
    state: State<'_, AgentsState>,
    target: QualifiedTarget,
    path: Vec<bool>,
    ratio: f32,
) -> Result<(), RuntimeError> {
    state.pane_set_split_ratio(target, path, ratio).await
}

#[tauri::command]
pub async fn pane_input(
    state: State<'_, AgentsState>,
    target: QualifiedTarget,
    events: Vec<InputDto>,
) -> Result<(), RuntimeError> {
    state.pane_input(target, events).await
}

#[tauri::command]
pub async fn tab_create(
    state: State<'_, AgentsState>,
    target: QualifiedTarget,
) -> Result<(), RuntimeError> {
    state.tab_create(target).await
}

/// Expand/restore one pane of the active tab (`pane.zoom`), spec 013; answers the engine's
/// `zoomed` for the menu label (spec 028).
#[tauri::command]
pub async fn pane_zoom(
    state: State<'_, AgentsState>,
    target: QualifiedTarget,
    mode: String,
) -> Result<PaneZoomReceipt, RuntimeError> {
    state.pane_zoom(target, mode).await
}

/// Set (or clear, with `label: null`) the manual name of one pane (`pane.rename`), spec 028.
#[tauri::command]
pub async fn pane_rename(
    state: State<'_, AgentsState>,
    target: QualifiedTarget,
    pane_id: String,
    label: Option<String>,
) -> Result<(), RuntimeError> {
    state.pane_rename(target, pane_id, label).await
}

/// Exchange the right-clicked pane with the confirmed focused pane (`pane.swap`), spec 028.
#[tauri::command]
pub async fn pane_swap(
    state: State<'_, AgentsState>,
    target: QualifiedTarget,
    pane_id: String,
) -> Result<(), RuntimeError> {
    state.pane_swap(target, pane_id).await
}

/// Choose whether right clicks go to the pane or open the Herdr menu (`pane.input.set`), spec 028.
#[tauri::command]
pub async fn pane_input_set(
    state: State<'_, AgentsState>,
    target: QualifiedTarget,
    pane_id: String,
    passthrough: bool,
) -> Result<(), RuntimeError> {
    state.pane_input_set(target, pane_id, passthrough).await
}

#[tauri::command]
pub async fn tab_focus(
    state: State<'_, AgentsState>,
    target: QualifiedTarget,
    tab_id: String,
) -> Result<(), RuntimeError> {
    state.tab_focus(target, tab_id).await
}

#[tauri::command]
pub async fn tab_close(
    state: State<'_, AgentsState>,
    target: QualifiedTarget,
    tab_id: String,
) -> Result<(), RuntimeError> {
    state.tab_close(target, tab_id).await
}

#[tauri::command]
pub async fn tab_rename(
    state: State<'_, AgentsState>,
    target: QualifiedTarget,
    tab_id: String,
    label: String,
) -> Result<(), RuntimeError> {
    state.tab_rename(target, tab_id, label).await
}

#[tauri::command]
pub async fn pane_close(
    state: State<'_, AgentsState>,
    target: QualifiedTarget,
) -> Result<(), RuntimeError> {
    state.pane_close(target).await
}
