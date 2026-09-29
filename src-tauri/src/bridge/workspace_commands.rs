//! Engine workspace actions of the composed window (spec 025, AC-025-01/03).
//!
//! The WebView acts on the workspaces it already lists from the host snapshot with exactly one
//! command per user intent: `workspace_focus` (switching the center), `workspace_create`
//! (`Abrir projeto…` and a closed project), `workspace_rename` and `workspace_close` (after the
//! inline confirmation). Every command validates endpoint, session, connection generation, boot
//! and the workspace against the live connection of that host; an unknown or offline endpoint is
//! refused unsent, never retargeted to Local. Nothing here creates a tab or splits a pane.
//!
//! Spec 077 adds `host_tab_close` here, not to the agents module: the bar lists a host's tabs from
//! the hub snapshot whenever the agents connection carries none (spec 074), so its × has to reach
//! the same hub lane `workspace_close` reaches. It closes a tab the host already lists; nothing
//! here creates one.
//!
//! `workspace_focus` rides the endpoint lane with the workspace as its explicit target, waiting
//! (without sending) for a workspace the engine just created to reach the host snapshot, exactly
//! like a project open. `workspace_create` has no existing target, so it rides the host's JSON
//! API lane: the connection's identity is checked and recorded at the send.

use std::path::Path;
use std::time::Instant;

use herdr_client::{LiveIdentity, RuntimeError};
use serde::Serialize;
use serde_json::{json, Value};

use super::project_hosts::FOCUS_SNAPSHOT_WAIT;
use crate::connections::commands::ConnectionsState;
use crate::connections::hub::{EndpointScope, HostHub};

/// Commands this module exposes to the WebView (registered by the window composition).
pub const COMMANDS: &[&str] = &[
    "workspace_focus",
    "workspace_create",
    "workspace_rename",
    "workspace_close",
    "host_tab_close",
];

const MAX_LABEL_CHARS: usize = 120;
const MAX_TAB_ID_CHARS: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceCreated {
    pub workspace_id: String,
}

fn invalid(code: &str, message: &str, endpoint: &str) -> RuntimeError {
    RuntimeError::new(code, message).with_endpoint(endpoint)
}

fn validate_endpoint(endpoint: &str) -> Result<(), RuntimeError> {
    if endpoint.is_empty() || endpoint.chars().any(char::is_control) {
        return Err(invalid(
            "invalid_endpoint",
            "invalid endpoint; nothing was sent",
            endpoint,
        ));
    }
    Ok(())
}

fn validate_workspace_id(endpoint: &str, workspace_id: &str) -> Result<(), RuntimeError> {
    if workspace_id.is_empty() || workspace_id.chars().any(char::is_control) {
        return Err(invalid(
            "invalid_workspace_id",
            "invalid workspace; nothing was sent",
            endpoint,
        ));
    }
    Ok(())
}

fn validate_label(endpoint: &str, label: &str) -> Result<(), RuntimeError> {
    if label.is_empty()
        || label.chars().count() > MAX_LABEL_CHARS
        || label.chars().any(char::is_control)
    {
        return Err(invalid(
            "invalid_workspace_label",
            "the workspace name must be 1 to 120 characters long",
            endpoint,
        ));
    }
    Ok(())
}

/// Engine tab id (`w1:t1`, `t_w1_1`): an opaque token of at most 128 characters, made only of the
/// characters the engine mints ids from (`../herdr/src/app/ids.rs:parse_tab_id`). Anything else is
/// refused here, before any connection is touched.
fn validate_tab_id(endpoint: &str, tab_id: &str) -> Result<(), RuntimeError> {
    let shaped = !tab_id.is_empty()
        && tab_id.chars().count() <= MAX_TAB_ID_CHARS
        && tab_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '-' | '.'));
    if !shaped {
        return Err(invalid(
            "invalid_input",
            "invalid tab; nothing was sent",
            endpoint,
        ));
    }
    Ok(())
}

fn validate_cwd(endpoint: &str, cwd: &str) -> Result<(), RuntimeError> {
    if !Path::new(cwd).is_absolute() || cwd.chars().any(char::is_control) {
        return Err(invalid(
            "invalid_workspace_cwd",
            "the workspace cwd must be an absolute path on the endpoint's machine",
            endpoint,
        ));
    }
    Ok(())
}

/// Runs one explicit-target workspace command, waiting out `workspace_not_in_snapshot` for a
/// workspace the engine just created (nothing is sent while waiting, nothing is retried).
fn run_scoped(
    hub: &HostHub,
    identity: &LiveIdentity,
    scope: &EndpointScope,
    method: &str,
    params: Value,
) -> Result<Value, RuntimeError> {
    let deadline = Instant::now() + FOCUS_SNAPSHOT_WAIT;
    loop {
        let revision = hub.revision();
        match hub.run_endpoint_scoped(identity, scope, method, params.clone()) {
            Err(error) if error.code == "workspace_not_in_snapshot" => {
                let now = Instant::now();
                if now >= deadline {
                    return Err(error);
                }
                hub.wait_changed(revision, deadline - now);
            }
            result => return result,
        }
    }
}

/// `workspace.focus`: exactly one call on the workspace's own connection (AC-025-03).
pub fn focus(
    connections: &ConnectionsState,
    endpoint: &str,
    workspace_id: &str,
) -> Result<(), RuntimeError> {
    validate_endpoint(endpoint)?;
    validate_workspace_id(endpoint, workspace_id)?;
    let hub = connections.hub();
    let identity = hub.live_identity(endpoint)?;
    let scope = EndpointScope::Workspace(workspace_id.to_owned());
    run_scoped(
        hub,
        &identity,
        &scope,
        "workspace.focus",
        json!({ "workspace_id": workspace_id }),
    )
    .map(|_| ())
}

/// `workspace.create`: a new workspace with `cwd`, focused when `focus`. The identity of the
/// host's current connection is validated and recorded at the send (no Local fallback).
pub fn create(
    connections: &ConnectionsState,
    endpoint: &str,
    cwd: &str,
    label: &str,
    focus: bool,
) -> Result<WorkspaceCreated, RuntimeError> {
    validate_endpoint(endpoint)?;
    validate_cwd(endpoint, cwd)?;
    validate_label(endpoint, label)?;
    let value = connections.hub().run_action(
        endpoint,
        "workspace.create",
        json!({ "cwd": cwd, "label": label, "focus": focus }),
    )?;
    let workspace_id = value
        .pointer("/workspace/workspace_id")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            invalid(
                "protocol_error",
                "unexpected response: workspace.create without workspace_id",
                endpoint,
            )
        })?
        .to_owned();
    Ok(WorkspaceCreated { workspace_id })
}

pub fn rename(
    connections: &ConnectionsState,
    endpoint: &str,
    workspace_id: &str,
    label: &str,
) -> Result<(), RuntimeError> {
    validate_endpoint(endpoint)?;
    validate_workspace_id(endpoint, workspace_id)?;
    validate_label(endpoint, label)?;
    let hub = connections.hub();
    let identity = hub.live_identity(endpoint)?;
    let scope = EndpointScope::Workspace(workspace_id.to_owned());
    run_scoped(
        hub,
        &identity,
        &scope,
        "workspace.rename",
        json!({ "workspace_id": workspace_id, "label": label }),
    )
    .map(|_| ())
}

pub fn close(
    connections: &ConnectionsState,
    endpoint: &str,
    workspace_id: &str,
) -> Result<(), RuntimeError> {
    validate_endpoint(endpoint)?;
    validate_workspace_id(endpoint, workspace_id)?;
    let hub = connections.hub();
    let identity = hub.live_identity(endpoint)?;
    let scope = EndpointScope::Workspace(workspace_id.to_owned());
    run_scoped(
        hub,
        &identity,
        &scope,
        "workspace.close",
        json!({ "workspace_id": workspace_id }),
    )
    .map(|_| ())
}

/// `tab.close` on the host's own connection (spec 077, AC-077-01). The top bar lists a host's tabs
/// from the hub snapshot whenever the agents connection carries none (spec 074), and the × must
/// then close the tab through the same hub lane `close` uses — the agents connection would answer
/// nothing. Endpoint and tab are validated first; an offline or unknown host gets the error of
/// `live_identity`, never Local.
pub fn tab_close(
    connections: &ConnectionsState,
    endpoint: &str,
    tab_id: &str,
) -> Result<(), RuntimeError> {
    validate_endpoint(endpoint)?;
    validate_tab_id(endpoint, tab_id)?;
    let hub = connections.hub();
    let identity = hub.live_identity(endpoint)?;
    let scope = EndpointScope::Tab(tab_id.to_owned());
    run_scoped(
        hub,
        &identity,
        &scope,
        "tab.close",
        json!({ "tab_id": tab_id }),
    )
    .map(|_| ())
}

async fn blocking<T: Send + 'static>(
    run: impl FnOnce() -> Result<T, RuntimeError> + Send + 'static,
) -> Result<T, RuntimeError> {
    tauri::async_runtime::spawn_blocking(run)
        .await
        .map_err(|_| {
            RuntimeError::new(
                "workspace_command_interrupted",
                "the action ended without a confirmed result; check the state before repeating it",
            )
        })?
}

#[tauri::command]
pub async fn workspace_focus(
    state: tauri::State<'_, ConnectionsState>,
    endpoint: String,
    workspace_id: String,
) -> Result<(), RuntimeError> {
    let connections = state.inner().clone();
    blocking(move || focus(&connections, &endpoint, &workspace_id)).await
}

#[tauri::command]
pub async fn workspace_create(
    state: tauri::State<'_, ConnectionsState>,
    endpoint_profile_id: String,
    cwd: String,
    label: String,
    focus: bool,
) -> Result<WorkspaceCreated, RuntimeError> {
    let connections = state.inner().clone();
    blocking(move || create(&connections, &endpoint_profile_id, &cwd, &label, focus)).await
}

#[tauri::command]
pub async fn workspace_rename(
    state: tauri::State<'_, ConnectionsState>,
    endpoint: String,
    workspace_id: String,
    label: String,
) -> Result<(), RuntimeError> {
    let connections = state.inner().clone();
    blocking(move || rename(&connections, &endpoint, &workspace_id, &label)).await
}

#[tauri::command]
pub async fn workspace_close(
    state: tauri::State<'_, ConnectionsState>,
    endpoint: String,
    workspace_id: String,
) -> Result<(), RuntimeError> {
    let connections = state.inner().clone();
    blocking(move || close(&connections, &endpoint, &workspace_id)).await
}

#[tauri::command]
pub async fn host_tab_close(
    state: tauri::State<'_, ConnectionsState>,
    endpoint: String,
    tab_id: String,
) -> Result<(), RuntimeError> {
    let connections = state.inner().clone();
    blocking(move || tab_close(&connections, &endpoint, &tab_id)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connections::commands::ConnectionsConfig;
    use crate::connections::hub::{ApiLane, Connected, EndpointLane, HostKind, HostSpec};
    use herdr_client::protocol::wire::{ClientPaneInputEvent, ClientShellSnapshot};
    use herdr_client::{
        ConnectOptions, GatewayEvent, Negotiated, QualifiedTarget, RuntimeGateway, SurfaceGeometry,
    };
    use std::path::PathBuf;
    use std::sync::mpsc::Receiver;
    use std::sync::{Arc, Mutex};

    fn offline() -> ConnectionsState {
        ConnectionsState::new(ConnectionsConfig {
            prefs_dir: PathBuf::from("/tmp/herdr-desktop-test-prefs"),
            herdr_config_dir: PathBuf::from("/tmp/herdr-desktop-test-config"),
            herdr_state_dir: PathBuf::from("/tmp/herdr-desktop-test-state"),
            local_session: None,
            local_auto_start: false,
            herdr_bin: PathBuf::from("herdr"),
            isolated_ssh: None,
            geometry: SurfaceGeometry {
                cols: 80,
                rows: 24,
                cell_width_px: 9,
                cell_height_px: 18,
            },
        })
    }

    #[test]
    fn unknown_or_offline_endpoint_is_refused_without_sending() {
        let connections = offline();
        for error in [
            focus(&connections, "ssh-dev", "w1").unwrap_err(),
            close(&connections, "ssh-dev", "w1").unwrap_err(),
            rename(&connections, "ssh-dev", "w1", "novo").unwrap_err(),
            create(&connections, "ssh-dev", "/srv/a", "a", true).unwrap_err(),
        ] {
            assert_eq!(error.code, "endpoint_unknown");
            assert_eq!(error.endpoint.as_deref(), Some("ssh-dev"));
        }
    }

    #[test]
    fn malformed_arguments_are_refused_before_any_connection_is_touched() {
        let connections = offline();
        assert_eq!(
            focus(&connections, "local", "").unwrap_err().code,
            "invalid_workspace_id"
        );
        assert_eq!(
            create(&connections, "local", "relativo", "a", true)
                .unwrap_err()
                .code,
            "invalid_workspace_cwd"
        );
        assert_eq!(
            create(&connections, "local", "/srv/a", "", true)
                .unwrap_err()
                .code,
            "invalid_workspace_label"
        );
        assert_eq!(
            rename(&connections, "local", "w1", "x".repeat(121).as_str())
                .unwrap_err()
                .code,
            "invalid_workspace_label"
        );
    }

    #[test]
    fn no_workspace_command_creates_a_tab_or_splits_a_pane() {
        for command in COMMANDS {
            assert!(!command.contains("pane") && !command.contains("split"));
            // Spec 077: `host_tab_close` closes a tab the host already lists; no command here
            // creates one (`tab.create` stays on the agents connection).
            assert!(!command.contains("tab") || *command == "host_tab_close");
        }
    }

    // ---------------------------------------------------------------------------------------
    // Spec 077 (AC-077-01) — `host_tab_close` on a fake SSH connection of the hub.
    // ---------------------------------------------------------------------------------------

    /// Every endpoint command the fake connection received, with its parameters.
    #[derive(Default)]
    struct Wire {
        endpoint: Vec<(String, Value)>,
    }

    struct FakeGateway {
        endpoint: String,
        session: String,
        boot: String,
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
        fn connect(&mut self, _: ConnectOptions) -> Result<Negotiated, RuntimeError> {
            Err(not_in_fake())
        }
        fn take_events(&mut self) -> Option<Receiver<GatewayEvent>> {
            None
        }
        fn api_request(&self, _: &str, _: Value) -> Result<Value, RuntimeError> {
            Err(not_in_fake())
        }
        fn endpoint_request(&self, _: &str, _: Value) -> Result<Value, RuntimeError> {
            Err(not_in_fake())
        }
        fn send_input(
            &self,
            _: &QualifiedTarget,
            _: Vec<ClientPaneInputEvent>,
        ) -> Result<(), RuntimeError> {
            Ok(())
        }
        fn resize(&self, _: SurfaceGeometry) -> Result<(), RuntimeError> {
            Ok(())
        }
        fn set_focus(&self, _: bool) -> Result<(), RuntimeError> {
            Ok(())
        }
        fn detach(&mut self) {}
        fn is_connected(&self) -> bool {
            true
        }
    }

    struct FakeApi;
    impl ApiLane for FakeApi {
        fn request(&self, _: &str, _: Value) -> Result<Value, RuntimeError> {
            Err(not_in_fake())
        }
    }

    struct FakeLane(Arc<Mutex<Wire>>);
    impl EndpointLane for FakeLane {
        fn request(&self, _: &str, method: &str, params: Value) -> Result<Value, RuntimeError> {
            self.0
                .lock()
                .unwrap()
                .endpoint
                .push((method.to_owned(), params));
            Ok(json!({ "type": "ok" }))
        }
        fn methods(&self) -> Vec<String> {
            ["tab.close", "workspace.close"]
                .iter()
                .map(|m| (*m).to_owned())
                .collect()
        }
    }

    const SSH: &str = "ssh-mac-mini";
    const BOOT: &str = "boot-077";

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
        snap.revision = 1;
        snap
    }

    /// An SSH host of the hub, online on `BOOT`, whose snapshot lists the tab `w1:t1` (exactly
    /// the shape the bar reads from `HostDto.tabs` when the agents connection carries none).
    fn connected_ssh() -> (ConnectionsState, Arc<Mutex<Wire>>) {
        let connections = offline();
        let hub = connections.hub();
        hub.add_host(HostSpec {
            endpoint: SSH.into(),
            label: "mac-mini".into(),
            kind: HostKind::Ssh,
            session: "hd077-remote".into(),
            target: Some("user@mac-mini".into()),
            visible: true,
        })
        .unwrap();
        let now = Instant::now();
        let ticket = hub.request_connect(SSH, now).unwrap().expect("ticket");
        hub.finish_connect(
            &ticket,
            Ok(Connected {
                gateway: Box::new(FakeGateway {
                    endpoint: SSH.into(),
                    session: "hd077-remote".into(),
                    boot: BOOT.into(),
                }),
                api: Arc::new(FakeApi),
            }),
            now,
        );
        let wire = Arc::new(Mutex::new(Wire::default()));
        assert!(hub.install_endpoint_lane(SSH, ticket.token, Arc::new(FakeLane(wire.clone()))));
        hub.apply_event(
            SSH,
            ticket.token,
            GatewayEvent::Snapshot(Box::new(snapshot(BOOT))),
            now,
        );
        (connections, wire)
    }

    // Would catch: the × of the bar routed to another method, to another host or without the
    // tab id the engine expects — the bug of spec 077 was no call at all.
    #[test]
    fn host_tab_close_sends_tab_close_with_the_tab_id_on_that_host() {
        let (connections, wire) = connected_ssh();
        tab_close(&connections, SSH, "w1:t1").unwrap();
        assert_eq!(
            wire.lock().unwrap().endpoint,
            vec![("tab.close".to_owned(), json!({ "tab_id": "w1:t1" }))]
        );
    }

    // Would catch: a tab the host no longer lists closed anyway (the snapshot guard dropped).
    #[test]
    fn a_tab_outside_the_host_snapshot_is_refused_without_sending() {
        let (connections, wire) = connected_ssh();
        let error = tab_close(&connections, SSH, "w1:t9").unwrap_err();
        assert_eq!(error.code, "tab_not_in_snapshot");
        assert_eq!(error.endpoint.as_deref(), Some(SSH));
        assert!(wire.lock().unwrap().endpoint.is_empty());
    }

    // Would catch: an offline host answered by the Local connection instead of its own error.
    #[test]
    fn closing_a_tab_of_an_offline_host_is_refused_without_a_local_fallback() {
        let connections = offline();
        let error = tab_close(&connections, "ssh-dev", "w1:t1").unwrap_err();
        assert_eq!(error.code, "endpoint_unknown");
        assert_eq!(error.endpoint.as_deref(), Some("ssh-dev"));
    }

    // Would catch: an unbounded or control-laden tab id reaching a connection.
    #[test]
    fn a_malformed_tab_id_is_refused_before_any_connection_is_touched() {
        let (connections, wire) = connected_ssh();
        for bad in ["", "w1:t1\n", "w1 t1", "a".repeat(129).as_str()] {
            let error = tab_close(&connections, SSH, bad).unwrap_err();
            assert_eq!(error.code, "invalid_input", "{bad:?}");
            assert_eq!(error.endpoint.as_deref(), Some(SSH), "{bad:?}");
        }
        assert!(wire.lock().unwrap().endpoint.is_empty());
    }

    // Would catch: the command declared in the module but missing from the literal registry.
    #[test]
    fn host_tab_close_is_declared_by_the_module() {
        assert!(COMMANDS.contains(&"host_tab_close"));
    }
}
