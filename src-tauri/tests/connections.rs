//! Spec 003 — SSH connections and reconnection (seam: bridge contract + connection states
//! with fake gateways/runners and a controlled clock; one native E2E local+SSH with
//! interruption/reconnection against a disposable sshd and two named Herdr sessions).
//!
//! The backend modules are compiled here through `#[path]`: composing them into
//! `src-tauri/src/lib.rs` belongs to spec 007. The module tree mirrors the future crate
//! (`crate::connections`, `crate::bridge::ssh`).
//!
//! AC-003-01: Local and SSH with the same pane id w1:p1 — input addressed to SSH reaches only
//!            SSH; after losing SSH, Local still accepts input and the remote pane shows
//!            Reconectando with input disabled.
//! AC-003-02: after a reconnection with a new boot/generation, a current snapshot with an old
//!            surface keeps input blocked; only the reconciled snapshot/surface pair enables it.
//!            Commands pending with an unknown result are never re-sent.
//! AC-003-03: unknown host key, MFA or incompatible server put the host in Precisa de atenção
//!            with explicit configuration; nothing accepts a key, installs, restarts or stops a
//!            server automatically.

#[allow(dead_code)]
#[path = "../src/connections/mod.rs"]
mod connections;

mod theme {
    pub use herdr_desktop::theme::*;
}

#[allow(dead_code)]
#[path = "../src/bridge"]
mod bridge {
    pub mod ssh;
}

#[allow(dead_code)]
#[path = "../../scripts/feature-harness/native.rs"]
mod native_harness;

#[cfg(target_os = "linux")]
#[allow(dead_code)]
#[path = "../../scripts/feature-harness/window.rs"]
mod window_harness;

use std::collections::VecDeque;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, sync_channel, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use herdr_client::protocol::endpoint::{
    HEALTH_CHECK_CAPABILITY, PRESENTATION_EFFECTS_FENCE_CAPABILITY, SURFACE_INTEREST_CAPABILITY,
};
use herdr_client::protocol::wire::{
    CellData, ClientKeyCode, ClientPaneInputEvent, ClientShellSnapshot, CursorState, FrameData,
    PaneSurfaceFrame, PaneSurfacePane, PaneSurfacePatch, SurfaceRect,
};
use herdr_client::{
    ConnectOptions, GatewayEvent, LiveIdentity, Negotiated, QualifiedTarget, RuntimeError,
    RuntimeGateway, SessionName, SurfaceGeometry,
};
use serde_json::{json, Value};

use bridge::ssh::{ProcessOutput, SshChild, SshConnector, SshRunner};
use connections::commands::{ConnectionsConfig, ConnectionsState};
use connections::failure::{
    check_negotiated, check_server_status, classify_ssh_failure, AttentionReason, ConnectFailure,
};
use connections::hub::{
    ApiLane, Connected, HostHub, HostKind, HostSpec, InputBlock, VisibilityOutcome,
};
use connections::profiles::{ProfileStore, SshProfileDraft, STORE_FILE};
use connections::remote_binary::{
    configured_remote_binary, discovery_candidates, known_binary_candidate_script,
    parse_client_status, RemoteBinary,
};
use connections::ssh_options::{
    build_sftp, build_ssh, build_ssh_script, remote_command_line, IsolatedSshConfig,
    OpenSshCommand, ProfileId, RemoteHerdrCommand, SshIdentity, SshTarget,
};
use connections::state::{
    backoff_delay, HealthAction, HealthMonitor, LinkMachine, LinkPhase, BRIDGE_IDLE_TIMEOUT,
    HEALTH_INTERVAL, HEALTH_TIMEOUT,
};

// ---------------------------------------------------------------------------------------
// Fakes
// ---------------------------------------------------------------------------------------

/// Bytes that reached one fake engine connection.
#[derive(Debug, Default)]
struct Wire {
    inputs: Vec<(String, Vec<ClientPaneInputEvent>)>,
    detached: u32,
}

struct FakeGateway {
    endpoint: String,
    session: String,
    boot: String,
    wire: Arc<Mutex<Wire>>,
}

impl FakeGateway {
    fn boxed(endpoint: &str, session: &str, boot: &str, wire: &Arc<Mutex<Wire>>) -> Box<Self> {
        Box::new(Self {
            endpoint: endpoint.into(),
            session: session.into(),
            boot: boot.into(),
            wire: wire.clone(),
        })
    }
}

fn unsupported() -> RuntimeError {
    RuntimeError::new("unsupported_in_fake", "not used by the connections seam")
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
        Err(unsupported())
    }
    fn take_events(&mut self) -> Option<Receiver<GatewayEvent>> {
        None
    }
    fn api_request(&self, _method: &str, _params: Value) -> Result<Value, RuntimeError> {
        Err(unsupported())
    }
    fn endpoint_request(&self, _method: &str, _params: Value) -> Result<Value, RuntimeError> {
        Err(unsupported())
    }
    fn send_input(
        &self,
        target: &QualifiedTarget,
        events: Vec<ClientPaneInputEvent>,
    ) -> Result<(), RuntimeError> {
        target.validate(&self.identity().unwrap())?;
        self.wire
            .lock()
            .unwrap()
            .inputs
            .push((target.pane_id.clone(), events));
        Ok(())
    }
    fn resize(&self, _geometry: SurfaceGeometry) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn set_focus(&self, _focused: bool) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn detach(&mut self) {
        self.wire.lock().unwrap().detached += 1;
    }
    fn is_connected(&self) -> bool {
        true
    }
}

/// API lane that records calls; a scripted reply may block until the test releases it.
#[derive(Default)]
struct FakeApi {
    calls: Mutex<Vec<(String, Value)>>,
    replies: Mutex<VecDeque<Receiver<Result<Value, RuntimeError>>>>,
    entered: Mutex<Option<std::sync::mpsc::SyncSender<String>>>,
}

impl FakeApi {
    fn arc() -> Arc<Self> {
        Arc::new(Self::default())
    }
    /// Next call blocks until the returned sender provides its result.
    fn script(&self) -> std::sync::mpsc::SyncSender<Result<Value, RuntimeError>> {
        let (tx, rx) = sync_channel(1);
        self.replies.lock().unwrap().push_back(rx);
        tx
    }
    fn on_enter(&self) -> Receiver<String> {
        let (tx, rx) = sync_channel(8);
        *self.entered.lock().unwrap() = Some(tx);
        rx
    }
    fn methods(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|(m, _)| m.clone())
            .collect()
    }
}

impl ApiLane for FakeApi {
    fn request(&self, method: &str, params: Value) -> Result<Value, RuntimeError> {
        self.calls
            .lock()
            .unwrap()
            .push((method.to_owned(), params.clone()));
        if let Some(tx) = self.entered.lock().unwrap().as_ref() {
            let _ = tx.try_send(method.to_owned());
        }
        let scripted = self.replies.lock().unwrap().pop_front();
        match scripted {
            Some(rx) => rx
                .recv_timeout(Duration::from_secs(10))
                .expect("test released the scripted reply"),
            None => Ok(json!({ "type": "ok", "method": method })),
        }
    }
}

const LOCAL: &str = "local";
const SSH_ID: &str = "0123456789abcdef0123456789abcdef";

fn local_spec() -> HostSpec {
    HostSpec {
        endpoint: LOCAL.into(),
        label: "Este computador".into(),
        kind: HostKind::Local,
        session: "hd003-local-a".into(),
        target: None,
        visible: true,
    }
}

fn ssh_spec(visible: bool) -> HostSpec {
    HostSpec {
        endpoint: SSH_ID.into(),
        label: "dev-box".into(),
        kind: HostKind::Ssh,
        session: "hd003-remote-b".into(),
        target: Some("user@dev-box.example".into()),
        visible,
    }
}

fn snapshot(boot: &str) -> GatewayEvent {
    let mut snap: ClientShellSnapshot = serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../tests/fixtures/endpoint-snapshot-v1.json"
        ))
        .unwrap(),
    )
    .unwrap();
    snap.boot_id = boot.into();
    assert_eq!(snap.panes[0].pane_id, "w1:p1", "fixture premise");
    GatewayEvent::Snapshot(Box::new(snap))
}

fn pane_rect(width: u16, height: u16) -> PaneSurfacePane {
    let rect = SurfaceRect {
        x: 0,
        y: 0,
        width,
        height,
    };
    PaneSurfacePane {
        pane_id: "w1:p1".into(),
        content_revision: 1,
        rect,
        inner_rect: rect,
        scrollbar_rect: None,
        scroll: None,
        focused: true,
        mouse_reporting: false,
        sgr_pixel_mouse: false,
        alternate_screen_active: false,
        pixel_width: u32::from(width) * 9,
        pixel_height: u32::from(height) * 18,
    }
}

fn surface_frame(boot: &str, revision: u64, text: &str) -> PaneSurfaceFrame {
    let width = 12u16;
    let mut cells: Vec<CellData> = text
        .chars()
        .chain(std::iter::repeat(' '))
        .take(usize::from(width))
        .map(|c| CellData {
            symbol: c.to_string(),
            fg: 0,
            bg: 0,
            modifier: 0,
            skip: false,
            hyperlink: None,
        })
        .collect();
    cells.truncate(usize::from(width));
    PaneSurfaceFrame {
        boot_id: boot.into(),
        projection_revision: 1,
        surface_revision: revision,
        frame: FrameData {
            cells,
            width,
            height: 1,
            cursor: Some(CursorState {
                x: 0,
                y: 0,
                visible: true,
                shape: 0,
            }),
            hyperlinks: vec![],
            graphics: vec![],
        },
        panes: vec![pane_rect(width, 1)],
        splits: vec![],
        popup: None,
        graphics: Default::default(),
    }
}

fn surface(boot: &str, revision: u64, text: &str) -> GatewayEvent {
    GatewayEvent::Surface(Box::new(surface_frame(boot, revision, text)))
}

fn text(s: &str) -> Vec<ClientPaneInputEvent> {
    vec![ClientPaneInputEvent::TextCommit(s.into())]
}

fn lost() -> GatewayEvent {
    GatewayEvent::Disconnected(
        RuntimeError::new("connection_lost", "connection closed")
            .retryable()
            .with_endpoint(SSH_ID),
    )
}

/// Connects `endpoint` in the hub with a fake gateway and feeds snapshot + surface.
#[allow(clippy::too_many_arguments)]
fn online(
    hub: &HostHub,
    endpoint: &str,
    session: &str,
    boot: &str,
    wire: &Arc<Mutex<Wire>>,
    api: &Arc<FakeApi>,
    now: Instant,
    screen: &str,
) -> u64 {
    let ticket = hub
        .request_connect(endpoint, now)
        .unwrap()
        .expect("a connect ticket");
    hub.finish_connect(
        &ticket,
        Ok(Connected {
            gateway: FakeGateway::boxed(endpoint, session, boot, wire),
            api: api.clone(),
        }),
        now,
    );
    hub.apply_event(endpoint, ticket.token, snapshot(boot), now);
    hub.apply_event(endpoint, ticket.token, surface(boot, 1, screen), now);
    ticket.token
}

struct Pair {
    hub: HostHub,
    local_wire: Arc<Mutex<Wire>>,
    ssh_wire: Arc<Mutex<Wire>>,
    local_api: Arc<FakeApi>,
    ssh_api: Arc<FakeApi>,
    ssh_token: u64,
    t0: Instant,
}

fn pair() -> Pair {
    let hub = HostHub::new();
    hub.add_host(local_spec()).unwrap();
    hub.add_host(ssh_spec(true)).unwrap();
    let local_wire = Arc::new(Mutex::new(Wire::default()));
    let ssh_wire = Arc::new(Mutex::new(Wire::default()));
    let local_api = FakeApi::arc();
    let ssh_api = FakeApi::arc();
    let t0 = Instant::now();
    online(
        &hub,
        LOCAL,
        "hd003-local-a",
        "boot-local-7",
        &local_wire,
        &local_api,
        t0,
        "local$",
    );
    let ssh_token = online(
        &hub,
        SSH_ID,
        "hd003-remote-b",
        "boot-remote-1",
        &ssh_wire,
        &ssh_api,
        t0,
        "remote$",
    );
    Pair {
        hub,
        local_wire,
        ssh_wire,
        local_api,
        ssh_api,
        ssh_token,
        t0,
    }
}

fn host<'a>(snap: &'a Value, endpoint: &str) -> &'a Value {
    snap["hosts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|h| h["endpoint"] == endpoint)
        .unwrap_or_else(|| panic!("host {endpoint} missing in {snap}"))
}

fn dto(hub: &HostHub, now: Instant) -> Value {
    serde_json::to_value(hub.snapshot(now)).unwrap()
}

// ---------------------------------------------------------------------------------------
// Spec 010 (AC-010-02) — branch of the session projected from the client shell snapshot
// ---------------------------------------------------------------------------------------

/// Fixture snapshot edited as JSON before decoding (the wire types stay the engine's).
fn snapshot_with(boot: &str, edit: impl FnOnce(&mut Value)) -> GatewayEvent {
    let mut raw: Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../tests/fixtures/endpoint-snapshot-v1.json"
        ))
        .unwrap(),
    )
    .unwrap();
    raw["boot_id"] = json!(boot);
    edit(&mut raw);
    GatewayEvent::Snapshot(Box::new(serde_json::from_value(raw).unwrap()))
}

/// Would catch: no `branch` in the host DTO, a branch taken from the first workspace or from the
/// engine's global `focused_workspace_id` instead of the workspace of this connection's focused
/// tab, a stale branch kept after a snapshot without one, or a fabricated default.
#[test]
fn host_dto_projects_the_branch_of_the_focused_tab_workspace_or_none() {
    let p = pair();
    let now = p.t0;
    assert_eq!(
        host(&dto(&p.hub, now), LOCAL)["branch"],
        json!("main"),
        "fixture workspace w1 (focused tab w1:t1) is on main"
    );

    p.hub.apply_event(
        SSH_ID,
        p.ssh_token,
        snapshot_with("boot-remote-1", |raw| {
            raw["workspaces"].as_array_mut().unwrap().push(json!({
                "workspace_id": "w2", "active_tab_id": "w2:t1", "new_workspace_cwd": "/other",
                "number": 2, "label": "other", "custom_label": false, "branch": "feat/login",
                "git_ahead_behind": null, "tokens": [], "worktree": null, "focused": false,
                "agent_status": "idle"
            }));
            raw["tabs"].as_array_mut().unwrap().push(json!({
                "tab_id": "w2:t1", "workspace_id": "w2", "number": 1, "label": "other",
                "custom_label": false, "zoomed": false, "focused": true, "agent_status": "idle"
            }));
            raw["tabs"][0]["focused"] = json!(false);
            raw["focused_tab_id"] = json!("w2:t1");
            assert_eq!(
                raw["focused_workspace_id"], "w1",
                "global focus stays on w1"
            );
        }),
        now,
    );
    let both = dto(&p.hub, now);
    assert_eq!(host(&both, SSH_ID)["branch"], json!("feat/login"));
    assert_eq!(host(&both, LOCAL)["branch"], json!("main"));

    p.hub.apply_event(
        SSH_ID,
        p.ssh_token,
        snapshot_with("boot-remote-1", |raw| {
            raw["workspaces"][0]["branch"] = Value::Null;
        }),
        now,
    );
    let without = dto(&p.hub, now);
    assert!(
        without["hosts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|h| h.get("branch").is_some()),
        "branch is always present in the DTO (null when unknown): {without}"
    );
    assert_eq!(host(&without, SSH_ID)["branch"], Value::Null);
    assert_eq!(host(&without, LOCAL)["branch"], json!("main"));
}

/// Spec 025 (AC-025-01): the host DTO carries the engine's own workspace list, ordered by
/// `number`, with the cwd/branch of the workspace's focused pane — exactly what the projects
/// tree renders. Would catch: fields fabricated by the desktop, a lost/reordered list, a global
/// focus applied to every workspace, or a cwd taken from another workspace's pane.
#[test]
fn host_dto_projects_the_engine_workspace_list_ordered_by_number() {
    let p = pair();
    let now = p.t0;
    let initial = dto(&p.hub, now);
    let local = host(&initial, LOCAL);
    assert_eq!(local["workspaces"][0]["workspace_id"], json!("w1"));
    assert_eq!(local["workspaces"][0]["number"], json!(1));
    assert_eq!(local["workspaces"][0]["focused"], json!(true));
    assert_eq!(local["workspaces"][0]["cwd"], json!("/repo"));
    assert_eq!(local["workspaces"][0]["branch"], json!("main"));
    assert_eq!(local["workspaces"][0]["tab_count"], json!(1));
    assert_eq!(local["workspaces"][0]["pane_count"], json!(1));
    assert_eq!(local["workspaces"][0]["active_tab_id"], json!("w1:t1"));
    assert_eq!(
        local["workspaces"][0]["agent_status"],
        json!("unknown"),
        "a status of a newer server never becomes a fabricated one"
    );

    let workspace = |id: &str, number: usize, label: &str, branch: Option<&str>| {
        json!({
            "workspace_id": id, "active_tab_id": format!("{id}:t1"), "new_workspace_cwd": "/elsewhere",
            "number": number, "label": label, "custom_label": false,
            "branch": branch, "git_ahead_behind": null, "tokens": [], "worktree": null,
            "focused": false, "agent_status": "working"
        })
    };
    let tab = |workspace: &str, number: usize| {
        json!({
            "tab_id": format!("{workspace}:t1"), "workspace_id": workspace, "number": number,
            "label": "main", "custom_label": false, "zoomed": false, "focused": false,
            "agent_status": "working"
        })
    };
    let pane = |workspace: &str, cwd: &str| {
        json!({
            "pane_id": format!("{workspace}:p1"), "workspace_id": workspace, "tab_id": format!("{workspace}:t1"),
            "label": "shell", "cwd": cwd, "foreground_cwd": cwd, "focused": false,
            "right_click_passthrough": false
        })
    };
    // The engine snapshot of the remote host: three workspaces, registered out of number order.
    let remote = |raw: &mut Value, keep_w3: bool| {
        let mut workspaces = vec![
            workspace("w2", 3, "backend", Some("feat/backend")),
            workspace("w1", 1, "herdr", Some("master")),
        ];
        let mut tabs = vec![tab("w1", 1), tab("w2", 1)];
        let mut panes = vec![pane("w1", "/srv/herdr"), pane("w2", "/srv/backend")];
        if keep_w3 {
            workspaces.push(workspace("w3", 2, "frontend", None));
            tabs.push(tab("w3", 1));
            panes.push(pane("w3", "/srv/frontend"));
        }
        raw["workspaces"] = json!(workspaces);
        raw["tabs"] = json!(tabs);
        raw["panes"] = json!(panes);
        // w2's shell cd'ed into a subdirectory: the launch cwd stays the saved root.
        raw["panes"][1]["foreground_cwd"] = json!("/srv/backend/work");
        raw["workspaces"][1]["focused"] = json!(true);
        raw["tabs"][0]["focused"] = json!(true);
        raw["panes"][0]["focused"] = json!(true);
    };
    p.hub.apply_event(
        SSH_ID,
        p.ssh_token,
        snapshot_with("boot-remote-1", |raw| remote(raw, true)),
        now,
    );
    let with_three = dto(&p.hub, now);
    let ssh = host(&with_three, SSH_ID);
    let ids: Vec<&str> = ssh["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["workspace_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["w1", "w3", "w2"], "ordered by number, not by arrival");
    assert_eq!(ssh["workspaces"][0]["cwd"], json!("/srv/herdr"));
    assert_eq!(ssh["workspaces"][0]["branch"], json!("master"));
    assert_eq!(ssh["workspaces"][0]["focused"], json!(true));
    assert_eq!(ssh["workspaces"][1]["cwd"], json!("/srv/frontend"));
    assert_eq!(
        ssh["workspaces"][1]["branch"],
        Value::Null,
        "no git is a null branch, never a fabricated dash"
    );
    assert_eq!(
        ssh["workspaces"][2]["cwd"],
        json!("/srv/backend/work"),
        "the main cwd follows the live foreground cwd"
    );
    assert_eq!(
        ssh["workspaces"][2]["focused"],
        json!(false),
        "one focused workspace per host"
    );
    // Spec 025 AC-025-05: every cwd of the workspace travels, so a `cd` in a pane never turns
    // the open workspace into a closed saved project.
    let cwds = ssh["workspaces"][2]["cwds"].as_array().unwrap();
    assert!(cwds.contains(&json!("/srv/backend")), "{cwds:?}");
    assert!(cwds.contains(&json!("/srv/backend/work")), "{cwds:?}");
    assert!(
        !cwds.contains(&json!("/elsewhere")),
        "the follow-policy cwd never masquerades as a workspace root: {cwds:?}"
    );
    assert_eq!(
        host(&with_three, LOCAL)["workspaces"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "the other host keeps its own list"
    );

    // A workspace closed on the engine disappears from the host's list on the next snapshot.
    p.hub.apply_event(
        SSH_ID,
        p.ssh_token,
        snapshot_with("boot-remote-1", |raw| remote(raw, false)),
        now,
    );
    assert_eq!(
        host(&dto(&p.hub, now), SSH_ID)["workspaces"]
            .as_array()
            .unwrap()
            .len(),
        2,
        "a closed workspace is gone without restarting the GUI"
    );
}

/// AC-039-01: tabs and panes of an endpoint connection are derived from the ClientShellSnapshot
/// projection (2 workspaces x 2 abas, agents, cwd, focus) and updated when a tab is closed.
/// AC-039-03: install_latency sets only the handshake RTT duration.
#[test]
fn host_dto_projects_tabs_and_panes_from_client_shell_snapshot_and_closed_tab() {
    let p = pair();
    let now = p.t0;

    let fixture = |raw: &mut Value, include_closed_tab: bool| {
        raw["workspaces"] = json!([
            {
                "workspace_id": "w1", "active_tab_id": "w1:t1", "new_workspace_cwd": "/repo1",
                "number": 1, "label": "repo1", "custom_label": false, "branch": "main",
                "git_ahead_behind": null, "tokens": [], "worktree": null,
                "focused": true, "agent_status": "working"
            },
            {
                "workspace_id": "w2", "active_tab_id": "w2:t1", "new_workspace_cwd": "/repo2",
                "number": 2, "label": "repo2", "custom_label": false, "branch": "dev",
                "git_ahead_behind": null, "tokens": [], "worktree": null,
                "focused": false, "agent_status": "idle"
            }
        ]);
        let mut tabs = vec![
            json!({
                "tab_id": "w1:t1", "workspace_id": "w1", "number": 1, "label": "main",
                "custom_label": false, "zoomed": false, "focused": true, "agent_status": "working"
            }),
            json!({
                "tab_id": "w2:t1", "workspace_id": "w2", "number": 1, "label": "dev",
                "custom_label": false, "zoomed": false, "focused": false, "agent_status": "idle"
            }),
            json!({
                "tab_id": "w2:t2", "workspace_id": "w2", "number": 2, "label": "extra",
                "custom_label": false, "zoomed": false, "focused": false, "agent_status": "idle"
            }),
        ];
        if include_closed_tab {
            tabs.insert(1, json!({
                "tab_id": "w1:t2", "workspace_id": "w1", "number": 2, "label": "second",
                "custom_label": false, "zoomed": false, "focused": false, "agent_status": "blocked"
            }));
        }
        raw["tabs"] = json!(tabs);

        let mut panes = vec![
            json!({
                "pane_id": "w1:p1", "workspace_id": "w1", "tab_id": "w1:t1", "label": "shell1",
                "cwd": "/repo1", "foreground_cwd": "/repo1/src", "focused": true, "right_click_passthrough": false
            }),
            json!({
                "pane_id": "w1:p2", "workspace_id": "w1", "tab_id": "w1:t1", "label": "shell2",
                "cwd": "/repo1", "foreground_cwd": "/repo1", "focused": false, "right_click_passthrough": false
            }),
            json!({
                "pane_id": "w2:p1", "workspace_id": "w2", "tab_id": "w2:t1", "label": "shell3",
                "cwd": "/repo2", "foreground_cwd": "/repo2", "focused": false, "right_click_passthrough": false
            }),
            json!({
                "pane_id": "w2:p2", "workspace_id": "w2", "tab_id": "w2:t2", "label": "shell4",
                "cwd": "/repo2/docs", "foreground_cwd": "/repo2/docs", "focused": false, "right_click_passthrough": false
            }),
        ];
        if include_closed_tab {
            panes.push(json!({
                "pane_id": "w1:p3", "workspace_id": "w1", "tab_id": "w1:t2", "label": "shell-temp",
                "cwd": "/repo1/tmp", "foreground_cwd": "/repo1/tmp", "focused": false, "right_click_passthrough": false
            }));
        }
        raw["panes"] = json!(panes);

        let mut agents = vec![
            json!({
                "pane_id": "w1:p1", "workspace_id": "w1", "tab_id": "w1:t1", "name": "claude",
                "display_agent": "Claude", "agent": "claude", "title": "coding", "terminal_title": "Claude Coding",
                "terminal_title_stripped": "Claude Coding", "agent_status": "working",
                "state_change_seq": 1, "state_labels": [], "tokens": [], "focused": true
            }),
            json!({
                "pane_id": "w2:p1", "workspace_id": "w2", "tab_id": "w2:t1", "name": "codex",
                "display_agent": "Codex", "agent": "codex", "title": "testing", "terminal_title": "Codex Testing",
                "terminal_title_stripped": "Codex Testing", "agent_status": "idle",
                "state_change_seq": 2, "state_labels": [], "tokens": [], "focused": false
            }),
        ];
        if include_closed_tab {
            agents.push(json!({
                "pane_id": "w1:p3", "workspace_id": "w1", "tab_id": "w1:t2", "name": "researcher",
                "display_agent": "Researcher", "agent": "researcher", "title": "reading", "terminal_title": "Reading",
                "terminal_title_stripped": "Reading", "agent_status": "blocked",
                "state_change_seq": 3, "state_labels": [], "tokens": [], "focused": false
            }));
        }
        raw["agents"] = json!(agents);
        raw["focused_workspace_id"] = json!("w1");
        raw["focused_tab_id"] = json!("w1:t1");
        raw["focused_pane_id"] = json!("w1:p1");
    };

    // Apply snapshot with 2 workspaces x 2 tabs (4 tabs total)
    p.hub.apply_event(
        SSH_ID,
        p.ssh_token,
        snapshot_with("boot-ssh-1", |raw| fixture(raw, true)),
        now,
    );

    let snapshot_dto = dto(&p.hub, now);
    let ssh = host(&snapshot_dto, SSH_ID);

    // Verify tabs projection: 4 tabs
    let tabs = ssh["tabs"].as_array().expect("host tabs array");
    assert_eq!(tabs.len(), 4, "2 workspaces x 2 tabs = 4 tabs");

    // w1:t1 is focused and has 2 panes (w1:p1, w1:p2)
    let t1 = &tabs[0];
    assert_eq!(t1["tab_id"], "w1:t1");
    assert_eq!(t1["workspace_id"], "w1");
    assert_eq!(t1["number"], 1);
    assert_eq!(t1["label"], "main");
    assert_eq!(t1["focused"], true);
    assert_eq!(t1["pane_count"], 2);
    assert_eq!(t1["agent_status"], "working");

    // w1:t2 has 1 pane (w1:p3)
    let t2 = &tabs[1];
    assert_eq!(t2["tab_id"], "w1:t2");
    assert_eq!(t2["workspace_id"], "w1");
    assert_eq!(t2["number"], 2);
    assert_eq!(t2["label"], "second");
    assert_eq!(t2["focused"], false);
    assert_eq!(t2["pane_count"], 1);
    assert_eq!(t2["agent_status"], "blocked");

    // Verify panes projection:
    let panes = ssh["panes"].as_array().expect("host panes array");
    assert_eq!(panes.len(), 5);
    let p1 = panes.iter().find(|p| p["pane_id"] == "w1:p1").unwrap();
    assert_eq!(p1["workspace_id"], "w1");
    assert_eq!(p1["tab_id"], "w1:t1");
    assert_eq!(p1["focused"], true);
    assert_eq!(p1["agent"], "claude");
    assert_eq!(p1["title"], "coding");
    assert_eq!(p1["cwd"], "/repo1");
    assert_eq!(p1["foreground_cwd"], "/repo1/src");
    assert_eq!(p1["agent_status"], "working");

    // AC-039-03: install_latency receives only handshake duration
    assert!(p.hub.install_latency(SSH_ID, p.ssh_token, 1250));
    let snap_latency = dto(&p.hub, now);
    assert_eq!(host(&snap_latency, SSH_ID)["latency_ms"], 1250);

    // Event of tab closed: update snapshot without w1:t2
    p.hub.apply_event(
        SSH_ID,
        p.ssh_token,
        snapshot_with("boot-ssh-1", |raw| fixture(raw, false)),
        now,
    );

    let updated_dto = dto(&p.hub, now);
    let ssh_updated = host(&updated_dto, SSH_ID);
    let tabs_updated = ssh_updated["tabs"].as_array().expect("host tabs array");
    assert_eq!(
        tabs_updated.len(),
        3,
        "one tab was closed, now 3 tabs remain"
    );
    assert!(!tabs_updated.iter().any(|t| t["tab_id"] == "w1:t2"));

    let panes_updated = ssh_updated["panes"].as_array().expect("host panes array");
    assert_eq!(panes_updated.len(), 4, "pane of closed tab is removed");
    assert!(!panes_updated.iter().any(|p| p["pane_id"] == "w1:p3"));
}

/// Would catch: no server version in the host DTO (the status bar could only fabricate one), a
/// version installed by a superseded connection, or a version kept after the connection dropped.
#[test]
fn host_dto_projects_the_negotiated_server_version_of_the_installed_connection() {
    let p = pair();
    let now = p.t0;
    assert_eq!(
        host(&dto(&p.hub, now), SSH_ID)["server_version"],
        Value::Null
    );
    assert!(!p
        .hub
        .install_server_version(SSH_ID, p.ssh_token + 1000, "9.9.9".into()));
    assert_eq!(
        host(&dto(&p.hub, now), SSH_ID)["server_version"],
        Value::Null
    );
    assert!(p
        .hub
        .install_server_version(SSH_ID, p.ssh_token, "0.9.1-preview.3".into()));
    let snap = dto(&p.hub, now);
    assert_eq!(
        host(&snap, SSH_ID)["server_version"],
        json!("0.9.1-preview.3")
    );
    assert_eq!(host(&snap, LOCAL)["server_version"], Value::Null);
    p.hub.apply_event(SSH_ID, p.ssh_token, lost(), now);
    assert_eq!(
        host(&dto(&p.hub, now), SSH_ID)["server_version"],
        Value::Null
    );
}

/// AC-011-02: host DTO projects measured latency of the installed connection; refuses
/// latency installed by a superseded connection, or latency kept after the connection dropped.
#[test]
fn host_dto_projects_the_measured_latency_of_the_installed_connection() {
    let p = pair();
    let now = p.t0;
    assert_eq!(host(&dto(&p.hub, now), SSH_ID)["latency_ms"], Value::Null);
    assert!(!p.hub.install_latency(SSH_ID, p.ssh_token + 1000, 42));
    assert_eq!(host(&dto(&p.hub, now), SSH_ID)["latency_ms"], Value::Null);
    assert!(p.hub.install_latency(SSH_ID, p.ssh_token, 18));
    let snap = dto(&p.hub, now);
    assert_eq!(host(&snap, SSH_ID)["latency_ms"], json!(18));
    assert_eq!(host(&snap, LOCAL)["latency_ms"], Value::Null);
    p.hub.apply_event(SSH_ID, p.ssh_token, lost(), now);
    assert_eq!(host(&dto(&p.hub, now), SSH_ID)["latency_ms"], Value::Null);
}

// ---------------------------------------------------------------------------------------
// AC-003-01 — routing by endpoint with colliding pane ids; isolated loss
// ---------------------------------------------------------------------------------------

/// Would catch: routing by pane id alone (local receives the SSH bytes) or a lookup that
/// falls back to Local.
#[test]
fn input_to_ssh_pane_reaches_only_ssh_even_with_the_same_pane_id() {
    let p = pair();
    let ssh_target = p
        .hub
        .input_gate(SSH_ID, "w1:p1")
        .expect("ssh input enabled");
    let local_target = p
        .hub
        .input_gate(LOCAL, "w1:p1")
        .expect("local input enabled");
    assert_eq!(
        ssh_target.pane_id, local_target.pane_id,
        "premise: same pane id"
    );
    assert_ne!(ssh_target.boot_id, local_target.boot_id);

    p.hub
        .send_input(&ssh_target, text("echo remoto"))
        .expect("ssh input");

    let ssh = p.ssh_wire.lock().unwrap();
    assert_eq!(ssh.inputs, vec![("w1:p1".to_owned(), text("echo remoto"))]);
    assert!(p.local_wire.lock().unwrap().inputs.is_empty());
}

/// Negative contract: a target mixing SSH boot with the Local endpoint, or an unknown
/// endpoint, is refused and reaches no connection.
#[test]
fn mixed_or_unknown_targets_are_refused_without_falling_back_to_local() {
    let p = pair();
    let ssh_target = p.hub.input_gate(SSH_ID, "w1:p1").unwrap();

    // Local endpoint and session with the SSH boot: only the boot distinguishes it.
    let mut forged = ssh_target.clone();
    forged.endpoint = LOCAL.into();
    forged.session = "hd003-local-a".into();
    let error = p.hub.send_input(&forged, text("x")).unwrap_err();
    assert_eq!(error.code, "target_boot_stale");

    let mut unknown = ssh_target.clone();
    unknown.endpoint = "fedcba9876543210fedcba9876543210".into();
    let error = p.hub.send_input(&unknown, text("x")).unwrap_err();
    assert_eq!(error.code, "endpoint_unknown");
    assert_eq!(
        error.endpoint.as_deref(),
        Some("fedcba9876543210fedcba9876543210")
    );

    let mut other_session = ssh_target;
    other_session.session = "hd003-local-a".into();
    let error = p.hub.send_input(&other_session, text("x")).unwrap_err();
    assert_eq!(error.code, "target_session_mismatch");

    assert!(p.local_wire.lock().unwrap().inputs.is_empty());
    assert!(p.ssh_wire.lock().unwrap().inputs.is_empty());
}

/// AC-003-01 loss. Would catch: a global "disconnected" state that blocks Local, an SSH
/// pane that keeps input enabled on the cached surface, or queued input sent later.
#[test]
fn losing_ssh_keeps_local_input_and_shows_reconnecting_with_input_disabled() {
    let p = pair();
    let ssh_target = p.hub.input_gate(SSH_ID, "w1:p1").unwrap();
    let local_target = p.hub.input_gate(LOCAL, "w1:p1").unwrap();

    p.hub.apply_event(SSH_ID, p.ssh_token, lost(), p.t0);

    p.hub
        .send_input(&local_target, text("echo local"))
        .expect("local keeps accepting input");
    let refused = p
        .hub
        .send_input(&ssh_target, text("echo perdido"))
        .unwrap_err();
    assert_eq!(refused.code, "host_reconnecting");
    assert!(refused.retryable);
    assert_eq!(
        p.hub.input_gate(SSH_ID, "w1:p1").unwrap_err(),
        InputBlock::HostReconnecting
    );

    let snap = dto(&p.hub, p.t0);
    let ssh = host(&snap, SSH_ID);
    assert_eq!(ssh["phase"], "reconnecting");
    assert_eq!(ssh["phase_label"], "Reconnecting");
    assert_eq!(ssh["panes"][0]["input_enabled"], false);
    assert_eq!(ssh["panes"][0]["input_block"], "host_reconnecting");
    assert_eq!(ssh["cached"], true, "remote context kept as cache");
    assert_eq!(ssh["screen"][0], "remote$");
    let local = host(&snap, LOCAL);
    assert_eq!(local["phase"], "online");
    assert_eq!(local["phase_label"], "Online");
    assert_eq!(local["panes"][0]["input_enabled"], true);
    assert!(local["connection_error"].is_null());

    assert_eq!(
        p.local_wire.lock().unwrap().inputs,
        vec![("w1:p1".to_owned(), text("echo local"))]
    );
    assert!(p.ssh_wire.lock().unwrap().inputs.is_empty());
    assert_eq!(p.ssh_wire.lock().unwrap().detached, 1);
    assert_eq!(p.local_wire.lock().unwrap().detached, 0);
}

/// Would catch: the SSH failure (attention) written into the Local host or a global error.
#[test]
fn ssh_attention_failure_stays_on_the_ssh_host() {
    let hub = HostHub::new();
    hub.add_host(local_spec()).unwrap();
    hub.add_host(ssh_spec(true)).unwrap();
    let t0 = Instant::now();
    let wire = Arc::new(Mutex::new(Wire::default()));
    online(
        &hub,
        LOCAL,
        "hd003-local-a",
        "boot-local-7",
        &wire,
        &FakeApi::arc(),
        t0,
        "local$",
    );
    let ticket = hub.request_connect(SSH_ID, t0).unwrap().unwrap();
    hub.finish_connect(
        &ticket,
        Err(classify_ssh_failure(
            SSH_ID,
            Some(255),
            "Host key verification failed.\r\n",
        )),
        t0,
    );
    let snap = dto(&hub, t0);
    let ssh = host(&snap, SSH_ID);
    assert_eq!(ssh["phase"], "attention");
    assert_eq!(ssh["phase_label"], "Needs attention");
    assert_eq!(ssh["attention"], "host_key_unknown");
    assert_eq!(ssh["connection_error"]["code"], "ssh_host_key_unknown");
    let local = host(&snap, LOCAL);
    assert_eq!(local["phase"], "online");
    assert!(local["connection_error"].is_null());
    assert!(hub.input_gate(LOCAL, "w1:p1").is_ok());
}

/// Would catch: a duplicated endpoint id silently replacing the first host.
#[test]
fn duplicate_endpoint_ids_are_refused() {
    let hub = HostHub::new();
    hub.add_host(ssh_spec(true)).unwrap();
    let error = hub.add_host(ssh_spec(false)).unwrap_err();
    assert_eq!(error.code, "endpoint_duplicate");
    let error = hub.request_connect("nope", Instant::now()).unwrap_err();
    assert_eq!(error.code, "endpoint_unknown");
}

// ---------------------------------------------------------------------------------------
// AC-003-02 — reconnection with new boot/generation; no replay
// ---------------------------------------------------------------------------------------

fn reconnect(p: &Pair, boot: &str, now: Instant) -> u64 {
    let ticket = p
        .hub
        .due_connects(now)
        .into_iter()
        .find(|t| t.endpoint == SSH_ID)
        .unwrap();
    p.hub.finish_connect(
        &ticket,
        Ok(Connected {
            gateway: FakeGateway::boxed(SSH_ID, "hd003-remote-b", boot, &p.ssh_wire),
            api: p.ssh_api.clone(),
        }),
        now,
    );
    ticket.token
}

/// AC-003-02. Would catch: enabling input on the current snapshot alone, on a surface of
/// the previous boot, or on a surface that arrived before the new snapshot.
#[test]
fn current_snapshot_with_old_surface_keeps_input_blocked_until_reconciled() {
    let p = pair();
    let old_target = p.hub.input_gate(SSH_ID, "w1:p1").unwrap();
    assert_eq!(old_target.boot_id, "boot-remote-1");
    p.hub.apply_event(SSH_ID, p.ssh_token, lost(), p.t0);
    let t1 = p.t0 + backoff_delay(1);
    let token = reconnect(&p, "boot-remote-2", t1);

    // Connected, but nothing reconciled yet.
    assert_eq!(
        p.hub.input_gate(SSH_ID, "w1:p1").unwrap_err(),
        InputBlock::NoSnapshot
    );
    // Surface of the new boot before its snapshot: still blocked.
    p.hub
        .apply_event(SSH_ID, token, surface("boot-remote-2", 1, "novo$"), t1);
    assert_eq!(
        p.hub.input_gate(SSH_ID, "w1:p1").unwrap_err(),
        InputBlock::NoSnapshot
    );
    // A late full surface of the previous boot replaces it: snapshot current, surface old.
    p.hub
        .apply_event(SSH_ID, token, surface("boot-remote-1", 9, "velho$"), t1);
    p.hub
        .apply_event(SSH_ID, token, snapshot("boot-remote-2"), t1);
    assert_eq!(
        p.hub.input_gate(SSH_ID, "w1:p1").unwrap_err(),
        InputBlock::SurfaceFromPreviousBoot
    );
    let snap = dto(&p.hub, t1);
    assert_eq!(host(&snap, SSH_ID)["phase"], "online");
    assert_eq!(host(&snap, SSH_ID)["panes"][0]["input_enabled"], false);
    assert_eq!(
        host(&snap, SSH_ID)["panes"][0]["input_block"],
        "surface_from_previous_boot"
    );
    // A patch cannot reconcile a surface of another boot.
    p.hub.apply_event(
        SSH_ID,
        token,
        GatewayEvent::Patch(Box::new(PaneSurfacePatch {
            boot_id: "boot-remote-2".into(),
            projection_revision: 1,
            base_surface_revision: 9,
            surface_revision: 10,
            rows: vec![],
            panes: vec![pane_rect(12, 1)],
            cursor: None,
        })),
        t1,
    );
    assert!(p.hub.input_gate(SSH_ID, "w1:p1").is_err());
    // The old target is invalid for the new connection.
    let error = p.hub.send_input(&old_target, text("x")).unwrap_err();
    assert!(
        [
            "input_blocked",
            "target_generation_stale",
            "target_boot_stale"
        ]
        .contains(&error.code.as_str()),
        "{error:?}"
    );

    // Reconciled pair.
    p.hub
        .apply_event(SSH_ID, token, surface("boot-remote-2", 11, "novo$"), t1);
    let target = p.hub.input_gate(SSH_ID, "w1:p1").expect("reconciled");
    assert_eq!(target.boot_id, "boot-remote-2");
    assert!(target.connection_generation > old_target.connection_generation);
    // The old target stays refused even now.
    let error = p.hub.send_input(&old_target, text("x")).unwrap_err();
    assert_eq!(error.code, "target_generation_stale");
    p.hub.send_input(&target, text("echo novo")).unwrap();
    assert_eq!(
        p.ssh_wire.lock().unwrap().inputs,
        vec![("w1:p1".to_owned(), text("echo novo"))]
    );
}

/// Same generation, surface of the current boot but from the previous connection (cached
/// store): would catch gating only by boot id when the remote server did not reboot.
#[test]
fn reconnection_without_reboot_still_needs_a_surface_of_the_new_connection() {
    let p = pair();
    p.hub.apply_event(SSH_ID, p.ssh_token, lost(), p.t0);
    let t1 = p.t0 + backoff_delay(1);
    let token = reconnect(&p, "boot-remote-1", t1);
    p.hub
        .apply_event(SSH_ID, token, snapshot("boot-remote-1"), t1);
    assert_eq!(
        p.hub.input_gate(SSH_ID, "w1:p1").unwrap_err(),
        InputBlock::SurfaceFromPreviousConnection
    );
    p.hub
        .apply_event(SSH_ID, token, surface("boot-remote-1", 2, "remote$"), t1);
    assert!(p.hub.input_gate(SSH_ID, "w1:p1").is_ok());
}

/// AC-003-02 no replay. Would catch: re-sending the in-flight action after reconnecting, or
/// reporting it as failed/succeeded instead of unknown.
#[test]
fn action_pending_when_the_connection_drops_is_unknown_and_never_resent() {
    let p = pair();
    let release = p.ssh_api.script();
    let entered = p.ssh_api.on_enter();
    let hub = Arc::new(p.hub);
    let worker = {
        let hub = hub.clone();
        std::thread::spawn(move || {
            hub.run_action(
                SSH_ID,
                "workspace.create",
                json!({ "cwd": "/srv/remoto", "focus": false }),
            )
        })
    };
    assert_eq!(
        entered.recv_timeout(Duration::from_secs(5)).unwrap(),
        "workspace.create"
    );
    hub.apply_event(SSH_ID, p.ssh_token, lost(), p.t0);
    release
        .send(Err(RuntimeError::new(
            "connection_lost",
            "connection closed",
        )
        .retryable()))
        .unwrap();
    let error = worker.join().unwrap().unwrap_err();
    assert_eq!(error.code, "result_unknown");
    assert!(!error.retryable, "unknown mutations are not retryable");

    let t1 = p.t0 + backoff_delay(1);
    let pp = Pair {
        hub: Arc::try_unwrap(hub).ok().expect("single owner"),
        ..p
    };
    let token = reconnect(&pp, "boot-remote-2", t1);
    pp.hub
        .apply_event(SSH_ID, token, snapshot("boot-remote-2"), t1);
    pp.hub
        .apply_event(SSH_ID, token, surface("boot-remote-2", 1, "novo$"), t1);
    // Let any (wrong) replay run.
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(pp.ssh_api.methods(), vec!["workspace.create"]);
    let snap = dto(&pp.hub, t1);
    let actions = host(&snap, SSH_ID)["actions"].as_array().unwrap().clone();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0]["method"], "workspace.create");
    assert_eq!(actions[0]["outcome"], "unknown");
    assert!(pp.ssh_wire.lock().unwrap().inputs.is_empty());
}

/// Timeout of a mutation: result unknown, no retry, connection untouched.
#[test]
fn action_timeout_is_unknown_without_retry_and_keeps_the_connection_online() {
    let p = pair();
    let release = p.ssh_api.script();
    release
        .send(Err(RuntimeError::new("timeout", "timed out").retryable()))
        .unwrap();
    let error = p
        .hub
        .run_action(SSH_ID, "pane.send_text", json!({ "pane_id": "w1:p1" }))
        .unwrap_err();
    assert_eq!(error.code, "result_unknown");
    assert_eq!(p.ssh_api.methods(), vec!["pane.send_text"]);
    let snap = dto(&p.hub, p.t0);
    let ssh = host(&snap, SSH_ID);
    assert_eq!(ssh["phase"], "online");
    assert_eq!(ssh["actions"][0]["outcome"], "unknown");
    assert!(ssh["connection_error"].is_null());
}

/// TASK-003-05: action error is not a connection error. Would catch: a failing action that
/// flips the host to offline/attention or clears the other host's state.
#[test]
fn action_error_is_separate_from_connection_error() {
    let p = pair();
    let release = p.ssh_api.script();
    release
        .send(Err(RuntimeError::new(
            "workspace_not_found",
            "workspace inexistente",
        )))
        .unwrap();
    let error = p
        .hub
        .run_action(SSH_ID, "workspace.focus", json!({ "workspace_id": "w9" }))
        .unwrap_err();
    assert_eq!(error.code, "workspace_not_found");
    let snap = dto(&p.hub, p.t0);
    let ssh = host(&snap, SSH_ID);
    assert_eq!(ssh["phase"], "online");
    assert!(ssh["connection_error"].is_null());
    assert_eq!(ssh["action_error"]["code"], "workspace_not_found");
    assert_eq!(ssh["actions"][0]["outcome"], "failed");
    assert!(host(&snap, LOCAL)["action_error"].is_null());
    assert!(p.hub.input_gate(SSH_ID, "w1:p1").is_ok());

    let ok = p
        .hub
        .run_action(LOCAL, "workspace.list", json!({}))
        .unwrap();
    assert_eq!(ok["method"], "workspace.list");
    assert_eq!(p.local_api.methods(), vec!["workspace.list"]);
    assert_eq!(
        dto(&p.hub, p.t0)["hosts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|h| h["endpoint"] == LOCAL)
            .unwrap()["actions"][0]["outcome"],
        "succeeded"
    );
    // Actions are refused while the host is not online (never queued for later).
    p.hub.apply_event(SSH_ID, p.ssh_token, lost(), p.t0);
    let error = p
        .hub
        .run_action(SSH_ID, "workspace.list", json!({}))
        .unwrap_err();
    assert_eq!(error.code, "host_reconnecting");
    assert_eq!(p.ssh_api.methods(), vec!["workspace.focus"]);
}

/// TASK-003-05 independence. Would catch: a slow action holding the host lock so input and
/// frames of the same host (or of another host) wait for it.
#[test]
fn slow_action_does_not_block_input_or_frames() {
    let p = pair();
    let release = p.ssh_api.script();
    let entered = p.ssh_api.on_enter();
    let hub = Arc::new(p.hub);
    let worker = {
        let hub = hub.clone();
        std::thread::spawn(move || hub.run_action(SSH_ID, "agent.list", json!({})))
    };
    entered.recv_timeout(Duration::from_secs(5)).unwrap();

    let (done_tx, done_rx) = channel();
    {
        let hub = hub.clone();
        let token = p.ssh_token;
        std::thread::spawn(move || {
            let target = hub.input_gate(SSH_ID, "w1:p1").unwrap();
            hub.send_input(&target, text("ls")).unwrap();
            hub.apply_event(
                SSH_ID,
                token,
                surface("boot-remote-1", 2, "ls$"),
                Instant::now(),
            );
            let local = hub.input_gate(LOCAL, "w1:p1").unwrap();
            hub.send_input(&local, text("pwd")).unwrap();
            done_tx.send(()).unwrap();
        });
    }
    assert!(
        done_rx.recv_timeout(Duration::from_secs(2)).is_ok(),
        "input/frames waited for the slow action"
    );
    release.send(Ok(json!({ "agents": [] }))).unwrap();
    worker.join().unwrap().unwrap();
    assert_eq!(p.ssh_wire.lock().unwrap().inputs.len(), 1);
    assert_eq!(p.local_wire.lock().unwrap().inputs.len(), 1);
    assert_eq!(host(&dto(&hub, p.t0), SSH_ID)["screen"][0], "ls$");
}

/// Edge case "cancelamento preserva buffers e sessões": cancel detaches only this client,
/// keeps the cached screen, never schedules a retry, and a late connect result is dropped.
#[test]
fn cancel_preserves_cache_stops_retries_and_discards_late_results() {
    let p = pair();
    p.hub.apply_event(SSH_ID, p.ssh_token, lost(), p.t0);
    let t1 = p.t0 + backoff_delay(1);
    let ticket = p
        .hub
        .due_connects(t1)
        .into_iter()
        .find(|t| t.endpoint == SSH_ID)
        .unwrap();
    p.hub.cancel(SSH_ID).unwrap();
    let late_wire = Arc::new(Mutex::new(Wire::default()));
    p.hub.finish_connect(
        &ticket,
        Ok(Connected {
            gateway: FakeGateway::boxed(SSH_ID, "hd003-remote-b", "boot-remote-2", &late_wire),
            api: p.ssh_api.clone(),
        }),
        t1,
    );
    assert_eq!(
        late_wire.lock().unwrap().detached,
        1,
        "late gateway detached"
    );
    let far = t1 + Duration::from_secs(600);
    assert!(p.hub.due_connects(far).is_empty(), "cancel stops retries");
    let snap = dto(&p.hub, far);
    let ssh = host(&snap, SSH_ID);
    assert_eq!(ssh["phase"], "offline");
    assert_eq!(ssh["cancelled"], true);
    assert_eq!(ssh["screen"][0], "remote$", "buffer preserved");
    assert_eq!(host(&snap, LOCAL)["phase"], "online");
    // Explicit reconnection is still possible.
    assert!(p.hub.request_connect(SSH_ID, far).unwrap().is_some());
}

// ---------------------------------------------------------------------------------------
// TASK-003-04/05 — states, backoff, health with a controlled clock
// ---------------------------------------------------------------------------------------

/// Would catch: retry storms (no backoff), unbounded delays, or first attempt waiting.
#[test]
fn backoff_grows_and_is_capped() {
    let delays: Vec<u64> = (1..=8).map(|n| backoff_delay(n).as_secs()).collect();
    assert_eq!(delays, vec![1, 2, 4, 8, 16, 30, 30, 30]);
}

/// State machine with a controlled clock. Would catch: retrying before the deadline, a
/// failed first connection shown as Reconectando, or attention retried automatically.
#[test]
fn link_states_follow_connecting_online_reconnecting_attention_offline() {
    let t0 = Instant::now();
    let mut link = LinkMachine::new();
    assert_eq!(link.phase(), LinkPhase::Offline);
    assert!(
        !link.due(t0 + Duration::from_secs(3600)),
        "empty state starts nothing"
    );

    let first = link.request(t0).expect("explicit connect");
    assert_eq!(link.phase(), LinkPhase::Connecting);
    assert!(link.request(t0).is_none(), "no second attempt in flight");
    assert!(link.failed(
        first,
        &ConnectFailure::Transient(RuntimeError::new("ssh_unreachable", "x")),
        t0
    ));
    assert_eq!(link.phase(), LinkPhase::Offline);
    assert!(!link.due(t0 + Duration::from_millis(999)));
    assert!(link.due(t0 + Duration::from_secs(1)));

    let t1 = t0 + Duration::from_secs(1);
    let second = link.start_retry(t1).unwrap();
    assert!(link.succeeded(second, t1));
    assert_eq!(link.phase(), LinkPhase::Online);
    assert_eq!(link.attempt(), 0);

    let t2 = t1 + Duration::from_secs(30);
    link.lost(RuntimeError::new("connection_lost", "x"), t2);
    assert_eq!(link.phase(), LinkPhase::Reconnecting);
    assert!(!link.due(t2 + Duration::from_millis(999)));
    let r1 = link.start_retry(t2 + Duration::from_secs(1)).unwrap();
    assert_eq!(
        link.phase(),
        LinkPhase::Reconnecting,
        "retry keeps Reconectando"
    );
    link.failed(
        r1,
        &ConnectFailure::Transient(RuntimeError::new("ssh_unreachable", "x")),
        t2 + Duration::from_secs(1),
    );
    assert_eq!(link.attempt(), 2);
    assert!(!link.due(t2 + Duration::from_millis(2999)));
    assert!(link.due(t2 + Duration::from_secs(3)));

    let r2 = link.start_retry(t2 + Duration::from_secs(3)).unwrap();
    link.failed(
        r2,
        &ConnectFailure::Attention {
            reason: AttentionReason::ServerIncompatible,
            error: RuntimeError::new("ssh_server_incompatible", "x"),
        },
        t2 + Duration::from_secs(3),
    );
    assert_eq!(link.phase(), LinkPhase::Attention);
    assert!(
        !link.due(t2 + Duration::from_secs(3600)),
        "attention never retries alone"
    );
    assert!(link.start_retry(t2 + Duration::from_secs(3600)).is_none());
    // Stale token from before is ignored.
    assert!(!link.succeeded(r1, t2 + Duration::from_secs(4)));
    assert_eq!(link.phase(), LinkPhase::Attention);
    // Explicit retry after configuring.
    let explicit = link.request(t2 + Duration::from_secs(3601)).unwrap();
    assert_eq!(link.phase(), LinkPhase::Reconnecting);
    assert!(link.succeeded(explicit, t2 + Duration::from_secs(3601)));
    assert_eq!(link.phase(), LinkPhase::Online);
    // Spec 029: an explicit disconnect/cancel leaves no stale failure reason behind.
    link.cancel();
    assert_eq!(link.phase(), LinkPhase::Offline);
    assert_eq!(link.attention(), None);
    assert!(link.error().is_none());
    assert_eq!(
        [
            LinkPhase::Offline.label(),
            LinkPhase::Connecting.label(),
            LinkPhase::Online.label(),
            LinkPhase::Reconnecting.label(),
            LinkPhase::Attention.label(),
        ],
        [
            "Offline",
            "Connecting",
            "Online",
            "Reconnecting",
            "Needs attention"
        ]
    );
}

/// Health boundaries 5/10 s. Would catch: pinging early/late, expiring a probe before 10 s,
/// or heartbeats hiding a missing initial snapshot.
#[test]
fn health_ping_at_5s_and_expiry_at_10s_with_controlled_clock() {
    assert_eq!(HEALTH_INTERVAL, Duration::from_secs(5));
    assert_eq!(HEALTH_TIMEOUT, Duration::from_secs(10));
    let t0 = Instant::now();
    let ms = Duration::from_millis;
    let mut health = HealthMonitor::new(t0);
    health.ready();
    assert_eq!(health.action(t0 + ms(4999)), HealthAction::None);
    assert_eq!(health.action(t0 + ms(5000)), HealthAction::Ping);
    health.ping_sent(t0 + ms(5000));
    assert_eq!(health.action(t0 + ms(14_999)), HealthAction::None);
    assert_eq!(health.action(t0 + ms(15_000)), HealthAction::Expired);
    // Any message satisfies the outstanding probe.
    health.received(t0 + ms(14_000));
    assert_eq!(health.action(t0 + ms(15_000)), HealthAction::None);
    assert_eq!(health.action(t0 + ms(19_000)), HealthAction::Ping);

    let mut fresh = HealthMonitor::new(t0);
    fresh.ping_sent(t0 + ms(5000));
    fresh.received(t0 + ms(6000));
    assert_eq!(
        fresh.action(t0 + ms(9_999)),
        HealthAction::None,
        "still inside the initial snapshot budget"
    );
    assert_eq!(
        fresh.action(t0 + ms(10_000)),
        HealthAction::Expired,
        "heartbeats do not replace the initial snapshot"
    );
}

/// Boundary 60 s and metadata-only liveness. Would catch: an inactive host left silent until
/// the remote bridge idle timeout reaps it, or health pings dropped for metadata-only hosts.
#[test]
fn inactive_metadata_host_keeps_negotiated_ping_below_the_60s_bridge_idle_timeout() {
    assert_eq!(BRIDGE_IDLE_TIMEOUT, Duration::from_secs(60));
    let t0 = Instant::now();
    let ms = Duration::from_millis;
    let mut silent = HealthMonitor::new(t0);
    silent.ready();
    assert!(!silent.bridge_idle_expired(t0 + ms(59_999)));
    assert!(silent.bridge_idle_expired(t0 + ms(60_000)));

    // A quiet but responsive metadata-only host over 3 minutes, ticking every 250 ms.
    let mut health = HealthMonitor::new(t0);
    health.ready();
    let mut pings = Vec::new();
    let mut max_silence = Duration::ZERO;
    let mut now = t0;
    while now < t0 + Duration::from_secs(180) {
        now += ms(250);
        match health.action(now) {
            HealthAction::Ping => {
                health.ping_sent(now);
                pings.push(now - t0);
                // Pong arrives 40 ms later on the next tick.
                health.received(now + ms(40));
            }
            HealthAction::Expired => panic!("responsive host expired at {:?}", now - t0),
            HealthAction::None => {}
        }
        max_silence = max_silence.max(health.traffic_silence(now));
        assert!(!health.bridge_idle_expired(now));
    }
    assert_eq!(pings.first(), Some(&Duration::from_millis(5000)));
    // Ping at 5 s after the last reply, observed on 250 ms ticks: every 5.25 s.
    assert_eq!(pings.len(), 34, "{pings:?}");
    assert!(
        max_silence <= HEALTH_INTERVAL + Duration::from_millis(250),
        "{max_silence:?}"
    );
    assert!(max_silence < BRIDGE_IDLE_TIMEOUT);
}

/// Metadata-only hosts: connect without a surface and never repaint on frames. Would catch
/// an inactive host requesting a surface or bumping the revision (repaint) per frame.
#[test]
fn hidden_host_connects_metadata_only_and_ignores_frames() {
    let hub = HostHub::new();
    hub.add_host(ssh_spec(false)).unwrap();
    let t0 = Instant::now();
    let ticket = hub.request_connect(SSH_ID, t0).unwrap().unwrap();
    assert!(!ticket.surface_active, "inactive host is metadata-only");
    let wire = Arc::new(Mutex::new(Wire::default()));
    hub.finish_connect(
        &ticket,
        Ok(Connected {
            gateway: FakeGateway::boxed(SSH_ID, "hd003-remote-b", "boot-remote-1", &wire),
            api: FakeApi::arc(),
        }),
        t0,
    );
    hub.apply_event(SSH_ID, ticket.token, snapshot("boot-remote-1"), t0);
    let revision = hub.revision();
    for n in 0..15 {
        hub.apply_event(
            SSH_ID,
            ticket.token,
            surface("boot-remote-1", n + 1, "oculto"),
            t0,
        );
    }
    assert_eq!(hub.revision(), revision, "hidden frames caused repaints");
    assert_eq!(
        hub.input_gate(SSH_ID, "w1:p1").unwrap_err(),
        InputBlock::SurfaceHidden
    );
    let snap = dto(&hub, t0);
    assert_eq!(host(&snap, SSH_ID)["phase"], "online");
    assert_eq!(host(&snap, SSH_ID)["screen"], json!([]));
    assert_eq!(host(&snap, SSH_ID)["panes"][0]["pane_id"], "w1:p1");
    // Wait-based watch returns without a change.
    let started = Instant::now();
    assert_eq!(
        hub.wait_changed(revision, Duration::from_millis(100)),
        revision
    );
    assert!(started.elapsed() >= Duration::from_millis(90));
}

/// Hiding an online host renegotiates it metadata-only on a new generation (and showing it
/// again requests a surface). Would catch: a hidden host keeping its surface subscription or
/// its old generation/target staying valid.
#[test]
fn hiding_an_online_host_renegotiates_metadata_only_with_a_new_generation() {
    let p = pair();
    let old = p.hub.input_gate(SSH_ID, "w1:p1").unwrap();
    let t1 = p.t0 + Duration::from_secs(2);
    let ticket = match p.hub.set_visible(SSH_ID, false, t1).unwrap() {
        VisibilityOutcome::Renegotiate(ticket) => ticket,
        other => panic!("a host without surface_interest must renegotiate: {other:?}"),
    };
    assert!(!ticket.surface_active);
    assert_eq!(p.ssh_wire.lock().unwrap().detached, 1);
    assert_eq!(host(&dto(&p.hub, t1), SSH_ID)["phase"], "reconnecting");
    assert_eq!(host(&dto(&p.hub, t1), LOCAL)["phase"], "online");
    p.hub.finish_connect(
        &ticket,
        Ok(Connected {
            gateway: FakeGateway::boxed(SSH_ID, "hd003-remote-b", "boot-remote-1", &p.ssh_wire),
            api: p.ssh_api.clone(),
        }),
        t1,
    );
    p.hub
        .apply_event(SSH_ID, ticket.token, snapshot("boot-remote-1"), t1);
    assert_eq!(
        p.hub.input_gate(SSH_ID, "w1:p1").unwrap_err(),
        InputBlock::SurfaceHidden
    );
    assert_eq!(
        p.hub.send_input(&old, text("x")).unwrap_err().code,
        "input_blocked"
    );
    assert!(
        matches!(
            p.hub.set_visible(SSH_ID, false, t1).unwrap(),
            VisibilityOutcome::Unchanged
        ),
        "no change, no churn"
    );
    let shown = match p.hub.set_visible(SSH_ID, true, t1).unwrap() {
        VisibilityOutcome::Renegotiate(ticket) => ticket,
        other => panic!("showing must renegotiate here: {other:?}"),
    };
    assert!(shown.surface_active);
    assert!(p.ssh_wire.lock().unwrap().inputs.is_empty());
}

// ---------------------------------------------------------------------------------------
// AC-003-03 — attention, explicit configuration, no automatic key/install/restart/stop
// ---------------------------------------------------------------------------------------

/// Stderr fixtures from OpenSSH 10 / Herdr 0.9. Would catch: host key or MFA prompts treated
/// as transient (retry storm), unknown stderr treated as success, network loss as attention.
#[test]
fn ssh_failures_are_classified_into_attention_or_transient() {
    let attention = |exit, stderr: &str| match classify_ssh_failure(SSH_ID, exit, stderr) {
        ConnectFailure::Attention { reason, error } => {
            assert_eq!(error.endpoint.as_deref(), Some(SSH_ID));
            assert!(!error.retryable);
            Some(reason)
        }
        ConnectFailure::Transient(_) => None,
    };
    assert_eq!(
        attention(Some(255), "Host key verification failed.\r\n"),
        Some(AttentionReason::HostKeyUnknown)
    );
    assert_eq!(
        attention(
            Some(255),
            "No ED25519 host key is known for [127.0.0.1]:2222 and you have requested strict checking.\r\nHost key verification failed.\r\n"
        ),
        Some(AttentionReason::HostKeyUnknown)
    );
    assert_eq!(
        attention(
            Some(255),
            "@@@@@@@@@@@\r\n@    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @\r\nHost key verification failed.\r\n"
        ),
        Some(AttentionReason::HostKeyChanged)
    );
    assert_eq!(
        attention(
            Some(255),
            "user@dev-box: Permission denied (publickey,keyboard-interactive).\r\n"
        ),
        Some(AttentionReason::AuthenticationRequired)
    );
    assert_eq!(
        attention(
            Some(255),
            "Received disconnect from 10.0.0.2 port 22:2: Too many authentication failures\r\n"
        ),
        Some(AttentionReason::AuthenticationRequired)
    );
    assert_eq!(
        attention(Some(127), "bash: line 1: herdr: command not found\n"),
        Some(AttentionReason::HerdrMissing)
    );
    assert_eq!(
        attention(Some(1), "remote herdr server needs one final update before this bridge can attach; rerun `herdr --remote` from an interactive terminal to approve it\n"),
        Some(AttentionReason::ServerIncompatible)
    );
    for (exit, stderr, code) in [
        (
            Some(255),
            "ssh: connect to host 127.0.0.1 port 2222: Connection refused\r\n",
            "ssh_unreachable",
        ),
        (
            Some(255),
            "ssh: connect to host dev-box port 22: Connection timed out\r\n",
            "ssh_unreachable",
        ),
        (
            Some(255),
            "ssh: Could not resolve hostname dev-box: Name or service not known\r\n",
            "ssh_unreachable",
        ),
        (
            Some(255),
            "Connection closed by 127.0.0.1 port 2222\r\n",
            "ssh_unreachable",
        ),
        (
            Some(255),
            "kex_exchange_identification: something new\r\n",
            "ssh_failed",
        ),
        (Some(3), "unexpected output\n", "remote_command_failed"),
        (Some(0), "", "ssh_unexpected_exit"),
        (None, "", "ssh_terminated"),
    ] {
        match classify_ssh_failure(SSH_ID, exit, stderr) {
            ConnectFailure::Transient(error) => {
                assert_eq!(error.code, code, "{stderr}");
                assert!(error.retryable);
                assert!(!error.message.contains("127.0.0.1"), "stderr not echoed");
            }
            other => panic!("{stderr} classified as {other:?}"),
        }
    }
}

/// Server status from `herdr --session S status server --json`. Would catch: a stopped server
/// silently started by the bridge, or an incompatible/restart-needed server accepted.
#[test]
fn server_status_requires_a_running_compatible_generation_one_server() {
    let ok = r#"{"status":"running","running":true,"version":"0.9.0","protocol":22,"capabilities":{"live_handoff":true,"detached_server_daemon":true,"endpoint_protocol_generation":1,"surface_interest":true,"health_check":true},"compatible":true,"endpoint_compatible":true,"socket":"~/.config/herdr/sessions/x/herdr.sock","session":"x","restart_needed":false,"server_binary_stale":false}"#;
    assert!(check_server_status(SSH_ID, ok).is_ok());
    let reason = |raw: &str| match check_server_status(SSH_ID, raw) {
        Err(ConnectFailure::Attention { reason, .. }) => reason,
        other => panic!("{raw} -> {other:?}"),
    };
    assert_eq!(
        reason(r#"{"status":"not running","running":false}"#),
        AttentionReason::ServerNotRunning
    );
    assert_eq!(
        reason(&ok.replace(
            "\"endpoint_protocol_generation\":1",
            "\"endpoint_protocol_generation\":2"
        )),
        AttentionReason::ServerIncompatible
    );
    assert_eq!(
        reason(&ok.replace("\"restart_needed\":false", "\"restart_needed\":true")),
        AttentionReason::ServerIncompatible
    );
    assert_eq!(
        reason(&ok.replace(
            "\"endpoint_compatible\":true",
            "\"endpoint_compatible\":false"
        )),
        AttentionReason::ServerIncompatible
    );
    assert_eq!(
        reason(&ok.replace("\"health_check\":true", "\"health_check\":false")),
        AttentionReason::ServerIncompatible
    );
    assert_eq!(
        reason("error: unrecognized argument --json"),
        AttentionReason::ServerIncompatible
    );
}

/// Negotiated welcome for a remote host must include surface interest and health check.
#[test]
fn remote_negotiation_requires_surface_interest_and_health_check() {
    let identity = LiveIdentity {
        endpoint: SSH_ID.into(),
        session: "hd003-remote-b".into(),
        connection_generation: 1,
        boot_id: String::new(),
    };
    let negotiated = |caps: &[&str]| Negotiated {
        identity: identity.clone(),
        generation: 1,
        server_version: "0.9.0".into(),
        methods: vec!["workspace.list".into()],
        capabilities: caps.iter().map(|c| (*c).to_owned()).collect(),
    };
    assert!(check_negotiated(
        SSH_ID,
        &negotiated(&[
            SURFACE_INTEREST_CAPABILITY,
            PRESENTATION_EFFECTS_FENCE_CAPABILITY,
            HEALTH_CHECK_CAPABILITY
        ])
    )
    .is_ok());
    for caps in [
        &[SURFACE_INTEREST_CAPABILITY][..],
        &[HEALTH_CHECK_CAPABILITY][..],
        &[][..],
    ] {
        match check_negotiated(SSH_ID, &negotiated(caps)) {
            Err(ConnectFailure::Attention { reason, .. }) => {
                assert_eq!(reason, AttentionReason::ServerIncompatible)
            }
            other => panic!("{caps:?} -> {other:?}"),
        }
    }
    let mut wrong = negotiated(&[SURFACE_INTEREST_CAPABILITY, HEALTH_CHECK_CAPABILITY]);
    wrong.generation = 2;
    assert!(check_negotiated(SSH_ID, &wrong).is_err());
}

/// Fake process runner for the SSH connector.
#[derive(Default)]
struct FakeRunner {
    commands: Mutex<Vec<OpenSshCommand>>,
    inputs: Mutex<Vec<Option<Vec<u8>>>>,
    /// Commands handed to `spawn` (the bridge of a successful probe reaches this point).
    spawns: Mutex<Vec<OpenSshCommand>>,
    outputs: Mutex<VecDeque<io::Result<ProcessOutput>>>,
    /// Spec 058: when set, the first `output` records its command and then holds until the test
    /// releases it, so a probe can be observed while it is still in flight.
    gate: Mutex<Option<Receiver<()>>>,
}

impl SshRunner for FakeRunner {
    fn output(
        &self,
        command: &OpenSshCommand,
        _timeout: Duration,
        stdin: Option<&[u8]>,
    ) -> io::Result<ProcessOutput> {
        self.commands.lock().unwrap().push(command.clone());
        self.inputs.lock().unwrap().push(stdin.map(<[u8]>::to_vec));
        if let Some(gate) = self.gate.lock().unwrap().take() {
            let _ = gate.recv();
        }
        self.outputs
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Err(io::Error::other("unscripted")))
    }
    fn spawn(&self, command: &OpenSshCommand) -> io::Result<Box<dyn SshChild>> {
        self.commands.lock().unwrap().push(command.clone());
        self.inputs.lock().unwrap().push(None);
        self.spawns.lock().unwrap().push(command.clone());
        Err(io::Error::other("the fake runner never opens a bridge"))
    }
}

fn identity() -> SshIdentity {
    SshIdentity::new(
        ProfileId::parse(SSH_ID).unwrap(),
        "user@dev-box.example",
        Some(2222),
        "hd003-remote-b",
    )
    .unwrap()
}

fn remote_lines(runner: &FakeRunner) -> Vec<String> {
    let inputs = runner.inputs.lock().unwrap();
    runner
        .commands
        .lock()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(index, c)| {
            if c.args.last().is_some_and(|arg| arg == "/bin/sh -s") {
                String::from_utf8_lossy(inputs[index].as_deref().unwrap_or_default()).into_owned()
            } else {
                c.args.last().unwrap().clone()
            }
        })
        .collect()
}

/// AC-003-03 at the connector. Would catch: opening the client bridge (which can spawn a
/// daemon) after a failed probe, or running install/update/stop/restart commands.
#[test]
fn connector_stops_at_the_probe_on_attention_and_never_touches_the_server() {
    let options = ConnectOptions {
        geometry: SurfaceGeometry {
            cols: 80,
            rows: 24,
            cell_width_px: 9,
            cell_height_px: 18,
        },
        surface_active: true,
    };
    let cases: Vec<(Vec<io::Result<ProcessOutput>>, AttentionReason)> = vec![
        (
            vec![Ok(ProcessOutput {
                status: Some(255),
                stdout: vec![],
                stderr: "Host key verification failed.\r\n".into(),
            })],
            AttentionReason::HostKeyUnknown,
        ),
        (
            vec![Ok(ProcessOutput {
                status: Some(255),
                stdout: vec![],
                stderr: "Permission denied (keyboard-interactive).\r\n".into(),
            })],
            AttentionReason::AuthenticationRequired,
        ),
        (
            vec![Ok(ProcessOutput {
                status: Some(0),
                stdout: br#"{"status":"not running","running":false}"#.to_vec(),
                stderr: String::new(),
            })],
            AttentionReason::ServerNotRunning,
        ),
        (
            // Spec 029: the probe says the server is incompatible, so the desktop checks the
            // candidate binaries; the PATH client is current, so it is chosen and the server
            // probe is repeated (same verdict).
            vec![
                Ok(ProcessOutput {
                    status: Some(0),
                    stdout: br#"{"running":true,"capabilities":{"endpoint_protocol_generation":2,"surface_interest":true,"health_check":true},"endpoint_compatible":false,"restart_needed":true}"#.to_vec(),
                    stderr: String::new(),
                }),
                Ok(ProcessOutput {
                    status: Some(0),
                    stdout: b"/usr/bin/herdr\n".to_vec(),
                    stderr: String::new(),
                }),
                Ok(ProcessOutput {
                    status: Some(0),
                    stdout: b"$HOME/.local/bin/herdr\n".to_vec(),
                    stderr: String::new(),
                }),
                Ok(ProcessOutput {
                    status: Some(0),
                    stdout: CLIENT_OK.as_bytes().to_vec(),
                    stderr: String::new(),
                }),
                Ok(ProcessOutput {
                    status: Some(0),
                    stdout: br#"{"running":true,"capabilities":{"endpoint_protocol_generation":2,"surface_interest":true,"health_check":true},"endpoint_compatible":false,"restart_needed":true}"#.to_vec(),
                    stderr: String::new(),
                }),
            ],
            AttentionReason::ServerIncompatible,
        ),
        (
            vec![Err(io::Error::new(io::ErrorKind::NotFound, "ssh"))],
            AttentionReason::SshUnavailable,
        ),
    ];
    for (outputs, expected) in cases {
        let runner = Arc::new(FakeRunner::default());
        for output in outputs {
            runner.outputs.lock().unwrap().push_back(output);
        }
        let connector = SshConnector::new(identity(), None, runner.clone());
        match connector.connect(options.clone()) {
            Err(ConnectFailure::Attention { reason, .. }) => assert_eq!(reason, expected),
            Err(other) => panic!("{expected:?} -> {other:?}"),
            Ok(_) => panic!("{expected:?} connected"),
        }
        let lines = remote_lines(&runner);
        if expected == AttentionReason::ServerIncompatible {
            assert_eq!(
                lines.last().unwrap(),
                "exec '/usr/bin/herdr' --session 'hd003-remote-b' status server --json",
                "the discovered client repeated the server probe"
            );
            assert!(lines.contains(&"command -v herdr".to_owned()), "{lines:?}");
        } else {
            assert_eq!(
                lines,
                vec!["exec herdr --session 'hd003-remote-b' status server --json".to_owned()],
                "only the read-only probe ran for {expected:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// Spec 029 (AC-029-01) — remote binary discovery like the TUI: PATH, known paths, override,
// first candidate that serves the endpoint requirement; missing only without any candidate.
// ---------------------------------------------------------------------------------------

const CLIENT_OK: &str = r#"{"version":"0.9.1","protocol":22,"endpoint_protocol_generation":1,"endpoint_capabilities":["surface_interest","presentation_effects_fence","health_check"],"remote_host_bridge":true,"remote_bridge_idle_timeout":false}"#;
const CLIENT_OLD: &str = r#"{"version":"0.8.0","protocol":21,"endpoint_protocol_generation":1,"endpoint_capabilities":["surface_interest"],"remote_host_bridge":false,"remote_bridge_idle_timeout":false}"#;
const SERVER_OK: &str = r#"{"status":"running","running":true,"version":"0.9.1","protocol":22,"capabilities":{"live_handoff":true,"detached_server_daemon":true,"endpoint_protocol_generation":1,"surface_interest":true,"health_check":true},"compatible":true,"endpoint_compatible":true,"socket":"~/.config/herdr/sessions/x/herdr.sock","session":"x","restart_needed":false,"server_binary_stale":false}"#;
const HERDR_MISSING: &str = "zsh:1: command not found: herdr\n";

fn script(runner: &FakeRunner, output: ProcessOutput) {
    runner.outputs.lock().unwrap().push_back(Ok(output));
}

fn stdout(status: i32, text: &str) -> ProcessOutput {
    ProcessOutput {
        status: Some(status),
        stdout: text.as_bytes().to_vec(),
        stderr: String::new(),
    }
}

/// Would catch: a candidate order different from the TUI, a duplicated candidate, a relative or
/// mise-shim path accepted, or a discovery script without `$HOME/.local/bin/herdr` / homebrew /
/// mise / nix paths.
#[test]
fn remote_binary_candidates_match_the_tui_order_and_skip_shims() {
    let script = known_binary_candidate_script();
    let script_command = build_ssh_script(&identity(), None, &script);
    assert_eq!(
        script_command.args.last().map(String::as_str),
        Some("/bin/sh -s")
    );
    for needle in [
        "$home/.local/bin/herdr",
        "/opt/homebrew/bin/herdr",
        "/usr/local/bin/herdr",
        "/home/linuxbrew/.linuxbrew/bin/herdr",
        "mise/installs/herdr/",
        "mise/installs/github-ogulcancelik-herdr/",
        "$home/.nix-profile/bin/herdr",
        "/etc/profiles/per-user/$user/bin/herdr",
        "/nix/var/nix/profiles/default/bin/herdr",
        "/run/current-system/sw/bin/herdr",
    ] {
        assert!(script.contains(needle), "missing {needle} in the script");
    }
    assert!(
        script.contains("Darwin") && script.contains("Linux"),
        "the platform paths are chosen by uname inside the discovery script: {script}"
    );
    assert!(
        !script.contains("herdr install")
            && !script.contains("herdr update")
            && !script.contains("/mise/shims/"),
        "no install/update and no mise shims: {script}"
    );

    let candidates = discovery_candidates(
        "/usr/bin/herdr\n/Users/ec2-user/.local/bin/herdr\n",
        "/Users/ec2-user/.local/bin/herdr\n/mise/shims/herdr\nrelative/herdr\n\n",
    );
    assert_eq!(
        candidates,
        vec!["/usr/bin/herdr", "/Users/ec2-user/.local/bin/herdr"],
        "PATH first, known paths after, deduped, slash-prefixed only"
    );
    assert!(discovery_candidates("", "").is_empty());
}

/// Would catch: a client status accepted without generation 1, without any of the three
/// capabilities, or a version not read from the JSON.
#[test]
fn remote_client_status_requires_generation_one_and_all_capabilities() {
    let status = parse_client_status(CLIENT_OK).expect("parsable");
    assert!(status.supports_endpoint_requirement());
    assert_eq!(
        RemoteBinary {
            path: "/usr/bin/herdr".into(),
            version: status.version.clone(),
        }
        .version
        .as_deref(),
        Some("0.9.1")
    );
    for broken in [
        CLIENT_OLD,
        &CLIENT_OK.replace(
            "\"endpoint_protocol_generation\":1",
            "\"endpoint_protocol_generation\":2",
        ),
        &CLIENT_OK.replace("\"health_check\"", "\"other\""),
        &CLIENT_OK.replace("\"surface_interest\"", "\"other\""),
        &CLIENT_OK.replace("\"presentation_effects_fence\"", "\"other\""),
    ] {
        let parsed = parse_client_status(broken).expect("parsable");
        assert!(!parsed.supports_endpoint_requirement(), "accepted {broken}");
    }
    assert!(parse_client_status("not json").is_none());
}

/// AC-029-01: PATH empty, `~/.local/bin/herdr` present and serving the endpoint requirement ->
/// the desktop connects with that path; no candidate at all -> `remote_herdr_missing`.
#[test]
fn connector_discovers_the_binary_by_candidates_and_reports_missing_only_without_any() {
    let options = ConnectOptions {
        geometry: SurfaceGeometry {
            cols: 80,
            rows: 24,
            cell_width_px: 9,
            cell_height_px: 18,
        },
        surface_active: true,
    };
    // PATH empty + `~/.local/bin/herdr` present and compatible: connect proceeds to the bridge
    // with the discovered path (the fake runner never opens the bridge, so the spawn is the proof).
    let runner = Arc::new(FakeRunner::default());
    script(
        &runner,
        ProcessOutput {
            status: Some(127),
            stdout: vec![],
            stderr: HERDR_MISSING.into(),
        },
    );
    script(&runner, stdout(1, ""));
    script(
        &runner,
        ProcessOutput {
            status: Some(1),
            stdout: b"/Users/ec2-user/.local/bin/herdr\n".to_vec(),
            stderr: "zsh:22: no matches found: /Users/ec2-user/.local/share/mise/installs/herdr/*/bin/herdr\n".into(),
        },
    );
    script(&runner, stdout(0, CLIENT_OK));
    script(&runner, stdout(0, SERVER_OK));
    let connector = SshConnector::new(identity(), None, runner.clone());
    let error = match connector.connect(options.clone()) {
        Err(failure) => failure,
        Ok(_) => panic!("the fake runner never opens a bridge, but the connect succeeded"),
    };
    assert_eq!(error.error().code, "ssh_failed", "{error:?}");
    assert_eq!(
        remote_lines(&runner),
        vec![
            "exec herdr --session 'hd003-remote-b' status server --json".to_owned(),
            "command -v herdr".to_owned(),
            known_binary_candidate_script(),
            "test -x '/Users/ec2-user/.local/bin/herdr' && '/Users/ec2-user/.local/bin/herdr' status client --json".to_owned(),
            "exec '/Users/ec2-user/.local/bin/herdr' --session 'hd003-remote-b' status server --json".to_owned(),
            "exec '/Users/ec2-user/.local/bin/herdr' --session 'hd003-remote-b' remote-client-bridge".to_owned(),
        ],
        "same candidates and order as the TUI, first that serves chosen"
    );
    let spawns = runner.spawns.lock().unwrap();
    assert_eq!(spawns.len(), 1);
    assert_eq!(
        spawns[0].args.last().unwrap(),
        "exec '/Users/ec2-user/.local/bin/herdr' --session 'hd003-remote-b' remote-client-bridge"
    );

    // No candidate exists: only the missing classification, and no client-status probe.
    let runner = Arc::new(FakeRunner::default());
    script(
        &runner,
        ProcessOutput {
            status: Some(127),
            stdout: vec![],
            stderr: HERDR_MISSING.into(),
        },
    );
    script(&runner, stdout(1, ""));
    script(&runner, stdout(0, ""));
    let connector = SshConnector::new(identity(), None, runner.clone());
    match connector.connect(options.clone()) {
        Err(ConnectFailure::Attention { reason, error }) => {
            assert_eq!(reason, AttentionReason::HerdrMissing);
            assert_eq!(error.code, "remote_herdr_missing");
            assert!(!error.retryable);
        }
        Err(other) => panic!("no candidate -> {other:?}"),
        Ok(_) => panic!("no candidate connected"),
    }
    assert_eq!(
        remote_lines(&runner),
        vec![
            "exec herdr --session 'hd003-remote-b' status server --json".to_owned(),
            "command -v herdr".to_owned(),
            known_binary_candidate_script(),
        ],
        "no candidate: nothing else ran"
    );
    assert!(runner.spawns.lock().unwrap().is_empty());
}

/// AC-029-01 edge case and "first that serves": an old candidate is skipped with its version
/// remembered, the next compatible candidate is used; without any compatible one the failure is
/// `Herdr desatualizado no host` (with the version), never `não encontrado`.
#[test]
fn connector_skips_an_outdated_candidate_and_reports_its_version_when_none_serves() {
    let options = ConnectOptions {
        geometry: SurfaceGeometry {
            cols: 80,
            rows: 24,
            cell_width_px: 9,
            cell_height_px: 18,
        },
        surface_active: true,
    };
    // Two candidates: the first is old, the second serves.
    let runner = Arc::new(FakeRunner::default());
    script(
        &runner,
        ProcessOutput {
            status: Some(127),
            stdout: vec![],
            stderr: HERDR_MISSING.into(),
        },
    );
    script(&runner, stdout(1, ""));
    script(&runner, stdout(0, "/opt/herdr-old\n/opt/herdr-new\n"));
    script(&runner, stdout(0, CLIENT_OLD));
    script(&runner, stdout(0, CLIENT_OK));
    script(&runner, stdout(0, SERVER_OK));
    let connector = SshConnector::new(identity(), None, runner.clone());
    assert!(connector.connect(options.clone()).is_err());
    let spawns = runner.spawns.lock().unwrap();
    assert_eq!(
        spawns[0].args.last().unwrap(),
        "exec '/opt/herdr-new' --session 'hd003-remote-b' remote-client-bridge",
        "the first candidate that serves is used"
    );

    // Only an old candidate: outdated with its version, not missing.
    let runner = Arc::new(FakeRunner::default());
    script(
        &runner,
        ProcessOutput {
            status: Some(127),
            stdout: vec![],
            stderr: HERDR_MISSING.into(),
        },
    );
    script(&runner, stdout(1, ""));
    script(&runner, stdout(0, "/opt/herdr-old\n"));
    script(&runner, stdout(0, CLIENT_OLD));
    let connector = SshConnector::new(identity(), None, runner.clone());
    match connector.connect(options.clone()) {
        Err(ConnectFailure::Attention { reason, error }) => {
            assert_eq!(reason, AttentionReason::HerdrOutdated);
            assert_eq!(error.code, "remote_herdr_outdated");
            assert!(
                error.message.contains("0.8.0"),
                "found version travels: {error:?}"
            );
            assert!(!error.retryable);
        }
        Err(other) => panic!("outdated -> {other:?}"),
        Ok(_) => panic!("outdated connected"),
    }
    assert_eq!(
        AttentionReason::HerdrOutdated.message(),
        "the host's Herdr is outdated and does not serve endpoint generation 1"
    );
    assert!(
        AttentionReason::HerdrOutdated
            .guidance()
            .contains("herdr --remote"),
        "guidance is explicit configuration"
    );

    // A PATH binary that still serves is chosen without using any known-path candidate.
    let runner = Arc::new(FakeRunner::default());
    script(
        &runner,
        ProcessOutput {
            status: Some(127),
            stdout: vec![],
            stderr: HERDR_MISSING.into(),
        },
    );
    script(&runner, stdout(0, "/usr/bin/herdr\n"));
    script(&runner, stdout(0, ""));
    script(&runner, stdout(0, CLIENT_OK));
    script(&runner, stdout(0, SERVER_OK));
    let connector = SshConnector::new(identity(), None, runner.clone());
    assert!(connector.connect(options).is_err());
    assert_eq!(
        *runner.spawns.lock().unwrap()[0].args.last().unwrap(),
        "exec '/usr/bin/herdr' --session 'hd003-remote-b' remote-client-bridge"
    );
}

/// TASK-029-02: a configured override is used without the PATH probe, and an invalid one is
/// ignored (it never becomes a remote command).
#[test]
fn configured_remote_binary_override_is_used_first_and_validated() {
    let env = |key: &str| (key == "HERDR_REMOTE_BINARY").then(|| "/opt/herdr".to_owned());
    assert_eq!(
        configured_remote_binary(&env).as_deref(),
        Some("/opt/herdr")
    );
    for bad in [
        "",
        "herdr",
        " /opt/herdr ",
        "/opt/her\ndr",
        "-oProxyCommand=x",
    ] {
        let env = |key: &str| (key == "HERDR_REMOTE_BINARY").then(|| bad.to_owned());
        assert_eq!(configured_remote_binary(&env), None, "{bad:?}");
    }
    let empty = |_: &str| None;
    assert_eq!(configured_remote_binary(&empty), None);

    let options = ConnectOptions {
        geometry: SurfaceGeometry {
            cols: 80,
            rows: 24,
            cell_width_px: 9,
            cell_height_px: 18,
        },
        surface_active: true,
    };
    let runner = Arc::new(FakeRunner::default());
    script(&runner, stdout(0, CLIENT_OK));
    script(&runner, stdout(0, SERVER_OK));
    let connector = SshConnector::new(identity(), None, runner.clone())
        .with_remote_binary_override(Some("/opt/herdr".to_owned()));
    assert!(connector.connect(options).is_err());
    let lines = remote_lines(&runner);
    assert_eq!(
        lines,
        vec![
            "test -x '/opt/herdr' && '/opt/herdr' status client --json".to_owned(),
            "exec '/opt/herdr' --session 'hd003-remote-b' status server --json".to_owned(),
            "exec '/opt/herdr' --session 'hd003-remote-b' remote-client-bridge".to_owned(),
        ],
        "the override replaces the candidates, without the PATH probe"
    );
    assert_eq!(
        connector.remote_binary().map(|b| b.path),
        Some("/opt/herdr".to_owned())
    );
}

/// AC-029-01 tooltip evidence: the installed connection carries the chosen binary path and the
/// version of the client that served; a superseded connection cannot install it and a loss or an
/// explicit disconnect clears it.
#[test]
fn host_dto_projects_the_discovered_remote_binary_of_the_installed_connection() {
    let p = pair();
    let now = p.t0;
    assert_eq!(host(&dto(&p.hub, now), SSH_ID)["herdr_binary"], Value::Null);
    assert!(!p.hub.install_remote_binary(
        SSH_ID,
        p.ssh_token + 1000,
        RemoteBinary {
            path: "/opt/herdr".into(),
            version: Some("9.9.9".into()),
        }
    ));
    assert_eq!(host(&dto(&p.hub, now), SSH_ID)["herdr_binary"], Value::Null);
    assert!(p.hub.install_remote_binary(
        SSH_ID,
        p.ssh_token,
        RemoteBinary {
            path: "/Users/ec2-user/.local/bin/herdr".into(),
            version: Some("0.9.1".into()),
        }
    ));
    let snap = dto(&p.hub, now);
    assert_eq!(
        host(&snap, SSH_ID)["herdr_binary"],
        json!({ "path": "/Users/ec2-user/.local/bin/herdr", "version": "0.9.1" })
    );
    assert_eq!(host(&snap, LOCAL)["herdr_binary"], Value::Null);
    p.hub.apply_event(SSH_ID, p.ssh_token, lost(), now);
    assert_eq!(host(&dto(&p.hub, now), SSH_ID)["herdr_binary"], Value::Null);
}

/// AC-029-03: disconnect closes only the desktop connection (the fake gateway is detached, the
/// remote processes are not touched), the host stays offline without retries, its workspaces
/// leave the tree, and the cached screen is kept.
#[test]
fn disconnect_detaches_only_the_desktop_connection_and_clears_the_workspaces() {
    let p = pair();
    let now = p.t0;
    let snap = dto(&p.hub, now);
    assert!(!host(&snap, SSH_ID)["workspaces"]
        .as_array()
        .unwrap()
        .is_empty());
    p.hub.disconnect(SSH_ID).unwrap();
    assert_eq!(p.ssh_wire.lock().unwrap().detached, 1);
    assert_eq!(p.local_wire.lock().unwrap().detached, 0);

    let after = dto(&p.hub, now);
    let ssh = host(&after, SSH_ID);
    assert_eq!(ssh["phase"], "offline");
    assert_eq!(ssh["cancelled"], true);
    assert!(ssh["connection_error"].is_null(), "no stale failure reason");
    assert!(ssh["attention"].is_null());
    assert_eq!(ssh["workspaces"], json!([]), "workspaces leave the tree");
    assert_eq!(ssh["panes"], json!([]));
    assert_eq!(ssh["screen"][0], "remote$", "cached screen preserved");
    assert_eq!(host(&after, LOCAL)["phase"], "online");

    let far = now + Duration::from_secs(600);
    assert!(
        p.hub.due_connects(far).is_empty(),
        "disconnect cancels automatic retries"
    );
    // Reconnecting is explicit (the menu's Reconectar) and opens a new generation.
    let ticket = p.hub.request_connect(SSH_ID, far).unwrap().unwrap();
    p.hub.finish_connect(
        &ticket,
        Ok(Connected {
            gateway: FakeGateway::boxed(SSH_ID, "hd003-remote-b", "boot-remote-2", &p.ssh_wire),
            api: p.ssh_api.clone(),
        }),
        far,
    );
    assert_eq!(host(&dto(&p.hub, far), SSH_ID)["phase"], "online");
}

/// AC-029-03: removing the profile drops the host from the hub; a late result of an attempt that
/// was in flight is detached and never installed.
#[test]
fn removing_a_host_drops_it_and_detaches_a_late_attempt() {
    let hub = HostHub::new();
    hub.add_host(local_spec()).unwrap();
    hub.add_host(ssh_spec(true)).unwrap();
    let t0 = Instant::now();
    let ticket = hub.request_connect(SSH_ID, t0).unwrap().unwrap();
    hub.remove_host(SSH_ID).unwrap();
    let snap = serde_json::to_value(hub.snapshot(t0)).unwrap();
    assert!(
        snap["hosts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|h| h["endpoint"] != SSH_ID),
        "{snap}"
    );
    assert_eq!(
        hub.request_connect(SSH_ID, t0).unwrap_err().code,
        "endpoint_unknown"
    );
    let wire = Arc::new(Mutex::new(Wire::default()));
    hub.finish_connect(
        &ticket,
        Ok(Connected {
            gateway: FakeGateway::boxed(SSH_ID, "hd003-remote-b", "boot-remote-2", &wire),
            api: FakeApi::arc(),
        }),
        t0,
    );
    assert_eq!(wire.lock().unwrap().detached, 1, "late gateway detached");
}

/// SSH API lane over `remote-api-bridge`. Would catch: an old remote without the bridge
/// reported as a connection loss (retry storm / unknown result), a response with another id
/// accepted, or the request not written as one JSON line on the API command.
#[test]
fn ssh_api_lane_parses_results_and_reports_an_unsupported_remote_as_an_action_error() {
    let runner = Arc::new(FakeRunner::default());
    let connector = SshConnector::new(identity(), None, runner.clone());
    let lane = connector.api_lane();

    runner.outputs.lock().unwrap().push_back(Ok(ProcessOutput {
        status: Some(2),
        stdout: vec![],
        stderr: "unknown command: remote-api-bridge\nrun 'herdr --help' for usage\n".into(),
    }));
    let error = lane.request("workspace.list", json!({})).unwrap_err();
    assert_eq!(error.code, "remote_api_unsupported");
    assert_eq!(error.endpoint.as_deref(), Some(SSH_ID));
    assert!(!error.retryable);
    assert_eq!(
        remote_lines(&runner),
        vec!["exec herdr --session 'hd003-remote-b' remote-api-bridge".to_owned()]
    );
    // Spec 035 (AC-035-03): the missing subcommand is a capability of this host, not a per-call
    // failure. A second request is refused without spawning another doomed `remote-api-bridge`
    // (the measured `[conn] … api … exit 2` after every connect).
    let error = lane.request("agent.list", json!({})).unwrap_err();
    assert_eq!(error.code, "remote_api_unsupported");
    assert_eq!(
        remote_lines(&runner),
        vec!["exec herdr --session 'hd003-remote-b' remote-api-bridge".to_owned()],
        "no retry in a loop: one process for the whole connection"
    );

    // Echo the request id back: the fake reads it from the stdin the lane wrote.
    struct EchoRunner(Mutex<Vec<String>>);
    impl SshRunner for EchoRunner {
        fn output(
            &self,
            _command: &OpenSshCommand,
            _timeout: Duration,
            stdin: Option<&[u8]>,
        ) -> io::Result<ProcessOutput> {
            let line = String::from_utf8(stdin.unwrap().to_vec()).unwrap();
            assert!(line.ends_with('\n') && line.matches('\n').count() == 1);
            let request: Value = serde_json::from_str(line.trim()).unwrap();
            self.0
                .lock()
                .unwrap()
                .push(request["method"].as_str().unwrap().to_owned());
            let body = match request["method"].as_str().unwrap() {
                "workspace.list" => {
                    json!({ "id": request["id"], "result": { "workspaces": [{ "workspace_id": "w1" }] } })
                }
                "workspace.focus" => {
                    json!({ "id": request["id"], "error": { "code": "workspace_not_found", "message": "no" } })
                }
                _ => json!({ "id": "other", "result": {} }),
            };
            Ok(ProcessOutput {
                status: Some(0),
                stdout: format!("{body}\n").into_bytes(),
                stderr: String::new(),
            })
        }
        fn spawn(&self, _command: &OpenSshCommand) -> io::Result<Box<dyn SshChild>> {
            unreachable!()
        }
    }
    let echo = Arc::new(EchoRunner(Mutex::new(vec![])));
    let lane = SshConnector::new(identity(), None, echo.clone()).api_lane();
    assert_eq!(
        lane.request("workspace.list", json!({})).unwrap()["workspaces"][0]["workspace_id"],
        "w1"
    );
    assert_eq!(
        lane.request("workspace.focus", json!({})).unwrap_err().code,
        "workspace_not_found"
    );
    assert_eq!(
        lane.request("pane.list", json!({})).unwrap_err().code,
        "response_id_mismatch"
    );
    // A binary that never answered "unknown command" (here: a timeout on its first request)
    // keeps the per-call classification: the capability stays unknown, not disabled.
    let timed = Arc::new(FakeRunner::default());
    let connector = SshConnector::new(identity(), None, timed.clone());
    timed
        .outputs
        .lock()
        .unwrap()
        .push_back(Err(io::Error::new(io::ErrorKind::TimedOut, "t")));
    let lane = connector.api_lane();
    assert_eq!(
        lane.request("workspace.list", json!({})).unwrap_err().code,
        "timeout"
    );
    let runner = Arc::new(FakeRunner::default());
    let connector = SshConnector::new(identity(), None, runner.clone());
    runner
        .outputs
        .lock()
        .unwrap()
        .push_back(Err(io::Error::new(io::ErrorKind::TimedOut, "t")));
    let error = connector
        .api_lane()
        .request("workspace.list", json!({}))
        .unwrap_err();
    assert_eq!(error.message, "no answer while reading workspaces");
}

/// OpenSSH command seam consumed by 006. Would catch: accept-new/no host key policy, prompts
/// enabled, target injected as an option, session not passed to the remote side, or the
/// isolated test config leaking user files.
#[test]
fn openssh_commands_are_noninteractive_strict_and_carry_the_session() {
    let id = identity();
    let ssh = build_ssh(&id, None, RemoteHerdrCommand::ClientBridge);
    assert_eq!(ssh.program, "ssh");
    let args = ssh.args.join(" ");
    for required in [
        "-o BatchMode=yes",
        "-o NumberOfPasswordPrompts=0",
        "-o StrictHostKeyChecking=yes",
        "-o ConnectTimeout=10",
        "-o ConnectionAttempts=1",
        "-T",
        "-p 2222",
    ] {
        assert!(args.contains(required), "missing {required}: {args}");
    }
    for forbidden in [
        "accept-new",
        "StrictHostKeyChecking=no",
        "UserKnownHostsFile",
        "-F",
    ] {
        assert!(!args.contains(forbidden), "{forbidden} in {args}");
    }
    let n = ssh.args.len();
    assert_eq!(ssh.args[n - 3], "--");
    assert_eq!(ssh.args[n - 2], "user@dev-box.example");
    assert_eq!(
        ssh.args[n - 1],
        "exec herdr --session 'hd003-remote-b' remote-client-bridge"
    );

    let isolated = IsolatedSshConfig {
        identity_file: PathBuf::from("/var/tmp/hd003/keys/id_ed25519"),
        user_known_hosts_file: PathBuf::from("/var/tmp/hd003/etc/known_hosts"),
    };
    let test_ssh = build_ssh(&id, Some(&isolated), RemoteHerdrCommand::ApiBridge);
    let joined = test_ssh.args.join(" ");
    for required in [
        "-F none",
        "-o IdentitiesOnly=yes",
        "-o IdentityAgent=none",
        "-o GlobalKnownHostsFile=none",
        "-i /var/tmp/hd003/keys/id_ed25519",
        "-o UserKnownHostsFile=/var/tmp/hd003/etc/known_hosts",
        "-o StrictHostKeyChecking=yes",
    ] {
        assert!(joined.contains(required), "missing {required}: {joined}");
    }
    let sftp = build_sftp(&id, Some(&isolated));
    assert_eq!(sftp.program, "sftp");
    let joined = sftp.args.join(" ");
    assert!(joined.contains("-P 2222") && joined.contains("-o StrictHostKeyChecking=yes"));
    assert_eq!(sftp.args[sftp.args.len() - 2], "--");
    assert_eq!(sftp.args.last().unwrap(), "user@dev-box.example");

    let session = SessionName::parse("hd003-remote-b").unwrap();
    let lines: Vec<String> = [
        RemoteHerdrCommand::ServerStatus,
        RemoteHerdrCommand::ClientBridge,
        RemoteHerdrCommand::ApiBridge,
    ]
    .into_iter()
    .map(|c| remote_command_line(&session, c))
    .collect();
    for line in &lines {
        assert!(line.contains("--session 'hd003-remote-b'"), "{line}");
        for forbidden in [
            "install", "update", "stop", "restart", "kill", "handoff", "delete", "machine",
        ] {
            assert!(!line.contains(forbidden), "{forbidden} in {line}");
        }
    }
}

/// Target/session validation before any spawn (ports of validate_remote_target and
/// validate_name). Would catch option injection, passwords in the target or the default
/// session addressed implicitly.
#[test]
fn ssh_identity_validation_refuses_ambiguous_targets() {
    let pid = || ProfileId::parse(SSH_ID).unwrap();
    for (target, code) in [
        ("", "ssh_target_empty"),
        ("-oProxyCommand=touch /tmp/x", "ssh_target_option_like"),
        ("user:secret@host", "ssh_target_password"),
        ("ssh://user:secret@host:22", "ssh_target_password"),
        ("host\nProxyCommand", "ssh_target_invalid"),
        ("two words", "ssh_target_invalid"),
    ] {
        assert_eq!(
            SshTarget::parse(target).unwrap_err().code,
            code,
            "{target:?}"
        );
    }
    assert!(SshTarget::parse("user@dev-box.tail3a9.ts.net").is_ok());
    assert!(SshTarget::parse("ssh://user@dev-box:2200").is_ok());
    assert_eq!(
        SshIdentity::new(pid(), "host", Some(0), "hd003-x")
            .unwrap_err()
            .code,
        "ssh_port_invalid"
    );
    // Spec 016: `default` is a valid target (the engine's default session); junk is not.
    assert_eq!(
        SshIdentity::new(pid(), "host", None, "default")
            .unwrap()
            .session
            .as_str(),
        "default"
    );
    assert_eq!(
        SshIdentity::new(pid(), "host", None, "../x")
            .unwrap_err()
            .code,
        "invalid_session_name"
    );
    assert_eq!(
        ProfileId::parse("ABC").unwrap_err().code,
        "profile_id_invalid"
    );
    assert_eq!(ProfileId::generate().as_str().len(), 32);
    assert_ne!(ProfileId::generate(), ProfileId::generate());
}

// ---------------------------------------------------------------------------------------
// TASK-003-03 — GUI profile store; TUI catalog imported read-only
// ---------------------------------------------------------------------------------------

fn tui_catalog(dir: &std::path::Path) -> PathBuf {
    let path = dir.join("client").join("endpoints.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "selected_profile": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "ssh": [
                { "id": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "label": "build-box", "target": "ci@build-box", "session": "work", "enabled": true },
                { "id": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "label": "injetado", "target": "-oProxyCommand=x", "session": "work", "enabled": true },
                { "id": "cccccccccccccccccccccccccccccccc", "label": "padrão", "target": "me@laptop", "session": "default", "enabled": true }
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    path
}

/// Would catch: writing the TUI catalog (or its selection), importing invalid targets,
/// duplicating profiles on re-import, or spawning anything while importing.
#[test]
fn tui_profiles_are_imported_read_only_into_the_gui_store() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state-herdr");
    let catalog = tui_catalog(&state);
    let before = std::fs::read(&catalog).unwrap();
    let before_mtime = std::fs::metadata(&catalog).unwrap().modified().unwrap();
    let prefs = root.path().join("prefs");

    let mut store = ProfileStore::open(&prefs, &[&state]).unwrap();
    assert!(store.profiles().is_empty());
    let report = store.import_tui_catalog(&catalog).unwrap();
    // Spec 016: a profile on the engine's default session is a valid target now.
    assert_eq!(report.imported, vec!["build-box", "padrão"]);
    let skipped: Vec<(&str, &str)> = report
        .skipped
        .iter()
        .map(|s| (s.label.as_str(), s.code.as_str()))
        .collect();
    assert_eq!(skipped, vec![("injetado", "ssh_target_option_like")]);
    let again = store.import_tui_catalog(&catalog).unwrap();
    assert!(again.imported.is_empty());
    assert_eq!(again.already_present, 2);
    assert_eq!(store.profiles().len(), 2);
    let imported = &store.profiles()[0];
    assert_ne!(imported.id.as_str(), "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    assert_eq!(
        imported.imported_from.as_deref(),
        Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
    );

    // Edits go to the GUI store only.
    let edited = store
        .save(SshProfileDraft {
            id: Some(imported.id.as_str().to_owned()),
            label: "build-box (GUI)".into(),
            target: "ci@build-box".into(),
            port: Some(2200),
            session: "work".into(),
            auth: None,
        })
        .unwrap();
    assert_eq!(edited.port, Some(2200));
    assert_eq!(
        std::fs::read(&catalog).unwrap(),
        before,
        "TUI catalog untouched"
    );
    assert_eq!(
        std::fs::metadata(&catalog).unwrap().modified().unwrap(),
        before_mtime
    );
    let mut listing: Vec<String> = std::fs::read_dir(state.join("client"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    listing.sort();
    assert_eq!(
        listing,
        vec!["endpoints.json"],
        "no selection or temp files written"
    );

    // Reload from disk (GUI restart).
    let reloaded = ProfileStore::open(&prefs, &[&state]).unwrap();
    assert_eq!(reloaded.profiles(), store.profiles());
    let raw: Value =
        serde_json::from_str(&std::fs::read_to_string(prefs.join(STORE_FILE)).unwrap()).unwrap();
    assert_eq!(raw["version"], 1);
}

/// Spec 031 (AC-031-02): the auth choice of the dialog is persisted with the profile so a
/// retry/reconnect repeats it; a key-file choice stays absent (nothing extra is stored).
#[test]
fn ssh_agent_auth_is_persisted_and_key_auth_stays_absent() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state-herdr");
    std::fs::create_dir_all(&state).unwrap();
    let prefs = root.path().join("prefs");
    let mut store = ProfileStore::open(&prefs, &[&state]).unwrap();
    let agent = store
        .save(SshProfileDraft {
            id: None,
            label: "agent-box".into(),
            target: "user@agent-box".into(),
            port: None,
            session: "work".into(),
            auth: Some("ssh-agent".into()),
        })
        .unwrap();
    assert_eq!(agent.auth.as_deref(), Some("ssh-agent"));
    let key = store
        .save(SshProfileDraft {
            id: None,
            label: "key-box".into(),
            target: "user@key-box".into(),
            port: None,
            session: "work".into(),
            auth: Some("key".into()),
        })
        .unwrap();
    assert_eq!(
        key.auth, None,
        "key files are the OpenSSH default, not a stored value"
    );
    let reloaded = ProfileStore::open(&prefs, &[&state]).unwrap();
    assert_eq!(
        reloaded.get(agent.id.as_str()).unwrap().auth.as_deref(),
        Some("ssh-agent")
    );
    assert_eq!(reloaded.get(key.id.as_str()).unwrap().auth, None);
    let raw = std::fs::read_to_string(prefs.join(STORE_FILE)).unwrap();
    assert_eq!(raw.matches("\"auth\": \"ssh-agent\"").count(), 1, "{raw}");
    assert!(!raw.contains("\"auth\": \"key\""), "{raw}");
}

/// Negative contract for the store: invalid drafts, prefs inside the engine dirs, and a
/// corrupt store left untouched.
#[test]
fn profile_store_refuses_invalid_drafts_engine_dirs_and_corrupt_files() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state-herdr");
    std::fs::create_dir_all(&state).unwrap();
    assert_eq!(
        ProfileStore::open(&state.join("desktop"), &[&state])
            .unwrap_err()
            .code,
        "prefs_inside_engine_dir"
    );
    let prefs = root.path().join("prefs");
    let mut store = ProfileStore::open(&prefs, &[&state]).unwrap();
    for (draft, code) in [
        (
            SshProfileDraft {
                id: None,
                label: "  ".into(),
                target: "host".into(),
                port: None,
                session: "work".into(),
                auth: None,
            },
            "profile_label_invalid",
        ),
        (
            SshProfileDraft {
                id: None,
                label: "x".into(),
                target: "-oProxyCommand=x".into(),
                port: None,
                session: "work".into(),
                auth: None,
            },
            "ssh_target_option_like",
        ),
        (
            SshProfileDraft {
                id: Some("ffffffffffffffffffffffffffffffff".into()),
                label: "x".into(),
                target: "host".into(),
                port: None,
                session: "work".into(),
                auth: None,
            },
            "profile_not_found",
        ),
        (
            // Spec 031: the auth choice is a closed set; an unknown method writes nothing.
            SshProfileDraft {
                id: None,
                label: "x".into(),
                target: "host".into(),
                port: None,
                session: "work".into(),
                auth: Some("kerberos".into()),
            },
            "profile_auth_invalid",
        ),
    ] {
        assert_eq!(store.save(draft).unwrap_err().code, code);
    }
    assert!(
        !prefs.join(STORE_FILE).exists(),
        "invalid drafts wrote nothing"
    );

    std::fs::create_dir_all(&prefs).unwrap();
    std::fs::write(prefs.join(STORE_FILE), b"{not json").unwrap();
    assert_eq!(
        ProfileStore::open(&prefs, &[&state]).unwrap_err().code,
        "connections_store_corrupt"
    );
    std::fs::write(prefs.join(STORE_FILE), br#"{"version":9,"profiles":[]}"#).unwrap();
    assert_eq!(
        ProfileStore::open(&prefs, &[&state]).unwrap_err().code,
        "connections_store_unsupported"
    );
    assert_eq!(
        std::fs::read(prefs.join(STORE_FILE)).unwrap(),
        br#"{"version":9,"profiles":[]}"#
    );
}

// ---------------------------------------------------------------------------------------
// Spec 058 — hosts SSH reconnect when the app opens: the "Conectar ao abrir" preference and
// the per-host memory of having been connected, plus the background resume at startup.
// ---------------------------------------------------------------------------------------

const RESUME_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa58";
const RESUME_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbb58";
const RESUME_C: &str = "cccccccccccccccccccccccccccccc58";

/// `connections.json` exactly as the previous format declared it: the reader of that format had
/// no `connect_on_open`/`resume_on_open` and no `deny_unknown_fields`.
#[derive(Debug, serde::Deserialize)]
struct LegacyProfile {
    id: String,
    label: String,
    target: String,
    #[serde(default)]
    port: Option<u16>,
    session: String,
    #[serde(default)]
    auth: Option<String>,
    #[serde(default)]
    imported_from: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct LegacyStore {
    version: u32,
    #[serde(default)]
    profiles: Vec<LegacyProfile>,
}

/// Writes `<root>/prefs/connections.json` (v1) with the given profile objects.
fn write_store(root: &Path, profiles: Value) -> PathBuf {
    let prefs = root.join("prefs");
    std::fs::create_dir_all(&prefs).unwrap();
    std::fs::write(
        prefs.join(STORE_FILE),
        serde_json::to_vec_pretty(&json!({ "version": 1, "profiles": profiles })).unwrap(),
    )
    .unwrap();
    prefs
}

/// AC-058-01. Would catch: the preferences bumping the store version or rewriting the existing
/// fields, `connect_on_open` defaulting to off (a saved host would never come back), the resume
/// flag defaulting to on (a host the user never connected would be dialed at every start), an
/// edit of the profile resetting them, or a file written by the app that the previous format can
/// no longer read.
#[test]
fn connect_on_open_defaults_on_resume_defaults_off_and_the_v1_file_stays_readable() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state-herdr");
    std::fs::create_dir_all(&state).unwrap();
    let prefs = write_store(
        root.path(),
        json!([
            { "id": RESUME_A, "label": "mac-mini", "target": "user@mac-mini", "session": "work" },
            { "id": RESUME_B, "label": "dev-box", "target": "user@dev-box", "port": 2222, "session": "work", "auth": "ssh-agent" }
        ]),
    );

    let mut store = ProfileStore::open(&prefs, &[&state]).unwrap();
    for profile in store.profiles() {
        assert!(
            profile.connect_on_open,
            "{} came back with Conectar ao abrir off",
            profile.label
        );
        assert!(
            !profile.resume_on_open,
            "{} would be dialed at every start without ever being connected",
            profile.label
        );
    }

    // A successful connect and an explicit Desconectar move only the resume flag; the `…` menu
    // moves only the preference.
    store.set_resume_on_open(RESUME_A, true).unwrap();
    store.set_resume_on_open(RESUME_B, true).unwrap();
    store.set_resume_on_open(RESUME_B, false).unwrap();
    store.set_connect_on_open(RESUME_B, false).unwrap();
    assert_eq!(
        store
            .set_resume_on_open("ffffffffffffffffffffffffffffffff", true)
            .unwrap_err()
            .code,
        "profile_not_found"
    );

    // Editing the profile in the dialog keeps both preferences.
    store
        .save(SshProfileDraft {
            id: Some(RESUME_A.into()),
            label: "mac-mini (GUI)".into(),
            target: "user@mac-mini".into(),
            port: None,
            session: "work".into(),
            auth: None,
        })
        .unwrap();

    let mut reloaded = ProfileStore::open(&prefs, &[&state]).unwrap();
    let a = reloaded.get(RESUME_A).unwrap();
    assert_eq!((a.connect_on_open, a.resume_on_open), (true, true));
    assert_eq!(a.label, "mac-mini (GUI)");
    let b = reloaded.get(RESUME_B).unwrap();
    assert_eq!((b.connect_on_open, b.resume_on_open), (false, false));
    assert_eq!(
        (b.auth.as_deref(), b.port),
        (Some("ssh-agent"), Some(2222)),
        "the fields of the previous format are untouched"
    );

    // The file the app writes is still a v1 file the previous format reads.
    let raw: Value =
        serde_json::from_str(&std::fs::read_to_string(prefs.join(STORE_FILE)).unwrap()).unwrap();
    assert_eq!(raw["version"], 1);
    let legacy: LegacyStore = serde_json::from_value(raw).unwrap();
    assert_eq!(legacy.version, 1);
    assert_eq!(
        legacy
            .profiles
            .iter()
            .map(|p| (
                p.id.as_str(),
                p.label.as_str(),
                p.target.as_str(),
                p.port,
                p.session.as_str(),
                p.auth.as_deref(),
                p.imported_from.as_deref()
            ))
            .collect::<Vec<_>>(),
        vec![
            (
                RESUME_A,
                "mac-mini (GUI)",
                "user@mac-mini",
                None,
                "work",
                None,
                None
            ),
            (
                RESUME_B,
                "dev-box",
                "user@dev-box",
                Some(2222),
                "work",
                Some("ssh-agent"),
                None
            ),
        ]
    );

    // Removing the host removes the profile: there is nothing left to resume.
    reloaded.remove(RESUME_B).unwrap();
    assert!(ProfileStore::open(&prefs, &[&state])
        .unwrap()
        .get(RESUME_B)
        .is_none());
}

/// A `ConnectionsConfig` with the Local host of the window and the saved SSH profiles of
/// `<root>/prefs`, pointed at directories of this test only.
fn resume_config(root: &Path) -> ConnectionsConfig {
    ConnectionsConfig {
        prefs_dir: root.join("prefs"),
        herdr_config_dir: root.join("engine-config"),
        herdr_state_dir: root.join("engine-state"),
        local_session: Some(SessionName::parse("hd058-local").unwrap()),
        local_auto_start: false,
        herdr_bin: "herdr".into(),
        isolated_ssh: None,
        geometry: SurfaceGeometry {
            cols: 80,
            rows: 24,
            cell_width_px: 9,
            cell_height_px: 18,
        },
    }
}

fn saved_profile(root: &Path, id: &str) -> connections::profiles::SshProfile {
    ProfileStore::open(&root.join("prefs"), &[&root.join("engine-state")])
        .unwrap()
        .get(id)
        .cloned()
        .unwrap_or_else(|| panic!("{id} is not saved"))
}

fn host_phase(state: &ConnectionsState, endpoint: &str) -> LinkPhase {
    state
        .view()
        .hub
        .hosts
        .into_iter()
        .find(|host| host.endpoint == endpoint)
        .unwrap_or_else(|| panic!("{endpoint} is not a host"))
        .phase
}

/// `<target>` of every OpenSSH command the fake runner received (`ssh … -- <target> <line>`).
fn probed_targets(runner: &FakeRunner) -> Vec<String> {
    runner
        .commands
        .lock()
        .unwrap()
        .iter()
        .filter_map(|command| {
            let separator = command.args.iter().position(|arg| arg == "--")?;
            command.args.get(separator + 1).cloned()
        })
        .collect()
}

fn wait_until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// AC-058-01 at the commands. Would catch: a connect that does not record the host as connected
/// (nothing would come back at the next start), a `Desconectar` that leaves the resume on (the
/// host would come back against an explicit decision), or Local being taken for a saved profile.
#[test]
fn connect_and_disconnect_move_the_resume_flag_and_local_has_no_preference() {
    let root = tempfile::tempdir().unwrap();
    write_store(
        root.path(),
        json!([
            { "id": RESUME_A, "label": "mac-mini", "target": "user@mac-mini", "session": "work" }
        ]),
    );
    let runner = Arc::new(FakeRunner::default());
    let state = ConnectionsState::with_runner(resume_config(root.path()), runner);

    assert!(!saved_profile(root.path(), RESUME_A).resume_on_open);
    state.connect(RESUME_A).unwrap();
    assert!(
        saved_profile(root.path(), RESUME_A).resume_on_open,
        "the host the user connected is remembered"
    );
    state.disconnect(RESUME_A).unwrap();
    assert!(
        !saved_profile(root.path(), RESUME_A).resume_on_open,
        "an explicit Desconectar turns the resume off"
    );
    state.reconnect(RESUME_A).unwrap();
    assert!(
        saved_profile(root.path(), RESUME_A).resume_on_open,
        "a manual connect turns it back on"
    );

    // The preference is a saved-profile matter: Local is refused, exactly like disconnect/remove.
    assert_eq!(
        state.set_connect_on_open("local", false).unwrap_err().code,
        "endpoint_local"
    );
    assert!(state.set_connect_on_open(RESUME_A, false).is_ok());
    assert!(!saved_profile(root.path(), RESUME_A).connect_on_open);

    // Removing the host removes the profile with its preferences: nothing left to resume.
    state.remove_profile(RESUME_A).unwrap();
    assert!(ProfileStore::open(
        &root.path().join("prefs"),
        &[&root.path().join("engine-state")]
    )
    .unwrap()
    .get(RESUME_A)
    .is_none());
}

/// AC-058-02. Would catch: a resume that blocks the window while the first probe runs, a host
/// that was not connected (or whose `Conectar ao abrir` is off) being dialed anyway, a resume
/// that bypasses the Conectar button's path and accepts an unknown host key, or a failure that
/// starts an extra retry loop of its own.
#[test]
fn opening_the_app_resumes_only_the_connected_hosts_in_the_background() {
    let root = tempfile::tempdir().unwrap();
    write_store(
        root.path(),
        json!([
            { "id": RESUME_A, "label": "mac-mini", "target": "user@mac-mini", "session": "work", "connect_on_open": true, "resume_on_open": true },
            { "id": RESUME_B, "label": "dev-box", "target": "user@dev-box", "session": "work", "connect_on_open": true, "resume_on_open": false },
            { "id": RESUME_C, "label": "old-box", "target": "user@old-box", "session": "work", "connect_on_open": false, "resume_on_open": true }
        ]),
    );
    let runner = Arc::new(FakeRunner::default());
    // The only probe that may run holds until the test releases it: the window must already be
    // open while it is in flight.
    let (release, gate) = channel();
    *runner.gate.lock().unwrap() = Some(gate);
    // ... and then answers exactly like a host whose key is not known, the failure the Conectar
    // button produces today.
    script(
        &runner,
        ProcessOutput {
            status: Some(255),
            stdout: vec![],
            stderr: "Host key verification failed.\r\n".into(),
        },
    );
    let state = ConnectionsState::with_runner(resume_config(root.path()), runner.clone());

    let started = Instant::now();
    let resume = state.resume_connected_hosts().expect("resume thread");
    resume.join().unwrap();
    let handed_back = started.elapsed();
    wait_until("the probe of mac-mini", || {
        probed_targets(&runner).iter().any(|t| t == "user@mac-mini")
    });
    assert!(
        handed_back < Duration::from_secs(2),
        "the resume held the window for {handed_back:?} while the probe was still running"
    );
    release.send(()).unwrap();

    wait_until("mac-mini to need attention", || {
        host_phase(&state, RESUME_A) == LinkPhase::Attention
    });
    // The host key was not accepted and nothing else was run on the remote host.
    let host = state
        .view()
        .hub
        .hosts
        .into_iter()
        .find(|host| host.endpoint == RESUME_A)
        .unwrap();
    assert_eq!(host.attention, Some(AttentionReason::HostKeyUnknown));

    // Past the first automatic backoff: the failed resume is not retried in a loop of its own.
    std::thread::sleep(backoff_delay(1) + Duration::from_millis(500));
    let targets = probed_targets(&runner);
    assert_eq!(
        targets.iter().filter(|t| *t == "user@mac-mini").count(),
        1,
        "the resume used one attempt of the Conectar path: {targets:?}"
    );
    assert_eq!(
        targets
            .iter()
            .filter(|t| *t == "user@dev-box" || *t == "user@old-box")
            .count(),
        0,
        "only the host that was connected is resumed: {targets:?}"
    );
    assert_eq!(host_phase(&state, RESUME_B), LinkPhase::Offline);
    assert_eq!(host_phase(&state, RESUME_C), LinkPhase::Offline);
    assert_eq!(host_phase(&state, "local"), LinkPhase::Offline);
    assert!(
        !saved_profile(root.path(), RESUME_B).resume_on_open,
        "a host that was not resumed keeps its flag"
    );
}

/// IPC surface. Would catch: a command without handler, a generic shell/exec command, a
/// frontend bridge invoking undeclared commands, or a machine add / catalog write path.
#[test]
fn connection_commands_are_limited_and_match_the_frontend_bridge() {
    let source = include_str!("../src/connections/commands.rs");
    let forbidden = [
        "shell",
        "exec",
        "spawn",
        "open_url",
        "read_file",
        "write_file",
        "http",
        "fetch",
        "close",
        "install",
        "machine",
    ];
    for command in connections::commands::COMMANDS {
        assert!(
            source.contains(&format!("#[tauri::command]\npub fn {command}("))
                || source.contains(&format!("#[tauri::command]\npub async fn {command}(")),
            "{command} has no #[tauri::command] handler"
        );
        for word in forbidden {
            assert!(!command.contains(word), "{command} looks like {word}");
        }
    }
    assert_eq!(
        source.matches("#[tauri::command]").count(),
        connections::commands::COMMANDS.len()
    );
    let bridge = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../src/connections/bridge.ts"
    ))
    .unwrap();
    let mut invoked: Vec<&str> = bridge
        .split("invoke<")
        .skip(1)
        .filter_map(|rest| rest.split('"').nth(1))
        .collect();
    invoked.sort_unstable();
    invoked.dedup();
    let mut declared = connections::commands::COMMANDS.to_vec();
    declared.sort_unstable();
    assert_eq!(invoked, declared);
    // No machine add, catalog writes or server lifecycle in the connections backend.
    for file in [
        include_str!("../src/connections/profiles.rs"),
        include_str!("../src/connections/commands.rs"),
        include_str!("../src/connections/hub.rs"),
        include_str!("../src/bridge/ssh.rs"),
    ] {
        for needle in [
            "\"machine\"",
            "\"add\"",
            "session stop",
            "\"stop\"",
            "live-handoff",
        ] {
            assert!(
                !file.contains(needle),
                "{needle} found in the connections backend"
            );
        }
    }
    // The text input command maps only literal text and Enter.
    let events = connections::commands::text_events("echo x", true);
    assert_eq!(
        events,
        vec![
            ClientPaneInputEvent::TextCommit("echo x".into()),
            ClientPaneInputEvent::key_press(ClientKeyCode::Enter, 0)
        ]
    );
    assert_eq!(
        connections::commands::text_events("sem enter", false),
        vec![ClientPaneInputEvent::TextCommit("sem enter".into())]
    );
}

/// Keeps `RecvTimeoutError` in use for the harness helpers below on non-Linux hosts.
#[allow(dead_code)]
fn _timeout_kind(_e: RecvTimeoutError) {}

// ---------------------------------------------------------------------------------------
// Native E2E: real Tauri window with ConnectionsPanel/ConnectionDialog and the real backend
// against a disposable unprivileged sshd (own port, keys, authorized_keys and known_hosts)
// and two named disposable Herdr sessions (Local and "remote" reached through SSH).
// Flow: Local + SSH online → input to SSH only → SSH lost (Reconectando, input off, Local
// input still works) → remote server restarted (new boot) and sshd back → input only after
// reconciliation → TUI profiles imported read-only → unknown host key needs attention →
// GUI closed; engines, shells and known_hosts preserved.
// ---------------------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod e2e {
    use super::connections::commands::{self, ConnectionsConfig, ConnectionsState};
    use super::connections::ssh_options::IsolatedSshConfig;
    use super::native_harness::{self, PHASE_ENV, RESULT_ENV};
    use super::window_harness;
    use herdr_client::{
        ConnectOptions, LocalGateway, RuntimeGateway, SessionName, SurfaceGeometry,
    };
    use serde_json::{json, Value};
    use std::collections::BTreeMap;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const WINDOW_PHASE_TEST: &str = "e2e::e2e_connections_window";
    const SESSION_VARS: &[&str] = &[
        "HERDR_SOCKET_PATH",
        "HERDR_CLIENT_SOCKET_PATH",
        "HERDR_SESSION",
        "HERDR_WORKSPACE_ID",
        "HERDR_TAB_ID",
        "HERDR_PANE_ID",
    ];

    /// One GUI process: a Tauri window over the built frontend whose page runs
    /// `src/features/connections/e2e.ts` against the real connection commands.
    #[test]
    #[ignore = "window phase of e2e_connections_flow; fails when run outside the harness"]
    fn e2e_connections_window() {
        let phase = native_harness::current_phase();
        let params: Value =
            serde_json::from_str(&native_harness::required("HERDR_DESKTOP_E2E_PARAMS")).unwrap();
        let env = |k: &str| std::env::var(k).ok();
        let state = ConnectionsState::new(ConnectionsConfig {
            prefs_dir: PathBuf::from(native_harness::required("HERDR_DESKTOP_E2E_PREFS")),
            herdr_config_dir: herdr_client::session::herdr_config_dir(&env),
            herdr_state_dir: PathBuf::from(native_harness::required("HERDR_DESKTOP_E2E_STATE")),
            local_session: Some(
                SessionName::parse(&native_harness::required("HERDR_DESKTOP_E2E_SESSION")).unwrap(),
            ),
            local_auto_start: false,
            herdr_bin: "herdr".into(),
            isolated_ssh: Some(IsolatedSshConfig {
                identity_file: PathBuf::from(native_harness::required(
                    "HERDR_DESKTOP_E2E_SSH_IDENTITY",
                )),
                user_known_hosts_file: PathBuf::from(native_harness::required(
                    "HERDR_DESKTOP_E2E_SSH_KNOWN_HOSTS",
                )),
            }),
            geometry: SurfaceGeometry {
                cols: 100,
                rows: 30,
                cell_width_px: 9,
                cell_height_px: 18,
            },
        });
        let closing = state.clone();
        let builder = tauri::Builder::default()
            .manage(state)
            .on_window_event(move |_window, event| {
                if matches!(
                    event,
                    tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed
                ) {
                    // Closing the GUI detaches its clients; engines and remote servers live on.
                    closing.detach_all();
                }
            })
            .invoke_handler(tauri::generate_handler![
                commands::connections_list,
                commands::connections_watch,
                commands::connection_profile_save,
                commands::connection_profiles_import,
                commands::connection_connect,
                commands::connection_cancel,
                commands::connection_send_text,
                commands::connection_workspaces,
                window_harness::harness_report,
            ]);
        window_harness::run_feature_window(
            tauri::generate_context!(),
            builder,
            "connections",
            &phase,
            params,
            PathBuf::from(native_harness::required(RESULT_ENV)),
            Duration::from_secs(300),
        );
        panic!("the harness window returned without reporting done");
    }

    // --- helpers ------------------------------------------------------------------------

    fn run(cmd: &mut Command, what: &str) -> std::process::Output {
        let out = cmd
            .output()
            .unwrap_or_else(|e| panic!("{what}: could not start ({e})"));
        assert!(
            out.status.success(),
            "{what} failed ({}): {}{}",
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    fn herdr_plain(config: &Path) -> Command {
        let mut command = Command::new(
            std::env::var("HERDR_DESKTOP_HERDR_BIN").unwrap_or_else(|_| "herdr".into()),
        );
        for var in SESSION_VARS {
            command.env_remove(var);
        }
        command.env("HERDR_CONFIG_PATH", config);
        command
    }

    fn herdr(config: &Path, session: &str) -> Command {
        let mut command = herdr_plain(config);
        command.arg("--session").arg(session);
        command
    }

    fn session_running(session: &str) -> bool {
        let out = Command::new(
            std::env::var("HERDR_DESKTOP_HERDR_BIN").unwrap_or_else(|_| "herdr".into()),
        )
        .args(["session", "list"])
        .output()
        .unwrap();
        String::from_utf8_lossy(&out.stdout).lines().any(|l| {
            let mut cols = l.split_whitespace();
            cols.next() == Some(session) && cols.next() == Some("running")
        })
    }

    fn pane_text(config: &Path, session: &str, pane: &str) -> String {
        let out = herdr(config, session)
            .args(["pane", "read", pane, "--source", "recent", "--lines", "400"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Boot id and shell pid observed through a separate metadata-only connection.
    fn observe(session: &str) -> (String, u32) {
        let env = |k: &str| std::env::var(k).ok();
        let config_dir = herdr_client::session::herdr_config_dir(&env);
        let mut gateway = LocalGateway::new(&config_dir, SessionName::parse(session).unwrap());
        gateway
            .connect(ConnectOptions {
                geometry: SurfaceGeometry {
                    cols: 80,
                    rows: 24,
                    cell_width_px: 9,
                    cell_height_px: 18,
                },
                surface_active: false,
            })
            .expect("observer attach");
        let events = gateway.take_event_stream().unwrap();
        std::thread::spawn(move || while events.recv().is_ok() {});
        let deadline = Instant::now() + Duration::from_secs(20);
        let boot = loop {
            if let Some(identity) = gateway.identity() {
                break identity.boot_id;
            }
            assert!(Instant::now() < deadline, "observer got no boot id");
            std::thread::sleep(Duration::from_millis(20));
        };
        let mut pid = None;
        for _ in 0..100 {
            pid = gateway.api().pane_shell_pid("w1:p1").unwrap();
            if pid.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        gateway.detach();
        (boot, pid.expect("w1:p1 shell pid"))
    }

    /// Waits on a long-lived child from a background thread so it never becomes a zombie.
    fn reap(mut child: std::process::Child) {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }

    fn process_alive(pid: u32) -> bool {
        Path::new(&format!("/proc/{pid}")).exists()
    }

    fn cmdline(pid: u32) -> String {
        std::fs::read(format!("/proc/{pid}/cmdline"))
            .map(|b| String::from_utf8_lossy(&b).replace('\0', " "))
            .unwrap_or_default()
    }

    fn processes() -> BTreeMap<u32, (u32, String)> {
        let mut map = BTreeMap::new();
        for entry in std::fs::read_dir("/proc").unwrap().flatten() {
            let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
                continue;
            };
            let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
                continue;
            };
            let Some(tail) = stat.rsplit_once(')').map(|(_, t)| t.to_owned()) else {
                continue;
            };
            let ppid = tail
                .split_whitespace()
                .nth(1)
                .and_then(|p| p.parse().ok())
                .unwrap_or(0);
            map.insert(pid, (ppid, cmdline(pid)));
        }
        map
    }

    fn descendants(root: u32) -> Vec<u32> {
        let table = processes();
        let mut found = vec![root];
        let mut i = 0;
        while i < found.len() {
            let parent = found[i];
            found.extend(
                table
                    .iter()
                    .filter(|(_, (ppid, _))| *ppid == parent)
                    .map(|(pid, _)| *pid),
            );
            i += 1;
        }
        found.remove(0);
        found
    }

    fn kill(pid: u32) {
        let _ = Command::new("kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .status();
    }

    fn port_listening(port: u16) -> bool {
        std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
    }

    // --- disposable sshd ------------------------------------------------------------------

    struct Sshd {
        base: PathBuf,
        port: u16,
        pid: Option<u32>,
    }

    impl Sshd {
        fn create(base: &Path) -> Self {
            for dir in ["etc", "keys", "run", "log"] {
                std::fs::create_dir_all(base.join(dir)).unwrap();
            }
            for (key, comment) in [
                (base.join("etc/ssh_host_ed25519_key"), "hd003-host"),
                (base.join("keys/id_ed25519"), "hd003-client"),
            ] {
                run(
                    Command::new("ssh-keygen")
                        .args(["-q", "-t", "ed25519", "-N", "", "-C", comment, "-f"])
                        .arg(&key),
                    "ssh-keygen",
                );
            }
            std::fs::copy(
                base.join("keys/id_ed25519.pub"),
                base.join("etc/authorized_keys"),
            )
            .unwrap();
            let port = std::net::TcpListener::bind("127.0.0.1:0")
                .unwrap()
                .local_addr()
                .unwrap()
                .port();
            let user = String::from_utf8(run(Command::new("id").arg("-un"), "id -un").stdout)
                .unwrap()
                .trim()
                .to_owned();
            let sftp = ["/usr/lib/ssh/sftp-server", "/usr/libexec/sftp-server"]
                .into_iter()
                .find(|p| Path::new(p).exists())
                .unwrap_or("/usr/lib/ssh/sftp-server");
            let b = base.display();
            std::fs::write(
                base.join("etc/sshd_config"),
                format!(
                    "Port {port}\nListenAddress 127.0.0.1\nAddressFamily inet\nHostKey {b}/etc/ssh_host_ed25519_key\nPidFile {b}/run/sshd.pid\nAuthorizedKeysFile {b}/etc/authorized_keys\nAllowUsers {user}\nPubkeyAuthentication yes\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nPermitRootLogin no\nUsePAM no\nStrictModes no\nX11Forwarding no\nAllowAgentForwarding no\nAllowTcpForwarding no\nPermitTunnel no\nLoginGraceTime 30\nLogLevel VERBOSE\nSubsystem sftp {sftp}\nPerSourcePenalties no\n"
                ),
            )
            .unwrap();
            let host_pub =
                std::fs::read_to_string(base.join("etc/ssh_host_ed25519_key.pub")).unwrap();
            let key: Vec<&str> = host_pub.split_whitespace().take(2).collect();
            std::fs::write(
                base.join("etc/known_hosts"),
                format!("[127.0.0.1]:{port} {} {}\n", key[0], key[1]),
            )
            .unwrap();
            let sshd = Self {
                base: base.to_path_buf(),
                port,
                pid: None,
            };
            run(
                Command::new(sshd.binary())
                    .arg("-t")
                    .arg("-f")
                    .arg(sshd.config()),
                "sshd -t",
            );
            sshd
        }

        fn binary(&self) -> &'static str {
            ["/usr/bin/sshd", "/usr/sbin/sshd"]
                .into_iter()
                .find(|p| Path::new(p).exists())
                .expect("sshd binary")
        }

        fn config(&self) -> PathBuf {
            self.base.join("etc/sshd_config")
        }

        fn start(&mut self) {
            let _ = std::fs::remove_file(self.base.join("run/sshd.pid"));
            let log = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.base.join("log/sshd.log"))
                .unwrap();
            Command::new("setsid")
                .arg(self.binary())
                .args(["-D", "-e", "-f"])
                .arg(self.config())
                .stdin(Stdio::null())
                .stdout(log.try_clone().unwrap())
                .stderr(log)
                .spawn()
                .map(reap)
                .expect("start sshd");
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let pid = std::fs::read_to_string(self.base.join("run/sshd.pid"))
                    .ok()
                    .and_then(|s| s.trim().parse::<u32>().ok());
                if let Some(pid) = pid {
                    if port_listening(self.port) {
                        assert!(
                            cmdline(pid).contains(&self.config().display().to_string()),
                            "pid {pid} is not our sshd"
                        );
                        self.pid = Some(pid);
                        return;
                    }
                }
                assert!(Instant::now() < deadline, "sshd did not start");
                std::thread::sleep(Duration::from_millis(50));
            }
        }

        /// Kills the listener and every connection it serves (not the Herdr servers).
        fn stop(&mut self) -> Vec<String> {
            let Some(pid) = self.pid.take() else {
                return Vec::new();
            };
            assert!(cmdline(pid).contains(&self.config().display().to_string()));
            let children = descendants(pid);
            let killed: Vec<String> = children.iter().map(|p| cmdline(*p)).collect();
            kill(pid);
            for child in children {
                kill(child);
            }
            let deadline = Instant::now() + Duration::from_secs(10);
            while port_listening(self.port) || process_alive(pid) {
                assert!(Instant::now() < deadline, "sshd did not stop");
                std::thread::sleep(Duration::from_millis(50));
            }
            killed
        }
    }

    impl Drop for Sshd {
        fn drop(&mut self) {
            self.stop();
            for key in ["keys/id_ed25519", "etc/ssh_host_ed25519_key"] {
                let _ = Command::new("shred")
                    .arg("-u")
                    .arg(self.base.join(key))
                    .status();
            }
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    // --- disposable remote session ----------------------------------------------------------

    struct RemoteSession {
        name: String,
        config: PathBuf,
        work: PathBuf,
        log: PathBuf,
    }

    impl RemoteSession {
        fn start(&self) {
            let log = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.log)
                .unwrap();
            let mut command = Command::new("setsid");
            command
                .arg(std::env::var("HERDR_DESKTOP_HERDR_BIN").unwrap_or_else(|_| "herdr".into()));
            for var in SESSION_VARS {
                command.env_remove(var);
            }
            command
                .env("HERDR_CONFIG_PATH", &self.config)
                .env("HERDR_STARTUP_CWD", &self.work)
                .args(["--session", &self.name, "server"])
                .stdin(Stdio::null())
                .stdout(log.try_clone().unwrap())
                .stderr(log)
                .spawn()
                .map(reap)
                .expect("start remote herdr session");
            let deadline = Instant::now() + Duration::from_secs(15);
            while !herdr(&self.config, &self.name)
                .args(["pane", "list"])
                .output()
                .is_ok_and(|o| o.status.success())
            {
                assert!(Instant::now() < deadline, "remote session did not start");
                std::thread::sleep(Duration::from_millis(100));
            }
            let panes: Value = serde_json::from_slice(
                &run(
                    herdr(&self.config, &self.name).args(["pane", "list"]),
                    "pane list",
                )
                .stdout,
            )
            .unwrap();
            if panes["result"]["panes"]
                .as_array()
                .is_none_or(|p| p.is_empty())
            {
                run(
                    herdr(&self.config, &self.name)
                        .args(["workspace", "create", "--focus", "--cwd"])
                        .arg(&self.work),
                    "workspace create",
                );
            }
            let _ = herdr(&self.config, &self.name)
                .args([
                    "pane",
                    "wait-output",
                    "--timeout",
                    "10000",
                    "--regex",
                    "[$#>%] ?$",
                    "w1:p1",
                ])
                .output();
        }

        fn stop(&self) {
            let _ = herdr_plain(&self.config)
                .args(["session", "stop", &self.name])
                .output();
            let deadline = Instant::now() + Duration::from_secs(10);
            while session_running(&self.name) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }

    impl Drop for RemoteSession {
        fn drop(&mut self) {
            self.stop();
            let _ = herdr_plain(&self.config)
                .args(["session", "delete", &self.name])
                .output();
        }
    }

    fn read_report(path: &Path) -> Option<Value> {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
    }

    fn our_ssh_clients(identity: &Path) -> Vec<String> {
        let needle = identity.display().to_string();
        processes()
            .into_values()
            .map(|(_, cmd)| cmd)
            .filter(|cmd| cmd.starts_with("ssh ") && cmd.contains(&needle))
            .collect()
    }

    #[test]
    #[ignore = "needs the disposable session created by scripts/feature-harness/session.sh and a local sshd; run by just check-spec 003"]
    fn e2e_connections_flow() {
        let local = native_harness::required("HERDR_DESKTOP_E2E_SESSION");
        let pane = native_harness::required("HERDR_DESKTOP_E2E_PANE");
        let dir = PathBuf::from(native_harness::required("HERDR_DESKTOP_E2E_DIR"));
        let report = PathBuf::from(native_harness::required("HERDR_DESKTOP_E2E_REPORT"));
        assert!(
            local.starts_with("hd003-"),
            "never run against a non-disposable session: {local}"
        );
        assert_eq!(pane, "w1:p1", "premise: local pane id");
        std::fs::create_dir_all(&report).unwrap();
        let config = dir.join("config.toml");
        let stamp = std::process::id();
        let mut log = std::fs::File::create(report.join("e2e-connections.log")).unwrap();
        let mut note = |line: String| {
            eprintln!("{line}");
            writeln!(log, "{line}").unwrap();
        };

        // Disposable resources.
        let mut sshd = Sshd::create(&dir.join("ssh"));
        sshd.start();
        let remote = RemoteSession {
            name: format!("{local}-r"),
            config: config.clone(),
            work: dir.join("work-remote"),
            log: dir.join("remote-server.log"),
        };
        std::fs::create_dir_all(&remote.work).unwrap();
        remote.start();
        let identity = sshd.base.join("keys/id_ed25519");
        let known_hosts = sshd.base.join("etc/known_hosts");
        let known_hosts_before = std::fs::read(&known_hosts).unwrap();
        let (local_boot, local_shell) = observe(&local);
        let (remote_boot_1, remote_shell_1) = observe(&remote.name);
        assert_ne!(local_boot, remote_boot_1, "premise: distinct engines");
        note(format!(
            "local={local} boot={local_boot} shell={local_shell}; remote={} boot={remote_boot_1} shell={remote_shell_1}; sshd 127.0.0.1:{} pid {:?}",
            remote.name, sshd.port, sshd.pid
        ));

        let state_dir = dir.join("tui-state");
        let catalog = state_dir.join("client/endpoints.json");
        std::fs::create_dir_all(catalog.parent().unwrap()).unwrap();
        std::fs::write(
            &catalog,
            br#"{"version":1,"ssh":[{"id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","label":"build-box","target":"ci@build-box.invalid","session":"work","enabled":true}]}"#,
        )
        .unwrap();
        let catalog_before = std::fs::read(&catalog).unwrap();
        let prefs = tempfile::tempdir().unwrap();
        let user = String::from_utf8(run(Command::new("id").arg("-un"), "id").stdout)
            .unwrap()
            .trim()
            .to_owned();
        let params = json!({
            "ssh_target": format!("{user}@127.0.0.1"),
            "bad_target": format!("{user}@localhost"),
            "ssh_port": sshd.port.to_string(),
            "ssh_session": remote.name,
            "marker_ssh": format!("HD003_SSH_{stamp}"),
            "marker_local": format!("HD003_LOCAL_{stamp}"),
            "marker_ssh2": format!("HD003_SSH2_{stamp}"),
        });

        // GUI process.
        let result_path = report.join("e2e-connections-window.json");
        let _ = std::fs::remove_file(&result_path);
        let window_log = std::fs::File::create(report.join("e2e-connections-window.log")).unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                WINDOW_PHASE_TEST,
                "--exact",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(PHASE_ENV, "flow")
            .env(RESULT_ENV, &result_path)
            .env("HERDR_DESKTOP_E2E_PARAMS", params.to_string())
            .env("HERDR_DESKTOP_E2E_PREFS", prefs.path())
            .env("HERDR_DESKTOP_E2E_STATE", &state_dir)
            .env("HERDR_DESKTOP_E2E_SESSION", &local)
            .env("HERDR_DESKTOP_E2E_SSH_IDENTITY", &identity)
            .env("HERDR_DESKTOP_E2E_SSH_KNOWN_HOSTS", &known_hosts)
            .stdout(window_log.try_clone().unwrap())
            .stderr(window_log)
            .spawn()
            .expect("spawn GUI phase");
        let gui_pid = child.id();
        note(format!("GUI pid {gui_pid}"));

        let marker_ssh = params["marker_ssh"].as_str().unwrap().to_owned();
        let marker_local = params["marker_local"].as_str().unwrap().to_owned();
        let mut handled_input = false;
        let mut handled_lost = false;
        let mut remote_boot_2 = String::new();
        let mut remote_shell_2 = 0;
        let deadline = Instant::now() + Duration::from_secs(330);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < deadline, "GUI phase did not finish");
            let step = read_report(&result_path)
                .and_then(|r| r["step"].as_str().map(str::to_owned))
                .unwrap_or_default();
            if step == "ssh_input" && !handled_input {
                handled_input = true;
                let remote_text = pane_text(&config, &remote.name, "w1:p1");
                let local_text = pane_text(&config, &local, "w1:p1");
                assert!(
                    remote_text.contains(&marker_ssh),
                    "SSH pane did not receive the input"
                );
                assert!(
                    !local_text.contains(&marker_ssh),
                    "Local pane received SSH input"
                );
                note("engine check: SSH w1:p1 has the SSH marker; Local w1:p1 does not".into());
                let killed = sshd.stop();
                note(format!("SSH interrupted: sshd listener and {} connection processes stopped: {killed:?}", killed.len()));
            }
            if step == "ssh_lost" && !handled_lost {
                handled_lost = true;
                let local_text = pane_text(&config, &local, "w1:p1");
                assert!(
                    local_text.contains(&marker_local),
                    "Local input did not reach Local while SSH was lost"
                );
                note("engine check: Local accepted input while SSH was reconnecting".into());
                remote.stop();
                remote.start();
                let (boot, shell) = observe(&remote.name);
                assert_ne!(boot, remote_boot_1, "remote server did not get a new boot");
                remote_boot_2 = boot;
                remote_shell_2 = shell;
                sshd.start();
                note(format!("remote server restarted by the harness: boot {remote_boot_2} shell {remote_shell_2}; sshd back on pid {:?}", sshd.pid));
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        let r = read_report(&result_path).expect("final window report");
        note(format!("GUI exited {status}; window report: {r}"));
        assert!(status.success(), "GUI phase failed: {r}");
        assert!(r["error"].is_null(), "{r}");
        assert!(
            handled_input && handled_lost,
            "coordination steps not reached"
        );

        // Empty state: nothing connected until asked; onboarding shown.
        assert!(r["onboarding"]
            .as_str()
            .is_some_and(|t| t.contains("Nenhum host SSH")));
        assert_eq!(r["initial"][0]["phase"], "offline");

        // AC-003-01: same pane id on both hosts; SSH input only on SSH.
        let local_endpoint = r["local_online"]["endpoint"].as_str().unwrap();
        let ssh_endpoint = r["ssh_online"]["endpoint"].as_str().unwrap();
        assert_eq!(local_endpoint, "local");
        assert_eq!(ssh_endpoint.len(), 32);
        assert_eq!(r["local_online"]["panes"][0]["pane"], "w1:p1");
        assert_eq!(r["ssh_online"]["panes"][0]["pane"], "w1:p1");
        assert_eq!(r["local_online"]["boot"], local_boot.as_str());
        assert_eq!(r["ssh_online"]["boot"], remote_boot_1.as_str());
        assert!(r["ssh_echo"]["screen"]
            .as_str()
            .unwrap()
            .contains(&marker_ssh));
        assert!(!r["local_after_ssh"]["screen"]
            .as_str()
            .unwrap()
            .contains(&marker_ssh));
        // JSON API lane: Local lists through its socket; SSH through remote-api-bridge when the
        // remote Herdr has it (same binary as this host), otherwise an action error only.
        assert!(
            r["local_workspaces"].as_str().unwrap().contains("w1"),
            "{}",
            r["local_workspaces"]
        );
        let bridge_check = Command::new(
            std::env::var("HERDR_DESKTOP_HERDR_BIN").unwrap_or_else(|_| "herdr".into()),
        )
        .args(["remote-api-bridge", "--check"])
        .output()
        .unwrap();
        let remote_api = bridge_check.status.success()
            && String::from_utf8_lossy(&bridge_check.stdout).trim() == "herdr-api-bridge-v1";
        let ws = &r["workspaces"];
        note(format!(
            "remote-api-bridge available on this host: {remote_api}; SSH action result: {ws}"
        ));
        if remote_api {
            assert!(ws["listed"].as_str().unwrap().contains("w1"), "{ws}");
            assert!(ws["action_error"].is_null(), "{ws}");
        } else {
            assert!(ws["listed"].is_null(), "{ws}");
            assert!(
                ws["action_error"]
                    .as_str()
                    .unwrap()
                    .starts_with("remote_api_unsupported"),
                "{ws}"
            );
        }
        assert_eq!(
            ws["host"]["phase"], "online",
            "action error changed the connection: {ws}"
        );

        // AC-003-01 loss: Reconectando, input disabled, cache kept; Local still accepts input.
        let lost = &r["lost"];
        assert_eq!(lost["status"], "Reconnecting");
        assert_eq!(lost["panes"][0]["enabled"], false);
        assert_eq!(lost["panes"][0]["input_disabled"], true);
        assert_eq!(lost["panes"][0]["send_disabled"], true);
        assert_eq!(lost["panes"][0]["block"], "host_reconnecting");
        assert!(
            lost["screen"].as_str().unwrap().contains(&marker_ssh),
            "cache kept"
        );
        assert_eq!(r["local_while_lost"]["phase"], "online");
        assert!(r["local_echo"]["screen"]
            .as_str()
            .unwrap()
            .contains(&marker_local));
        assert_eq!(r["ssh_after_local_input"]["phase"], "reconnecting");
        assert!(!r["ssh_after_local_input"]["screen"]
            .as_str()
            .unwrap()
            .contains(&marker_local));

        // AC-003-02: after the new boot, input only on the reconciled pair.
        let timeline = r["timeline"].as_array().unwrap();
        note(format!(
            "reconnection timeline: {}",
            Value::Array(timeline.clone())
        ));
        for sample in timeline {
            if sample["enabled"] == true {
                assert_eq!(sample["phase"], "online", "{sample}");
                assert_eq!(
                    sample["boot"],
                    remote_boot_2.as_str(),
                    "input enabled before reconciliation: {sample}"
                );
            }
        }
        assert!(timeline
            .iter()
            .any(|s| s["phase"] == "reconnecting" && s["enabled"] == false));
        let reconnected = &r["reconnected"];
        assert_eq!(reconnected["boot"], remote_boot_2.as_str());
        assert!(
            reconnected["generation"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap()
                > r["ssh_online"]["generation"]
                    .as_str()
                    .unwrap()
                    .parse::<u64>()
                    .unwrap()
        );
        let marker_ssh2 = params["marker_ssh2"].as_str().unwrap();
        assert!(r["ssh_echo2"]["screen"]
            .as_str()
            .unwrap()
            .contains(marker_ssh2));
        assert!(pane_text(&config, &remote.name, "w1:p1").contains(marker_ssh2));
        assert!(!pane_text(&config, &local, "w1:p1").contains(marker_ssh2));
        // No replay: the first SSH command did not reach the restarted server.
        assert!(!pane_text(&config, &remote.name, "w1:p1").contains(&format!("echo {marker_ssh}")));

        // TASK-003-03: TUI profile imported read-only, never connected.
        assert!(
            r["import_text"].as_str().unwrap().contains("Importados: 1"),
            "{}",
            r["import_text"]
        );
        let imported = r["hosts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|h| h["label"] == "build-box")
            .expect("imported host");
        assert_eq!(imported["phase"], "offline");
        assert_eq!(
            std::fs::read(&catalog).unwrap(),
            catalog_before,
            "TUI catalog untouched"
        );
        let saved: Value =
            serde_json::from_slice(&std::fs::read(prefs.path().join("connections.json")).unwrap())
                .unwrap();
        assert_eq!(saved["profiles"].as_array().unwrap().len(), 3);

        // AC-003-03: unknown host key → attention with explicit guidance, no retry, key not accepted.
        let attention = &r["attention"];
        assert_eq!(attention["status"], "Needs attention");
        assert!(attention["detail"]
            .as_str()
            .unwrap()
            .contains("chave deste host não é conhecida"));
        assert!(attention["guidance"]
            .as_str()
            .unwrap()
            .contains("known_hosts"));
        assert_eq!(attention["actions"], json!(["Tentar novamente"]));
        assert_eq!(r["attention_later"]["phase"], "attention");
        assert_eq!(
            r["attention_later"]["attempt"], attention["attempt"],
            "no automatic retry"
        );
        assert_eq!(
            std::fs::read(&known_hosts).unwrap(),
            known_hosts_before,
            "host key not accepted"
        );
        let others: Vec<&Value> = r["hosts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|h| h["label"] == "Servidor SSH" || h["label"] == "This computer")
            .collect();
        assert!(
            others.iter().all(|h| h["phase"] == "online"),
            "attention affected other hosts"
        );

        // GUI closed: clients gone; engines, shells and remote server (not restarted) preserved.
        let deadline = Instant::now() + Duration::from_secs(10);
        while !our_ssh_clients(&identity).is_empty() {
            assert!(
                Instant::now() < deadline,
                "ssh clients left behind: {:?}",
                our_ssh_clients(&identity)
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(!process_alive(gui_pid));
        assert!(session_running(&local) && session_running(&remote.name));
        assert!(process_alive(local_shell), "local shell died");
        assert!(process_alive(remote_shell_2), "remote shell died");
        let (local_boot_after, _) = observe(&local);
        let (remote_boot_after, remote_shell_after) = observe(&remote.name);
        assert_eq!(local_boot_after, local_boot);
        assert_eq!(
            remote_boot_after, remote_boot_2,
            "the GUI restarted the remote server"
        );
        assert_eq!(remote_shell_after, remote_shell_2);
        note(format!(
            "after GUI exit: no ssh clients; sessions running; local shell {local_shell} and remote shell {remote_shell_2} alive; boots unchanged; known_hosts and TUI catalog byte-identical"
        ));
        std::fs::write(
            report.join("e2e-connections-summary.json"),
            serde_json::to_vec_pretty(&json!({
                "local_session": local,
                "remote_session": remote.name,
                "local_boot": local_boot,
                "remote_boot_before": remote_boot_1,
                "remote_boot_after_restart": remote_boot_2,
                "ssh_endpoint": ssh_endpoint,
                "pane": "w1:p1",
                "lost": lost,
                "timeline": timeline,
                "reconnected_generation": reconnected["generation"],
                "attention": attention,
                "import": r["import_text"],
                "remote_api_bridge_available": remote_api,
                "workspaces_local_api": r["local_workspaces"],
                "workspaces_ssh_api": r["workspaces"],
            }))
            .unwrap(),
        )
        .unwrap();
        drop(remote);
        drop(sshd);
    }
}
