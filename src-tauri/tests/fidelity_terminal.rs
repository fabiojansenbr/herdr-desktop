//! Spec 007 — AC-007-02 contracts of the terminal surface DTOs (frente Terminal).
//!
//! Pure conversions only (no engine, socket, window or display): frame metadata that the
//! WebView needs for mouse/selection/scrollback/links, validated mouse input, and the
//! validators an integrator runs before calling `pane.focus` / `pane.scroll` /
//! `pane.selection.read` or opening a link. Native window/PTY proof belongs to the E2E of 007.
//!
//! Fixture values are distinct on purpose: two panes `w1:p1` (left, focused, scrollback
//! offset 2 of 10, content 40, no mouse reporting) and `w1:p2` (right, inner x=10, mouse
//! reporting with SGR pixels, alternate screen, content 7); four link URIs, of which only the
//! first is a safe web URL.

use herdr_client::protocol::wire::{
    CellData, ClientMouseButton, ClientMouseGeometry, ClientMouseKind, ClientMousePosition,
    ClientPaneInputEvent, CursorState, FrameData, PaneSurfaceFrame, PaneSurfacePane,
    PaneSurfacePatch, PaneSurfacePatchRow, PaneSurfaceScrollMetrics, SurfaceGraphicsScene,
    SurfaceRect,
};
use herdr_desktop::terminal::{
    full_event, input_events_for_pane, metadata_event, patch_events, validate_focus_request,
    validate_link_request, validate_scroll_request, validate_selection_request, FocusRequestDto,
    FrameEvent, InputDto, LinkRequestDto, ScrollRequestDto, SelectionRequestDto, TextPointDto,
};
use serde_json::{json, Value};

const W: u16 = 20;
const H: u16 = 4;
const SAFE: &str = "https://herdr.dev/docs?x=1";

fn rect(x: u16, width: u16) -> SurfaceRect {
    SurfaceRect {
        x,
        y: 0,
        width,
        height: H,
    }
}

fn left() -> PaneSurfacePane {
    PaneSurfacePane {
        pane_id: "w1:p1".into(),
        content_revision: 40,
        rect: rect(0, 9),
        inner_rect: rect(0, 9),
        scrollbar_rect: None,
        scroll: Some(PaneSurfaceScrollMetrics {
            offset_from_bottom: 2,
            max_offset_from_bottom: 10,
            viewport_rows: 4,
        }),
        focused: true,
        mouse_reporting: false,
        sgr_pixel_mouse: false,
        alternate_screen_active: false,
        pixel_width: 81,
        pixel_height: 72,
    }
}

fn right() -> PaneSurfacePane {
    PaneSurfacePane {
        pane_id: "w1:p2".into(),
        content_revision: 7,
        rect: rect(10, 10),
        inner_rect: rect(10, 10),
        scrollbar_rect: None,
        scroll: None,
        focused: false,
        mouse_reporting: true,
        sgr_pixel_mouse: true,
        alternate_screen_active: true,
        pixel_width: 90,
        pixel_height: 72,
    }
}

fn cell(symbol: &str, hyperlink: Option<u32>) -> CellData {
    CellData {
        symbol: symbol.into(),
        fg: 0,
        bg: 0,
        modifier: 0,
        skip: false,
        hyperlink,
    }
}

fn surface() -> PaneSurfaceFrame {
    let mut cells: Vec<CellData> = (0..usize::from(W) * usize::from(H))
        .map(|_| cell(" ", None))
        .collect();
    // Left pane, row 1, columns 0..4: the safe link. Right pane, row 0, columns 10..12: unsafe.
    for x in 0..4 {
        cells[usize::from(W) + x] = cell("h", Some(0));
    }
    for c in &mut cells[10..12] {
        *c = cell("j", Some(1));
    }
    // Index beyond the table (row 2, column 5) must not resolve.
    cells[2 * usize::from(W) + 5] = cell("z", Some(9));
    PaneSurfaceFrame {
        boot_id: "boot-term-3".into(),
        projection_revision: 5,
        surface_revision: 12,
        frame: FrameData {
            cells,
            width: W,
            height: H,
            cursor: Some(CursorState {
                x: 1,
                y: 1,
                visible: true,
                shape: 0,
            }),
            hyperlinks: vec![
                SAFE.into(),
                "javascript:alert(1)".into(),
                "file:///etc/passwd".into(),
                "https://evil.example/\u{1b}]52;c;x\u{7}".into(),
            ],
            graphics: vec![],
        },
        panes: vec![left(), right()],
        splits: vec![],
        popup: None,
        graphics: SurfaceGraphicsScene::default(),
    }
}

fn to_json(event: &FrameEvent) -> Value {
    serde_json::to_value(event).expect("serializable")
}

fn mouse(v: Value) -> InputDto {
    serde_json::from_value(v).expect("mouse dto")
}

fn code(err: herdr_client::RuntimeError) -> String {
    err.code
}

// Would catch: hyperlink index dropped from cells (links impossible to identify), `h` emitted
// on every cell (payload growth), metadata lacking mouse/scroll/alt-screen, or a hostile URI with
// control characters forwarded to the WebView as-is (index positions must still line up).
#[test]
fn full_frame_carries_link_indices_and_metadata_with_sanitized_table() {
    let surface = surface();
    let full = to_json(&full_event(&surface));
    assert_eq!(full["type"], "full");
    let cells = full["cells"].as_array().unwrap();
    assert_eq!(cells[usize::from(W) + 2]["h"], 0);
    assert_eq!(cells[10]["h"], 1);
    assert!(cells[0].get("h").is_none(), "cells without link carry no h");

    let meta = to_json(&metadata_event(&surface));
    assert_eq!(meta["type"], "metadata");
    assert_eq!(meta["revision"], 12);
    let links = meta["hyperlinks"].as_array().unwrap();
    assert_eq!(links.len(), 4, "table positions preserved");
    assert_eq!(links[0], SAFE);
    assert_eq!(links[1], "javascript:alert(1)");
    assert_eq!(links[3], "", "control characters never reach the WebView");
    let panes = meta["panes"].as_array().unwrap();
    assert_eq!(panes.len(), 2);
    assert_eq!(panes[0]["pane_id"], "w1:p1");
    assert_eq!(panes[0]["content_revision"], 40);
    assert_eq!(
        panes[0]["scroll"],
        json!({"offset_from_bottom": 2, "max_offset_from_bottom": 10, "viewport_rows": 4})
    );
    assert_eq!(panes[0]["mouse_reporting"], false);
    assert_eq!(
        panes[1]["inner_rect"],
        json!({"x": 10, "y": 0, "width": 10, "height": 4})
    );
    assert_eq!(panes[1]["mouse_reporting"], true);
    assert_eq!(panes[1]["sgr_pixel_mouse"], true);
    assert_eq!(panes[1]["alternate_screen_active"], true);
    assert_eq!(panes[1]["pixel_width"], 90);
    assert_eq!(panes[1]["focused"], false);
    assert!(panes[1]["scroll"].is_null());
}

// Would catch: metadata of a real patch ignored (scroll offset/content revision frozen at the
// last full frame), a patch re-sending the link table, or an empty metadata event per patch.
#[test]
fn patch_metadata_follows_updated_panes_only() {
    let mut updated = left();
    updated.content_revision = 41;
    updated.scroll = Some(PaneSurfaceScrollMetrics {
        offset_from_bottom: 0,
        max_offset_from_bottom: 11,
        viewport_rows: 4,
    });
    let patch = PaneSurfacePatch {
        boot_id: "boot-term-3".into(),
        projection_revision: 5,
        base_surface_revision: 12,
        surface_revision: 13,
        rows: vec![PaneSurfacePatchRow {
            x: 0,
            y: 3,
            cells: vec![cell("q", None), cell("r", Some(0))],
        }],
        panes: vec![updated],
        cursor: None,
    };
    let (event, meta) = patch_events(&patch);
    let event = to_json(&event);
    assert_eq!(event["type"], "patch");
    assert_eq!(event["revision"], 13);
    assert_eq!(event["rows"][0]["cells"][1]["h"], 0);
    assert!(event["rows"][0]["cells"][0].get("h").is_none());
    let meta = to_json(&meta.expect("panes changed"));
    assert_eq!(meta["revision"], 13);
    assert!(
        meta["hyperlinks"].is_null(),
        "patch keeps the full frame table"
    );
    assert_eq!(meta["panes"].as_array().unwrap().len(), 1);
    assert_eq!(meta["panes"][0]["content_revision"], 41);
    assert_eq!(meta["panes"][0]["scroll"]["offset_from_bottom"], 0);

    let quiet = PaneSurfacePatch {
        panes: vec![],
        ..patch
    };
    assert!(patch_events(&quiet).1.is_none());
}

// Would catch: mouse kinds/buttons crossed, pane-local coordinates shifted, modifiers outside the
// crossterm set forwarded, or a pixel position sent to a pane without SGR pixel mode.
#[test]
fn mouse_input_maps_once_to_the_published_wire_event() {
    let surface = surface();
    let right = &surface.panes[1];
    let events = input_events_for_pane(
        Some(right),
        &[
            mouse(json!({"kind": "mouse", "action": "down", "button": "right", "column": 3, "row": 2, "modifiers": 0b0001_0101u8, "lines": 3})),
            mouse(json!({"kind": "mouse", "action": "drag", "button": "middle", "column": 9, "row": 3, "modifiers": 0, "lines": 3,
                "pixel": {"x": 88, "y": 70}, "geometry": {"cols": 10, "rows": 4, "width_px": 90, "height_px": 72}})),
            mouse(json!({"kind": "mouse", "action": "scroll_down", "column": 0, "row": 0, "modifiers": 0, "lines": 5})),
        ],
    )
    .expect("valid");
    assert_eq!(
        events,
        vec![
            ClientPaneInputEvent::Mouse {
                kind: ClientMouseKind::Down(ClientMouseButton::Right),
                position: ClientMousePosition::Cell { column: 3, row: 2 },
                geometry: None,
                modifiers: 0b0000_0101,
                lines: 3,
            },
            ClientPaneInputEvent::Mouse {
                kind: ClientMouseKind::Drag(ClientMouseButton::Middle),
                position: ClientMousePosition::Pixels {
                    x: 88,
                    y: 70,
                    column: 9,
                    row: 3
                },
                geometry: Some(ClientMouseGeometry {
                    cols: 10,
                    rows: 4,
                    width_px: 90,
                    height_px: 72
                }),
                modifiers: 0,
                lines: 3,
            },
            ClientPaneInputEvent::Mouse {
                kind: ClientMouseKind::ScrollDown,
                position: ClientMousePosition::Cell { column: 0, row: 0 },
                geometry: None,
                modifiers: 0,
                lines: 5,
            },
        ]
    );

    // Left pane has no SGR pixel mode: pixels degrade to the same cell; wheel is allowed without
    // mouse reporting (the engine applies scrollback), a click is not.
    let left = &surface.panes[0];
    let scroll = input_events_for_pane(
        Some(left),
        &[mouse(json!({"kind": "mouse", "action": "scroll_up", "column": 8, "row": 3, "modifiers": 0, "lines": 3,
            "pixel": {"x": 80, "y": 70}, "geometry": {"cols": 9, "rows": 4, "width_px": 81, "height_px": 72}}))],
    )
    .expect("wheel without reporting");
    assert_eq!(
        scroll,
        vec![ClientPaneInputEvent::Mouse {
            kind: ClientMouseKind::ScrollUp,
            position: ClientMousePosition::Cell { column: 8, row: 3 },
            geometry: None,
            modifiers: 0,
            lines: 3,
        }]
    );
    let click = input_events_for_pane(
        Some(left),
        &[mouse(
            json!({"kind": "mouse", "action": "down", "button": "left", "column": 1, "row": 1, "modifiers": 0, "lines": 3}),
        )],
    );
    assert_eq!(code(click.unwrap_err()), "mouse_not_reporting");
}

// Would catch: out-of-pane coordinates, invented kinds/buttons, pixel positions without geometry
// or at 0 / beyond the pane size, and mouse without a confirmed pane reaching the engine.
#[test]
fn invalid_mouse_input_is_rejected_before_the_engine() {
    let surface = surface();
    let right = &surface.panes[1];
    let geometry = json!({"cols": 10, "rows": 4, "width_px": 90, "height_px": 72});
    let cases = [
        json!({"kind": "mouse", "action": "down", "button": "left", "column": 10, "row": 0, "modifiers": 0, "lines": 3}),
        json!({"kind": "mouse", "action": "down", "button": "left", "column": 0, "row": 4, "modifiers": 0, "lines": 3}),
        json!({"kind": "mouse", "action": "down", "column": 0, "row": 0, "modifiers": 0, "lines": 3}),
        json!({"kind": "mouse", "action": "scroll_up", "button": "left", "column": 0, "row": 0, "modifiers": 0, "lines": 3}),
        json!({"kind": "mouse", "action": "click", "button": "left", "column": 0, "row": 0, "modifiers": 0, "lines": 3}),
        json!({"kind": "mouse", "action": "down", "button": "back", "column": 0, "row": 0, "modifiers": 0, "lines": 3}),
        json!({"kind": "mouse", "action": "scroll_up", "column": 0, "row": 0, "modifiers": 0, "lines": 0}),
        json!({"kind": "mouse", "action": "scroll_up", "column": 0, "row": 0, "modifiers": 0, "lines": 65}),
        json!({"kind": "mouse", "action": "moved", "column": 0, "row": 0, "modifiers": 0, "lines": 3, "pixel": {"x": 5, "y": 5}}),
        json!({"kind": "mouse", "action": "moved", "column": 0, "row": 0, "modifiers": 0, "lines": 3, "pixel": {"x": 0, "y": 5}, "geometry": geometry}),
        json!({"kind": "mouse", "action": "moved", "column": 0, "row": 0, "modifiers": 0, "lines": 3, "pixel": {"x": 91, "y": 5}, "geometry": geometry}),
        json!({"kind": "mouse", "action": "moved", "column": 0, "row": 0, "modifiers": 0, "lines": 3, "pixel": {"x": 5, "y": 5},
            "geometry": {"cols": 11, "rows": 4, "width_px": 90, "height_px": 72}}),
    ];
    for case in cases {
        let result = input_events_for_pane(Some(right), &[mouse(case.clone())]);
        assert_eq!(
            result.map_err(code),
            Err("invalid_input".to_string()),
            "{case}"
        );
    }
    // r1: SGR pixel geometry from an old size/DPI (180x144 for a 90x72 pane) is stale.
    let stale_pixels = input_events_for_pane(
        Some(right),
        &[mouse(
            json!({"kind": "mouse", "action": "moved", "column": 1, "row": 1, "modifiers": 0, "lines": 3,
            "pixel": {"x": 20, "y": 30}, "geometry": {"cols": 10, "rows": 4, "width_px": 180, "height_px": 144}}),
        )],
    );
    assert_eq!(stale_pixels.map_err(code), Err("invalid_input".to_string()));
    // Without SGR pixel mode the pixels are not used: stale pixel sizes still degrade to the cell.
    let degraded = input_events_for_pane(
        Some(&surface.panes[0]),
        &[mouse(json!({"kind": "mouse", "action": "scroll_down", "column": 2, "row": 1, "modifiers": 0, "lines": 3,
            "pixel": {"x": 150, "y": 100}, "geometry": {"cols": 9, "rows": 4, "width_px": 162, "height_px": 144}}))],
    )
    .expect("non-SGR pane degrades to cells");
    assert_eq!(
        degraded,
        vec![ClientPaneInputEvent::Mouse {
            kind: ClientMouseKind::ScrollDown,
            position: ClientMousePosition::Cell { column: 2, row: 1 },
            geometry: None,
            modifiers: 0,
            lines: 3,
        }]
    );
    let no_pane = input_events_for_pane(
        None,
        &[mouse(
            json!({"kind": "mouse", "action": "scroll_up", "column": 0, "row": 0, "modifiers": 0, "lines": 3}),
        )],
    );
    assert_eq!(code(no_pane.unwrap_err()), "no_target");
    // Keyboard input is unaffected by the pane lookup.
    let keys = input_events_for_pane(
        None,
        &[serde_json::from_value(json!({"kind": "text", "text": "ç"})).unwrap()],
    )
    .expect("text needs no pane metadata");
    assert_eq!(keys, vec![ClientPaneInputEvent::TextCommit("ç".into())]);
}

// Would catch: selection rows taken as viewport rows, a stale content revision accepted, columns
// outside the pane or rows beyond the buffer forwarded to pane.selection.read.
#[test]
fn selection_request_uses_absolute_rows_and_current_content() {
    let surface = surface();
    let ok = SelectionRequestDto {
        pane_id: "w1:p1".into(),
        anchor: TextPointDto { row: 13, col: 8 },
        cursor: TextPointDto { row: 8, col: 0 },
        content_revision: 40,
    };
    assert_eq!(validate_selection_request(&surface, &ok).unwrap(), ok);
    let stale = SelectionRequestDto {
        content_revision: 39,
        ..ok.clone()
    };
    assert_eq!(
        code(validate_selection_request(&surface, &stale).unwrap_err()),
        "stale_content"
    );
    let col = SelectionRequestDto {
        anchor: TextPointDto { row: 8, col: 9 },
        ..ok.clone()
    };
    assert_eq!(
        code(validate_selection_request(&surface, &col).unwrap_err()),
        "invalid_selection"
    );
    // Buffer rows = max_offset (10) + viewport (4) = 14 → last absolute row is 13.
    let row = SelectionRequestDto {
        anchor: TextPointDto { row: 14, col: 0 },
        ..ok.clone()
    };
    assert_eq!(
        code(validate_selection_request(&surface, &row).unwrap_err()),
        "invalid_selection"
    );
    // Alternate screen without scroll metrics: only the visible rows exist.
    let alt = SelectionRequestDto {
        pane_id: "w1:p2".into(),
        anchor: TextPointDto { row: 3, col: 9 },
        cursor: TextPointDto { row: 4, col: 0 },
        content_revision: 7,
    };
    assert_eq!(
        code(validate_selection_request(&surface, &alt).unwrap_err()),
        "invalid_selection"
    );
    let unknown = SelectionRequestDto {
        pane_id: "w9:p9".into(),
        ..ok
    };
    assert_eq!(
        code(validate_selection_request(&surface, &unknown).unwrap_err()),
        "pane_not_found"
    );
}

// Would catch: an offset beyond the scrollback forwarded unclamped, scroll requested for a pane
// without scrollback (alternate screen), or focus requested again for the focused pane.
#[test]
fn scroll_and_focus_requests_are_validated_against_the_surface() {
    let surface = surface();
    let scroll = |pane: &str, offset: u64| {
        validate_scroll_request(
            &surface,
            &ScrollRequestDto {
                pane_id: pane.into(),
                offset_from_bottom: offset,
            },
        )
    };
    assert_eq!(scroll("w1:p1", 99).unwrap().offset_from_bottom, 10);
    assert_eq!(scroll("w1:p1", 0).unwrap().offset_from_bottom, 0);
    assert_eq!(code(scroll("w1:p2", 1).unwrap_err()), "scroll_unavailable");
    assert_eq!(code(scroll("w0:p0", 1).unwrap_err()), "pane_not_found");

    let focus = |pane: &str| {
        validate_focus_request(
            &surface,
            &FocusRequestDto {
                pane_id: pane.into(),
            },
        )
    };
    assert!(focus("w1:p2").unwrap(), "unfocused pane needs pane.focus");
    assert!(!focus("w1:p1").unwrap(), "focused pane needs no request");
    assert_eq!(code(focus("p2").unwrap_err()), "pane_not_found");
}

// Would catch: a link opened for a URI the cell no longer holds, non-web schemes (javascript:,
// file:) or sanitized URIs accepted, index beyond the table resolving, and stale content.
#[test]
fn link_request_resolves_only_safe_web_uris_at_the_cell() {
    let surface = surface();
    let link = |pane: &str, uri: &str, row: u16, col: u16, content: u64| {
        validate_link_request(
            &surface,
            &LinkRequestDto {
                pane_id: pane.into(),
                uri: uri.into(),
                viewport_row: row,
                col,
                content_revision: content,
            },
        )
    };
    assert_eq!(link("w1:p1", SAFE, 1, 3, 40).unwrap(), SAFE);
    assert_eq!(
        code(link("w1:p1", "https://herdr.dev/other", 1, 3, 40).unwrap_err()),
        "stale_link"
    );
    assert_eq!(
        code(link("w1:p1", SAFE, 1, 4, 40).unwrap_err()),
        "stale_link"
    );
    assert_eq!(
        code(link("w1:p1", SAFE, 1, 3, 41).unwrap_err()),
        "stale_content"
    );
    assert_eq!(
        code(link("w1:p1", SAFE, 1, 9, 40).unwrap_err()),
        "invalid_link"
    );
    assert_eq!(code(link("w1:p1", "", 2, 5, 40).unwrap_err()), "stale_link");
    // Right pane is pane-local: column 0 is surface column 10.
    assert_eq!(
        code(link("w1:p2", "javascript:alert(1)", 0, 0, 7).unwrap_err()),
        "unsafe_link"
    );
}
