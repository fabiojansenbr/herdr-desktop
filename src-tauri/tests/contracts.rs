//! Contract tests for the 001 acceptance criteria (seam: src-tauri/tests).
//!
//! AC-001-02: fixture cells (á, 界, emoji, combining, cursor, coloured rows) through full
//! frame + patches produce the expected grid; stale/foreign patches are rejected.
//! AC-001-03: fatal bootstrap exits non-zero with the literal line and nothing else;
//! an absent server is recoverable, and a server closing the socket after attach yields a
//! retryable `Disconnected` (the executable-level proof of both lives in e2e_linux.rs).
//! Edge cases: boot/generation divergence invalidates the target with no Local fallback;
//! RuntimeError never serialises env/credentials; empty state spawns nothing.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use herdr_client::bootstrap::{
    bootstrap_from_env, BootstrapError, ENV_FAIL_BOOTSTRAP, ENV_SESSION, FATAL_LINE,
};
use herdr_client::protocol::wire::{
    CellData, CursorState, FrameData, PaneSurfaceFrame, PaneSurfacePane, PaneSurfacePatch,
    PaneSurfacePatchRow, SurfaceGraphicsScene, SurfaceRect,
};
use herdr_client::{
    ApplyOutcome, ConnectOptions, FileCapabilities, FileProvider, FileUri, FrameStore,
    LiveIdentity, LocalGateway, QualifiedTarget, RecoveryAction, RuntimeBinding, RuntimeError,
    RuntimeGateway, SessionName, SessionPaths, StaleReason, SurfaceGeometry, SurfaceState,
};

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fixtures")
        .join(name)
}

fn load_surface_fixture() -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(fixture_path("surface-cells-v1.json")).unwrap())
        .unwrap()
}

fn cell(v: &serde_json::Value) -> CellData {
    CellData {
        symbol: v["s"].as_str().unwrap().to_owned(),
        fg: v["fg"].as_u64().unwrap_or(0) as u32,
        bg: v["bg"].as_u64().unwrap_or(0) as u32,
        modifier: v["m"].as_u64().unwrap_or(0) as u16,
        skip: false,
        hyperlink: None,
    }
}

fn cursor(v: &serde_json::Value) -> Option<CursorState> {
    if v.is_null() {
        return None;
    }
    Some(CursorState {
        x: v["x"].as_u64().unwrap() as u16,
        y: v["y"].as_u64().unwrap() as u16,
        visible: v["visible"].as_bool().unwrap(),
        shape: v["shape"].as_u64().unwrap() as u8,
    })
}

fn rect(v: &serde_json::Value) -> SurfaceRect {
    let a: Vec<u16> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_u64().unwrap() as u16)
        .collect();
    SurfaceRect {
        x: a[0],
        y: a[1],
        width: a[2],
        height: a[3],
    }
}

fn fixture_pane(v: &serde_json::Value) -> PaneSurfacePane {
    PaneSurfacePane {
        pane_id: v["pane_id"].as_str().unwrap().to_owned(),
        content_revision: v["content_revision"].as_u64().unwrap(),
        rect: rect(&v["rect"]),
        inner_rect: rect(&v["inner_rect"]),
        scrollbar_rect: None,
        scroll: None,
        focused: true,
        mouse_reporting: false,
        sgr_pixel_mouse: false,
        alternate_screen_active: false,
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn fixture_full(fx: &serde_json::Value) -> PaneSurfaceFrame {
    let full = &fx["full"];
    let width = fx["width"].as_u64().unwrap() as u16;
    let height = fx["height"].as_u64().unwrap() as u16;
    let cells: Vec<CellData> = full["rows"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|row| row.as_array().unwrap().iter().map(cell))
        .collect();
    PaneSurfaceFrame {
        boot_id: fx["boot_id"].as_str().unwrap().to_owned(),
        projection_revision: full["projection_revision"].as_u64().unwrap(),
        surface_revision: full["surface_revision"].as_u64().unwrap(),
        frame: FrameData {
            cells,
            width,
            height,
            cursor: cursor(&full["cursor"]),
            hyperlinks: Vec::new(),
            graphics: Vec::new(),
        },
        panes: vec![fixture_pane(&full["pane"])],
        splits: Vec::new(),
        popup: None,
        graphics: SurfaceGraphicsScene::default(),
    }
}

fn fixture_patch(fx: &serde_json::Value, p: &serde_json::Value) -> PaneSurfacePatch {
    let boot_id = p["boot_id"]
        .as_str()
        .unwrap_or(fx["boot_id"].as_str().unwrap())
        .to_owned();
    PaneSurfacePatch {
        boot_id,
        projection_revision: p["projection_revision"].as_u64().unwrap(),
        base_surface_revision: p["base_surface_revision"].as_u64().unwrap(),
        surface_revision: p["surface_revision"].as_u64().unwrap(),
        rows: p["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| PaneSurfacePatchRow {
                x: r["x"].as_u64().unwrap() as u16,
                y: r["y"].as_u64().unwrap() as u16,
                cells: r["cells"].as_array().unwrap().iter().map(cell).collect(),
            })
            .collect(),
        panes: vec![fixture_pane(&fx["full"]["pane"])],
        cursor: cursor(&p["cursor"]),
    }
}

// AC-001-02 — would catch: wrong row-major indexing, wide-char continuation cells being
// dropped or duplicated, combining marks split, colours/modifiers not carried per cell,
// cursor not taken from the last patch, or patches applied out of order.
#[test]
fn full_frame_plus_patches_yield_expected_grid() {
    let fx = load_surface_fixture();
    let mut store = FrameStore::new();
    assert!(!store.input_allowed(), "empty store must not accept input");
    assert_eq!(store.apply_full(fixture_full(&fx)), Ok(false));
    assert!(store.input_allowed());
    assert_eq!(store.revision(), Some(10));

    for patch in fx["patches"].as_array().unwrap() {
        assert_eq!(
            store.apply_patch(fixture_patch(&fx, patch)),
            ApplyOutcome::Applied
        );
    }
    let expected = &fx["expected_after_patches"];
    assert_eq!(
        store.revision(),
        Some(expected["surface_revision"].as_u64().unwrap())
    );
    assert_eq!(store.cursor().cloned(), cursor(&expected["cursor"]));
    let rows: Vec<String> = expected["text_rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(store.text_rows(), rows);

    let surface = store.surface().unwrap();
    let width = usize::from(surface.frame.width);
    for (key, want) in expected["cells"].as_object().unwrap() {
        let (x, y) = key.split_once(',').unwrap();
        let idx = y.parse::<usize>().unwrap() * width + x.parse::<usize>().unwrap();
        let got = &surface.frame.cells[idx];
        assert_eq!(got.symbol, want["s"].as_str().unwrap(), "cell {key}");
        if let Some(fg) = want["fg"].as_u64() {
            assert_eq!(u64::from(got.fg), fg, "fg of {key}");
        }
        if let Some(bg) = want["bg"].as_u64() {
            assert_eq!(u64::from(got.bg), bg, "bg of {key}");
        }
        if let Some(m) = want["m"].as_u64() {
            assert_eq!(u64::from(got.modifier), m, "modifier of {key}");
        }
    }
    // The wide grapheme still spans two cells: symbol then empty continuation.
    assert_eq!(surface.frame.cells[3].symbol, "🦀");
    assert_eq!(surface.frame.cells[4].symbol, "");
    assert_eq!(store.stats().patches_applied, 2);
}

// AC-001-02 / edge "Indisponibilidade" — would catch: a stale patch being applied
// (revision gap), a foreign boot's patch being applied, or input staying enabled while stale.
#[test]
fn stale_or_foreign_patches_are_rejected_and_block_input_until_full_frame() {
    let fx = load_surface_fixture();
    let mut store = FrameStore::new();
    store.apply_full(fixture_full(&fx)).unwrap();
    for patch in fx["patches"].as_array().unwrap() {
        store.apply_patch(fixture_patch(&fx, patch));
    }
    let before = store.text_rows();

    let foreign = fixture_patch(&fx, &fx["foreign_boot_patch"]);
    assert_eq!(
        foreign.base_surface_revision, 12,
        "fixture: foreign patch chains on the current revision"
    );
    assert_eq!(
        store.apply_patch(foreign),
        ApplyOutcome::Rejected(StaleReason::BootChanged)
    );
    assert_eq!(store.state(), SurfaceState::Stale(StaleReason::BootChanged));
    assert!(!store.input_allowed(), "no input on a stale surface");
    assert_eq!(store.recovery(), Some(RecoveryAction::RequestFullSurface));
    assert_eq!(
        store.text_rows(),
        before,
        "rejected patch must not touch the grid"
    );

    // Recovery is a full frame; a gap patch afterwards is rejected too.
    let mut recovered = fixture_full(&fx);
    recovered.surface_revision = 20;
    assert_eq!(store.apply_full(recovered), Ok(false));
    assert!(store.input_allowed());
    let gap = fixture_patch(&fx, &fx["stale_patch"]);
    assert_eq!(
        store.apply_patch(gap),
        ApplyOutcome::Rejected(StaleReason::RevisionGap)
    );
    assert_eq!(store.revision(), Some(20));
    assert!(!store.input_allowed());
    assert_eq!(store.stats().patches_rejected, 2);
}

// Edge "Identidade" — would catch: an action proceeding against a different boot or a stale
// connection generation, or an error that names Local as a fallback.
#[test]
fn qualified_target_rejects_boot_or_generation_divergence_without_local_fallback() {
    let live = LiveIdentity {
        endpoint: "ssh:build-box".into(),
        session: "work".into(),
        connection_generation: 3,
        boot_id: "boot-1111".into(),
    };
    let target = QualifiedTarget::new(&live, Some("w1".into()), "w1:p1");
    assert_eq!(target.validate(&live), Ok(()));

    let rebooted = LiveIdentity {
        boot_id: "boot-2222".into(),
        ..live.clone()
    };
    let err = target.validate(&rebooted).unwrap_err();
    assert_eq!(err.code, "target_boot_stale");
    assert_eq!(err.endpoint.as_deref(), Some("ssh:build-box"));
    assert!(
        !err.message.to_lowercase().contains("local"),
        "no silent fallback to Local: {}",
        err.message
    );

    let regenerated = LiveIdentity {
        connection_generation: 4,
        ..live.clone()
    };
    assert_eq!(
        target.validate(&regenerated).unwrap_err().code,
        "target_generation_stale"
    );

    let other_endpoint = LiveIdentity {
        endpoint: "local".into(),
        ..live.clone()
    };
    assert_eq!(
        target.validate(&other_endpoint).unwrap_err().code,
        "target_endpoint_mismatch"
    );

    let binding = RuntimeBinding {
        project_id: "p".into(),
        connection_generation: 3,
        boot_id: "boot-1111".into(),
        workspace_id: "w1".into(),
    };
    assert!(binding.is_valid_for(&live));
    assert!(!binding.is_valid_for(&rebooted));
    assert!(!binding.is_valid_for(&regenerated));
}

// CONTRATOS "RuntimeError" — would catch: env values, credentials or OS error text leaking
// into the serialised error.
#[test]
fn runtime_error_serialises_only_code_message_retryable_endpoint() {
    let err = RuntimeError::from_io_kind(
        std::io::ErrorKind::ConnectionRefused,
        "the Herdr server is unavailable",
    )
    .with_endpoint("local");
    let json = serde_json::to_value(&err).unwrap();
    let keys: Vec<&String> = json.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["code", "endpoint", "message", "retryable"]);
    assert_eq!(json["code"], "server_unavailable");
    assert_eq!(json["retryable"], true);
    assert!(
        !json["message"].as_str().unwrap().contains('/'),
        "no paths in message"
    );

    let denied = RuntimeError::from_io_kind(std::io::ErrorKind::PermissionDenied, "socket");
    assert_eq!(denied.code, "permission_denied");
    assert!(!denied.retryable);
}

// AC-001-03 (fatal) — would catch: exit code 0, missing literal line, a panic/backtrace
// instead of the line, or an env value/secret echoed to stderr/stdout.
#[test]
fn injected_fatal_bootstrap_exits_nonzero_with_single_line_and_no_secret() {
    let bin = env!("CARGO_BIN_EXE_herdr-desktop");
    let secret = "s3cr3t-token-do-not-print-9f8e7d";
    let output = Command::new(bin)
        // Spec 067 (AC-067-04): every launch of the real binary in the harnesses fixes the
        // window's language, so nothing it prints or shows follows the host's `LANG`.
        .env("HERDR_DESKTOP_LOCALE", "pt")
        .env(ENV_FAIL_BOOTSTRAP, "1")
        .env("HERDR_DESKTOP_TEST_SECRET", secret)
        .env("SSH_AUTH_SOCK", "/tmp/should-not-appear.sock")
        .output()
        .expect("spawn herdr-desktop");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(2), "stderr: {stderr}");
    assert!(
        stderr.lines().any(|l| l.starts_with(FATAL_LINE)),
        "stderr: {stderr}"
    );
    for haystack in [&stderr, &stdout] {
        assert!(!haystack.contains(secret));
        assert!(!haystack.contains("should-not-appear"));
        assert!(!haystack.contains("panicked"));
        assert!(!haystack.contains("backtrace"));
        assert!(!haystack.contains("RUST_BACKTRACE"));
    }
    assert_eq!(stderr.lines().count(), 1, "exactly one line: {stderr}");
}

// AC-001-03 (fatal, invalid session) — would catch: the invalid value being echoed, or
// an invalid session name being accepted (which would later address the wrong socket).
// The `default` name is not invalid anymore (spec 016): it resolves the engine's default
// session without ever launching a window here (the config is read from the library).
#[test]
fn invalid_session_name_is_fatal_without_echoing_the_value() {
    let bin = env!("CARGO_BIN_EXE_herdr-desktop");
    let output = Command::new(bin)
        .env("HERDR_DESKTOP_LOCALE", "pt")
        .env(ENV_SESSION, "../../etc/passwd")
        .output()
        .expect("spawn herdr-desktop");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr.starts_with(FATAL_LINE), "{stderr}");
    assert!(stderr.contains("invalid_session_name"));
    assert!(!stderr.contains("passwd"));

    // Zero-config (spec 016): an absent override resolves the `default` session and enables
    // the automatic start; the explicit `default` value is a valid override too.
    let empty = |_: &str| -> Option<String> { None };
    let config = bootstrap_from_env(&empty).unwrap();
    assert_eq!(
        config.session.as_ref().map(SessionName::as_str),
        Some("default")
    );
    assert!(config.auto_start);
    let explicit = |key: &str| -> Option<String> {
        match key {
            "HERDR_DESKTOP_SESSION" => Some("default".into()),
            _ => None,
        }
    };
    let named = bootstrap_from_env(&explicit).unwrap();
    assert_eq!(
        named.session.as_ref().map(SessionName::as_str),
        Some("default")
    );
    assert!(
        !named.auto_start,
        "an explicit override keeps the manual start"
    );

    let env = |key: &str| -> Option<String> {
        match key {
            "HERDR_DESKTOP_SESSION" => Some("bad name".into()),
            _ => None,
        }
    };
    assert_eq!(
        bootstrap_from_env(&env),
        Err(BootstrapError::InvalidSession {
            code: "invalid_session_name".into()
        })
    );
}

// AC-001-03 (recoverable) + edge "Estado vazio" — would catch: connect() panicking or
// returning a non-retryable error when the socket is absent, or a stale socket file being
// treated as a live server.
#[test]
fn absent_server_is_recoverable_and_no_process_is_spawned() {
    let dir = tempfile::tempdir().unwrap();
    let session = SessionName::parse("hd001-contract").unwrap();
    let paths = SessionPaths::for_session(dir.path(), &session);
    assert!(paths
        .client_socket
        .ends_with("sessions/hd001-contract/herdr-client.sock"));
    assert!(!herdr_client::bootstrap::session_available(&paths));

    let mut gateway = LocalGateway::new(dir.path(), session.clone());
    let err = gateway
        .connect(ConnectOptions {
            geometry: SurfaceGeometry {
                cols: 80,
                rows: 24,
                cell_width_px: 8,
                cell_height_px: 16,
            },
            surface_active: true,
        })
        .unwrap_err();
    assert_eq!(err.code, "server_unavailable");
    assert!(err.retryable, "absent server must be recoverable");
    assert_eq!(err.endpoint.as_deref(), Some("local"));
    assert!(!gateway.is_connected());
    assert!(gateway.identity().is_none());

    // A stale socket file (nobody listening) is still "unavailable", not a hang.
    std::fs::create_dir_all(&paths.data_dir).unwrap();
    #[cfg(unix)]
    {
        let _listener = std::os::unix::net::UnixListener::bind(&paths.client_socket).unwrap();
        drop(_listener);
        assert!(!herdr_client::bootstrap::session_available(&paths));
        let err = gateway
            .connect(ConnectOptions {
                geometry: SurfaceGeometry {
                    cols: 80,
                    rows: 24,
                    cell_width_px: 8,
                    cell_height_px: 16,
                },
                surface_active: true,
            })
            .unwrap_err();
        assert!(err.retryable);
    }

    // Zero-config (spec 016): no override resolves the `default` session and enables the
    // automatic start; the gateway itself spawns nothing (the connector owns the start) and
    // the default session's client socket is the config dir's own, exactly like the CLI.
    let env = |_: &str| -> Option<String> { None };
    let config = bootstrap_from_env(&env).unwrap();
    let default_session = config.session.clone().unwrap();
    assert_eq!(default_session.as_str(), "default");
    assert!(config.auto_start);
    let default_paths = SessionPaths::for_session(dir.path(), &default_session);
    assert_eq!(
        default_paths.client_socket,
        dir.path().join("herdr-client.sock")
    );
    assert!(!herdr_client::bootstrap::session_available(&default_paths));
    assert!(
        !herdr_client::bootstrap::wait_for_session(&default_paths, Duration::from_millis(50))
            .is_ok(),
        "nothing was started for the absent default session"
    );
}

// CONTRATOS "FileProvider" — would catch: a remote provider announcing write, or the default
// write path succeeding, or URIs losing their provider/host qualification.
#[test]
fn file_provider_contract_remote_is_read_only_and_uris_are_qualified() {
    struct RemoteStub;
    impl FileProvider for RemoteStub {
        fn provider_id(&self) -> &str {
            "sftp"
        }
        fn capabilities(&self) -> FileCapabilities {
            FileCapabilities {
                list: true,
                read: true,
                stat: true,
                write: false,
            }
        }
        fn list(&self, _dir: &FileUri) -> Result<Vec<herdr_client::FileEntry>, RuntimeError> {
            Ok(Vec::new())
        }
        fn read(&self, _file: &FileUri, _max: u64) -> Result<Vec<u8>, RuntimeError> {
            Ok(Vec::new())
        }
        fn stat(&self, uri: &FileUri) -> Result<herdr_client::FileStat, RuntimeError> {
            Ok(herdr_client::FileStat {
                uri: uri.clone(),
                kind: herdr_client::FileKind::File,
                size: 0,
                modified_unix_ms: None,
                read_only: true,
            })
        }
    }
    let provider = RemoteStub;
    assert!(!provider.capabilities().write);
    let uri = FileUri::remote("sftp", "build-box", "/srv/app/main.rs");
    assert!(uri.is_remote());
    assert_eq!(uri.to_string(), "sftp://build-box/srv/app/main.rs");
    let err = provider.write(&uri, b"x").unwrap_err();
    assert_eq!(err.code, "write_unsupported");
    assert_eq!(err.endpoint.as_deref(), Some("build-box"));
    let local = FileUri::local("C:\\repo\\a.txt");
    assert!(!local.is_remote());
    assert_eq!(
        local.path, "C:\\repo\\a.txt",
        "paths are opaque, never normalised"
    );
}

// AC-001-03 (recoverable, connection lost after attach) — would catch: the reader ending
// silently on EOF or on a reset (no `Disconnected` event), reporting the loss as
// non-retryable or under a fatal/`io_error` code, a second spurious event after the loss,
// later input failing non-retryably, the reconnect attempt hanging or becoming fatal, or a
// path leaking into the message. The server side is a socket this test owns and closes
// deliberately, in both shapes Linux produces: clean EOF (server drained the input first)
// and ECONNRESET (server closed with the client's input still unread).
#[cfg(unix)]
#[test]
fn server_closing_the_socket_after_attach_yields_recoverable_disconnect() {
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc::RecvTimeoutError;

    use herdr_client::protocol::endpoint::{
        EndpointClientHello, EndpointServerWelcome, ENDPOINT_HELLO_KIND, ENDPOINT_WELCOME_KIND,
    };
    use herdr_client::protocol::wire::{ClientMessage, ClientPaneInputEvent, ServerMessage};
    use herdr_client::protocol::{decode_message, read_frame, write_message, MAX_FRAME_SIZE};
    use herdr_client::GatewayEvent;

    let dir = tempfile::tempdir().unwrap();
    let fx = load_surface_fixture();
    for (session_name, drain_input_before_close) in
        [("hd001-loss-eof", true), ("hd001-loss-reset", false)]
    {
        let session = SessionName::parse(session_name).unwrap();
        let paths = SessionPaths::for_session(dir.path(), &session);
        std::fs::create_dir_all(&paths.data_dir).unwrap();
        // No availability probe here: a probe connection would be the one the server accepts.
        let listener = UnixListener::bind(&paths.client_socket).unwrap();
        assert!(paths.client_socket.exists());

        let surface = fixture_full(&fx);
        let boot_id = surface.boot_id.clone();
        let (close_tx, close_rx) = std::sync::mpsc::channel::<()>();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let hello: ClientMessage =
                decode_message(&read_frame(&mut stream, MAX_FRAME_SIZE).unwrap()).unwrap();
            let ClientMessage::EndpointControl { kind, data } = hello else {
                panic!("expected the endpoint hello first, got {hello:?}");
            };
            assert_eq!(kind, ENDPOINT_HELLO_KIND);
            let hello: EndpointClientHello = serde_json::from_str(&data).unwrap();
            assert_eq!(hello.generation, 1);
            let welcome: EndpointServerWelcome = serde_json::from_str(
                &std::fs::read_to_string(fixture_path("endpoint-welcome-v1.json")).unwrap(),
            )
            .unwrap();
            write_message(
                &mut stream,
                &ServerMessage::EndpointControl {
                    kind: ENDPOINT_WELCOME_KIND.into(),
                    data: serde_json::to_string(&welcome).unwrap(),
                },
            )
            .unwrap();
            write_message(&mut stream, &ServerMessage::PaneSurface(surface)).unwrap();
            close_rx.recv().unwrap();
            if drain_input_before_close {
                // Input sent while attached really reached the server; draining it makes
                // the close a clean EOF for the client.
                let input: ClientMessage =
                    decode_message(&read_frame(&mut stream, MAX_FRAME_SIZE).unwrap()).unwrap();
                let ClientMessage::ClientShellPaneInput { pane_id, events } = input else {
                    panic!("expected pane input, got {input:?}");
                };
                assert_eq!(pane_id, "w1:p1");
                assert_eq!(
                    events,
                    vec![ClientPaneInputEvent::TextCommit("before".into())]
                );
            }
            // The server goes away abruptly: no ServerShutdown, just EOF/reset on the socket.
            drop(stream);
        });

        let geometry = SurfaceGeometry {
            cols: 80,
            rows: 24,
            cell_width_px: 8,
            cell_height_px: 16,
        };
        let mut gateway = LocalGateway::new(dir.path(), session);
        let negotiated = gateway
            .connect(ConnectOptions {
                geometry,
                surface_active: true,
            })
            .expect("generation-1 negotiation against the test server");
        assert_eq!(negotiated.generation, 1);
        assert!(gateway.is_connected());
        let events = gateway.take_event_stream().unwrap();
        let mut store = FrameStore::new();
        match events.recv_timeout(Duration::from_secs(5)).unwrap() {
            GatewayEvent::Surface(surface) => store.apply_full(*surface).unwrap(),
            other => panic!("expected the full surface first, got {other:?}"),
        };
        assert!(store.input_allowed(), "attached and live");
        let identity = gateway.identity().expect("boot id learnt from the surface");
        assert_eq!(identity.boot_id, boot_id);
        let target = QualifiedTarget::new(&identity, None, "w1:p1");
        gateway
            .send_input(
                &target,
                vec![ClientPaneInputEvent::TextCommit("before".into())],
            )
            .expect("input flows while attached");

        // --- server side closes -----------------------------------------------------------
        close_tx.send(()).unwrap();
        server.join().unwrap();
        let lost = match events.recv_timeout(Duration::from_secs(5)) {
            Ok(GatewayEvent::Disconnected(error)) => error,
            other => panic!(
                "[{session_name}] expected Disconnected after the server closed, got {other:?}"
            ),
        };
        assert_eq!(lost.code, "connection_lost", "[{session_name}] {lost}");
        assert!(
            lost.retryable,
            "[{session_name}] loss after attach must be recoverable: {lost}"
        );
        assert_eq!(lost.endpoint.as_deref(), Some("local"));
        assert!(
            !lost.message.contains(dir.path().to_str().unwrap()) && !lost.message.contains('/'),
            "no path in the message: {}",
            lost.message
        );
        // The reader ends after reporting the loss: exactly one terminal event, no echo.
        assert_eq!(
            events.recv_timeout(Duration::from_millis(300)).unwrap_err(),
            RecvTimeoutError::Disconnected
        );

        // Input after the loss fails recoverably (never a panic, never fatal)...
        let refused = gateway
            .send_input(
                &target,
                vec![ClientPaneInputEvent::TextCommit("after".into())],
            )
            .unwrap_err();
        assert!(refused.retryable, "[{session_name}] {refused}");
        // ...and a retry is a plain recoverable failure: nobody listens, nothing is spawned.
        assert!(!herdr_client::bootstrap::session_available(&paths));
        let retry = gateway
            .connect(ConnectOptions {
                geometry,
                surface_active: true,
            })
            .unwrap_err();
        assert_eq!(retry.code, "server_unavailable");
        assert!(retry.retryable);
        assert!(!gateway.is_connected());
        assert!(gateway.identity().is_none());
    }
}
