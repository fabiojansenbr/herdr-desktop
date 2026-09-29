//! Spec 004 — agents, tabs and split panes (seam: `AgentsCore` over fake gateways that
//! record every API/endpoint/input call with distinct answers per host and pane; one native
//! E2E with a deterministic fake agent in a disposable session).
//!
//! The backend module is compiled here through `#[path]`: composing it into
//! `src-tauri/src/lib.rs` belongs to spec 007. It reuses `terminal::InputDto` (001), which
//! is compiled the same way so `crate::terminal` resolves identically in both crates.
//!
//! AC-004-01: agent kinds come from the server; start/prompt go through the JSON API of the
//!            qualified host/pane; an absent method disables only its action; an unknown
//!            prompt outcome (timeout) is never re-sent automatically.
//! AC-004-02: working/blocked/idle/done/unknown are kept distinct (unknown never becomes
//!            done); opening a blocked agent's pane is a focus action that never sends input.
//! AC-004-03: split/focus/ratio use announced endpoint commands; geometry is the server's
//!            surface v1 topology; input goes only to the pane the server confirmed focused.

#[allow(dead_code)]
#[path = "../src/terminal.rs"]
mod terminal;

#[allow(dead_code)]
#[path = "../src/bridge/agent_commands.rs"]
mod agent_commands;

#[allow(dead_code)]
#[path = "../../scripts/feature-harness/native.rs"]
mod native_harness;

#[cfg(target_os = "linux")]
#[allow(dead_code)]
#[path = "../../scripts/feature-harness/window.rs"]
mod window_harness;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use agent_commands::{
    autonomy_flags, autonomy_flags_of, AgentStatus, AgentsCore, PromptOutcome, COMMANDS,
    MAX_AUTONOMY_KINDS,
};
use herdr_client::protocol::wire::{
    key_modifiers, CellData, ClientKeyCode, ClientPaneInputEvent, FrameData, PaneSurfaceFrame,
    PaneSurfacePane, PaneSurfaceSplit, PaneSurfaceSplitDirection, SurfaceGraphicsScene,
    SurfaceRect,
};
use herdr_client::{
    ConnectOptions, GatewayEvent, LiveIdentity, Negotiated, QualifiedTarget, RuntimeError,
    RuntimeGateway, SurfaceGeometry,
};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------------------
// Fake host: one engine per endpoint, distinct boot, agents and answers per pane
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Call {
    Api(String, Value),
    Endpoint(String, Value),
    Input(String, Vec<ClientPaneInputEvent>),
}

#[derive(Debug, Default)]
struct FakeHost {
    endpoint: String,
    session: String,
    boot_id: String,
    /// API methods this server knows (anything else is `unknown variant`).
    api_methods: Vec<String>,
    kinds: Vec<String>,
    /// pane_id -> (name, kind, status, launch_pending)
    agents: BTreeMap<String, (String, String, String, bool)>,
    tabs: Vec<(String, String, bool)>,
    calls: Vec<Call>,
    /// pane_id -> engine `state_change_seq` (1 when absent), bumped on every reported change.
    seqs: BTreeMap<String, u64>,
    /// pane_id -> engine `terminal_title_stripped`.
    titles: BTreeMap<String, String>,
    /// pane_id -> screen the engine answers to `agent.read {source: detection}`.
    detection: BTreeMap<String, String>,
    /// pane_id -> screen the engine answers to `pane.read` (spec 012 home thumbnails).
    screens: BTreeMap<String, String>,
    /// Error returned by `agent.prompt` (e.g. timeout).
    prompt_error: Option<RuntimeError>,
    /// Error returned by every API call (engine unavailable).
    api_down: Option<RuntimeError>,
    /// Reply this engine gives to one endpoint method (`pane.split` answers `pane_info` with the
    /// pane it created); every other method answers `{"type":"ok"}`.
    endpoint_replies: BTreeMap<String, Value>,
}

type Host = Arc<Mutex<FakeHost>>;

const ALL_API: &[&str] = &[
    "agent.list",
    "agent.get",
    "agent.read",
    "agent.start",
    "agent.prompt",
    "server.agent_manifests",
    "tab.list",
];

fn host(endpoint: &str, session: &str, boot: &str) -> Host {
    Arc::new(Mutex::new(FakeHost {
        endpoint: endpoint.into(),
        session: session.into(),
        boot_id: boot.into(),
        api_methods: ALL_API.iter().map(|m| m.to_string()).collect(),
        kinds: vec!["pi".into(), "claude".into()],
        tabs: vec![("w1:t1".into(), "1".into(), true)],
        ..FakeHost::default()
    }))
}

struct FakeGateway {
    endpoint: String,
    host: Host,
    generation: u64,
    identity_known: bool,
}

fn gateway(host: &Host, generation: u64) -> FakeGateway {
    FakeGateway {
        endpoint: host.lock().unwrap().endpoint.clone(),
        host: host.clone(),
        generation,
        identity_known: true,
    }
}

fn agent_json(pane: &str, entry: &(String, String, String, bool)) -> Value {
    let workspace = pane.split(':').next().unwrap();
    json!({
        "terminal_id": format!("term-{pane}"),
        "name": entry.0,
        "agent": entry.1,
        "agent_status": entry.2,
        "workspace_id": workspace,
        "tab_id": format!("{workspace}:t1"),
        "pane_id": pane,
        "focused": false,
        "launch_pending": entry.3,
        "interactive_ready": !entry.3,
        "state_change_seq": 1,
        "revision": 1,
    })
}

/// `agent.list`/`agent.get` entry with the per-pane engine data this host was given: the
/// state change sequence and the stripped terminal title (spec 014 seams).
fn agent_json_of(host: &FakeHost, pane: &str, entry: &(String, String, String, bool)) -> Value {
    let mut value = agent_json(pane, entry);
    value["state_change_seq"] = json!(host.seqs.get(pane).copied().unwrap_or(1));
    if let Some(title) = host.titles.get(pane) {
        value["terminal_title"] = json!(format!("{title} — raw"));
        value["terminal_title_stripped"] = json!(title);
    }
    value
}

impl RuntimeGateway for FakeGateway {
    fn endpoint(&self) -> &str {
        &self.endpoint
    }

    fn identity(&self) -> Option<LiveIdentity> {
        if !self.identity_known {
            return None;
        }
        let host = self.host.lock().unwrap();
        Some(LiveIdentity {
            endpoint: host.endpoint.clone(),
            session: host.session.clone(),
            connection_generation: self.generation,
            boot_id: host.boot_id.clone(),
        })
    }

    fn connect(&mut self, _options: ConnectOptions) -> Result<Negotiated, RuntimeError> {
        Err(RuntimeError::new("unsupported_in_fake", "connect"))
    }

    fn take_events(&mut self) -> Option<std::sync::mpsc::Receiver<GatewayEvent>> {
        None
    }

    fn api_request(&self, method: &str, params: Value) -> Result<Value, RuntimeError> {
        let mut host = self.host.lock().unwrap();
        host.calls.push(Call::Api(method.into(), params.clone()));
        if let Some(error) = host.api_down.clone() {
            return Err(error);
        }
        if !host.api_methods.iter().any(|m| m == method) {
            return Err(RuntimeError::new(
                "invalid_request:id_mismatch",
                format!("invalid request: unknown variant `{method}`, expected one of `ping`"),
            ));
        }
        let object_empty = params.as_object().is_some_and(|o| o.is_empty());
        match method {
            "agent.start" if object_empty => Err(RuntimeError::new(
                "invalid_request:id_mismatch",
                "invalid request: missing field `name`",
            )),
            "agent.get" if object_empty => Err(RuntimeError::new(
                "invalid_request:id_mismatch",
                "invalid request: missing field `target`",
            )),
            "agent.get" => {
                let pane = params["target"].as_str().unwrap().to_owned();
                // The engine settles a managed launch when agent.get reconciles it.
                let entry = host.agents.get_mut(&pane).expect("get of a known agent");
                entry.3 = false;
                let entry = entry.clone();
                let agent = agent_json_of(&host, &pane, &entry);
                Ok(json!({ "type": "agent_info", "agent": agent }))
            }
            "agent.prompt" if object_empty => Err(RuntimeError::new(
                "invalid_request:id_mismatch",
                "invalid request: missing field `target`",
            )),
            "server.agent_manifests" => Ok(json!({
                "type": "agent_manifest_status",
                "manifests": host.kinds.iter().map(|k| json!({ "agent": k, "source_kind": "remote" })).collect::<Vec<_>>(),
            })),
            "agent.list" => Ok(json!({
                "type": "agent_list",
                "agents": host.agents.iter().map(|(p, e)| agent_json_of(&host, p, e)).collect::<Vec<_>>(),
            })),
            "agent.read" if object_empty => Err(RuntimeError::new(
                "invalid_request:id_mismatch",
                "invalid request: missing field `target`",
            )),
            // Engine read of one pane: the tail of the requested source, `lines` at most.
            "agent.read" => {
                let pane = params["target"].as_str().unwrap_or_default().to_owned();
                let text = match params["source"].as_str() {
                    Some("detection") => host.detection.get(&pane).cloned().unwrap_or_default(),
                    other => {
                        return Err(RuntimeError::new(
                            "invalid_request:id_mismatch",
                            format!("invalid request: unknown source {other:?}"),
                        ))
                    }
                };
                let lines: usize = params["lines"].as_u64().unwrap_or(80) as usize;
                let all: Vec<&str> = text.split_inclusive('\n').collect();
                let tail: String = all[all.len().saturating_sub(lines)..].concat();
                Ok(json!({ "type": "pane_read", "read": {
                    "pane_id": pane, "source": "detection", "format": "text",
                    "text": tail, "revision": 1, "truncated": all.len() > lines,
                } }))
            }
            "tab.list" => Ok(json!({
                "type": "tab_list",
                "tabs": host.tabs.iter().map(|(id, label, focused)| json!({
                    "tab_id": id, "workspace_id": id.split(':').next().unwrap(),
                    "label": label, "focused": focused, "number": 1, "agent_status": "unknown",
                })).collect::<Vec<_>>(),
            })),
            "pane.close" if object_empty => Err(RuntimeError::new(
                "invalid_request:id_mismatch",
                "invalid request: missing field `pane_id`",
            )),
            "pane.close" => Ok(json!({ "type": "ok" })),
            "pane.read" if object_empty => Err(RuntimeError::new(
                "invalid_request:id_mismatch",
                "invalid request: missing field `pane_id`",
            )),
            // Engine read of one pane's visible screen, `lines` at most (spec 012).
            "pane.read" => {
                let pane = params["pane_id"].as_str().unwrap_or_default().to_owned();
                let text = host.screens.get(&pane).cloned().unwrap_or_default();
                let all: Vec<&str> = text.lines().collect();
                let lines: usize = params["lines"].as_u64().unwrap_or(20) as usize;
                let visible: Vec<&str> = all[all.len().saturating_sub(lines)..].to_vec();
                Ok(json!({ "type": "pane_read", "read": {
                    "pane_id": pane, "source": "visible", "format": "text",
                    "text": format!("{}\n", visible.join("\n")), "revision": 2,
                    "truncated": all.len() > lines,
                } }))
            }
            "agent.start" => {
                let pane = params["pane_id"].as_str().unwrap().to_owned();
                let entry = (
                    params["name"].as_str().unwrap().to_owned(),
                    params["kind"].as_str().unwrap().to_owned(),
                    "unknown".to_owned(),
                    true,
                );
                let agent = agent_json(&pane, &entry);
                host.agents.insert(pane, entry);
                Ok(json!({ "type": "agent_started", "agent": agent, "argv": [params["kind"]] }))
            }
            "agent.prompt" => {
                if let Some(error) = host.prompt_error.clone() {
                    return Err(error);
                }
                let pane = params["target"].as_str().unwrap();
                let entry = host
                    .agents
                    .get(pane)
                    .cloned()
                    .expect("prompt to a known agent");
                Ok(json!({ "type": "agent_prompted", "agent": agent_json(pane, &entry) }))
            }
            other => panic!("fake host has no answer for {other}"),
        }
    }

    fn endpoint_request(&self, method: &str, params: Value) -> Result<Value, RuntimeError> {
        let mut host = self.host.lock().unwrap();
        host.calls.push(Call::Endpoint(method.into(), params));
        Ok(host
            .endpoint_replies
            .get(method)
            .cloned()
            .unwrap_or_else(|| json!({ "type": "ok" })))
    }

    fn send_input(
        &self,
        target: &QualifiedTarget,
        events: Vec<ClientPaneInputEvent>,
    ) -> Result<(), RuntimeError> {
        let live = self.identity().unwrap();
        target.validate(&live)?;
        self.host
            .lock()
            .unwrap()
            .calls
            .push(Call::Input(target.pane_id.clone(), events));
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

/// Endpoint-lane methods announced by a generation-1 welcome (subset used here).
fn announced() -> Vec<String> {
    [
        "layout.set_split_ratio",
        "pane.focus",
        "pane.split",
        "tab.create",
        "tab.focus",
        "tab.rename",
    ]
    .iter()
    .map(|m| m.to_string())
    .collect()
}

fn calls(host: &Host) -> Vec<Call> {
    host.lock().unwrap().calls.clone()
}

fn clear(host: &Host) {
    host.lock().unwrap().calls.clear();
}

fn target(host: &Host, generation: u64, pane: &str) -> QualifiedTarget {
    let h = host.lock().unwrap();
    QualifiedTarget {
        endpoint: h.endpoint.clone(),
        session: h.session.clone(),
        connection_generation: generation,
        boot_id: h.boot_id.clone(),
        workspace_id: Some(pane.split(':').next().unwrap().into()),
        pane_id: pane.into(),
    }
}

fn rect(x: u16, y: u16, width: u16, height: u16) -> SurfaceRect {
    SurfaceRect {
        x,
        y,
        width,
        height,
    }
}

fn pane(id: &str, r: SurfaceRect, focused: bool) -> PaneSurfacePane {
    PaneSurfacePane {
        pane_id: id.into(),
        content_revision: 1,
        rect: r,
        inner_rect: r,
        scrollbar_rect: None,
        scroll: None,
        focused,
        mouse_reporting: false,
        sgr_pixel_mouse: false,
        alternate_screen_active: false,
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// A 100x30 surface of `boot` with the given panes and splits.
fn surface(
    boot: &str,
    revision: u64,
    panes: Vec<PaneSurfacePane>,
    splits: Vec<PaneSurfaceSplit>,
) -> PaneSurfaceFrame {
    let (width, height) = (100u16, 30u16);
    PaneSurfaceFrame {
        boot_id: boot.into(),
        projection_revision: 1,
        surface_revision: revision,
        frame: FrameData {
            cells: vec![
                CellData {
                    symbol: " ".into(),
                    fg: 0,
                    bg: 0,
                    modifier: 0,
                    skip: false,
                    hyperlink: None,
                };
                usize::from(width) * usize::from(height)
            ],
            width,
            height,
            cursor: None,
            hyperlinks: Vec::new(),
            graphics: Vec::new(),
        },
        panes,
        splits,
        popup: None,
        graphics: SurfaceGraphicsScene::default(),
    }
}

/// Two panes side by side, divider at column 30 (ratio 0.3), `focused` confirmed by the server.
fn two_panes(boot: &str, revision: u64, focused: &str) -> PaneSurfaceFrame {
    surface(
        boot,
        revision,
        vec![
            pane("w1:p1", rect(0, 0, 30, 30), focused == "w1:p1"),
            pane("w1:p2", rect(31, 0, 69, 30), focused == "w1:p2"),
        ],
        vec![PaneSurfaceSplit {
            direction: PaneSurfaceSplitDirection::Horizontal,
            pos: 30,
            area: rect(0, 0, 100, 30),
            hit_rect: rect(30, 0, 1, 30),
            path: vec![],
        }],
    )
}

fn one_pane(boot: &str, revision: u64) -> PaneSurfaceFrame {
    surface(
        boot,
        revision,
        vec![pane("w1:p1", rect(0, 0, 100, 30), true)],
        vec![],
    )
}

/// Local host "boot-lumen" (gen 3) with agents in w1:p1 (idle, pi) and w1:p2 (blocked,
/// claude); discovery done, a two-pane surface committed with w1:p2 focused.
fn ready_core() -> (Host, FakeGateway, AgentsCore) {
    let h = host("local", "hd004-alpha", "boot-lumen");
    {
        let mut hl = h.lock().unwrap();
        hl.agents.insert(
            "w1:p1".into(),
            ("revisor".into(), "pi".into(), "idle".into(), false),
        );
        hl.agents.insert(
            "w1:p2".into(),
            ("migrador".into(), "claude".into(), "blocked".into(), false),
        );
    }
    let gw = gateway(&h, 3);
    let mut core = AgentsCore::new(&announced());
    core.discover(&gw).unwrap();
    core.apply_surface(two_panes("boot-lumen", 7, "w1:p2"))
        .unwrap();
    clear(&h);
    (h, gw, core)
}

fn api_methods(host: &Host) -> Vec<String> {
    calls(host)
        .into_iter()
        .filter_map(|c| match c {
            Call::Api(m, _) => Some(m),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------------------
// AC-004-01 — kinds, start and prompt over the API of the qualified host
// ---------------------------------------------------------------------------------------

/// Would catch: kinds hard-coded in the desktop (detection copied), discovery that starts or
/// prompts something, or `pane.read` polling.
#[test]
fn discovery_lists_server_kinds_and_agents_and_starts_nothing() {
    let h = host("local", "hd004-alpha", "boot-lumen");
    h.lock().unwrap().kinds = vec!["droid".into(), "amp".into()];
    h.lock().unwrap().agents.insert(
        "w1:p1".into(),
        ("revisor".into(), "droid".into(), "working".into(), false),
    );
    let gw = gateway(&h, 1);
    let mut core = AgentsCore::new(&announced());
    core.discover(&gw).unwrap();

    assert_eq!(core.kinds(), &["droid".to_string(), "amp".to_string()]);
    let agents = core.agents();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].pane_id, "w1:p1");
    assert_eq!(agents[0].kind.as_deref(), Some("droid"));
    assert_eq!(agents[0].status, AgentStatus::Working);
    let methods = api_methods(&h);
    for forbidden in [
        "agent.prompt",
        "pane.read",
        "pane.send_text",
        "pane.send_input",
    ] {
        let executed = calls(&h).iter().any(|c| matches!(c, Call::Api(m, p) if m == forbidden && !p.as_object().is_some_and(|o| o.is_empty())));
        assert!(!executed, "discovery executed {forbidden}: {methods:?}");
    }
    // Discovery may only probe `pane.read` with empty params (the engine rejects the request
    // before reading); the loop above already proves no read was executed.
    let read_probes: Vec<Value> = calls(&h)
        .into_iter()
        .filter_map(|c| match c {
            Call::Api(m, p) if m == "pane.read" => Some(p),
            _ => None,
        })
        .collect();
    assert_eq!(
        read_probes,
        vec![json!({})],
        "discovery may only probe pane.read with empty params"
    );
    assert!(calls(&h)
        .iter()
        .all(|c| !matches!(c, Call::Endpoint(..) | Call::Input(..))));
    let caps = core.capabilities();
    assert!(caps.start_agent && caps.send_prompt && caps.split && caps.focus && caps.split_ratio);
}

/// Would catch: agent.start sent over the endpoint lane, to another pane/host, with a kind the
/// server did not announce, or without the name the user typed.
#[test]
fn start_uses_the_api_of_the_qualified_host_and_pane() {
    let local = host("local", "hd004-alpha", "boot-lumen");
    let remote = host("ssh-lab", "hd004-beta", "boot-orbita");
    remote.lock().unwrap().kinds = vec!["codex".into()];
    let gw_local = gateway(&local, 2);
    let gw_remote = gateway(&remote, 5);
    let mut core_local = AgentsCore::new(&announced());
    let mut core_remote = AgentsCore::new(&announced());
    core_local.discover(&gw_local).unwrap();
    core_remote.discover(&gw_remote).unwrap();
    core_local
        .apply_surface(two_panes("boot-lumen", 1, "w1:p1"))
        .unwrap();
    core_remote
        .apply_surface(two_panes("boot-orbita", 1, "w1:p2"))
        .unwrap();
    clear(&local);
    clear(&remote);

    let agent = core_remote
        .start_agent(
            &gw_remote,
            &target(&remote, 5, "w1:p2"),
            "codex",
            "orbita-dev",
        )
        .unwrap();

    assert_eq!(agent.pane_id, "w1:p2");
    assert_eq!(agent.name.as_deref(), Some("orbita-dev"));
    assert_eq!(
        calls(&remote),
        vec![Call::Api(
            "agent.start".into(),
            json!({ "name": "orbita-dev", "kind": "codex", "pane_id": "w1:p2" })
        )]
    );
    assert!(calls(&local).is_empty(), "nothing reached the other host");

    // A kind announced only by the other host is refused before any call.
    let error = core_local
        .start_agent(&gw_local, &target(&local, 2, "w1:p1"), "codex", "x")
        .unwrap_err();
    assert_eq!(error.code, "agent_kind_unavailable");
    assert!(calls(&local).is_empty());
}

/// Would catch: a started agent left "starting" forever (the engine settles a managed launch
/// only when `agent.get` reconciles it), reconciliation of agents that are not launching, or
/// output reads used for it.
#[test]
fn launch_reconciliation_uses_agent_get_for_launching_agents_only() {
    let (h, gw, mut core) = ready_core();
    h.lock().unwrap().agents.remove("w1:p1");
    core.refresh_agents(&gw).unwrap();
    core.start_agent(&gw, &target(&h, 3, "w1:p1"), "pi", "novo")
        .unwrap();
    assert_eq!(core.launching_panes(), vec!["w1:p1".to_string()]);
    clear(&h);

    let agent = core.reconcile_launch(&gw, "w1:p1").unwrap();
    assert!(!agent.launch_pending && agent.ready);
    assert_eq!(
        calls(&h),
        vec![Call::Api("agent.get".into(), json!({ "target": "w1:p1" }))]
    );
    assert!(core.launching_panes().is_empty());
    clear(&h);

    let error = core.reconcile_launch(&gw, "w1:p2").unwrap_err();
    assert_eq!(error.code, "agent_not_launching");
    assert!(calls(&h).is_empty());
}

/// Would catch: an empty or whitespace name/prompt reaching the engine, or a start on a pane
/// that is not part of the committed server topology.
#[test]
fn start_and_prompt_require_explicit_input_and_a_known_pane() {
    let (h, gw, mut core) = ready_core();
    let error = core
        .start_agent(&gw, &target(&h, 3, "w1:p1"), "pi", "   ")
        .unwrap_err();
    assert_eq!(error.code, "invalid_agent_name");
    let error = core
        .start_agent(&gw, &target(&h, 3, "w1:p9"), "pi", "fantasma")
        .unwrap_err();
    assert_eq!(error.code, "target_pane_missing");
    let error = core
        .send_prompt(&gw, &target(&h, 3, "w1:p1"), " \n ", false)
        .unwrap_err();
    assert_eq!(error.code, "empty_prompt");
    assert!(calls(&h).is_empty());
}

/// Would catch: prompts routed by name instead of pane, the prompt text altered, or the
/// result not carrying the server's agent state.
#[test]
fn prompt_goes_to_the_target_pane_and_returns_the_server_state() {
    let (h, gw, mut core) = ready_core();
    let outcome = core
        .send_prompt(
            &gw,
            &target(&h, 3, "w1:p1"),
            "Revise o módulo de busca",
            false,
        )
        .unwrap();
    match outcome {
        PromptOutcome::Sent { agent } => {
            assert_eq!(agent.pane_id, "w1:p1");
            assert_eq!(agent.status, AgentStatus::Idle);
        }
        other => panic!("expected sent, got {other:?}"),
    }
    assert_eq!(
        calls(&h),
        vec![Call::Api(
            "agent.prompt".into(),
            json!({ "target": "w1:p1", "text": "Revise o módulo de busca" })
        )]
    );
}

/// Would catch: a retry loop on timeout, a timeout reported as failure (inviting a blind
/// resend) or a second send accepted without the user acknowledging the unknown outcome.
#[test]
fn unknown_prompt_outcome_is_not_repeated_without_explicit_acknowledgement() {
    let (h, gw, mut core) = ready_core();
    h.lock().unwrap().prompt_error = Some(
        RuntimeError::new("timeout", "the endpoint did not answer in time")
            .retryable()
            .with_endpoint("local"),
    );
    let outcome = core
        .send_prompt(&gw, &target(&h, 3, "w1:p1"), "Gerar relatório", false)
        .unwrap();
    match &outcome {
        PromptOutcome::Unknown { error } => assert_eq!(error.code, "timeout"),
        other => panic!("expected unknown, got {other:?}"),
    }
    assert_eq!(api_methods(&h), vec!["agent.prompt"], "exactly one send");

    // Same pane, no acknowledgement: refused without reaching the engine.
    h.lock().unwrap().prompt_error = None;
    let error = core
        .send_prompt(&gw, &target(&h, 3, "w1:p1"), "Gerar relatório", false)
        .unwrap_err();
    assert_eq!(error.code, "prompt_outcome_unknown");
    assert_eq!(api_methods(&h), vec!["agent.prompt"]);

    // Explicit user acknowledgement sends once more, and only once.
    let outcome = core
        .send_prompt(&gw, &target(&h, 3, "w1:p1"), "Gerar relatório", true)
        .unwrap();
    assert!(matches!(outcome, PromptOutcome::Sent { .. }));
    assert_eq!(api_methods(&h), vec!["agent.prompt", "agent.prompt"]);
}

/// Would catch: a definite engine refusal reported as "unknown" (or vice versa), which would
/// either hide the error or block the pane forever.
#[test]
fn definite_prompt_errors_stay_errors_and_do_not_lock_the_pane() {
    let (h, gw, mut core) = ready_core();
    h.lock().unwrap().prompt_error = Some(
        RuntimeError::new("agent_not_ready", "agent w1:p1 is not ready").with_endpoint("local"),
    );
    let error = core
        .send_prompt(&gw, &target(&h, 3, "w1:p1"), "Primeiro", false)
        .unwrap_err();
    assert_eq!(error.code, "agent_not_ready");
    h.lock().unwrap().prompt_error = None;
    let outcome = core
        .send_prompt(&gw, &target(&h, 3, "w1:p1"), "Segundo", false)
        .unwrap();
    assert!(matches!(outcome, PromptOutcome::Sent { .. }));
}

/// Would catch: a missing method disabling every agent action, or a disabled action still
/// sending its request.
#[test]
fn absent_method_disables_only_its_action() {
    let h = host("local", "hd004-alpha", "boot-lumen");
    h.lock()
        .unwrap()
        .api_methods
        .retain(|m| m != "agent.prompt");
    h.lock().unwrap().agents.insert(
        "w1:p1".into(),
        ("revisor".into(), "pi".into(), "idle".into(), false),
    );
    let gw = gateway(&h, 1);
    let mut endpoint_methods = announced();
    endpoint_methods.retain(|m| m != "layout.set_split_ratio");
    let mut core = AgentsCore::new(&endpoint_methods);
    core.discover(&gw).unwrap();
    core.apply_surface(two_panes("boot-lumen", 1, "w1:p1"))
        .unwrap();
    let caps = core.capabilities();
    assert!(!caps.send_prompt, "agent.prompt absent");
    assert!(!caps.split_ratio, "layout.set_split_ratio not announced");
    assert!(
        caps.start_agent && caps.list_agents && caps.split && caps.focus && caps.open_attention
    );
    clear(&h);

    let error = core
        .send_prompt(&gw, &target(&h, 1, "w1:p1"), "oi", false)
        .unwrap_err();
    assert_eq!(error.code, "unsupported_method");
    let error = core
        .set_split_ratio(&gw, &target(&h, 1, "w1:p1"), &[], 0.4)
        .unwrap_err();
    assert_eq!(error.code, "unsupported_method");
    assert!(calls(&h).is_empty());

    // The other actions still work.
    core.start_agent(&gw, &target(&h, 1, "w1:p2"), "pi", "outro")
        .unwrap();
    core.split(&gw, &target(&h, 1, "w1:p1"), "down").unwrap();
    // agent.start, pane.split and the tab list the split reconciles (spec 013).
    assert_eq!(calls(&h).len(), 3);
}

/// Would catch: a server without the manifest listing leaving start enabled with guessed kinds.
#[test]
fn server_without_kind_listing_disables_start_only() {
    let h = host("local", "hd004-alpha", "boot-lumen");
    h.lock()
        .unwrap()
        .api_methods
        .retain(|m| m != "server.agent_manifests");
    let gw = gateway(&h, 1);
    let mut core = AgentsCore::new(&announced());
    core.discover(&gw).unwrap();
    assert!(core.kinds().is_empty());
    let caps = core.capabilities();
    assert!(!caps.start_agent);
    assert!(caps.send_prompt && caps.list_agents);
}

// ---------------------------------------------------------------------------------------
// Identity and availability (edge cases)
// ---------------------------------------------------------------------------------------

/// Would catch: an action on a stale boot/generation or another endpoint/session executed
/// anyway (or re-targeted to the local host).
#[test]
fn divergent_identity_invalidates_the_target_before_any_call() {
    let (h, gw, mut core) = ready_core();
    let mut stale_boot = target(&h, 3, "w1:p1");
    stale_boot.boot_id = "boot-anterior".into();
    let mut stale_gen = target(&h, 3, "w1:p1");
    stale_gen.connection_generation = 2;
    let mut other_host = target(&h, 3, "w1:p1");
    other_host.endpoint = "ssh-lab".into();
    let mut other_session = target(&h, 3, "w1:p1");
    other_session.session = "hd004-beta".into();

    for (t, code) in [
        (&stale_boot, "target_boot_stale"),
        (&stale_gen, "target_generation_stale"),
        (&other_host, "target_endpoint_mismatch"),
        (&other_session, "target_session_mismatch"),
    ] {
        assert_eq!(core.start_agent(&gw, t, "pi", "x").unwrap_err().code, code);
        assert_eq!(core.send_prompt(&gw, t, "x", false).unwrap_err().code, code);
        assert_eq!(core.open_attention(&gw, t).unwrap_err().code, code);
        assert_eq!(core.split(&gw, t, "right").unwrap_err().code, code);
        assert_eq!(core.focus_pane(&gw, t).unwrap_err().code, code);
        assert_eq!(
            core.set_split_ratio(&gw, t, &[], 0.5).unwrap_err().code,
            code
        );
        assert_eq!(
            core.send_input(&gw, t, vec![ClientPaneInputEvent::TextCommit("x".into())])
                .unwrap_err()
                .code,
            code
        );
        assert_eq!(core.create_tab(&gw, t).unwrap_err().code, code);
        // "w1:t1" is a tab the server listed: only the identity can refuse it.
        assert_eq!(core.focus_tab(&gw, t, "w1:t1").unwrap_err().code, code);
    }
    assert!(calls(&h).is_empty(), "{:?}", calls(&h));
}

/// Would catch: actions allowed before the server identity is known.
#[test]
fn unknown_identity_is_retryable_and_sends_nothing() {
    let (h, mut gw, mut core) = ready_core();
    gw.identity_known = false;
    let error = core
        .send_prompt(&gw, &target(&h, 3, "w1:p1"), "oi", false)
        .unwrap_err();
    assert_eq!(error.code, "boot_unknown");
    assert!(error.retryable);
    assert!(calls(&h).is_empty());
}

/// Would catch: an engine outage wiping the agent list or the typed prompt state, or the
/// error escaping to another host's core.
#[test]
fn engine_unavailable_keeps_last_known_agents_and_reports_on_the_action() {
    let (h, gw, mut core) = ready_core();
    h.lock().unwrap().api_down = Some(
        RuntimeError::new("server_unavailable", "the Herdr API is unavailable")
            .retryable()
            .with_endpoint("local"),
    );
    let error = core.refresh_agents(&gw).unwrap_err();
    assert_eq!(error.code, "server_unavailable");
    assert_eq!(core.agents().len(), 2, "last known agents kept");
    let error = core
        .send_prompt(&gw, &target(&h, 3, "w1:p1"), "oi", false)
        .unwrap_err();
    assert_eq!(error.code, "server_unavailable");
    assert_eq!(error.endpoint.as_deref(), Some("local"));
}

// ---------------------------------------------------------------------------------------
// AC-004-02 — states and attention
// ---------------------------------------------------------------------------------------

/// Would catch: an unknown/missing/new status mapped to done or idle, or two states merged.
#[test]
fn statuses_are_distinct_and_unknown_never_becomes_done() {
    let cases = [
        (Some("working"), AgentStatus::Working),
        (Some("blocked"), AgentStatus::Blocked),
        (Some("idle"), AgentStatus::Idle),
        (Some("done"), AgentStatus::Done),
        (Some("unknown"), AgentStatus::Unknown),
        (Some("finished"), AgentStatus::Unknown),
        (Some("DONE"), AgentStatus::Unknown),
        (None, AgentStatus::Unknown),
    ];
    for (wire, expected) in cases {
        assert_eq!(AgentStatus::from_wire(wire), expected, "{wire:?}");
    }
    let wire: Vec<Value> = [
        AgentStatus::Working,
        AgentStatus::Blocked,
        AgentStatus::Idle,
        AgentStatus::Done,
        AgentStatus::Unknown,
    ]
    .iter()
    .map(|s| serde_json::to_value(s).unwrap())
    .collect();
    assert_eq!(
        wire,
        vec![
            json!("working"),
            json!("blocked"),
            json!("idle"),
            json!("done"),
            json!("unknown")
        ]
    );

    // An agent the server reports without a status stays unknown after a refresh.
    let (h, gw, mut core) = ready_core();
    h.lock().unwrap().agents.insert(
        "w1:p1".into(),
        ("revisor".into(), "pi".into(), "sem-status".into(), false),
    );
    core.refresh_agents(&gw).unwrap();
    let p1 = core.agents().iter().find(|a| a.pane_id == "w1:p1").unwrap();
    assert_eq!(p1.status, AgentStatus::Unknown);
}

/// Would catch: "open pane" answering the approval (input, send_keys, prompt) instead of only
/// focusing the pane that asked for it.
#[test]
fn opening_a_blocked_agent_focuses_its_pane_and_sends_no_input() {
    let (h, gw, mut core) = ready_core();
    core.apply_surface(two_panes("boot-lumen", 8, "w1:p1"))
        .unwrap();
    core.open_attention(&gw, &target(&h, 3, "w1:p2")).unwrap();
    assert_eq!(
        calls(&h),
        vec![Call::Endpoint(
            "pane.focus".into(),
            json!({ "pane_id": "w1:p2" })
        )]
    );
}

/// Would catch: a prompt typed into a blocked agent (which would answer its approval).
#[test]
fn prompt_to_a_blocked_agent_is_refused_without_reaching_the_engine() {
    let (h, gw, mut core) = ready_core();
    let error = core
        .send_prompt(&gw, &target(&h, 3, "w1:p2"), "y", false)
        .unwrap_err();
    assert_eq!(error.code, "agent_blocked");
    assert!(calls(&h).is_empty());
}

/// Spec 014 (sibling of `discovery_lists_server_kinds_and_agents_and_starts_nothing`): the
/// panel needs the engine's terminal title, its state change sequence and the last line of the
/// detection snapshot of an agent that asked for the user.
///
/// Would catch: a detection read during discovery, a read per refresh (polling) instead of one
/// per transition, a read of an agent that is not waiting, a line kept after the state changed
/// again, or a title/sequence invented by the desktop.
#[test]
fn detection_line_is_read_once_per_transition_to_waiting_and_never_in_discovery() {
    let h = host("local", "hd004-alpha", "boot-lumen");
    {
        let mut hl = h.lock().unwrap();
        hl.agents.insert(
            "w1:p1".into(),
            ("revisor".into(), "pi".into(), "working".into(), false),
        );
        hl.agents.insert(
            "w1:p2".into(),
            ("migrador".into(), "claude".into(), "blocked".into(), false),
        );
        hl.seqs.insert("w1:p1".into(), 4);
        hl.seqs.insert("w1:p2".into(), 9);
        hl.titles
            .insert("w1:p1".into(), "Refatorando faturamento".into());
        hl.detection.insert(
            "w1:p1".into(),
            "> revisar migração\nAprovar: sqlx migrate run\n".into(),
        );
        hl.detection
            .insert("w1:p2".into(), "linha do agente já bloqueado\n".into());
    }
    let gw = gateway(&h, 3);
    let mut core = AgentsCore::new(&announced());
    core.discover(&gw).unwrap();

    // Discovery reads no detection snapshot, not even of the agent already waiting (w1:p2): no
    // transition was observed on this connection.
    let read_params: Vec<Value> = calls(&h)
        .into_iter()
        .filter_map(|c| match c {
            Call::Api(m, p) if m == "agent.read" => Some(p),
            _ => None,
        })
        .collect();
    assert_eq!(
        read_params,
        vec![json!({})],
        "discovery may only probe agent.read with empty params"
    );
    let discovered = core.agents().to_vec();
    let p1 = discovered.iter().find(|a| a.pane_id == "w1:p1").unwrap();
    let p2 = discovered.iter().find(|a| a.pane_id == "w1:p2").unwrap();
    assert_eq!(
        p1.terminal_title.as_deref(),
        Some("Refatorando faturamento")
    );
    assert_eq!(p1.state_change_seq, 4);
    assert_eq!(p2.state_change_seq, 9);
    assert_eq!(p1.detection_last_line, None);
    assert_eq!(
        p2.detection_last_line, None,
        "waiting since before the connection"
    );

    // A refresh without any change reads nothing.
    clear(&h);
    core.refresh_agents(&gw).unwrap();
    assert_eq!(api_methods(&h), vec!["agent.list"]);

    // w1:p1 asks for the user (new sequence): exactly one detection read, its last line kept.
    {
        let mut hl = h.lock().unwrap();
        hl.agents.get_mut("w1:p1").unwrap().2 = "blocked".into();
        hl.seqs.insert("w1:p1".into(), 5);
    }
    clear(&h);
    let agents = core.refresh_agents(&gw).unwrap();
    assert_eq!(
        calls(&h),
        vec![
            Call::Api("agent.list".into(), json!({})),
            Call::Api(
                "agent.read".into(),
                json!({ "target": "w1:p1", "source": "detection", "lines": 1 })
            ),
        ]
    );
    let p1 = agents.iter().find(|a| a.pane_id == "w1:p1").unwrap();
    assert_eq!(p1.status, AgentStatus::Blocked);
    assert_eq!(
        p1.detection_last_line.as_deref(),
        Some("Aprovar: sqlx migrate run")
    );
    assert_eq!(
        agents
            .iter()
            .find(|a| a.pane_id == "w1:p2")
            .unwrap()
            .detection_last_line,
        None,
        "no read for an agent whose transition was never observed"
    );

    // Further refreshes of the same waiting state read nothing and keep the same line.
    clear(&h);
    let agents = core.refresh_agents(&gw).unwrap();
    assert_eq!(
        api_methods(&h),
        vec!["agent.list"],
        "one read per transition"
    );
    assert_eq!(
        agents
            .iter()
            .find(|a| a.pane_id == "w1:p1")
            .unwrap()
            .detection_last_line
            .as_deref(),
        Some("Aprovar: sqlx migrate run")
    );

    // The agent leaves the waiting state: the line of the past transition is dropped, and no
    // read happens for a state that does not ask for the user.
    {
        let mut hl = h.lock().unwrap();
        hl.agents.get_mut("w1:p1").unwrap().2 = "working".into();
        hl.seqs.insert("w1:p1".into(), 6);
    }
    clear(&h);
    let agents = core.refresh_agents(&gw).unwrap();
    assert_eq!(api_methods(&h), vec!["agent.list"]);
    let p1 = agents.iter().find(|a| a.pane_id == "w1:p1").unwrap();
    assert_eq!(p1.state_change_seq, 6);
    assert_eq!(p1.detection_last_line, None);

    // Waiting again with a new sequence: one more read, with the screen of this transition.
    {
        let mut hl = h.lock().unwrap();
        hl.agents.get_mut("w1:p1").unwrap().2 = "blocked".into();
        hl.seqs.insert("w1:p1".into(), 7);
        hl.detection
            .insert("w1:p1".into(), "Aprovar: cargo test billing\n".into());
    }
    clear(&h);
    let agents = core.refresh_agents(&gw).unwrap();
    assert_eq!(
        api_methods(&h),
        vec!["agent.list", "agent.read"],
        "a new transition is read once"
    );
    assert_eq!(
        agents
            .iter()
            .find(|a| a.pane_id == "w1:p1")
            .unwrap()
            .detection_last_line
            .as_deref(),
        Some("Aprovar: cargo test billing")
    );
}

/// Would catch: the transition being keyed only by `state_change_seq`, which would leave a
/// server that does not move that field without any line (or read it on every refresh).
#[test]
fn a_state_change_without_a_new_sequence_is_still_one_transition() {
    let h = host("local", "hd004-alpha", "boot-lumen");
    {
        let mut hl = h.lock().unwrap();
        hl.agents.insert(
            "w1:p1".into(),
            ("revisor".into(), "pi".into(), "working".into(), false),
        );
        // The sequence never changes on this server.
        hl.seqs.insert("w1:p1".into(), 1);
        hl.detection
            .insert("w1:p1".into(), "Aprovar: rm -rf build\n".into());
    }
    let gw = gateway(&h, 3);
    let mut core = AgentsCore::new(&announced());
    core.discover(&gw).unwrap();
    h.lock().unwrap().agents.get_mut("w1:p1").unwrap().2 = "blocked".into();
    clear(&h);
    let agents = core.refresh_agents(&gw).unwrap();
    assert_eq!(api_methods(&h), vec!["agent.list", "agent.read"]);
    assert_eq!(
        agents[0].detection_last_line.as_deref(),
        Some("Aprovar: rm -rf build")
    );
    clear(&h);
    core.refresh_agents(&gw).unwrap();
    assert_eq!(
        api_methods(&h),
        vec!["agent.list"],
        "one read per transition"
    );
}

/// Would catch: a server without `agent.read` breaking the refresh, or a failed read being
/// retried on every refresh (polling) or shown as a line the engine never sent.
#[test]
fn a_refresh_survives_a_server_without_detection_reads() {
    let h = host("local", "hd004-alpha", "boot-lumen");
    {
        let mut hl = h.lock().unwrap();
        hl.api_methods.retain(|m| m != "agent.read");
        hl.agents.insert(
            "w1:p1".into(),
            ("revisor".into(), "pi".into(), "working".into(), false),
        );
        hl.seqs.insert("w1:p1".into(), 2);
    }
    let gw = gateway(&h, 3);
    let mut core = AgentsCore::new(&announced());
    core.discover(&gw).unwrap();
    {
        let mut hl = h.lock().unwrap();
        hl.agents.get_mut("w1:p1").unwrap().2 = "blocked".into();
        hl.seqs.insert("w1:p1".into(), 3);
    }
    clear(&h);
    let agents = core.refresh_agents(&gw).unwrap();
    assert_eq!(api_methods(&h), vec!["agent.list"]);
    assert_eq!(agents[0].status, AgentStatus::Blocked);
    assert_eq!(agents[0].detection_last_line, None);

    // A server that offers the method but fails the read: one attempt, no line, no retry.
    let h2 = host("local2", "hd004-beta", "boot-nox");
    {
        let mut hl = h2.lock().unwrap();
        hl.agents.insert(
            "w1:p1".into(),
            ("revisor".into(), "pi".into(), "working".into(), false),
        );
        hl.seqs.insert("w1:p1".into(), 2);
    }
    let gw2 = gateway(&h2, 1);
    let mut core2 = AgentsCore::new(&announced());
    core2.discover(&gw2).unwrap();
    {
        let mut hl = h2.lock().unwrap();
        hl.agents.get_mut("w1:p1").unwrap().2 = "blocked".into();
        hl.seqs.insert("w1:p1".into(), 3);
        // No detection screen for this pane: the read answers an empty snapshot.
        hl.detection.insert("w1:p1".into(), String::new());
    }
    clear(&h2);
    let agents = core2.refresh_agents(&gw2).unwrap();
    assert_eq!(api_methods(&h2), vec!["agent.list", "agent.read"]);
    assert_eq!(agents[0].detection_last_line, None);
    clear(&h2);
    core2.refresh_agents(&gw2).unwrap();
    assert_eq!(api_methods(&h2), vec!["agent.list"], "no retry per refresh");
}

/// Would catch: a home thumbnail (`pane.read`, spec 012) that polls, reads a pane that is not an
/// agent of the qualified host, sends to a stale target or bypasses the line bound.
#[test]
fn pane_read_snapshots_one_agent_pane_once_and_refuses_stale_or_unknown_targets() {
    let h = host("local", "hd012-alpha", "boot-lumen");
    {
        let mut hl = h.lock().unwrap();
        hl.api_methods.push("pane.read".into());
        hl.agents.insert(
            "w1:p1".into(),
            ("claude".into(), "claude".into(), "working".into(), false),
        );
        hl.screens
            .insert("w1:p1".into(), "linha um\nlinha dois\nlinha tres".into());
    }
    let gw = gateway(&h, 3);
    let mut core = AgentsCore::new(&announced());
    core.discover(&gw).unwrap();
    // Discovery probes the method with empty params only (the engine rejects before reading).
    assert!(calls(&h).iter().all(|c| {
        !matches!(c, Call::Api(m, p) if m == "pane.read" && !p.as_object().is_some_and(|o| o.is_empty()))
    }));
    clear(&h);

    let qualified = target(&h, 3, "w1:p1");
    let snapshot = core.read_pane(&gw, &qualified, "w1:p1", 2).unwrap();
    assert_eq!(snapshot.pane_id, "w1:p1");
    assert_eq!(snapshot.text, "linha dois\nlinha tres\n");
    assert_eq!(snapshot.revision, 2);
    assert!(snapshot.truncated);
    assert_eq!(api_methods(&h), vec!["pane.read"], "exactly one request");
    let params: Vec<Value> = calls(&h)
        .into_iter()
        .filter_map(|c| match c {
            Call::Api(m, p) if m == "pane.read" => Some(p),
            _ => None,
        })
        .collect();
    assert_eq!(
        params,
        vec![json!({ "pane_id": "w1:p1", "source": "visible", "lines": 2 })]
    );

    // A pane the engine did not list as an agent is refused without any request.
    clear(&h);
    let error = core.read_pane(&gw, &qualified, "w1:p9", 2).unwrap_err();
    assert_eq!(error.code, "agent_not_found");
    assert!(calls(&h).is_empty(), "{:?}", calls(&h));

    // A stale boot/session/generation is refused without any request.
    let mut stale = qualified.clone();
    stale.boot_id = "boot-other".into();
    let error = core.read_pane(&gw, &stale, "w1:p1", 2).unwrap_err();
    assert!(!error.code.is_empty());
    assert!(calls(&h).is_empty(), "{:?}", calls(&h));

    // A server without the method disables the read instead of polling something else.
    let h2 = host("local2", "hd012-beta", "boot-nox");
    h2.lock().unwrap().agents.insert(
        "w1:p1".into(),
        ("claude".into(), "claude".into(), "working".into(), false),
    );
    let gw2 = gateway(&h2, 1);
    let mut core2 = AgentsCore::new(&announced());
    core2.discover(&gw2).unwrap();
    let qualified2 = target(&h2, 1, "w1:p1");
    let error = core2.read_pane(&gw2, &qualified2, "w1:p1", 2).unwrap_err();
    assert!(error.message.contains("pane.read"), "{}", error.message);
    clear(&h2);
    assert!(calls(&h2).is_empty());
}

/// Would catch: the event stream subscribing to output (pane.read-like polling) or missing
/// per-pane status changes.
#[test]
fn event_subscriptions_track_status_and_layout_without_output() {
    let subs = agent_commands::event_subscriptions(&["w1:p1".to_string(), "w2:p4".to_string()]);
    let list = subs["subscriptions"].as_array().unwrap();
    let types: Vec<&str> = list.iter().map(|s| s["type"].as_str().unwrap()).collect();
    for required in [
        "pane.agent_detected",
        "pane.updated",
        "pane.created",
        "pane.closed",
        "tab.created",
        "tab.focused",
        "tab.closed",
        // Spec 027: renaming and workspace lifecycle events also change the window's lists.
        "tab.renamed",
        "workspace.created",
        "workspace.closed",
        "workspace.renamed",
    ] {
        assert!(types.contains(&required), "missing {required}");
    }
    let per_pane: Vec<&str> = list
        .iter()
        .filter(|s| s["type"] == "pane.agent_status_changed")
        .map(|s| s["pane_id"].as_str().unwrap())
        .collect();
    assert_eq!(per_pane, vec!["w1:p1", "w2:p4"]);
    assert!(!types.iter().any(|t| t.contains("output")));
}

/// Spec 027 AC-027-01/03. Would catch: a tab/workspace/pane lifecycle event the watcher does not
/// classify as structural (never forwarded to the window), or a content/status event treated as
/// structural (a reconciliation per keystroke). The watcher reads the wire name the engine
/// serializes, so both its `snake_case` and the dotted subscription spelling are accepted.
#[test]
fn lifecycle_events_are_structural_without_output_or_status_events() {
    for name in [
        "tab_created",
        "tab_closed",
        "tab_renamed",
        "tab_focused",
        "tab.moved",
        "workspace_created",
        "workspace.closed",
        "workspace_renamed",
        "pane_created",
        "pane.closed",
    ] {
        assert!(agent_commands::structural_event(name), "{name}");
    }
    for name in [
        "pane_agent_status_changed",
        "pane.updated",
        "pane.agent_detected",
        "pane_output_changed",
        "pane.exited",
        "layout.updated",
        "workspace.updated",
    ] {
        assert!(!agent_commands::structural_event(name), "{name}");
    }
}

/// The forwarded event the WebView coalesces (spec 027); its wire contract is a `structure`
/// type carrying the engine's own event names.
#[test]
fn the_forwarded_structural_event_serializes_with_the_engine_names() {
    let event = agent_commands::AgentsEvent::Structure {
        events: vec!["tab_closed".into(), "workspace_closed".into()],
    };
    assert_eq!(
        serde_json::to_value(event).unwrap(),
        json!({ "type": "structure", "events": ["tab_closed", "workspace_closed"] })
    );
}

// ---------------------------------------------------------------------------------------
// AC-004-03 — splits, focus, ratio and confirmed input
// ---------------------------------------------------------------------------------------

/// Would catch: geometry computed by the GUI instead of copied from the surface v1 frame, a
/// ratio derived from pane widths, or a topology event on every content frame.
#[test]
fn topology_is_the_server_surface_and_changes_only_with_it() {
    let (_h, _gw, mut core) = ready_core();
    let topology = core.topology().unwrap();
    assert_eq!(topology.width, 100);
    assert_eq!(topology.focused_pane_id.as_deref(), Some("w1:p2"));
    let boxes: Vec<(String, u16, u16, u16, u16, bool)> = topology
        .panes
        .iter()
        .map(|p| (p.pane_id.clone(), p.x, p.y, p.width, p.height, p.focused))
        .collect();
    assert_eq!(
        boxes,
        vec![
            ("w1:p1".into(), 0, 0, 30, 30, false),
            ("w1:p2".into(), 31, 0, 69, 30, true)
        ]
    );
    assert_eq!(topology.splits.len(), 1);
    let split = &topology.splits[0];
    assert_eq!(split.direction, "right");
    assert!(split.path.is_empty());
    assert!((split.ratio - 0.3).abs() < 1e-6, "{}", split.ratio);

    // Same topology, new content revision: no topology change reported.
    assert!(core
        .apply_surface(two_panes("boot-lumen", 8, "w1:p2"))
        .unwrap()
        .is_none());
    // Focus moved on the server: reported.
    let changed = core
        .apply_surface(two_panes("boot-lumen", 9, "w1:p1"))
        .unwrap()
        .expect("focus change is a topology change");
    assert_eq!(changed.focused_pane_id.as_deref(), Some("w1:p1"));
    // Back to one pane: reported, split gone.
    let single = core
        .apply_surface(one_pane("boot-lumen", 10))
        .unwrap()
        .unwrap();
    assert_eq!(single.panes.len(), 1);
    assert!(single.splits.is_empty());
}

/// Would catch: split/focus/ratio sent through the JSON API instead of announced endpoint
/// commands, a ratio outside the server range, or a split path the surface never showed.
#[test]
fn split_focus_and_ratio_use_announced_endpoint_commands() {
    let (h, gw, mut core) = ready_core();
    core.split(&gw, &target(&h, 3, "w1:p1"), "down").unwrap();
    core.focus_pane(&gw, &target(&h, 3, "w1:p1")).unwrap();
    core.set_split_ratio(&gw, &target(&h, 3, "w1:p1"), &[], 0.65)
        .unwrap();
    let recorded = calls(&h);
    // The split also reads the tab list again (spec 013: the pane count of the tab must reach the
    // window without depending on an engine event).
    assert_eq!(
        recorded
            .iter()
            .filter(|c| matches!(c, Call::Api(m, _) if m == "tab.list"))
            .count(),
        1
    );
    let recorded: Vec<Call> = recorded
        .into_iter()
        .filter(|c| !matches!(c, Call::Api(m, _) if m == "tab.list"))
        .collect();
    assert_eq!(recorded.len(), 3);
    assert_eq!(
        recorded[0],
        Call::Endpoint(
            "pane.split".into(),
            json!({ "target_pane_id": "w1:p1", "direction": "down", "focus": false })
        )
    );
    assert_eq!(
        recorded[1],
        Call::Endpoint("pane.focus".into(), json!({ "pane_id": "w1:p1" }))
    );
    match &recorded[2] {
        Call::Endpoint(method, params) => {
            assert_eq!(method, "layout.set_split_ratio");
            assert_eq!(params["pane_id"], "w1:p1");
            assert_eq!(params["path"], json!([]));
            assert!((params["ratio"].as_f64().unwrap() - 0.65).abs() < 1e-6);
        }
        other => panic!("{other:?}"),
    }
    clear(&h);

    for (ratio, code) in [(0.05, "invalid_ratio"), (f32::NAN, "invalid_ratio")] {
        assert_eq!(
            core.set_split_ratio(&gw, &target(&h, 3, "w1:p1"), &[], ratio)
                .unwrap_err()
                .code,
            code
        );
    }
    assert_eq!(
        core.set_split_ratio(&gw, &target(&h, 3, "w1:p1"), &[true], 0.5)
            .unwrap_err()
            .code,
        "split_not_found"
    );
    assert_eq!(
        core.split(&gw, &target(&h, 3, "w1:p1"), "diagonal")
            .unwrap_err()
            .code,
        "invalid_split_direction"
    );
    assert!(calls(&h).is_empty());
}

/// Spec 075 AC-075-01. Would catch: `pane.split` discarding the engine's `PaneInfo`, leaving the
/// window to guess which pane was created, or the core refusing an `agent.start` on that pane
/// because the surface frame with it has not arrived yet.
#[test]
fn split_answers_the_created_pane_and_the_core_accepts_it_as_a_target() {
    let (h, gw, mut core) = ready_core();
    h.lock().unwrap().endpoint_replies.insert(
        "pane.split".into(),
        json!({ "type": "pane_info", "pane": {
            "pane_id": "w1:p9", "terminal_id": "term-w1:p9", "workspace_id": "w1",
            "tab_id": "w1:t1", "focused": false, "agent_status": "unknown", "revision": 1,
        } }),
    );
    let receipt = core.split(&gw, &target(&h, 3, "w1:p1"), "right").unwrap();
    assert_eq!(receipt.pane_id.as_deref(), Some("w1:p9"));
    // The committed surface still has the two panes of the previous frame: nothing was invented
    // in the topology, and the new pane is a valid target all the same.
    let panes: Vec<String> = core
        .topology()
        .unwrap()
        .panes
        .iter()
        .map(|p| p.pane_id.clone())
        .collect();
    assert_eq!(panes, vec!["w1:p1".to_owned(), "w1:p2".to_owned()]);
    let agent = core
        .start_agent(&gw, &target(&h, 3, "w1:p9"), "claude", "novo")
        .expect("the pane the engine just created is a known target");
    assert_eq!(agent.pane_id, "w1:p9");

    // An engine whose reply carries no pane: the receipt says so, and nothing is guessed.
    h.lock().unwrap().endpoint_replies.remove("pane.split");
    let receipt = core.split(&gw, &target(&h, 3, "w1:p1"), "right").unwrap();
    assert_eq!(receipt.pane_id, None);
    assert_eq!(
        core.start_agent(&gw, &target(&h, 3, "w1:p8"), "claude", "outro")
            .unwrap_err()
            .code,
        "target_pane_missing"
    );
}

/// Would catch: input delivered to the pane the GUI *requested* focus for before the server
/// confirmed it, or to the previously focused pane.
#[test]
fn input_goes_only_to_the_server_confirmed_pane() {
    let (h, gw, mut core) = ready_core();
    // w1:p2 is confirmed. Input to w1:p1 (only requested) is refused.
    core.focus_pane(&gw, &target(&h, 3, "w1:p1")).unwrap();
    clear(&h);
    let error = core
        .send_input(
            &gw,
            &target(&h, 3, "w1:p1"),
            vec![ClientPaneInputEvent::TextCommit("ls".into())],
        )
        .unwrap_err();
    assert_eq!(error.code, "pane_not_confirmed");
    assert!(calls(&h).is_empty());

    let ctrl_e = ClientPaneInputEvent::key_press(ClientKeyCode::Char('e'), key_modifiers::CONTROL);
    core.send_input(&gw, &target(&h, 3, "w1:p2"), vec![ctrl_e.clone()])
        .unwrap();
    assert_eq!(
        calls(&h),
        vec![Call::Input("w1:p2".into(), vec![ctrl_e.clone()])]
    );

    // The server confirms w1:p1: input now goes there, and no longer to w1:p2.
    core.apply_surface(two_panes("boot-lumen", 8, "w1:p1"))
        .unwrap();
    clear(&h);
    assert_eq!(
        core.send_input(&gw, &target(&h, 3, "w1:p2"), vec![ctrl_e.clone()])
            .unwrap_err()
            .code,
        "pane_not_confirmed"
    );
    core.send_input(&gw, &target(&h, 3, "w1:p1"), vec![ctrl_e.clone()])
        .unwrap();
    assert_eq!(calls(&h), vec![Call::Input("w1:p1".into(), vec![ctrl_e])]);
}

/// Would catch: input accepted over a stale surface (rejected patch / new boot) using the old
/// topology.
#[test]
fn stale_surface_blocks_input_until_a_full_frame() {
    let (h, gw, mut core) = ready_core();
    core.mark_stale();
    let error = core
        .send_input(
            &gw,
            &target(&h, 3, "w1:p2"),
            vec![ClientPaneInputEvent::TextCommit("x".into())],
        )
        .unwrap_err();
    assert_eq!(error.code, "surface_stale");
    assert!(calls(&h).is_empty());
    core.apply_surface(two_panes("boot-lumen", 12, "w1:p2"))
        .unwrap();
    core.send_input(
        &gw,
        &target(&h, 3, "w1:p2"),
        vec![ClientPaneInputEvent::TextCommit("x".into())],
    )
    .unwrap();
    assert_eq!(calls(&h).len(), 1);
}

/// Would catch: tab actions sent through the API or without the target's workspace.
#[test]
fn tabs_use_announced_commands_and_list_from_the_server() {
    let (h, gw, mut core) = ready_core();
    h.lock().unwrap().tabs = vec![
        ("w1:t1".into(), "principal".into(), false),
        ("w1:t2".into(), "testes".into(), true),
    ];
    let tabs = core.refresh_tabs(&gw).unwrap();
    assert_eq!(
        tabs.iter()
            .map(|t| (t.tab_id.as_str(), t.label.as_str(), t.focused))
            .collect::<Vec<_>>(),
        vec![("w1:t1", "principal", false), ("w1:t2", "testes", true)]
    );
    clear(&h);
    core.create_tab(&gw, &target(&h, 3, "w1:p1")).unwrap();
    core.focus_tab(&gw, &target(&h, 3, "w1:p1"), "w1:t1")
        .unwrap();
    assert_eq!(
        calls(&h),
        vec![
            Call::Endpoint(
                "tab.create".into(),
                json!({ "workspace_id": "w1", "focus": true })
            ),
            // Spec 013: the created tab is listed from the engine's own list, not from an event.
            Call::Api("tab.list".into(), json!({})),
            Call::Endpoint("tab.focus".into(), json!({ "tab_id": "w1:t1" })),
        ]
    );
    clear(&h);
    assert_eq!(
        core.focus_tab(&gw, &target(&h, 3, "w1:p1"), "w2:t9")
            .unwrap_err()
            .code,
        "tab_not_found"
    );
    assert!(calls(&h).is_empty());
}

/// Would catch: `tab.create`/`tab.focus` sent for a pane outside the server-confirmed tab, or
/// `tab.create` trusting a WebView `workspace_id` that is not the validated pane's workspace.
#[test]
fn tab_actions_validate_the_target_pane_and_its_workspace() {
    let (h, gw, mut core) = ready_core();
    // A pane the server never showed in this tab (another workspace, and one in w1).
    let foreign = target(&h, 3, "w2:p4");
    let absent = target(&h, 3, "w1:p9");
    for t in [&foreign, &absent] {
        assert_eq!(
            core.create_tab(&gw, t).unwrap_err().code,
            "target_pane_missing"
        );
        assert_eq!(
            core.focus_tab(&gw, t, "w1:t1").unwrap_err().code,
            "target_pane_missing"
        );
    }
    // Valid pane, but the WebView claims another workspace.
    let mut spoofed = target(&h, 3, "w1:p2");
    spoofed.workspace_id = Some("w7".into());
    assert_eq!(
        core.create_tab(&gw, &spoofed).unwrap_err().code,
        "target_workspace_mismatch"
    );
    assert!(calls(&h).is_empty(), "{:?}", calls(&h));

    // Without a workspace from the WebView, it is derived from the validated pane.
    let mut derived = target(&h, 3, "w1:p2");
    derived.workspace_id = None;
    core.create_tab(&gw, &derived).unwrap();
    assert_eq!(
        calls(&h),
        vec![
            Call::Endpoint(
                "tab.create".into(),
                json!({ "workspace_id": "w1", "focus": true })
            ),
            Call::Api("tab.list".into(), json!({})),
        ]
    );
}

/// Would catch: `tab.rename` skipped, sent twice, or with a payload other than TabRenameParams.
#[test]
fn tab_rename_sends_the_engine_method_once() {
    let (h, gw, mut core) = ready_core();
    clear(&h);
    core.rename_tab(&gw, &target(&h, 3, "w1:p1"), "w1:t1", "workers")
        .unwrap();
    assert_eq!(
        calls(&h),
        vec![
            Call::Endpoint(
                "tab.rename".into(),
                json!({ "tab_id": "w1:t1", "label": "workers" })
            ),
            Call::Api("tab.list".into(), json!({})),
        ]
    );
    assert_eq!(
        core.rename_tab(&gw, &target(&h, 3, "w1:p1"), "w1:t9", "x")
            .unwrap_err()
            .code,
        "tab_not_found"
    );
}

// ---------------------------------------------------------------------------------------
// Spec 076 — autonomy flags per kind: the table lives here, the WebView only says on/off
// ---------------------------------------------------------------------------------------

/// AC-076-01 — would catch: `agent.start` sent without the kind's autonomy flags when the user
/// asked for auto mode, a kind with no entry inventing arguments, one of the two-flag kinds
/// losing a flag, or flags reaching a start the user did not mark autonomous.
#[test]
fn autonomous_start_carries_the_kinds_flags_and_nothing_otherwise() {
    let h = host("local", "hd076-alpha", "boot-lumen");
    h.lock().unwrap().kinds = vec![
        "codex".into(),
        "cursor".into(),
        "devin".into(),
        "claude".into(),
    ];
    let gw = gateway(&h, 3);
    let mut core = AgentsCore::new(&announced());
    core.discover(&gw).unwrap();
    core.apply_surface(two_panes("boot-lumen", 7, "w1:p1"))
        .unwrap();

    for (kind, args) in [
        (
            "codex",
            Some(json!(["--dangerously-bypass-approvals-and-sandbox"])),
        ),
        ("cursor", Some(json!(["--yolo", "--trust"]))),
        ("claude", Some(json!(["--dangerously-skip-permissions"]))),
        ("devin", None),
    ] {
        clear(&h);
        core.start_agent_with(&gw, &target(&h, 3, "w1:p1"), kind, "auto", true)
            .unwrap();
        let mut expected = json!({ "name": "auto", "kind": kind, "pane_id": "w1:p1" });
        if let Some(args) = args {
            expected["args"] = args;
        }
        assert_eq!(
            calls(&h),
            vec![Call::Api("agent.start".into(), expected)],
            "{kind} with auto mode"
        );
    }

    // Auto mode off: no kind carries arguments, not even the ones with a table entry, and the
    // flagless entry point every other caller uses behaves the same way.
    for kind in ["codex", "cursor", "claude", "devin"] {
        let manual = vec![Call::Api(
            "agent.start".into(),
            json!({ "name": "manual", "kind": kind, "pane_id": "w1:p1" }),
        )];
        clear(&h);
        core.start_agent_with(&gw, &target(&h, 3, "w1:p1"), kind, "manual", false)
            .unwrap();
        assert_eq!(calls(&h), manual, "{kind} without auto mode");
        clear(&h);
        core.start_agent(&gw, &target(&h, 3, "w1:p1"), kind, "manual")
            .unwrap();
        assert_eq!(calls(&h), manual, "{kind} through the flagless start");
    }
}

/// AC-076-01 — would catch: the front unable to read the flags it must show, a read that loses
/// the asked order, an unbounded query, or a start IPC that would let the WebView send its own
/// argument list instead of the on/off switch.
#[test]
fn the_flags_table_is_readable_and_the_start_ipc_takes_no_free_arguments() {
    assert_eq!(autonomy_flags("claude"), ["--dangerously-skip-permissions"]);
    assert_eq!(autonomy_flags("agy"), ["--dangerously-skip-permissions"]);
    assert_eq!(autonomy_flags("cursor"), ["--yolo", "--trust"]);
    assert_eq!(autonomy_flags("hermes"), ["--yolo", "--accept-hooks"]);
    assert_eq!(autonomy_flags("copilot"), ["--allow-all"]);
    assert_eq!(autonomy_flags("opencode"), ["--auto"]);
    assert_eq!(autonomy_flags("grok"), ["--always-approve"]);
    assert_eq!(autonomy_flags("omp"), ["--auto-approve"]);
    assert!(autonomy_flags("devin").is_empty());

    let asked: Vec<String> = ["gemini", "devin", "pi"]
        .iter()
        .map(|k| (*k).to_owned())
        .collect();
    let answer = autonomy_flags_of(&asked).unwrap();
    assert_eq!(
        answer
            .iter()
            .map(|entry| (entry.kind.as_str(), entry.flags.clone()))
            .collect::<Vec<_>>(),
        [
            ("gemini", vec!["--yolo".to_owned()]),
            ("devin", Vec::new()),
            ("pi", vec!["--approve".to_owned()]),
        ]
    );
    assert!(COMMANDS.contains(&"agent_autonomy_flags"));
    let refused =
        autonomy_flags_of(&vec!["claude".to_owned(); MAX_AUTONOMY_KINDS + 1]).unwrap_err();
    assert_eq!(refused.code, "invalid_input");

    // The start command carries a boolean, never a list the WebView could fill with arguments.
    let source = include_str!("../src/bridge/agent_commands.rs");
    let start = source
        .split("pub async fn agent_start(")
        .nth(1)
        .expect("agent_start command");
    let signature = start.split(')').next().unwrap();
    assert!(
        signature.contains("autonomous: Option<bool>"),
        "{signature}"
    );
    assert!(!signature.contains("Vec<"), "{signature}");
}

// ---------------------------------------------------------------------------------------
// Command registry
// ---------------------------------------------------------------------------------------

/// Would catch: a module command without a handler function, a generic shell/fs/API
/// passthrough command, or the WebView bridge invoking a name the backend does not declare.
#[test]
fn agent_commands_are_limited_and_match_the_frontend_bridge() {
    let source = include_str!("../src/bridge/agent_commands.rs");
    let forbidden = [
        "shell",
        "exec",
        "spawn",
        "open_url",
        "read_file",
        "write_file",
        "http",
        "fetch",
        "request",
        "raw",
        "keys",
        "approve",
    ];
    for command in COMMANDS {
        assert!(
            source.contains(&format!("#[tauri::command]\npub async fn {command}("))
                || source.contains(&format!("#[tauri::command]\npub fn {command}(")),
            "{command} has no #[tauri::command] handler"
        );
        for word in forbidden {
            assert!(!command.contains(word), "{command} looks like {word}");
        }
    }
    assert_eq!(source.matches("#[tauri::command]").count(), COMMANDS.len());

    let bridge = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../src/agents/bridge.ts"
    ))
    .unwrap();
    let mut invoked: Vec<&str> = bridge
        .split("invoke<")
        .skip(1)
        .filter_map(|rest| rest.split('"').nth(1))
        .collect();
    invoked.sort_unstable();
    invoked.dedup();
    let mut declared: Vec<&str> = COMMANDS.to_vec();
    declared.sort_unstable();
    assert_eq!(invoked, declared);
}

// ---------------------------------------------------------------------------------------
// Native E2E: real Tauri window with AgentPanel and the real backend/IPC against a disposable
// session. A deterministic fake `pi` agent (bash, no network/credentials) is put first on the
// PATH of the session's first pane; it reports its state through `herdr pane report-agent`
// and logs every prompt and every raw byte it receives while waiting for approval.
// Flow: start agent → prompt → blocked (GUI answers nothing) → split → ratio 0.3 → focus
// the new pane and type into it → "Abrir pane" → GUI shortcut (ratio) → Ctrl+E → "y" → idle.
// ---------------------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod e2e {
    use super::agent_commands::{self, AgentsState};
    use super::native_harness::{self, run_phase};
    use super::window_harness;
    use herdr_client::api::ApiClient;
    use herdr_client::{SessionName, SessionPaths};
    use serde_json::{json, Value};
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    const WINDOW_PHASE_TEST: &str = "e2e::e2e_agents_window";

    const FAKE_AGENT: &str = r#"#!/usr/bin/env bash
# hd004 deterministic fake agent: no network, no credentials, no model.
log="${HD004_FAKE_LOG:?}"
case "${HERDR_SESSION:-}" in hd004-*) ;; *) echo "refusing: not a disposable hd004 session" >&2; exit 3 ;; esac
[ -n "${HERDR_SOCKET_PATH:-}" ] && [ -n "${HERDR_PANE_ID:-}" ] || { echo "refusing: no pane socket" >&2; exit 3; }
now() { date +%s%3N; }
report() {
  "${HERDR_BIN_PATH:-herdr}" pane report-agent "$HERDR_PANE_ID" --source hd004-fake --agent pi --state "$1" >/dev/null 2>&1
  echo "state $1 $(now)" >>"$log"
}
echo "start pid=$$ argv=$* $(now)" >>"$log"
printf 'fake-agent hd004 pronto\n> '
report idle
while IFS= read -r line; do
  echo "prompt $line $(now)" >>"$log"
  report working
  printf 'trabalhando: %s\n' "$line"
  sleep 1.5
  printf 'Aprovar alteração? [y/n] '
  report blocked
  old=$(stty -g)
  stty raw -echo
  deadline=$((SECONDS + 120))
  answered=0
  while [ "$SECONDS" -lt "$deadline" ]; do
    if IFS= read -rsn1 -d '' -t 1 c; then
      hex=$(printf '%s' "$c" | od -An -tx1 | tr -d ' \n')
      [ -z "$hex" ] && hex=00
      echo "byte $hex $(now)" >>"$log"
      if [ "$c" = "y" ]; then answered=1; break; fi
    fi
  done
  stty "$old"
  if [ "$answered" = 1 ]; then
    printf '\naprovado\n> '
    echo "approved $(now)" >>"$log"
  else
    echo "approval_timeout $(now)" >>"$log"
  fi
  report idle
done
"#;

    /// One GUI process: a Tauri window over the built frontend whose page runs
    /// `src/features/agents/e2e.ts` against the real agent commands.
    #[test]
    #[ignore = "window phase of e2e_agents_flow; fails when run outside the harness"]
    fn e2e_agents_window() {
        let phase = native_harness::current_phase();
        let session = SessionName::parse(&native_harness::required("HERDR_DESKTOP_E2E_SESSION"))
            .expect("disposable session name");
        let params: Value =
            serde_json::from_str(&native_harness::required("HERDR_DESKTOP_E2E_PARAMS")).unwrap();
        let herdr_config = herdr_client::session::herdr_config_dir(&|k| std::env::var(k).ok());
        let builder = tauri::Builder::default()
            .manage(AgentsState::new(herdr_config, Some(session)))
            .invoke_handler(tauri::generate_handler![
                agent_commands::agents_connect,
                agent_commands::agents_overview,
                agent_commands::agents_detach,
                agent_commands::agent_start,
                agent_commands::agent_prompt,
                agent_commands::agent_open_attention,
                agent_commands::pane_split,
                agent_commands::pane_focus,
                agent_commands::pane_set_split_ratio,
                agent_commands::pane_input,
                agent_commands::tab_create,
                agent_commands::tab_focus,
                agent_commands::tab_close,
                window_harness::harness_report,
            ]);
        window_harness::run_feature_window(
            tauri::generate_context!(),
            builder,
            "agents",
            &phase,
            params,
            PathBuf::from(native_harness::required(native_harness::RESULT_ENV)),
            Duration::from_secs(240),
        );
        panic!("the harness window returned without reporting done");
    }

    fn wait_file_containing(path: &Path, needle: &str, timeout: Duration) -> String {
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(text) = std::fs::read_to_string(path) {
                if text.contains(needle) {
                    return text;
                }
            }
            assert!(
                Instant::now() < deadline,
                "{} never contained {needle:?}",
                path.display()
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn pane_text(api: &ApiClient, pane: &str) -> String {
        api.request(
            "pane.read",
            json!({ "pane_id": pane, "source": "recent", "lines": 60 }),
        )
        .unwrap()["read"]["text"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    }

    #[test]
    #[ignore = "needs the disposable session created by scripts/feature-harness/session.sh; run by just check-spec 004"]
    fn e2e_agents_flow() {
        let session = native_harness::required("HERDR_DESKTOP_E2E_SESSION");
        let first_pane = native_harness::required("HERDR_DESKTOP_E2E_PANE");
        let report = PathBuf::from(native_harness::required("HERDR_DESKTOP_E2E_REPORT"));
        let dir = PathBuf::from(native_harness::required("HERDR_DESKTOP_E2E_DIR"));
        assert!(
            session.starts_with("hd004-"),
            "never run against a non-disposable session: {session}"
        );
        std::fs::create_dir_all(&report).unwrap();
        let session_name = SessionName::parse(&session).unwrap();
        let herdr_config = herdr_client::session::herdr_config_dir(&|k| std::env::var(k).ok());
        let paths = SessionPaths::for_session(&herdr_config, &session_name);
        // Observer of the test itself (JSON API of the disposable session only).
        let api = ApiClient::new(&paths.api_socket, "local");

        let mut log = std::fs::File::create(report.join("e2e-agents.log")).unwrap();
        let mut note = |line: String| {
            eprintln!("{line}");
            writeln!(log, "{line}").unwrap();
        };

        // --- Fake agent first on the PATH of the ready pane --------------------------------------
        let bin = dir.join("fakebin");
        std::fs::create_dir_all(&bin).unwrap();
        let fake = bin.join("pi");
        std::fs::write(&fake, FAKE_AGENT).unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let fake_log = dir.join("fake-agent.log");
        let which = dir.join("which-pi");
        let _ = std::fs::remove_file(&fake_log);
        let _ = std::fs::remove_file(&which);
        api.request(
            "pane.send_text",
            json!({
                "pane_id": first_pane,
                "text": format!(
                    "export PATH='{}':\"$PATH\" HD004_FAKE_LOG='{}'; hash -r; command -v pi > '{}'; clear\n",
                    bin.display(), fake_log.display(), which.display()
                ),
            }),
        )
        .unwrap();
        let resolved = wait_file_containing(&which, "pi", Duration::from_secs(15));
        assert_eq!(
            resolved.trim(),
            fake.display().to_string(),
            "pi must resolve to the fake agent before any start (never a real agent)"
        );
        let manifests = api.request("server.agent_manifests", json!({})).unwrap();
        let server_kinds: Vec<String> = manifests["manifests"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| m["agent"].as_str().map(str::to_owned))
            .collect();
        note(format!(
            "session={session} pane={first_pane} fake agent={} server kinds={server_kinds:?}",
            fake.display()
        ));
        assert!(server_kinds.iter().any(|k| k == "pi"));

        let agent_name = format!("hd004-fake-{}", std::process::id());
        let prompt = "Tarefa HD004 enviada pela GUI";
        let echo_marker = "HD004-P2-OK";
        let params = json!({
            "pane_id": first_pane,
            "kind": "pi",
            "agent_name": agent_name,
            "prompt": prompt,
            "echo_marker": echo_marker,
        })
        .to_string();
        let work = tempfile::tempdir().unwrap();
        let env = [
            ("HERDR_DESKTOP_E2E_PARAMS", params.as_str()),
            ("HERDR_DESKTOP_E2E_SESSION", session.as_str()),
        ];

        // --- The window ---------------------------------------------------------------------------
        let gui = run_phase(WINDOW_PHASE_TEST, "flow", work.path(), &env);
        let r = &gui.result;
        note(format!("GUI (pid {}) window report: {r}", gui.pid));
        assert!(r["error"].is_null(), "{r}");

        // AC-004-01: kinds from the server; start through agent.start on the chosen pane.
        let gui_kinds: Vec<String> = serde_json::from_value(r["loaded"]["kinds"].clone()).unwrap();
        assert_eq!(gui_kinds, server_kinds, "kinds shown are the server's");
        assert!(r["loaded"]["onboarding"]
            .as_str()
            .unwrap_or_default()
            .contains("Nenhum agente"));
        assert_eq!(r["loaded"]["start_enabled_without_name"], false);
        let agent = api
            .request("agent.get", json!({ "target": first_pane }))
            .unwrap()["agent"]
            .clone();
        assert_eq!(agent["name"], agent_name.as_str());
        assert_eq!(agent["agent"], "pi");
        assert_eq!(r["started"]["name"], agent_name.as_str());

        let fake_text = std::fs::read_to_string(&fake_log).unwrap();
        note(format!("fake agent log:\n{fake_text}"));
        let lines: Vec<&str> = fake_text.lines().collect();
        assert_eq!(
            lines.iter().filter(|l| l.starts_with("start ")).count(),
            1,
            "one agent process started"
        );
        let prompts: Vec<&str> = lines
            .iter()
            .copied()
            .filter(|l| l.starts_with("prompt "))
            .collect();
        assert_eq!(
            prompts.len(),
            1,
            "the prompt arrived exactly once: {prompts:?}"
        );
        assert!(prompts[0].starts_with(&format!("prompt {prompt} ")));
        assert_eq!(r["outcome"], "sent");

        // AC-004-02: the GUI showed working → blocked → idle/done from the server, with
        // distinct text; it answered nothing while blocked; the answer came after the user acted.
        let seen: Vec<(String, String)> = r["seen"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                (
                    s["status"].as_str().unwrap().to_owned(),
                    s["text"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        let statuses: Vec<&str> = seen.iter().map(|(s, _)| s.as_str()).collect();
        let pos = |name: &str| statuses.iter().position(|s| *s == name);
        let working = pos("working").expect("GUI showed working");
        let blocked = pos("blocked").expect("GUI showed blocked");
        assert!(working < blocked, "{statuses:?}");
        let final_status = r["final"]["status"].as_str().unwrap();
        assert!(statuses[blocked..].contains(&final_status), "{statuses:?}");
        let mut texts: Vec<&str> = seen.iter().map(|(_, t)| t.as_str()).collect();
        texts.sort_unstable();
        texts.dedup();
        let mut distinct_statuses = statuses.clone();
        distinct_statuses.sort_unstable();
        distinct_statuses.dedup();
        assert_eq!(
            texts.len(),
            distinct_statuses.len(),
            "one text per state: {seen:?}"
        );
        let server_final = api
            .request("agent.get", json!({ "target": first_pane }))
            .unwrap()["agent"]["agent_status"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            final_status, server_final,
            "GUI shows the state the server reports"
        );
        assert_eq!(r["blocked"]["still_blocked_after_wait"], true);
        assert_eq!(r["blocked"]["prompt_disabled"], true);

        // AC-004-03: split/ratio reflected from the server; input to the confirmed pane.
        let layout = api
            .request("pane.layout", json!({ "pane_id": first_pane }))
            .unwrap()["layout"]
            .clone();
        note(format!("server layout at the end: {layout}"));
        let second_pane = r["layout"]["afterFocus"]["second_pane"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_ne!(second_pane, first_pane);
        assert_eq!(
            r["layout"]["afterSplit"]["panes"].as_array().unwrap().len(),
            2
        );
        let ratio_after_apply = r["layout"]["afterRatio"]["ratio"].as_f64().unwrap();
        assert!(
            (ratio_after_apply - 0.3).abs() < 0.02,
            "{ratio_after_apply}"
        );
        let ratio_after_shortcut = r["shortcuts"]["ratio_after"].as_f64().unwrap();
        assert!(
            (ratio_after_shortcut - 0.2).abs() < 0.02,
            "{ratio_after_shortcut}"
        );
        let server_ratio = layout["splits"][0]["ratio"].as_f64().unwrap();
        assert!(
            (server_ratio - 0.2).abs() < 1e-3,
            "server ratio {server_ratio}"
        );
        // GUI pane cells after the shortcut equal the server's pane rects.
        let server_rects: Vec<(String, Vec<u64>)> = layout["panes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                (
                    p["pane_id"].as_str().unwrap().to_owned(),
                    ["x", "y", "width", "height"]
                        .iter()
                        .map(|k| p["rect"][k].as_u64().unwrap())
                        .collect(),
                )
            })
            .collect();
        let gui_rects: Vec<(String, Vec<u64>)> = r["panes_final"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                (
                    p["pane"].as_str().unwrap().to_owned(),
                    p["cells"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|c| c.as_u64().unwrap())
                        .collect(),
                )
            })
            .collect();
        note(format!(
            "server rects {server_rects:?} gui rects {gui_rects:?}"
        ));
        assert_eq!(gui_rects, server_rects, "GUI geometry is the server's");
        assert_eq!(layout["focused_pane_id"], first_pane.as_str());

        let second_text = pane_text(&api, &second_pane);
        assert!(
            second_text.contains(echo_marker),
            "typed command reached the confirmed pane: {second_text}"
        );
        assert!(
            !fake_text.contains(echo_marker),
            "input typed for the second pane never reached the agent pane"
        );
        assert_eq!(r["layout"]["afterFocus"]["echo_prevented"], true);

        // Bytes the agent received while waiting for approval: exactly Ctrl+E once, then "y".
        // Nothing arrived before the user started pressing keys (the GUI did not answer).
        let keys_started = r["shortcuts"]["keys_started_at"].as_u64().unwrap();
        let bytes: Vec<(String, u64)> = lines
            .iter()
            .filter_map(|l| l.strip_prefix("byte "))
            .map(|rest| {
                let mut parts = rest.split(' ');
                (
                    parts.next().unwrap().to_owned(),
                    parts.next().unwrap().parse().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            bytes.iter().map(|(h, _)| h.as_str()).collect::<Vec<_>>(),
            vec!["05", "79"],
            "GUI shortcut did not leak; Ctrl+E arrived once; then the user's answer"
        );
        assert!(
            bytes.iter().all(|(_, at)| *at >= keys_started),
            "no byte before the user acted: {bytes:?} vs {keys_started}"
        );
        assert!(fake_text.contains("approved "));
        assert_eq!(r["shortcuts"]["gui_prevented"], true);
        assert_eq!(r["shortcuts"]["ctrl_e_prevented"], true);

        // Closing the GUI kept the agent process alive in the engine.
        let pid: u32 = lines
            .iter()
            .find_map(|l| l.strip_prefix("start pid="))
            .and_then(|rest| rest.split(' ').next())
            .and_then(|p| p.parse().ok())
            .unwrap();
        assert!(
            native_harness::process_alive(pid),
            "agent survived the GUI close"
        );

        std::fs::write(
            report.join("e2e-agents-summary.json"),
            serde_json::to_string_pretty(&json!({
                "session": session,
                "gui_process": gui.pid,
                "first_pane": first_pane,
                "second_pane": second_pane,
                "server_kinds": server_kinds,
                "agent": { "name": agent_name, "kind": "pi", "pid": pid, "final_status": server_final },
                "statuses_seen": seen,
                "prompt_deliveries": prompts.len(),
                "approval_bytes": bytes.iter().map(|(h, _)| h.clone()).collect::<Vec<_>>(),
                "ratio": { "applied": ratio_after_apply, "after_gui_shortcut": ratio_after_shortcut, "server": server_ratio },
                "rects": { "gui": gui_rects, "server": server_rects },
                "echo_in_second_pane": true,
            }))
            .unwrap(),
        )
        .unwrap();
    }
}
