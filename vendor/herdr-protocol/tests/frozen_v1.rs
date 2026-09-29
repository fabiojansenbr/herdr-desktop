//! Byte-level compatibility of the mirrored generation-1 contract.
//!
//! Every digest below is copied verbatim from `src/protocol/wire.rs` tests in the
//! reference engine (commit 03749ae3970a74e077fc16b9327bddfc957771c9). If any of
//! them fails, the mirror diverged from the frozen codec: fix the mirror, never
//! the digest. Each test names the wrong behaviour it would catch.

use herdr_protocol::endpoint::*;
use herdr_protocol::wire::*;
use herdr_protocol::{decode_message, encode_message, read_message, write_message, MAX_FRAME_SIZE};
use sha2::{Digest, Sha256};

fn encoded_sha256(value: &impl serde::Serialize) -> String {
    format!("{:x}", Sha256::digest(encode_message(value).unwrap()))
}

fn fixture(name: &str) -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/");
    std::fs::read_to_string(format!("{path}{name}")).unwrap_or_else(|e| panic!("{name}: {e}"))
}

// Would catch: any field reordering / type change in ClientShellResize or ClientSurfaceSize.
#[test]
fn client_shell_resize_digest_is_frozen() {
    let msg = ClientMessage::ClientShellResize {
        cell_width_px: 8,
        cell_height_px: 16,
        surface_size: ClientSurfaceSize { cols: 74, rows: 29 },
        pixel_mouse: true,
    };
    assert_eq!(
        encoded_sha256(&msg),
        "676d6376202750e72c45ff511e256b6154d3d20c0ee088d3792fa3a69d9704b9"
    );
}

// Would catch: divergence in ClientPaneInputEvent::Key layout, ClientKeyCode variant order,
// Option encoding of shifted_codepoint/physical_key_id/windows_record, or WindowsKeyRecord.
#[test]
fn client_shell_pane_input_digest_is_frozen() {
    let windows_record = WindowsKeyRecord {
        key_down: true,
        repeat_count: 1,
        virtual_key_code: 0x37,
        virtual_scan_code: 0x08,
        unicode: 0,
        control_key_state: 0x0008,
    };
    let message = ClientMessage::ClientShellPaneInput {
        pane_id: "w1:p2".into(),
        events: vec![
            ClientPaneInputEvent::Key {
                code: ClientKeyCode::Char('l'),
                modifiers: key_modifiers::SHIFT,
                kind: ClientKeyKind::Release,
                repeat_count: 1,
                shifted_codepoint: Some('L' as u32),
                generated_text: None,
                tracks_release: true,
                physical_key_id: None,
                windows_record: None,
            },
            ClientPaneInputEvent::Key {
                code: ClientKeyCode::Char('7'),
                modifiers: key_modifiers::CONTROL,
                kind: ClientKeyKind::Press,
                repeat_count: 1,
                shifted_codepoint: None,
                generated_text: None,
                tracks_release: true,
                physical_key_id: Some(0x08),
                windows_record: Some(windows_record),
            },
        ],
    };
    let decoded: ClientMessage = decode_message(&encode_message(&message).unwrap()).unwrap();
    assert_eq!(decoded, message);
    assert_eq!(
        encoded_sha256(&message),
        "f558384bb53dfd2baf1fa72e1709d88be6891da79e51f88513905afc085065e6"
    );
}

// Would catch: ClientShellEndpointRequest / ClientShellEndpointResponseChunk field drift.
#[test]
fn endpoint_request_and_response_chunk_digests_are_frozen() {
    let request = ClientMessage::ClientShellEndpointRequest {
        boot_id: "boot-a".into(),
        request: r#"{"id":"request-a","method":"session.snapshot","params":{}}"#.into(),
    };
    assert_eq!(
        encoded_sha256(&request),
        "de5693585a01f6b0d5ee07c51b6ddf79ee9f67dbf183255822d31f35210f5ffb"
    );
    let response = ServerMessage::ClientShellEndpointResponseChunk {
        boot_id: "boot-a".into(),
        request_id: "request-a".into(),
        final_chunk: true,
        data: br#"{"id":"request-a","result":{"type":"ok"}}"#.to_vec(),
    };
    let decoded: ServerMessage = decode_message(&encode_message(&response).unwrap()).unwrap();
    assert_eq!(decoded, response);
    assert_eq!(
        encoded_sha256(&response),
        "bc14dbb5263d3097fe6d3e70a4b6d71aa9c2fa4ae3206d692a7182512bffdd1d"
    );
}

// Would catch: ClipboardImage / ClientClipboardImageTarget variant order drift.
#[test]
fn clipboard_image_digest_is_frozen() {
    let msg = ClientMessage::ClipboardImage {
        target: ClientClipboardImageTarget::Pane("w1:p1".into()),
        extension: "png".to_owned(),
        data: vec![0x89, b'P', b'N', b'G'],
    };
    assert_eq!(
        encoded_sha256(&msg),
        "1c02110be0671faf318b3f4d8f507748d5a0a98cd474832669c5290d97ef19ab"
    );
}

// Would catch: FrameData/CellData/CursorState layout drift, packed colour encoding,
// modifier bits, hyperlink index encoding, or PaneSurfaceFrame field order.
#[test]
fn pane_surface_digest_is_frozen() {
    let frame = FrameData {
        cells: vec![
            CellData {
                symbol: "H".into(),
                fg: WireColor::Red.to_u32(),
                bg: WireColor::Black.to_u32(),
                modifier: modifier::BOLD,
                skip: false,
                hyperlink: None,
            },
            CellData {
                symbol: "i".into(),
                fg: WireColor::Green.to_u32(),
                bg: WireColor::Reset.to_u32(),
                modifier: modifier::ITALIC,
                skip: false,
                hyperlink: None,
            },
            CellData {
                symbol: "!".into(),
                fg: WireColor::Rgb(255, 128, 0).to_u32(),
                bg: WireColor::Indexed(220).to_u32(),
                modifier: modifier::BOLD | modifier::UNDERLINED,
                skip: false,
                hyperlink: Some(0),
            },
            CellData {
                symbol: " ".into(),
                fg: WireColor::Reset.to_u32(),
                bg: WireColor::Reset.to_u32(),
                modifier: 0,
                skip: true,
                hyperlink: None,
            },
            CellData {
                symbol: "→".into(),
                fg: WireColor::Cyan.to_u32(),
                bg: WireColor::Blue.to_u32(),
                modifier: modifier::REVERSED,
                skip: false,
                hyperlink: None,
            },
            CellData {
                symbol: "🦀".into(),
                fg: WireColor::Yellow.to_u32(),
                bg: WireColor::Magenta.to_u32(),
                modifier: 0,
                skip: false,
                hyperlink: None,
            },
        ],
        width: 3,
        height: 2,
        cursor: Some(CursorState {
            x: 0,
            y: 0,
            visible: true,
            shape: 6,
        }),
        hyperlinks: vec!["https://example.com".to_owned()],
        graphics: Vec::new(),
    };
    let msg = ServerMessage::PaneSurface(PaneSurfaceFrame {
        boot_id: "boot-1".into(),
        projection_revision: 1,
        surface_revision: 1,
        frame,
        panes: Vec::new(),
        splits: Vec::new(),
        popup: None,
        graphics: SurfaceGraphicsScene::default(),
    });
    let decoded: ServerMessage = decode_message(&encode_message(&msg).unwrap()).unwrap();
    assert_eq!(decoded, msg);
    assert_eq!(
        encoded_sha256(&msg),
        "7c016f7b21ddb5ac79212cf65a968b93eb292b5305b941263e89ffaa40158ee3"
    );
}

// Would catch: PaneSurfacePatch / PaneSurfacePatchRow layout drift.
#[test]
fn pane_surface_patch_digest_is_frozen() {
    let msg = ServerMessage::PaneSurfacePatch(PaneSurfacePatch {
        boot_id: "boot-1".into(),
        projection_revision: 3,
        base_surface_revision: 7,
        surface_revision: 8,
        rows: vec![PaneSurfacePatchRow {
            x: 2,
            y: 4,
            cells: vec![CellData {
                symbol: "x".into(),
                fg: 1,
                bg: 2,
                modifier: 3,
                skip: false,
                hyperlink: None,
            }],
        }],
        panes: Vec::new(),
        cursor: Some(CursorState {
            x: 2,
            y: 4,
            visible: true,
            shape: 2,
        }),
    });
    assert_eq!(
        encoded_sha256(&msg),
        "0814b99a1dc6eaf7918424aa416c066509cbfb73b72344a809c27cde78cb6dbd"
    );
}

// Would catch: SurfaceGraphics* layout drift (assets, placements, keys, formats).
#[test]
fn graphics_scene_digest_is_frozen() {
    let key = SurfaceGraphicsAssetKey {
        source: SurfaceGraphicsSource::Terminal {
            target: SurfaceGraphicsTarget::Pane {
                pane_id: "w1:p1".into(),
            },
            image_id: 7,
        },
        image_width: 2,
        image_height: 1,
        format: SurfaceGraphicsFormat::Rgba,
        data_len: 8,
        data_fingerprint: 42,
    };
    let message = ServerMessage::PaneSurface(PaneSurfaceFrame {
        boot_id: "boot-1".into(),
        projection_revision: 2,
        surface_revision: 3,
        frame: FrameData {
            cells: Vec::new(),
            width: 0,
            height: 0,
            cursor: None,
            hyperlinks: Vec::new(),
            graphics: Vec::new(),
        },
        panes: Vec::new(),
        splits: Vec::new(),
        popup: None,
        graphics: SurfaceGraphicsScene {
            assets: vec![SurfaceGraphicsAsset {
                key: key.clone(),
                data: vec![255, 0, 0, 255, 0, 255, 0, 255],
            }],
            placements: vec![SurfaceGraphicsPlacement {
                asset: key,
                logical_placement_id: 9,
                x: 1,
                y: 2,
                cols: 2,
                rows: 1,
                source_x: 0,
                source_y: 0,
                source_width: 2,
                source_height: 1,
                x_offset: 0,
                y_offset: 0,
                z: -1,
                scrollback_offset: 0,
            }],
            retained_assets: Vec::new(),
        },
    });
    assert_eq!(
        encoded_sha256(&message),
        "49c4efec0f1456c8ca4112ddf6ead1ab75d0224007576c2ccc18c3fca55a69f0"
    );
}

// Would catch: ServerMessage::Graphics drift.
#[test]
fn server_graphics_digest_is_frozen() {
    let msg = ServerMessage::Graphics {
        bytes: b"\x1b_Ga=d,d=A,q=2;\x1b\\".to_vec(),
    };
    assert_eq!(
        encoded_sha256(&msg),
        "28a420f92e0e05e6760a8c140baf307c360c6d1b1aa68027481b324f87e22c44"
    );
}

// Would catch: a variant inserted/removed/reordered in ClientMessage.
#[test]
fn client_message_tags_are_frozen() {
    fn tag(msg: &ClientMessage) -> u8 {
        encode_message(msg).unwrap()[0]
    }
    assert_eq!(
        tag(&ClientMessage::TerminalHello {
            version: PROTOCOL_VERSION,
            cols: 1,
            rows: 1,
            cell_width_px: 0,
            cell_height_px: 0,
            pixel_mouse: false
        }),
        0
    );
    assert_eq!(tag(&ClientMessage::Input { data: Vec::new() }), 1);
    assert_eq!(
        tag(&ClientMessage::ClipboardImage {
            target: ClientClipboardImageTarget::DirectTerminal,
            extension: String::new(),
            data: Vec::new()
        }),
        2
    );
    assert_eq!(
        tag(&ClientMessage::Resize {
            cols: 1,
            rows: 1,
            cell_width_px: 0,
            cell_height_px: 0,
            pixel_mouse: false
        }),
        3
    );
    assert_eq!(tag(&ClientMessage::Detach), 4);
    assert_eq!(
        tag(&ClientMessage::AttachTerminal {
            terminal_id: String::new(),
            takeover: false
        }),
        5
    );
    assert_eq!(
        tag(&ClientMessage::AttachScroll {
            source: AttachScrollSource::Wheel,
            direction: AttachScrollDirection::Up,
            lines: 1,
            column: None,
            row: None,
            modifiers: 0
        }),
        6
    );
    assert_eq!(
        tag(&ClientMessage::ObserveTerminal {
            target: String::new()
        }),
        7
    );
    assert_eq!(
        tag(&ClientMessage::ControlTerminal {
            target: String::new(),
            takeover: false
        }),
        8
    );
    assert_eq!(
        tag(&ClientMessage::GraphicsTransmissionResult {
            transfer_id: 0,
            image_id: 0,
            success: true
        }),
        9
    );
    assert_eq!(
        tag(&ClientMessage::GraphicsTransmissionStarted {
            transfer_id: 0,
            image_id: 0
        }),
        10
    );
    assert_eq!(
        tag(&ClientMessage::ClientShellHello {
            version: PROTOCOL_VERSION,
            cell_width_px: 0,
            cell_height_px: 0,
            surface_size: ClientSurfaceSize { cols: 1, rows: 1 },
            pixel_mouse: false,
            direct_graphics: false,
            endpoint_keybindings: false,
            mouse_capture: false
        }),
        11
    );
    assert_eq!(
        tag(&ClientMessage::ClientShellResize {
            cell_width_px: 0,
            cell_height_px: 0,
            surface_size: ClientSurfaceSize { cols: 1, rows: 1 },
            pixel_mouse: false
        }),
        12
    );
    assert_eq!(
        tag(&ClientMessage::ClientShellPaneInput {
            pane_id: String::new(),
            events: Vec::new()
        }),
        13
    );
    assert_eq!(
        tag(&ClientMessage::ClientShellPopupInput {
            terminal_id: String::new(),
            events: Vec::new()
        }),
        14
    );
    assert_eq!(
        tag(&ClientMessage::ClientShellEndpointRequest {
            boot_id: String::new(),
            request: String::new()
        }),
        15
    );
    assert_eq!(
        tag(&ClientMessage::AttachMouse {
            kind: ClientMouseKind::Moved,
            position: ClientMousePosition::Cell { column: 0, row: 0 },
            geometry: None,
            modifiers: 0,
            lines: 0
        }),
        16
    );
    assert_eq!(
        tag(&ClientMessage::ClientShellHostTheme {
            update: ClientHostThemeUpdate::Appearance(ClientHostAppearance::Dark)
        }),
        17
    );
    assert_eq!(tag(&ClientMessage::ClientShellFocus { focused: true }), 18);
    assert_eq!(
        tag(&ClientMessage::ClientShellMouseCapture { enabled: true }),
        19
    );
    assert_eq!(
        encode_message(&ClientMessage::EndpointControl {
            kind: String::new(),
            data: String::new()
        })
        .unwrap(),
        [20, 0, 0]
    );
}

// Would catch: a variant inserted/removed/reordered in ServerMessage.
#[test]
fn server_message_tags_are_frozen() {
    fn tag(msg: &ServerMessage) -> u8 {
        encode_message(msg).unwrap()[0]
    }
    let empty_frame = || PaneSurfaceFrame {
        boot_id: "boot".into(),
        projection_revision: 1,
        surface_revision: 1,
        frame: FrameData {
            cells: Vec::new(),
            width: 0,
            height: 0,
            cursor: None,
            hyperlinks: Vec::new(),
            graphics: Vec::new(),
        },
        panes: Vec::new(),
        splits: Vec::new(),
        popup: None,
        graphics: SurfaceGraphicsScene::default(),
    };
    assert_eq!(
        tag(&ServerMessage::Welcome {
            version: PROTOCOL_VERSION,
            encoding: RenderEncoding::SemanticFrame,
            error: None
        }),
        0
    );
    assert_eq!(
        tag(&ServerMessage::Terminal(TerminalFrame {
            seq: 0,
            width: 0,
            height: 0,
            full: true,
            bytes: Vec::new()
        })),
        1
    );
    assert_eq!(tag(&ServerMessage::Graphics { bytes: Vec::new() }), 2);
    assert_eq!(tag(&ServerMessage::ServerShutdown { reason: None }), 3);
    assert_eq!(
        tag(&ServerMessage::Notify {
            kind: NotifyKind::Sound,
            message: String::new(),
            body: None
        }),
        4
    );
    assert_eq!(
        tag(&ServerMessage::Clipboard {
            data: String::new()
        }),
        5
    );
    assert_eq!(tag(&ServerMessage::WindowTitle { title: None }), 6);
    assert_eq!(tag(&ServerMessage::ReloadSoundConfig), 7);
    assert_eq!(
        tag(&ServerMessage::MouseCapture {
            enabled: false,
            sgr_pixels: false
        }),
        8
    );
    assert_eq!(tag(&ServerMessage::TerminalBell { count: 1 }), 9);
    assert_eq!(
        tag(&ServerMessage::GraphicsTransmissionRetired {
            transfer_id: 0,
            image_id: 0
        }),
        11
    );
    assert_eq!(tag(&ServerMessage::PaneSurface(empty_frame())), 13);
    assert_eq!(
        tag(&ServerMessage::ClientShellError {
            message: String::new(),
        }),
        15
    );
    assert_eq!(
        tag(&ServerMessage::DirectTerminalKeyboardProtocol {
            flags: 0,
            modify_other_keys_level: 0
        }),
        16
    );
    assert_eq!(
        tag(&ServerMessage::ClientShellKeyboardReportAll { enabled: false }),
        17
    );
    assert_eq!(
        tag(&ServerMessage::ClientShellEndpointResponseChunk {
            boot_id: String::new(),
            request_id: String::new(),
            final_chunk: true,
            data: Vec::new(),
        }),
        18
    );
    assert_eq!(
        tag(&ServerMessage::PaneSurfacePatch(PaneSurfacePatch {
            boot_id: String::new(),
            projection_revision: 0,
            base_surface_revision: 0,
            surface_revision: 0,
            rows: Vec::new(),
            panes: Vec::new(),
            cursor: None,
        })),
        19
    );
    assert_eq!(
        encode_message(&ServerMessage::EndpointControl {
            kind: String::new(),
            data: String::new(),
        })
        .unwrap(),
        [20, 0, 0]
    );
}

// Would catch: ClientShellSnapshot binary layout drift (the variant is still decodable even
// though generation 1 delivers snapshots as JSON named controls).
#[test]
fn client_shell_snapshot_binary_roundtrips_and_has_tag_12() {
    let msg = ServerMessage::ClientShellSnapshot(Box::new(ClientShellSnapshot {
        boot_id: "boot-1".into(),
        revision: 1,
        config_diagnostic: Some("endpoint config warning".into()),
        product_announcement: Some(ClientShellProductAnnouncement {
            version: "0.8.2".into(),
            id: "client-shell".into(),
            title: "Client shell".into(),
            body: "### New\n- Client-owned chrome".into(),
            preview: false,
        }),
        update_available: Some("0.8.3".into()),
        update_install_command: "herdr update".into(),
        server_keybindings_toml: Some("[keys]\nprefix = \"ctrl+a\"\n".into()),
        latest_release_notes_available: true,
        integration_updates_available: true,
        worktree_directory: "/tmp/herdr-worktrees".into(),
        release_notes: Some(ClientShellReleaseNotes {
            version: "0.8.3".into(),
            body: "### New\n- Update ready".into(),
            preview: true,
        }),
        focused_workspace_id: Some("w1".into()),
        focused_tab_id: Some("w1:t1".into()),
        focused_pane_id: Some("w1:p1".into()),
        tab_bar_right: vec![ClientShellTabStatusSegment {
            text: "host".into(),
            accent: false,
        }],
        tab_bar_right_separator: " · ".into(),
        agent_view_label: None,
        agent_order: Vec::new(),
        workspaces: vec![ClientShellWorkspace {
            workspace_id: "w1".into(),
            active_tab_id: "w1:t1".into(),
            new_workspace_cwd: "/tmp".into(),
            number: 1,
            label: "shell".into(),
            custom_label: false,
            branch: Some("main".into()),
            git_ahead_behind: None,
            tokens: Vec::new(),
            worktree: None,
            focused: true,
            agent_status: AgentStatus::Idle,
        }],
        tabs: vec![ClientShellTab {
            tab_id: "w1:t1".into(),
            workspace_id: "w1".into(),
            number: 1,
            label: "main".into(),
            custom_label: true,
            zoomed: false,
            focused: true,
            agent_status: AgentStatus::Idle,
        }],
        panes: vec![ClientShellPane {
            pane_id: "w1:p1".into(),
            workspace_id: "w1".into(),
            tab_id: "w1:t1".into(),
            label: None,
            cwd: Some("/repo".into()),
            foreground_cwd: Some("/repo".into()),
            focused: true,
            right_click_passthrough: false,
        }],
        agents: Vec::new(),
        commands: vec![ClientShellCommand {
            command_id: "cmd_0123456789abcdef0123456789abcdef".into(),
            binding_label: "prefix+z".into(),
            binding_labels: vec!["prefix+z".into()],
            action: ClientShellCommandAction::Shell,
            description: Some("deploy".into()),
        }],
    }));
    let encoded = encode_message(&msg).unwrap();
    assert_eq!(encoded[0], 12);
    let decoded: ServerMessage = decode_message(&encoded).unwrap();
    assert_eq!(decoded, msg);
}

// Would catch: framing regressions (prefix endianness, oversize check, trailing bytes).
#[test]
fn framing_uses_u32le_prefix_and_rejects_oversized_or_trailing_payloads() {
    let msg = ClientMessage::Detach;
    let mut buffer = Vec::new();
    write_message(&mut buffer, &msg).unwrap();
    assert_eq!(buffer, [1, 0, 0, 0, 4]);

    let mut cursor = std::io::Cursor::new(buffer.clone());
    let decoded: ClientMessage = read_message(&mut cursor, MAX_FRAME_SIZE).unwrap();
    assert_eq!(decoded, msg);

    let oversized = [0xff, 0xff, 0xff, 0x7f, 0];
    let err =
        read_message::<_, ClientMessage>(&mut std::io::Cursor::new(oversized), 1024).unwrap_err();
    assert!(matches!(
        err,
        herdr_protocol::FramingError::Oversized { .. }
    ));

    let trailing = [2, 0, 0, 0, 4, 9];
    let err =
        read_message::<_, ClientMessage>(&mut std::io::Cursor::new(trailing), 1024).unwrap_err();
    assert!(matches!(err, herdr_protocol::FramingError::Bincode(_)));

    let truncated = [9, 0, 0, 0, 4];
    let err =
        read_message::<_, ClientMessage>(&mut std::io::Cursor::new(truncated), 1024).unwrap_err();
    assert!(matches!(err, herdr_protocol::FramingError::UnexpectedEof));
}

// Would catch: the mirrored hello/welcome rejecting the published generation-1 fixtures,
// or accepting a hello that lacks a required core codec.
#[test]
fn frozen_generation_one_handshake_fixtures_decode() {
    let hello: EndpointClientHello =
        serde_json::from_str(&fixture("endpoint-hello-v1.json")).unwrap();
    assert_eq!(hello.generation, ENDPOINT_PROTOCOL_GENERATION);
    assert!(hello.supports_required_codecs());
    assert!(
        hello.surface_active,
        "legacy hello defaults to an active surface"
    );

    let welcome: EndpointServerWelcome =
        serde_json::from_str(&fixture("endpoint-welcome-v1.json")).unwrap();
    assert_eq!(welcome.generation, ENDPOINT_PROTOCOL_GENERATION);
    assert_eq!(welcome.snapshot_codec, SNAPSHOT_CODEC_V1);
    assert_eq!(welcome.surface_codec, SURFACE_CODEC_V1);
    assert_eq!(welcome.input_codec, INPUT_CODEC_V1);
    assert_eq!(welcome.blob_codec, BLOB_CODEC_V1);
    assert!(welcome.capabilities.is_empty());
    assert_eq!(welcome.methods, vec!["pane.focus".to_string()]);
    assert!(welcome.error.is_none());

    let mut degraded = hello.clone();
    degraded.blob_codecs.clear();
    assert!(!degraded.supports_required_codecs());
}

// Would catch: hello/welcome breaking on future named fields (must be ignored, not rejected).
#[test]
fn handshake_json_ignores_future_named_fields() {
    let mut value: serde_json::Value =
        serde_json::from_str(&fixture("endpoint-hello-v1.json")).unwrap();
    value["future_feature"] = serde_json::json!({"enabled": true});
    let decoded: EndpointClientHello = serde_json::from_value(value).unwrap();
    assert_eq!(
        decoded.surface_size,
        ClientSurfaceSize { cols: 80, rows: 24 }
    );

    let mut value: serde_json::Value =
        serde_json::from_str(&fixture("endpoint-welcome-v1.json")).unwrap();
    value["future_service"] = serde_json::json!("v2");
    let decoded: EndpointServerWelcome = serde_json::from_value(value).unwrap();
    assert_eq!(decoded.server_version, "0.8.2");
}

// Would catch: the JSON snapshot codec rejecting unknown agent statuses / command actions
// or dropping the focused pane id that QualifiedTarget depends on.
#[test]
fn frozen_generation_one_snapshot_fixture_decodes_tolerantly() {
    let snapshot: ClientShellSnapshot =
        serde_json::from_str(&fixture("endpoint-snapshot-v1.json")).unwrap();
    assert_eq!(snapshot.boot_id, "boot-v1");
    assert_eq!(snapshot.revision, 7);
    assert_eq!(snapshot.focused_pane_id.as_deref(), Some("w1:p1"));
    assert_eq!(snapshot.workspaces[0].agent_status, AgentStatus::Unknown);
    assert_eq!(snapshot.tabs[0].agent_status, AgentStatus::Working);
    assert_eq!(snapshot.agents[0].agent_status, AgentStatus::Blocked);
    assert_eq!(snapshot.commands[0].action, ClientShellCommandAction::Shell);

    let mut value: serde_json::Value =
        serde_json::from_str(&fixture("endpoint-snapshot-v1.json")).unwrap();
    value["future_projection"] = serde_json::json!({"enabled": true});
    value["commands"][0]["action"] = serde_json::json!("FutureAction");
    let decoded: ClientShellSnapshot = serde_json::from_value(value).unwrap();
    assert_eq!(
        decoded.commands[0].action,
        ClientShellCommandAction::Unknown
    );
}

// Would catch: the published method list or its digests being altered by the mirror.
#[test]
fn method_shape_fixture_is_the_published_v1_set() {
    let shapes: std::collections::BTreeMap<String, String> =
        serde_json::from_str(&fixture("endpoint-method-shapes-v1.json")).unwrap();
    assert_eq!(shapes.len(), 38);
    assert_eq!(
        shapes.get("pane.resize").map(String::as_str),
        Some("9fbcc70b8908c43f47b0ff85d0a9040ce18c9179a029ed7d8f2616a05d8c301d")
    );
    assert!(shapes.keys().all(|k| k.as_str() < "zzz"));
    let keys: Vec<&String> = shapes.keys().collect();
    assert!(
        keys.windows(2).all(|w| w[0] < w[1]),
        "published list is sorted"
    );
}

// Would catch: packed colour decoding drifting from color_to_u32/u32_to_color.
#[test]
fn packed_colours_roundtrip_and_unknown_tags_fall_back_to_reset() {
    for colour in [
        WireColor::Reset,
        WireColor::Black,
        WireColor::White,
        WireColor::Indexed(220),
        WireColor::Rgb(255, 128, 0),
    ] {
        assert_eq!(WireColor::from_u32(colour.to_u32()), colour);
    }
    assert_eq!(WireColor::Rgb(255, 128, 0).to_u32(), 0x02_FF_80_00);
    assert_eq!(WireColor::Indexed(220).to_u32(), 0x01_00_00_DC);
    assert_eq!(WireColor::White.to_u32(), 0x10);
    assert_eq!(WireColor::from_u32(0x00_00_00_42), WireColor::Reset);
    assert_eq!(WireColor::from_u32(0x07_00_00_00), WireColor::Reset);
    assert_eq!(underline_style(0x3000 | modifier::BOLD), 3);
}
