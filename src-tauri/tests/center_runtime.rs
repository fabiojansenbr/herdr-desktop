//! Spec 013 — runtime seams the center region needs, authorised in the spec log: the engine data
//! the WebView did not receive (`tab.list` pane count and aggregated state, the `cwd` of each pane
//! from `pane.list`) and the qualified `pane.zoom` action behind its own capability.
//!
//! Seam: `AgentsCore` over a fake gateway that records every API/endpoint call, like spec 004's
//! own tests, in this file so the parallel fronts do not edit the same test module.
//!
//! AC-013-01: the tabs of the session with their pane counts (only the active tab has metadata).
//! AC-013-02: the path of each pane and the expand button that zooms the pane in the engine.

#[allow(dead_code)]
#[path = "../src/terminal.rs"]
mod terminal;

#[allow(dead_code)]
#[path = "../src/bridge/agent_commands.rs"]
mod agent_commands;

use std::sync::{Arc, Mutex};

use agent_commands::{AgentsCore, COMMANDS};
use herdr_client::protocol::wire::{
    CellData, FrameData, PaneSurfaceFrame, PaneSurfacePane, SurfaceGraphicsScene, SurfaceRect,
};
use herdr_client::{
    ConnectOptions, GatewayEvent, LiveIdentity, Negotiated, QualifiedTarget, RuntimeError,
    RuntimeGateway, SurfaceGeometry,
};
use serde_json::{json, Value};

const ENDPOINT: &str = "local";
const SESSION: &str = "hd013";
const BOOT: &str = "boot-013";

#[derive(Debug, Clone, PartialEq)]
enum Call {
    Api(String, Value),
    Endpoint(String, Value),
}

#[derive(Debug, Default)]
struct FakeHost {
    api_methods: Vec<String>,
    /// (pane_id, tab_id, cwd)
    panes: Vec<(String, String, Option<String>)>,
    /// pane_id → manual name the engine reports in `pane.list` (spec 028).
    pane_labels: std::collections::BTreeMap<String, String>,
    /// (tab_id, label, focused, pane_count, agent_status)
    tabs: Vec<(String, String, bool, usize, String)>,
    calls: Vec<Call>,
    zoom_error: Option<RuntimeError>,
}

type Host = Arc<Mutex<FakeHost>>;

fn host() -> Host {
    Arc::new(Mutex::new(FakeHost {
        api_methods: [
            "agent.list",
            "agent.get",
            "agent.start",
            "agent.prompt",
            "server.agent_manifests",
            "tab.list",
            "pane.list",
        ]
        .iter()
        .map(|m| (*m).to_owned())
        .collect(),
        panes: vec![
            ("w1:p1".into(), "w1:t1".into(), Some("/work/erp-api".into())),
            ("w1:p2".into(), "w1:t1".into(), None),
            ("w1:p3".into(), "w1:t2".into(), Some("/work/site".into())),
        ],
        tabs: vec![
            ("w1:t1".into(), "api".into(), true, 2, "working".into()),
            (
                "w1:t2".into(),
                "migrations".into(),
                false,
                1,
                "blocked".into(),
            ),
        ],
        ..FakeHost::default()
    }))
}

struct FakeGateway {
    host: Host,
}

impl RuntimeGateway for FakeGateway {
    fn endpoint(&self) -> &str {
        ENDPOINT
    }

    fn identity(&self) -> Option<LiveIdentity> {
        Some(LiveIdentity {
            endpoint: ENDPOINT.into(),
            session: SESSION.into(),
            connection_generation: 1,
            boot_id: BOOT.into(),
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
        if !host.api_methods.iter().any(|m| m == method) {
            return Err(RuntimeError::new(
                "invalid_request:id_mismatch",
                format!("invalid request: unknown variant `{method}`, expected one of `ping`"),
            ));
        }
        let empty = params.as_object().is_some_and(|o| o.is_empty());
        match method {
            "agent.start" | "agent.get" | "agent.prompt" if empty => Err(RuntimeError::new(
                "invalid_request:id_mismatch",
                "invalid request: missing field",
            )),
            "server.agent_manifests" => Ok(
                json!({ "type": "agent_manifest_status", "manifests": [{ "agent": "pi", "source_kind": "remote" }] }),
            ),
            "agent.list" => Ok(json!({ "type": "agent_list", "agents": [] })),
            "tab.list" => Ok(json!({
                "type": "tab_list",
                "tabs": host.tabs.iter().map(|(id, label, focused, panes, status)| json!({
                    "tab_id": id, "workspace_id": id.split(':').next().unwrap(), "label": label,
                    "focused": focused, "number": 1, "pane_count": panes, "agent_status": status,
                })).collect::<Vec<_>>(),
            })),
            "pane.list" => Ok(json!({
                "type": "pane_list",
                "panes": host.panes.iter().map(|(pane, tab, cwd)| {
                    let mut value = json!({
                        "pane_id": pane, "terminal_id": format!("term-{pane}"),
                        "workspace_id": pane.split(':').next().unwrap(), "tab_id": tab,
                        "focused": false, "agent_status": "unknown",
                    });
                    if let Some(cwd) = cwd {
                        value["cwd"] = json!(cwd);
                    }
                    if let Some(label) = host.pane_labels.get(pane) {
                        value["label"] = json!(label);
                    }
                    value
                }).collect::<Vec<_>>(),
            })),
            other => panic!("fake host has no answer for {other}"),
        }
    }

    fn endpoint_request(&self, method: &str, params: Value) -> Result<Value, RuntimeError> {
        let mut host = self.host.lock().unwrap();
        host.calls
            .push(Call::Endpoint(method.into(), params.clone()));
        if method == "pane.zoom" {
            if let Some(error) = host.zoom_error.clone() {
                return Err(error);
            }
            // Shape of `herdr pane zoom` as the reference engine answers (fidelity_native.rs).
            return Ok(json!({
                "type": "pane_zoom",
                "zoom": {
                    "zoomed": params["mode"] != "off",
                    "pane_id": params["pane_id"],
                },
            }));
        }
        Ok(json!({ "type": "ok" }))
    }

    fn send_input(
        &self,
        _target: &QualifiedTarget,
        _events: Vec<herdr_client::protocol::wire::ClientPaneInputEvent>,
    ) -> Result<(), RuntimeError> {
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

fn announced() -> Vec<String> {
    [
        "pane.focus",
        "pane.split",
        "pane.zoom",
        "pane.rename",
        "pane.swap",
        "pane.input.set",
        "tab.create",
    ]
    .iter()
    .map(|m| (*m).to_owned())
    .collect()
}

fn target(pane: &str) -> QualifiedTarget {
    QualifiedTarget {
        endpoint: ENDPOINT.into(),
        session: SESSION.into(),
        connection_generation: 1,
        boot_id: BOOT.into(),
        workspace_id: None,
        pane_id: pane.into(),
    }
}

/// Committed surface with two panes side by side (engine border box around each inner rect).
fn two_panes(revision: u64) -> PaneSurfaceFrame {
    let rect = |x: u16, y: u16, width: u16, height: u16| SurfaceRect {
        x,
        y,
        width,
        height,
    };
    let pane = |id: &str, x: u16| PaneSurfacePane {
        pane_id: id.into(),
        content_revision: 1,
        rect: rect(x, 0, 40, 22),
        inner_rect: rect(x + 1, 1, 38, 20),
        scrollbar_rect: None,
        scroll: None,
        focused: id == "w1:p1",
        mouse_reporting: false,
        sgr_pixel_mouse: false,
        alternate_screen_active: false,
        pixel_width: 0,
        pixel_height: 0,
    };
    let (width, height) = (80u16, 22u16);
    PaneSurfaceFrame {
        boot_id: BOOT.into(),
        projection_revision: revision,
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
        panes: vec![pane("w1:p1", 0), pane("w1:p2", 40)],
        splits: Vec::new(),
        popup: None,
        graphics: SurfaceGraphicsScene::default(),
    }
}

fn core(host: &Host) -> (AgentsCore, FakeGateway) {
    let gateway = FakeGateway { host: host.clone() };
    let mut core = AgentsCore::new(&announced());
    core.discover(&gateway).expect("discovery");
    (core, gateway)
}

fn calls(host: &Host) -> Vec<Call> {
    host.lock().unwrap().calls.clone()
}

// Would catch: the tab list reaching the WebView without the engine's pane count or aggregated
// state, so the tab bar would have to count panes it cannot see (only the active tab has metadata).
#[test]
fn tabs_carry_the_engine_pane_count_and_aggregated_agent_status() {
    let host = host();
    let (mut core, gateway) = core(&host);
    let tabs = core.refresh_tabs(&gateway).expect("tab list");
    let seen: Vec<(String, usize, String)> = tabs
        .iter()
        .map(|t| (t.tab_id.clone(), t.pane_count, t.agent_status.clone()))
        .collect();
    assert_eq!(
        seen,
        vec![
            ("w1:t1".to_owned(), 2, "working".to_owned()),
            ("w1:t2".to_owned(), 1, "blocked".to_owned()),
        ]
    );
    // An engine that omits the fields publishes neither a count nor a state it did not report.
    host.lock().unwrap().tabs = vec![("w1:t9".into(), "x".into(), true, 0, "unknown".into())];
    let tabs = core.refresh_tabs(&gateway).expect("tab list");
    assert_eq!(
        (tabs[0].pane_count, tabs[0].agent_status.as_str()),
        (0, "unknown")
    );
}

// Would catch: the pane path taken from the project root or from another pane; the topology
// publishing a cwd for a pane the engine did not report one for.
#[test]
fn the_topology_carries_the_cwd_the_engine_reported_for_each_pane() {
    let host = host();
    let (mut core, gateway) = core(&host);
    core.refresh_panes(&gateway).expect("pane list");
    core.apply_surface(two_panes(1)).expect("surface");
    let topology = core.topology().expect("topology");
    let cwds: Vec<(String, Option<String>)> = topology
        .panes
        .iter()
        .map(|p| (p.pane_id.clone(), p.cwd.clone()))
        .collect();
    assert_eq!(
        cwds,
        vec![
            ("w1:p1".to_owned(), Some("/work/erp-api".to_owned())),
            ("w1:p2".to_owned(), None),
        ]
    );
    assert!(
        calls(&host).contains(&Call::Api("pane.list".into(), json!({}))),
        "pane.list must be the source of the paths: {:?}",
        calls(&host)
    );
}

// Would catch: an expand button announced by the GUI on a server that does not offer pane.zoom.
#[test]
fn zoom_is_a_capability_of_its_own_announced_by_the_endpoint() {
    let host = host();
    let (core, _) = core(&host);
    assert!(core.capabilities().zoom);
    let mut without = AgentsCore::new(&["pane.focus".to_owned()]);
    let gateway = FakeGateway { host: host.clone() };
    without.discover(&gateway).expect("discovery");
    assert!(!without.capabilities().zoom);
    let refused = without
        .zoom_pane(&gateway, &target("w1:p1"), "toggle")
        .expect_err("zoom without the method must be refused");
    assert_eq!(refused.code, "unsupported_method");
    assert!(
        !calls(&host)
            .iter()
            .any(|c| matches!(c, Call::Endpoint(m, _) if m == "pane.zoom")),
        "nothing may be sent when the server does not announce pane.zoom"
    );
}

// Would catch: a zoom sent for a pane of another host/boot/generation, for a pane outside the
// committed topology, or with a mode the engine does not accept.
#[test]
fn zoom_sends_one_qualified_request_for_a_pane_of_the_committed_topology() {
    let host = host();
    let (mut core, gateway) = core(&host);
    core.apply_surface(two_panes(1)).expect("surface");
    core.zoom_pane(&gateway, &target("w1:p2"), "toggle")
        .expect("zoom");
    let sent: Vec<Value> = calls(&host)
        .into_iter()
        .filter_map(|c| match c {
            Call::Endpoint(m, params) if m == "pane.zoom" => Some(params),
            _ => None,
        })
        .collect();
    assert_eq!(sent, vec![json!({ "pane_id": "w1:p2", "mode": "toggle" })]);

    let stale = QualifiedTarget {
        boot_id: "boot-other".into(),
        ..target("w1:p2")
    };
    assert_eq!(
        core.zoom_pane(&gateway, &stale, "toggle")
            .expect_err("stale boot")
            .code,
        "target_boot_stale"
    );
    assert_eq!(
        core.zoom_pane(&gateway, &target("w1:p9"), "toggle")
            .expect_err("unknown pane")
            .code,
        "target_pane_missing"
    );
    assert_eq!(
        core.zoom_pane(&gateway, &target("w1:p2"), "maximise")
            .expect_err("unknown mode")
            .code,
        "invalid_zoom_mode"
    );
    let sent_after = calls(&host)
        .into_iter()
        .filter(|c| matches!(c, Call::Endpoint(m, _) if m == "pane.zoom"))
        .count();
    assert_eq!(sent_after, 1, "a refused zoom sends nothing");
}

// Would catch: the engine's `zoom.zoomed` dropped on the way back, so the context menu could
// never offer `Desfazer zoom` from the engine's own answer (spec 028 edge case).
#[test]
fn zoom_answers_the_engines_zoomed_flag() {
    let host = host();
    let (mut core, gateway) = core(&host);
    core.apply_surface(two_panes(1)).expect("surface");
    let zoomed = core
        .zoom_pane(&gateway, &target("w1:p2"), "toggle")
        .expect("zoom");
    assert_eq!(
        (zoomed.pane_id.as_str(), zoomed.zoomed),
        ("w1:p2", Some(true))
    );
    let restored = core
        .zoom_pane(&gateway, &target("w1:p2"), "off")
        .expect("restore");
    assert_eq!(restored.zoomed, Some(false));
}

// Would catch: `pane.rename` without the engine method, sent twice, for a pane outside the
// topology, or with an untrimmed/oversized name; the manual name of `pane.list` not reaching the
// topology, so `Limpar nome do pane` would never appear or disappear (spec 028 AC-028-03).
#[test]
fn pane_rename_sends_one_engine_request_and_publishes_the_manual_name() {
    let host = host();
    let (mut core, gateway) = core(&host);
    host.lock()
        .unwrap()
        .pane_labels
        .insert("w1:p2".into(), "build".into());
    core.refresh_panes(&gateway).expect("pane list");
    core.apply_surface(two_panes(1)).expect("surface");
    let label = |core: &AgentsCore| {
        core.topology()
            .expect("topology")
            .panes
            .iter()
            .find(|p| p.pane_id == "w1:p2")
            .and_then(|p| p.label.clone())
    };
    assert_eq!(label(&core).as_deref(), Some("build"));

    core.rename_pane(&gateway, &target("w1:p1"), "w1:p2", Some(" api "))
        .expect("rename");
    let sent: Vec<Value> = calls(&host)
        .into_iter()
        .filter_map(|c| match c {
            Call::Endpoint(m, params) if m == "pane.rename" => Some(params),
            _ => None,
        })
        .collect();
    assert_eq!(sent, vec![json!({ "pane_id": "w1:p2", "label": "api" })]);

    // The engine cleared the manual name: the refresh publishes the topology without it.
    host.lock().unwrap().pane_labels.remove("w1:p2");
    core.rename_pane(&gateway, &target("w1:p1"), "w1:p2", None)
        .expect("clear");
    let sent: Vec<Value> = calls(&host)
        .into_iter()
        .filter_map(|c| match c {
            Call::Endpoint(m, params) if m == "pane.rename" => Some(params),
            _ => None,
        })
        .collect();
    assert_eq!(
        sent,
        vec![
            json!({ "pane_id": "w1:p2", "label": "api" }),
            json!({ "pane_id": "w1:p2" }),
        ],
        "clearing sends no label field"
    );
    assert_eq!(label(&core), None, "the cleared name left the topology");
    assert_eq!(
        core.rename_pane(&gateway, &target("w1:p1"), "w9:p9", Some("x"))
            .expect_err("unknown pane")
            .code,
        "target_pane_missing"
    );
    assert_eq!(
        core.rename_pane(&gateway, &target("w1:p1"), "w1:p2", Some(&"x".repeat(129)))
            .expect_err("oversized name")
            .code,
        "invalid_pane_label"
    );
}

// Would catch: an unqualified or one-sided swap, a swap of the focused pane with itself, or the
// source not focused again (the TUI focuses the source after `pane.swap`).
#[test]
fn swap_exchanges_the_clicked_pane_and_refocuses_the_source() {
    let host = host();
    let (mut core, gateway) = core(&host);
    core.apply_surface(two_panes(1)).expect("surface");
    core.swap_pane(&gateway, &target("w1:p1"), "w1:p2")
        .expect("swap");
    let sent: Vec<(String, Value)> = calls(&host)
        .into_iter()
        .filter_map(|c| match c {
            Call::Endpoint(m, params) if m == "pane.swap" || m == "pane.focus" => Some((m, params)),
            _ => None,
        })
        .collect();
    assert_eq!(
        sent,
        vec![
            (
                "pane.swap".into(),
                json!({ "source_pane_id": "w1:p1", "target_pane_id": "w1:p2" })
            ),
            ("pane.focus".into(), json!({ "pane_id": "w1:p1" })),
        ]
    );
    assert_eq!(
        core.swap_pane(&gateway, &target("w1:p1"), "w1:p1")
            .expect_err("same pane")
            .code,
        "swap_same_pane"
    );
    assert_eq!(
        core.swap_pane(&gateway, &target("w1:p1"), "w9:p9")
            .expect_err("unknown pane")
            .code,
        "target_pane_missing"
    );
    assert_eq!(
        core.swap_pane(
            &gateway,
            &QualifiedTarget {
                boot_id: "boot-other".into(),
                ..target("w1:p1")
            },
            "w1:p2"
        )
        .expect_err("stale boot")
        .code,
        "target_boot_stale"
    );
}

// Would catch: the toggle sent without the pane, with the wrong engine target, or skipped.
#[test]
fn right_click_toggle_sends_pane_input_set_with_the_engine_target() {
    let host = host();
    let (mut core, gateway) = core(&host);
    core.apply_surface(two_panes(1)).expect("surface");
    core.set_pane_right_click(&gateway, &target("w1:p1"), "w1:p2", true)
        .expect("to pane");
    core.set_pane_right_click(&gateway, &target("w1:p1"), "w1:p2", false)
        .expect("to menu");
    let sent: Vec<Value> = calls(&host)
        .into_iter()
        .filter_map(|c| match c {
            Call::Endpoint(m, params) if m == "pane.input.set" => Some(params),
            _ => None,
        })
        .collect();
    assert_eq!(
        sent,
        vec![
            json!({ "pane_id": "w1:p2", "right_click": "pane" }),
            json!({ "pane_id": "w1:p2", "right_click": "herdr" }),
        ]
    );
}

// Would catch: the command exposed to the WebView without being listed by the module that owns
// the agents commands (the registry test checks the composed window against this list).
#[test]
fn pane_zoom_is_one_of_the_agents_commands() {
    assert!(COMMANDS.contains(&"pane_zoom"), "{COMMANDS:?}");
    assert_eq!(
        COMMANDS.iter().filter(|c| **c == "pane_zoom").count(),
        1,
        "listed once"
    );
}

// Would catch (the gate r3 of spec 013 found the symptom): a path refresh republishing a topology
// with a different focus/pane set, which sent the window's actions to a pane the engine no longer
// lists. Copying the paths may only change `cwd`.
#[test]
fn copying_the_paths_never_changes_the_focus_or_the_pane_set() {
    let host = host();
    let (mut core, gateway) = core(&host);
    core.refresh_panes(&gateway).expect("pane list");
    core.apply_surface(two_panes(1)).expect("surface");
    let mut topology = core.topology().expect("topology");
    let before = topology.clone();

    let mut paths = std::collections::BTreeMap::new();
    paths.insert("w1:p1".to_owned(), "/work/novo".to_owned());
    // A path of a pane that is not in this topology (another tab/host) is ignored.
    paths.insert("w9:p9".to_owned(), "/outro".to_owned());
    assert!(agent_commands::apply_paths(&mut topology, &paths));

    assert_eq!(topology.focused_pane_id, before.focused_pane_id);
    assert_eq!(
        topology
            .panes
            .iter()
            .map(|p| p.pane_id.clone())
            .collect::<Vec<_>>(),
        before
            .panes
            .iter()
            .map(|p| p.pane_id.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        topology.panes.iter().map(|p| p.focused).collect::<Vec<_>>(),
        before.panes.iter().map(|p| p.focused).collect::<Vec<_>>()
    );
    assert_eq!(
        topology
            .panes
            .iter()
            .map(|p| p.cwd.clone())
            .collect::<Vec<_>>(),
        vec![Some("/work/novo".to_owned()), None]
    );
    // Reapplying the same paths changes nothing, so no event is published on every engine event.
    assert!(!agent_commands::apply_paths(&mut topology, &paths));
}

// Would catch: a committed surface reaching the window without the paths the engine reported, so
// the frames would have no path until some later event.
#[test]
fn a_committed_surface_carries_the_paths_already_read() {
    let host = host();
    let (mut core, gateway) = core(&host);
    core.refresh_panes(&gateway).expect("pane list");
    let published = core
        .apply_surface(two_panes(1))
        .expect("surface")
        .expect("a new topology");
    assert_eq!(
        published
            .panes
            .iter()
            .map(|p| p.cwd.clone())
            .collect::<Vec<_>>(),
        vec![Some("/work/erp-api".to_owned()), None]
    );
}

// Would catch (the gate r4 of spec 013 found it): a tab accepted by the engine that only reaches
// the window if the `tab.created` event arrives, leaving the tab bar stale when it does not.
#[test]
fn a_tab_created_by_the_command_is_listed_without_waiting_for_an_engine_event() {
    let host = host();
    let (mut core, gateway) = core(&host);
    core.apply_surface(two_panes(1)).expect("surface");
    core.refresh_tabs(&gateway).expect("tab list");
    assert_eq!(core.tabs().len(), 2);
    // The engine accepts the request and the new tab exists from then on (no event is delivered).
    host.lock()
        .unwrap()
        .tabs
        .push(("w1:t3".into(), "novo".into(), false, 1, "idle".into()));
    core.create_tab(&gateway, &target("w1:p1"))
        .expect("tab create");
    assert_eq!(
        core.tabs()
            .iter()
            .map(|t| t.tab_id.clone())
            .collect::<Vec<_>>(),
        vec!["w1:t1".to_owned(), "w1:t2".to_owned(), "w1:t3".to_owned()]
    );
    let sent = calls(&host);
    let create = sent
        .iter()
        .position(|c| matches!(c, Call::Endpoint(m, _) if m == "tab.create"))
        .expect("tab.create sent");
    let refreshed = sent
        .iter()
        .rposition(|c| matches!(c, Call::Api(m, _) if m == "tab.list"))
        .expect("tab.list read");
    assert!(refreshed > create, "the tab list is read after the create");
}

// Would catch: a split that adds a pane to the tab without the window ever seeing the new count.
#[test]
fn a_split_refreshes_the_tab_list_too() {
    let host = host();
    let (mut core, gateway) = core(&host);
    core.apply_surface(two_panes(1)).expect("surface");
    core.refresh_tabs(&gateway).expect("tab list");
    host.lock().unwrap().tabs[0].3 = 3;
    core.split(&gateway, &target("w1:p1"), "right")
        .expect("split");
    assert_eq!(core.tabs()[0].pane_count, 3);
}

// Would catch: a refresh failure swallowed, so the window keeps showing a list it cannot trust
// with no error anywhere.
#[test]
fn a_failed_tab_refresh_after_an_accepted_action_is_reported() {
    let host = host();
    let (mut core, gateway) = core(&host);
    core.apply_surface(two_panes(1)).expect("surface");
    core.refresh_tabs(&gateway).expect("tab list");
    host.lock().unwrap().api_methods.retain(|m| m != "tab.list");
    core.create_tab(&gateway, &target("w1:p1"))
        .expect("the action itself succeeded");
    let error = core
        .take_tabs_error()
        .expect("the refusal of the refresh is kept");
    // The engine's own refusal reaches the caller (here the method is gone from this server).
    assert_eq!(error.code, "invalid_request:id_mismatch");
    assert!(core.take_tabs_error().is_none(), "reported once");
}

// Would catch: the window focused on a tab that is not in the list it shows (the connection
// confirmed another tab and no event announced it), or a refresh on every confirmation.
#[test]
fn an_unknown_confirmed_tab_focus_reconciles_the_list_once() {
    let host = host();
    let (mut core, gateway) = core(&host);
    core.apply_surface(two_panes(1)).expect("surface");
    core.refresh_tabs(&gateway).expect("tab list");
    let reads = |h: &Host| {
        calls(h)
            .iter()
            .filter(|c| matches!(c, Call::Api(m, _) if m == "tab.list"))
            .count()
    };
    let before = reads(&host);
    // A tab the list already has needs no read.
    let tabs = core.reconcile_tabs_for_focus(&gateway, Some("w1:t2"));
    assert_eq!(tabs.len(), 2);
    assert_eq!(reads(&host), before);
    // One the engine created meanwhile is read once, and then it is known.
    host.lock()
        .unwrap()
        .tabs
        .push(("w1:t7".into(), "nova".into(), true, 1, "idle".into()));
    let tabs = core.reconcile_tabs_for_focus(&gateway, Some("w1:t7"));
    assert!(tabs.iter().any(|t| t.tab_id == "w1:t7"));
    assert_eq!(reads(&host), before + 1);
    let tabs = core.reconcile_tabs_for_focus(&gateway, Some("w1:t7"));
    assert_eq!(tabs.len(), 3);
    assert_eq!(reads(&host), before + 1, "no read for a tab already listed");
}
