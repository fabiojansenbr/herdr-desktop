//! Contracts of the `resize-dpi` and `a11y-navigation` phases of the spec 007 native flow
//! (tests/fidelity-native/view_flow.rs): pure evaluators over literal fixtures. No display,
//! compositor, engine, key, clipboard or browser runs here. Each negative names the wrong
//! behavior it catches.

#[allow(dead_code)]
#[path = "../../tests/fidelity-native/corpus.rs"]
mod corpus;
#[allow(dead_code)]
#[path = "../../tests/fidelity-native/display.rs"]
mod display;
#[allow(dead_code)]
#[path = "../../tests/fidelity-native/geometry.rs"]
mod geometry;
#[allow(dead_code, unused_imports)]
#[path = "../../tests/fidelity-native/mouse_flow.rs"]
mod mouse_flow;
#[allow(dead_code, unused_imports)]
#[path = "../../tests/fidelity-native/paste_flow.rs"]
mod paste_flow;
#[allow(dead_code)]
#[path = "../../tests/fidelity-native/plan.rs"]
mod plan;
#[cfg(target_os = "linux")]
#[allow(dead_code)]
#[path = "../../tests/fidelity-native/supervisor.rs"]
mod supervisor;
#[allow(dead_code, unused_imports)]
#[path = "../../tests/fidelity-native/view_flow.rs"]
mod view_flow;

use serde_json::{json, Value};
use view_flow::*;

const SOCK: &str = "/tmp/hd7L-4242/sway-ipc.1000.77.sock";

fn exp() -> ViewExpectations {
    ViewExpectations::new("local", "/tmp/hd7L-4242", SOCK)
        .unwrap()
        .with_window(WINDOW_PID)
}

const WINDOW_PID: u32 = 4343;

fn id() -> Value {
    json!({"pane_id":"w1-p1","generation":"g7","boot_prefix":"b0a1","endpoint":"local"})
}

#[test]
fn phase_names_and_checks_equal_plan() {
    for (name, checks) in [
        (RESIZE_PHASE, &RESIZE_CHECKS[..]),
        (A11Y_PHASE, &A11Y_CHECKS[..]),
    ] {
        let spec = plan::FLOW.iter().find(|p| p.name == name).unwrap();
        assert_eq!(spec.checks, checks, "{name}");
        // Readiness declaration: linked into the one native flow (Prepared), which is not a
        // pass; the phase passes only from its native outcomes.
        assert!(
            matches!(spec.readiness, plan::Readiness::Prepared),
            "{name} is linked"
        );
    }
}

#[test]
fn expectations_refuse_user_sway_socket() {
    // Wrong behavior: commanding the user's compositor via SWAYSOCK.
    assert!(ViewExpectations::new(
        "local",
        "/run/user/1000",
        "/run/user/1000/sway-ipc.1000.5.sock"
    )
    .is_err());
    assert!(ViewExpectations::new("local", "/tmp/hd7L-1", "/tmp/other/sway-ipc.sock").is_err());
    assert!(ViewExpectations::new("", "/tmp/hd7L-1", "/tmp/hd7L-1/sway-ipc.1.2.sock").is_err());
}

#[test]
fn output_command_is_literal_per_stage_and_start_has_none() {
    assert_eq!(
        STAGES.map(|s| s.name),
        ["start", "grow", "scale2", "restore"]
    );
    assert_eq!(output_command(&STAGES[0]), None);
    assert_eq!(
        output_command(&STAGES[2]).unwrap(),
        [
            "output",
            "HEADLESS-1",
            "resolution",
            "1600x900",
            "scale",
            "2"
        ]
    );
    let launch = swaymsg_args(SOCK, &output_command(&STAGES[1]).unwrap());
    assert_eq!(&launch[..2], ["-s", SOCK]);
    assert_eq!(RESIZE_STEPS.len(), 7);
    assert_eq!(RESIZE_STEPS[0], "resize-observe-start");
    assert_eq!(RESIZE_STEPS[6], "resize-observe-restore");
}

// ---- resize fixtures: cell 9x18 CSS px, CSD offset (0, 37), terminal area = inner - (200, 60).
fn stage_view(w: u32, h: u32, s: u32, painted: u64, full: u64) -> (Value, Value, Value) {
    let (lw, lh) = (w / s, h / s);
    let (iw, ih) = (lw, lh - 37);
    let (tw, th) = (iw - 200, ih - 60);
    let (cols, rows) = (tw / 9, th / 18);
    let (cw, ch) = (cols * 9, rows * 18);
    // The engine composes the tab: the pane inner_rect is one column narrower than the surface
    // (stable gutter, as GUI r4 observed stty 74 against a 75-column canvas). The PTY has the pane.
    let pane_cols = cols - 1;
    let outputs = json!([{"name":"HEADLESS-1","active":true,"scale":s as f64,
        "current_mode":{"width":w,"height":h},"rect":{"x":0,"y":0,"width":lw,"height":lh}}]);
    let page = json!({"identity":id(),"inner_width":iw,"inner_height":ih,"dpr":s as f64,
        "terminal_rect":{"width":tw,"height":th},"metrics":{"cellWidth":9,"cellHeight":18},
        "canvas":{"width":cw*s,"height":ch*s,"css_width":format!("{cw}px"),"css_height":format!("{ch}px"),
                  "transform":[s,0,0,s,0,0],"rect":{"left":0,"top":0,"width":cw,"height":ch}},
        "surface":{"cols":cols,"rows":rows},
        "panes":[{"pane_id":"w1-p1","inner_rect":{"x":0,"y":0,"width":pane_cols,"height":rows}}],
        "probe":{"painted_rows":painted,"full_frames":full}});
    let pty = json!([rows, pane_cols]);
    (outputs, page, pty)
}

fn crisp_screen(w: u32, h: u32, doubled: f64, edge: f64) -> Value {
    json!({"width":w,"height":h,"ink_blocks":400,"doubled_fraction":doubled,"edge_p95":edge})
}

struct ResizeWorld {
    page: Value,
    ledger: Ledger,
}

fn resize_world(tweak: impl Fn(&str, &mut Value, &mut Value)) -> ResizeWorld {
    let mut ledger = Ledger::default();
    let mut stages = Vec::new();
    let mut painted = 0;
    for (i, st) in STAGES.iter().enumerate() {
        let (outputs, mut page, pty) = stage_view(st.width, st.height, st.scale, painted, i as u64);
        painted += 200;
        if let Some(cmd) = output_command(st) {
            let mut apply = json!({"step":format!("resize-apply-{}", st.name),"identity":id(),
                "socket":SOCK,"command":cmd,"outputs":outputs.clone()});
            tweak(&format!("apply-{}", st.name), &mut apply, &mut page);
            ledger
                .record(apply["step"].as_str().unwrap().to_owned().as_str(), apply)
                .unwrap();
        }
        let edge = if st.scale == 2 { 180.0 } else { 200.0 };
        let mut observe = json!({"step":format!("resize-observe-{}", st.name),"identity":id(),
            "socket":SOCK,"outputs":outputs,"pty_size":pty,
            "screen":crisp_screen(st.width, st.height, 0.2, edge)});
        tweak(&format!("observe-{}", st.name), &mut observe, &mut page);
        let step = observe["step"].as_str().unwrap().to_owned();
        if !step.is_empty() {
            ledger.record(&step, observe).unwrap();
        }
        page["stage"] = json!(st.name);
        stages.push(page);
    }
    ResizeWorld {
        page: json!({"stages":stages,"error":null}),
        ledger,
    }
}

fn resize(w: &ResizeWorld) -> Result<Vec<(&'static str, bool)>, String> {
    resize_checks(RESIZE_PHASE, &w.page, &w.ledger, &exp())
}

#[test]
fn resize_positive_passes_both_checks() {
    let w = resize_world(|_, _, _| {});
    assert_eq!(
        resize(&w).unwrap(),
        vec![(RESIZE_CHECKS[0], true), (RESIZE_CHECKS[1], true)]
    );
}

fn geometry_false(tweak: impl Fn(&str, &mut Value, &mut Value)) {
    let r = resize(&resize_world(tweak)).unwrap();
    assert_eq!(r[0], (RESIZE_CHECKS[0], false), "{r:?}");
}

#[test]
fn resize_rejects_stale_identity() {
    // Remounted pane / new generation passed off as the same terminal.
    geometry_false(|k, a, p| {
        if k == "observe-scale2" {
            a["identity"]["generation"] = json!("g8");
            p["identity"]["generation"] = json!("g8");
        }
    });
}

#[test]
fn resize_rejects_desired_dpr_not_observed() {
    // Page trusting the command instead of window.devicePixelRatio.
    geometry_false(|k, _, p| {
        if k == "observe-scale2" {
            p["dpr"] = json!(1.0);
        }
    });
}

#[test]
fn resize_rejects_backing_store_not_scaled() {
    // Canvas CSS size kept but backing not multiplied by DPR (blurry upscale).
    geometry_false(|k, _, p| {
        if k == "observe-scale2" {
            p["canvas"]["width"] = json!(p["canvas"]["width"].as_u64().unwrap() / 2);
        }
    });
    // Transform left at identity after a DPR change.
    geometry_false(|k, _, p| {
        if k == "observe-scale2" {
            p["canvas"]["transform"] = json!([1, 0, 0, 1, 0, 0]);
        }
    });
}

#[test]
fn resize_rejects_pty_geometry_not_confirmed() {
    // Engine never received the resize: PTY keeps the old size.
    geometry_false(|k, a, _| {
        if k == "observe-grow" {
            a["pty_size"] = json!([33, 120]);
        }
    });
    // Compositor never applied the mode.
    geometry_false(|k, a, _| {
        if k == "observe-grow" {
            a["outputs"][0]["current_mode"]["width"] = json!(1280);
        }
    });
    // Command sent to another socket.
    geometry_false(|k, a, _| {
        if k == "apply-grow" {
            a["socket"] = json!("/run/user/1000/sway-ipc.1000.5.sock");
        }
    });
}

#[test]
fn resize_evaluator_basis_is_canvas_rect_not_terminal_rect() {
    // When terminal_rect has padding/border (e.g. +25px width and +12px height) that would yield
    // more columns/rows than the canvas, the evaluator must use canvas.rect as the basis so the
    // confirmed canvas dimensions and PTY size match.
    let w = resize_world(|k, _, p| {
        if k.starts_with("observe") {
            let tw = p["terminal_rect"]["width"].as_f64().unwrap();
            let th = p["terminal_rect"]["height"].as_f64().unwrap();
            p["terminal_rect"]["width"] = json!(tw + 25.0);
            p["terminal_rect"]["height"] = json!(th + 12.0);
        }
    });
    let r = resize(&w).unwrap();
    assert_eq!(
        r[0],
        (RESIZE_CHECKS[0], true),
        "canvas rect should be the evaluator basis, ignoring padded terminal_rect: {r:?}"
    );
}

#[test]
fn resize_pty_is_compared_with_the_pane_inner_rect_not_the_canvas_surface() {
    // Wrong behavior (r1-r4): stty compared with floor(canvas/cell), the whole composed surface.
    // A PTY sized like the surface (one column wider than the pane inner_rect) is not confirmed.
    geometry_false(|k, a, p| {
        if k.starts_with("observe") {
            a["pty_size"] = json!([p["surface"]["rows"], p["surface"]["cols"]]);
        }
    });
    // A page whose pane inner_rect lies (equal to the surface) while stty shows the pane: false.
    geometry_false(|k, _, p| {
        if k == "observe-grow" {
            p["panes"][0]["inner_rect"]["width"] = p["surface"]["cols"].clone();
        }
    });
}

#[test]
fn resize_surface_must_match_canvas_and_contain_the_pane() {
    // Surface cols the frame reported differ from what the canvas paints.
    geometry_false(|k, _, p| {
        if k == "observe-grow" {
            p["surface"]["cols"] = json!(p["surface"]["cols"].as_u64().unwrap() + 1);
        }
    });
    // Pane inner_rect outside the surface (x + width > cols), even with stty equal to it.
    geometry_false(|k, a, p| {
        if k == "observe-start" {
            p["panes"][0]["inner_rect"]["x"] = json!(2);
            a["pty_size"] = json!([p["surface"]["rows"], p["panes"][0]["inner_rect"]["width"]]);
        }
    });
}

#[test]
fn resize_without_frame_geometry_of_the_confirmed_pane_is_an_error() {
    // Wrong behavior: falling back to the canvas surface when the pane geometry was not received.
    let w = resize_world(|k, _, p| {
        if k == "observe-scale2" {
            p["panes"][0]["pane_id"] = json!("w1-p2");
        }
    });
    let e = resize(&w).unwrap_err();
    assert!(e.contains("inner_rect") && e.contains("w1-p1"), "{e}");
    let w = resize_world(|k, _, p| {
        if k == "observe-start" {
            p.as_object_mut().unwrap().remove("surface");
        }
    });
    assert!(resize(&w).unwrap_err().contains("surface"));
}

#[test]
fn resize_missing_or_double_step_is_not_a_pass() {
    // Skipped stage treated as observed.
    let w = resize_world(|k, a, _| {
        if k == "observe-grow" {
            a["step"] = json!("");
        }
    });
    assert!(resize(&w).unwrap_err().contains("resize-observe-grow"));
    // Replayed observation.
    let mut ledger = Ledger::default();
    ledger.record("resize-apply-grow", json!({})).unwrap();
    assert!(ledger.record("resize-apply-grow", json!({})).is_err());
    // Page reporting a stage twice.
    let mut w = resize_world(|_, _, _| {});
    let dup = w.page["stages"][1].clone();
    w.page["stages"].as_array_mut().unwrap().insert(2, dup);
    assert_eq!(resize(&w).unwrap()[0], (RESIZE_CHECKS[0], false));
}

#[test]
fn resize_crisp_requires_fresh_paint_full_frame_and_sharp_screen() {
    let crisp_false = |tweak: &dyn Fn(&str, &mut Value, &mut Value)| {
        let r = resize(&resize_world(tweak)).unwrap();
        assert_eq!(r[1], (RESIZE_CHECKS[1], false), "{r:?}");
    };
    // No repaint after the scale change.
    crisp_false(&|k, _, p| {
        if k == "observe-scale2" {
            p["probe"]["painted_rows"] = json!(200);
        }
    });
    // No Full frame after the resize.
    crisp_false(&|k, _, p| {
        if k == "observe-restore" {
            p["probe"]["full_frames"] = json!(2);
        }
    });
    // Compositor nearest-doubling a 1x buffer.
    crisp_false(&|k, a, _| {
        if k == "observe-scale2" {
            a["screen"]["doubled_fraction"] = json!(0.97);
        }
    });
    // Smoothed upscale: soft edges.
    crisp_false(&|k, a, _| {
        if k == "observe-scale2" {
            a["screen"]["edge_p95"] = json!(90.0);
        }
    });
    // Screenshot of logical size (not the physical output).
    crisp_false(&|k, a, _| {
        if k == "observe-scale2" {
            a["screen"]["width"] = json!(800);
        }
    });
    // Probe without full_frames (current product): error, never true.
    let w = resize_world(|_, _, p| {
        p["probe"].as_object_mut().unwrap().remove("full_frames");
    });
    assert!(resize(&w).unwrap_err().contains("full_frames"));
}

// ---- PPM crispness
fn ppm(w: usize, h: usize, px: impl Fn(usize, usize) -> u8) -> Vec<u8> {
    let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
    for y in 0..h {
        for x in 0..w {
            let v = px(x, y);
            out.extend([v, v, v]);
        }
    }
    out
}

/// 1x glyph pattern: vertical strokes every 3 px, 1 px wide, with horizontal bars.
fn glyph1(x: usize, y: usize) -> bool {
    x.is_multiple_of(3) || (y.is_multiple_of(5) && x % 3 != 2)
}

#[test]
fn crispness_distinguishes_native_doubled_and_smoothed() {
    let rect = Crop {
        x: 0,
        y: 0,
        width: 120,
        height: 80,
    };
    // Native 2x: strokes of odd physical width (not block aligned).
    let native = parse_ppm(&ppm(120, 80, |x, y| {
        if x % 6 < 3 && (x + y) % 7 != 0 {
            230
        } else {
            20
        }
    }))
    .unwrap();
    let doubled = parse_ppm(&ppm(
        120,
        80,
        |x, y| if glyph1(x / 2, y / 2) { 230 } else { 20 },
    ))
    .unwrap();
    let smooth = parse_ppm(&ppm(120, 80, |x, y| {
        let v = |xx: usize| if glyph1(xx / 2, y / 2) { 230u32 } else { 20 };
        ((v(x) + v(x.saturating_sub(1)) + v(x + 1)) / 3) as u8
    }))
    .unwrap();
    let n = crispness(&native, rect).unwrap();
    let d = crispness(&doubled, rect).unwrap();
    let s = crispness(&smooth, rect).unwrap();
    assert!(n.ink_blocks >= 50 && n.doubled_fraction <= 0.5, "{n:?}");
    assert!(d.doubled_fraction > 0.9, "{d:?}");
    assert!(s.edge_p95 < 0.8 * n.edge_p95, "{s:?} vs {n:?}");
    assert!(parse_ppm(b"P5\n1 1\n255\n\0").is_err());
    assert!(parse_ppm(b"P6\n2 2\n255\n\0\0\0").is_err());
    assert!(crispness(
        &native,
        Crop {
            x: 100,
            y: 0,
            width: 40,
            height: 10,
        },
    )
    .is_err());
}

fn nearest_x2_doubled(img: &Image, c: Crop) -> Image {
    let mut doubled = img.clone();
    for y in c.y..c.y + c.height {
        for x in c.x..c.x + c.width {
            let src_x = (x / 2) * 2;
            let src_y = (y / 2) * 2;
            let src_idx = (src_y * img.width + src_x) * 3;
            let dst_idx = (y * img.width + x) * 3;
            doubled.rgb[dst_idx..dst_idx + 3].copy_from_slice(&img.rgb[src_idx..src_idx + 3]);
        }
    }
    doubled
}

#[test]
#[ignore = "requires HERDR_DESKTOP_RAW_PPM_DIR with r1 PPM evidence"]
fn offline_negative_crisp_control_on_raw_r1_ppms() {
    use std::path::Path;
    let dir_str = std::env::var("HERDR_DESKTOP_RAW_PPM_DIR")
        .expect("HERDR_DESKTOP_RAW_PPM_DIR environment variable is required");
    let dir = Path::new(&dir_str);
    assert!(
        dir.exists(),
        "HERDR_DESKTOP_RAW_PPM_DIR directory does not exist: {dir_str}"
    );
    let cases = [
        (
            "start",
            "screen-resize-observe-start.ppm",
            Crop {
                x: 334,
                y: 137,
                width: 675,
                height: 551,
            },
        ),
        (
            "grow",
            "screen-resize-observe-grow.ppm",
            Crop {
                x: 334,
                y: 137,
                width: 999,
                height: 722,
            },
        ),
        (
            "scale2",
            "screen-resize-observe-scale2.ppm",
            Crop {
                x: 124,
                y: 274,
                width: 1440,
                height: 532,
            },
        ),
        (
            "restore",
            "screen-resize-observe-restore.ppm",
            Crop {
                x: 62,
                y: 137,
                width: 1206,
                height: 551,
            },
        ),
    ];
    let mut results = serde_json::Map::new();
    for (name, file, crop) in cases {
        let bytes = std::fs::read(dir.join(file)).expect("read PPM");
        let img = parse_ppm(&bytes).expect("parse PPM");
        let pos = crispness(&img, crop).expect("crispness positive");
        let neg_img = nearest_x2_doubled(&img, crop);
        let neg = crispness(&neg_img, crop).expect("crispness negative");
        println!("[CRISP_OFFLINE] {name}:");
        println!(
            "  positive: ink_blocks={}, doubled_fraction={:.4}, edge_p95={:.1}",
            pos.ink_blocks, pos.doubled_fraction, pos.edge_p95
        );
        println!(
            "  negative: ink_blocks={}, doubled_fraction={:.4}, edge_p95={:.1}",
            neg.ink_blocks, neg.doubled_fraction, neg.edge_p95
        );
        results.insert(
            name.to_string(),
            json!({
                "positive": {
                    "ink_blocks": pos.ink_blocks,
                    "doubled_fraction": pos.doubled_fraction,
                    "edge_p95": pos.edge_p95,
                },
                "negative": {
                    "ink_blocks": neg.ink_blocks,
                    "doubled_fraction": neg.doubled_fraction,
                    "edge_p95": neg.edge_p95,
                }
            }),
        );
        // Positive ink blocks >= 50
        assert!(pos.ink_blocks >= 50, "{name} positive ink blocks < 50");
        // Positive doubled fraction <= 0.5
        assert!(
            pos.doubled_fraction <= 0.5,
            "{name} positive doubled fraction > 0.5"
        );
        // Negative doubled fraction must fail (> 0.5, specifically 1.0 for nearest 2x)
        assert!(
            neg.doubled_fraction > 0.5,
            "{name} negative doubled fraction <= 0.5"
        );
    }
    let evidence_r2 = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("evidencias/007/native-view-live/r2");
    if evidence_r2.exists() {
        let json_path = evidence_r2.join("crisp-negative-control.json");
        std::fs::write(
            &json_path,
            serde_json::to_string_pretty(&Value::Object(results)).unwrap(),
        )
        .expect("write crisp-negative-control.json");
    }
}

// ---- a11y
#[test]
fn key_plan_maps_to_wtype_and_refuses_other_keys() {
    assert_eq!(wtype_args("Tab").unwrap(), ["-k", "Tab"]);
    assert_eq!(
        wtype_args("shift+Tab").unwrap(),
        ["-M", "shift", "-k", "Tab", "-m", "shift"]
    );
    assert_eq!(
        wtype_args("ctrl+shift+F6").unwrap(),
        ["-M", "ctrl", "-M", "shift", "-k", "F6", "-m", "shift", "-m", "ctrl"]
    );
    // Arbitrary text typed into the terminal.
    assert!(wtype_args("rm -rf").is_err());
    // Main sweep (Planner decisions r10 + r12): chord, one Tab per stop (terminal last) plus exactly
    // one Tab for the declared document wrap, chord, Shift+Tab back into the terminal; `expected`
    // lists the terminal twice (N stops + re-entry).
    const F6: &str = "ctrl+shift+F6";
    const MAIN: &str = "a11y-sweep-main";
    let plan = json!([F6, "Tab", "Tab", "Tab", "Tab", F6, "shift+Tab"]);
    // The wrap may follow any stop before the terminal.
    assert!(validate_key_plan(MAIN, &plan, 4, Some(0)).is_ok());
    assert!(validate_key_plan(MAIN, &plan, 4, Some(1)).is_ok());
    assert!(validate_key_plan(MAIN, &plan, 3, Some(0)).is_err());
    assert!(validate_key_plan(MAIN, &plan, 5, Some(0)).is_err());
    // Would catch (r12): the extra Tab accepted without a declaration, or declared on the terminal
    // (the r8/r9 wrap after the last stop) or past the stops.
    assert!(validate_key_plan(MAIN, &plan, 4, None).is_err());
    assert!(validate_key_plan(MAIN, &plan, 4, Some(2)).is_err());
    assert!(validate_key_plan(MAIN, &plan, 4, Some(3)).is_err());
    // Would catch: the r10 plan (no wrap Tab, every stop after the wrap one key late, GUI r11)
    // still accepted once a wrap is declared.
    let r10 = json!([F6, "Tab", "Tab", "Tab", F6, "shift+Tab"]);
    assert!(validate_key_plan(MAIN, &r10, 4, Some(0)).is_err());
    assert!(validate_key_plan(MAIN, &r10, 4, None).is_err());
    // Would catch: the r8/r9 plan (wrap Tab through the end of the document to the first stop,
    // re-entry by Shift+Tab) still accepted with its N+1 expected list, with or without a declaration.
    let wrap = json!([F6, "Tab", "Tab", "Tab", F6, "Tab", "shift+Tab"]);
    for (e, w) in [(4, None), (5, None), (4, Some(0)), (4, Some(1))] {
        assert!(validate_key_plan(MAIN, &wrap, e, w).is_err(), "{e} {w:?}");
    }
    for bad in [
        // Plain Tab plan (r6): Tab kept by the terminal, no exit.
        json!(["Tab", "Tab", "Tab", "Tab", "shift+Tab"]),
        // No lead chord.
        json!(["Tab", "Tab", "Tab", "Tab", F6, "shift+Tab"]),
        // Extra Tab before the lead chord (outside the sweep).
        json!(["Tab", F6, "Tab", "Tab", "Tab", F6, "shift+Tab"]),
        // Two extra Tabs.
        json!([F6, "Tab", "Tab", "Tab", "Tab", "Tab", F6, "shift+Tab"]),
        // Extra Tab after the closing Shift+Tab (outside the sweep).
        json!([F6, "Tab", "Tab", "Tab", "Tab", F6, "shift+Tab", "Tab"]),
        // Terminal left mid-sweep (second chord not on the last stop).
        json!([F6, "Tab", "Tab", "Tab", F6, "Tab", "shift+Tab"]),
        // A third chord.
        json!([F6, "Tab", "Tab", "Tab", "Tab", F6, F6, "shift+Tab"]),
        // Shift+Tab not last.
        json!([F6, "Tab", "Tab", "Tab", "Tab", "shift+Tab", F6]),
    ] {
        assert!(validate_key_plan(MAIN, &bad, 4, Some(0)).is_err(), "{bad}");
    }
    assert!(validate_key_plan(
        MAIN,
        &json!([F6, "Tab", "Tab", F6, "shift+Tab"]),
        2,
        Some(0)
    )
    .is_err());
    // The modal keeps the plain plan; the main shape is not a dialog plan, and only the main sweep
    // declares a wrap.
    let dialog = json!(["Tab", "Tab", "Tab", "shift+Tab"]);
    assert!(validate_key_plan("a11y-sweep-dialog", &dialog, 3, None).is_ok());
    assert!(validate_key_plan("a11y-sweep-dialog", &dialog, 3, Some(0)).is_err());
    assert!(validate_key_plan("a11y-sweep-dialog", &r10, 4, None).is_err());
    assert!(validate_key_plan("a11y-open-dialog", &json!(["Return"]), 0, None).is_ok());
    assert!(validate_key_plan("a11y-open-dialog", &json!(["Tab"]), 0, None).is_err());
    // Close step: exactly one native Escape.
    assert_eq!(wtype_args("Escape").unwrap(), ["-k", "Escape"]);
    assert!(validate_key_plan("a11y-close-dialog", &json!(["Escape"]), 0, None).is_ok());
    for bad in [json!([]), json!(["Return"]), json!(["Escape", "Escape"])] {
        assert!(
            validate_key_plan("a11y-close-dialog", &bad, 0, None).is_err(),
            "{bad}"
        );
    }
    // Escape inside a sweep would close the dialog mid-measure.
    let esc = json!(["Tab", "Escape", "Tab", "Tab", "shift+Tab"]);
    assert!(validate_key_plan("a11y-sweep-dialog", &esc, 3, None).is_err());
}

/// Page declaration of the main sweep (r12): expected stops `ids` (+ the terminal again), the
/// `following`/`preceding` id split and `wrap_after`.
fn wrap_detail(ids: &[&str], following: &[&str], preceding: &[&str], wrap_after: Value) -> Value {
    let mut expected: Vec<Value> = ids.iter().map(|i| json!({"id": i})).collect();
    if let Some(last) = expected.last().cloned() {
        expected.push(last);
    }
    json!({"expected": expected, "wrap_after": wrap_after,
        "split": {"following": following, "preceding": preceding}})
}

// Planner decision r12. Would catch: a declared index trusted without the split (off by one, on the
// terminal), a split that is not the page's expected stops in order, an empty side, or a missing
// declaration read as "no wrap".
#[test]
fn main_wrap_checks_declared_index_against_the_split() {
    let ids = ["copy", "a", "opener", "term"];
    let ok = |d: &Value| main_wrap(d, d["expected"].as_array().unwrap());
    assert_eq!(
        ok(&wrap_detail(
            &ids,
            &["copy", "a"],
            &["opener", "term"],
            json!(1)
        )),
        Ok(1)
    );
    assert_eq!(
        ok(&wrap_detail(
            &ids,
            &["copy"],
            &["a", "opener", "term"],
            json!(0)
        )),
        Ok(0)
    );
    for (why, d) in [
        (
            "index before the last following stop",
            wrap_detail(&ids, &["copy", "a"], &["opener", "term"], json!(0)),
        ),
        (
            "index on the first preceding stop",
            wrap_detail(&ids, &["copy", "a"], &["opener", "term"], json!(2)),
        ),
        (
            "index on the terminal",
            wrap_detail(&ids, &["copy", "a", "opener"], &["term"], json!(3)),
        ),
        (
            "negative index",
            wrap_detail(&ids, &["copy", "a"], &["opener", "term"], json!(-1)),
        ),
        (
            "index not a number",
            wrap_detail(&ids, &["copy", "a"], &["opener", "term"], json!("1")),
        ),
        (
            "split out of the expected order",
            wrap_detail(&ids, &["a", "copy"], &["opener", "term"], json!(1)),
        ),
        (
            "split missing a stop",
            wrap_detail(&ids, &["copy"], &["opener", "term"], json!(0)),
        ),
        (
            "split with an extra stop",
            wrap_detail(&ids, &["copy", "a", "x"], &["opener", "term"], json!(2)),
        ),
        (
            "nothing follows the marker",
            wrap_detail(&ids, &[], &["copy", "a", "opener", "term"], json!(0)),
        ),
        (
            "nothing precedes (no terminal after the wrap)",
            wrap_detail(&ids, &["copy", "a", "opener", "term"], &[], json!(3)),
        ),
    ] {
        assert!(ok(&d).is_err(), "{why}");
    }
    // Declaration fields are required and named in the error.
    for k in ["wrap_after", "split"] {
        let mut d = wrap_detail(&ids, &["copy", "a"], &["opener", "term"], json!(1));
        d.as_object_mut().unwrap().remove(k);
        let e = ok(&d).unwrap_err();
        assert!(e.contains(k), "{e}");
    }
    let mut d = wrap_detail(&ids, &["copy", "a"], &["opener", "term"], json!(1));
    d["split"]["preceding"] = json!("opener,term");
    assert!(ok(&d).is_err());
}

fn ctl(id: &str, name: &str) -> Value {
    json!({"id":id,"tag":"button","role":"button","name":name,"name_source":"text",
        "disabled":false,"tabindex":0,"focus_visible":true,
        "style":{"outline_style":"solid","outline_width":"2px","outline_color":"rgb(80, 160, 255)",
                 "box_shadow":"none","border_color":"rgb(0, 0, 0)","background_color":"rgb(0, 0, 0)"},
        "baseline":{"outline_style":"none","outline_width":"0px","outline_color":"rgb(0, 0, 0)",
                 "box_shadow":"none","border_color":"rgb(0, 0, 0)","background_color":"rgb(0, 0, 0)"}})
}

fn key(k: &str, shift: bool) -> Value {
    json!({"key":k,"shift":shift,"ctrl":false,"trusted":true})
}

fn chord() -> Vec<Value> {
    vec![
        json!({"key":"Control","shift":false,"ctrl":true,"trusted":true}),
        json!({"key":"Shift","shift":true,"ctrl":true,"trusted":true}),
        json!({"key":"F6","shift":true,"ctrl":true,"trusted":true}),
    ]
}

/// Main sweep of the r10 plan (Planner decision): the last id is the stop the exit chord leaves
/// (the terminal in the page); every stop is focused once, then the chord leaves the last one and
/// Shift+Tab returns into it, so it is expected and focused again at the end.
/// r12: the first stop is the only one following the exit marker (`wrap_after` 0) and one extra
/// Tab is seen for the document wrap (no focusin for it).
fn sweep(ids: &[&str]) -> Value {
    let mut expected: Vec<Value> = ids
        .iter()
        .map(|i| ctl(i, &format!("Controle {i}")))
        .collect();
    expected.push(expected[ids.len() - 1].clone());
    let focus = expected.clone();
    let mut keys = chord();
    keys.extend(ids.iter().map(|_| key("Tab", false)));
    keys.push(key("Tab", false));
    keys.extend(chord());
    keys.push(key("Shift", true));
    keys.push(key("Tab", true));
    json!({"identity":id(),"expected":expected,"focus":focus,"keys_seen":keys,
        "wrap_after":0,"split":{"following":&ids[..1],"preceding":&ids[1..]}})
}

/// Parent key plan of [`sweep`] with `n` stops (one extra Tab for the declared wrap).
fn main_plan(n: usize) -> Value {
    let mut k = vec![json!("ctrl+shift+F6")];
    k.extend(vec![json!("Tab"); n + 1]);
    k.push(json!("ctrl+shift+F6"));
    k.push(json!("shift+Tab"));
    json!(k)
}

/// Dialog sweep of an autofocus modal (Planner precision 2026-09-17, fixture narrowed on
/// purpose): the product focuses the first field INSIDE the modal on open (never the harness),
/// so N Tabs visit start+1.., wrap and end back on start, and shift+Tab reaches start-1 (wrapping).
/// The previous fixture modelled focus still outside (on the opener) and first..last, last-1,
/// which rejects a correct modal. Three controls so forward/backward and wrap are distinct.
fn modal_sweep(ids: &[&str], start: usize) -> Value {
    let expected: Vec<Value> = ids.iter().map(|i| ctl(i, &format!("Campo {i}"))).collect();
    let n = ids.len();
    let mut focus: Vec<Value> = (1..=n).map(|k| expected[(start + k) % n].clone()).collect();
    focus.push(expected[(start + n - 1) % n].clone());
    let mut keys: Vec<Value> = ids.iter().map(|_| key("Tab", false)).collect();
    keys.push(key("Shift", true));
    keys.push(key("Tab", true));
    json!({"identity":id(),"expected":expected,"focus":focus,"keys_seen":keys})
}

/// Actual element carrying the open dialog, as the page records it after the native Return.
fn modal() -> Value {
    json!({"tag":"dialog","role":"dialog","name":"Conectar a um servidor herdr",
        "name_source":"aria-labelledby","modal":true})
}

fn a11y_world() -> (Value, Ledger) {
    let mut ledger = Ledger::default();
    let plan = |n: usize| {
        let mut k = vec![json!("Tab"); n];
        k.push(json!("shift+Tab"));
        k
    };
    ledger
        .record(
            "a11y-sweep-main",
            json!({"step":"a11y-sweep-main","identity":id(),"keys":main_plan(3)}),
        )
        .unwrap();
    ledger
        .record(
            "a11y-open-dialog",
            json!({"step":"a11y-open-dialog","identity":id(),"keys":["Return"]}),
        )
        .unwrap();
    ledger
        .record(
            "a11y-sweep-dialog",
            json!({"step":"a11y-sweep-dialog","identity":id(),"keys":plan(3)}),
        )
        .unwrap();
    ledger
        .record(
            "a11y-close-dialog",
            json!({"step":"a11y-close-dialog","identity":id(),"keys":["Escape"]}),
        )
        .unwrap();
    let page = json!({"error":null,"main":sweep(&["a", "b", "c"]),"dialog_open_keys":[key("Enter", false)],
        "dialog_opener":ctl("opener", "Nova conexão SSH"),
        "dialog_start_focus":ctl("d", "Campo d"),"dialog_modal":modal(),
        "dialog_close":{"identity":id(),"keys_seen":[key("Escape", false)],"closed":true,
            "focus_after":ctl("opener", "Nova conexão SSH")},
        "dialog":modal_sweep(&["d", "e", "f"], 0),
        "states":[{"status":"working","own_text":"Trabalhando","carrier_text":""},
                  {"status":"online","own_text":"","carrier_text":"Conexão: local (LOCAL) - online"}]});
    (page, ledger)
}

fn a11y(page: &Value, ledger: &Ledger) -> Vec<(&'static str, bool)> {
    a11y_checks(A11Y_PHASE, page, ledger, &exp()).unwrap()
}

#[test]
fn a11y_positive_passes_all_checks() {
    let (page, ledger) = a11y_world();
    assert_eq!(
        a11y(&page, &ledger),
        A11Y_CHECKS.map(|c| (c, true)).to_vec()
    );
}

/// Main sweep that meets the terminal: `order` is the expected list the page reports, `focus_ids`
/// the focus it recorded and `plan` the keys the parent pressed; the page declares the wrap after
/// its first stop (`split` following = first id, preceding = the other distinct stops).
fn terminal_main(order: &[&str], focus_ids: &[&str], plan: &[&str]) -> (Value, Ledger) {
    let n = order.len().saturating_sub(1).max(1);
    let (following, preceding) = order[..n].split_at(1);
    terminal_main_wrap(order, focus_ids, plan, json!(0), following, preceding)
}

fn terminal_main_wrap(
    order: &[&str],
    focus_ids: &[&str],
    plan: &[&str],
    wrap_after: Value,
    following: &[&str],
    preceding: &[&str],
) -> (Value, Ledger) {
    let (mut page, mut ledger) = a11y_world();
    let control = |i: &str| {
        let mut c = ctl(i, &format!("Controle {i}"));
        if i == "term" {
            c["tag"] = json!("textarea");
            c["classes"] = json!(["ime-target"]);
        }
        c
    };
    let seen: Vec<Value> = plan
        .iter()
        .map(|k| match *k {
            "ctrl+shift+F6" => json!({"key":"F6","shift":true,"ctrl":true,"trusted":true}),
            "shift+Tab" => key("Tab", true),
            _ => key("Tab", false),
        })
        .collect();
    page["main"] = json!({"identity":id(),
        "expected":order.iter().map(|i| control(i)).collect::<Vec<_>>(),
        "focus":focus_ids.iter().map(|i| control(i)).collect::<Vec<_>>(),
        "keys_seen":seen,"wrap_after":wrap_after,
        "split":{"following":following,"preceding":preceding}});
    ledger.steps.insert(
        "a11y-sweep-main".into(),
        json!({"step":"a11y-sweep-main","identity":id(),"keys":plan}),
    );
    (page, ledger)
}

// Planner decision r10 (GUI r9: the wrap Tab through the end of the document re-entered by host
// behaviour and the second chord after it reached the primary button, never the terminal). Would
// catch: the evaluator refusing the r10 plan (still wanting the N+2 re-entry count), or still
// accepting the r8/r9 plan with the wrap Tab and the first stop listed again.
#[test]
fn a11y_main_sweep_leaves_the_terminal_with_the_exit_chord() {
    const F6: &str = "ctrl+shift+F6";
    // r12: one extra Tab for the wrap declared after the first stop.
    let plan = [F6, "Tab", "Tab", "Tab", "Tab", F6, "shift+Tab"];
    let (page, ledger) = terminal_main(
        &["copy", "a", "term", "term"],
        &["copy", "a", "term", "term"],
        &plan,
    );
    assert_eq!(
        a11y(&page, &ledger),
        A11Y_CHECKS.map(|c| (c, true)).to_vec()
    );
    let focus_fails = |why: &str, order: &[&str], focus: &[&str], plan: &[&str]| {
        let (page, ledger) = terminal_main(order, focus, plan);
        assert!(!a11y(&page, &ledger)[0].1, "{why}");
    };
    // r8/r9 plan with focus as the old evaluator wanted it: the wrap plan is refused.
    let r9 = [F6, "Tab", "Tab", "Tab", "Tab", F6, "Tab", "shift+Tab"];
    focus_fails(
        "wrap plan",
        &["copy", "a", "term", "copy"],
        &["copy", "a", "term", "copy", "term"],
        &r9,
    );
    // r9 as observed: the re-entry never happened.
    focus_fails(
        "r9 observed",
        &["copy", "a", "term", "copy"],
        &["copy", "a", "term"],
        &r9,
    );
    // r10 plan with the first stop listed as the re-entry (wrap expectation kept by the page).
    focus_fails(
        "re-entry listed as the first stop",
        &["copy", "a", "term", "copy"],
        &["copy", "a", "term", "copy"],
        &plan,
    );
    // Second chord did not leave the terminal's marker: Shift+Tab reached the stop before it.
    focus_fails(
        "Shift+Tab landed before the terminal",
        &["copy", "a", "term", "term"],
        &["copy", "a", "term", "a"],
        &plan,
    );
    // A stop proven twice inside the sweep (not each control once).
    focus_fails(
        "stop repeated",
        &["copy", "a", "copy", "term", "term"],
        &["copy", "a", "copy", "term", "term"],
        &[F6, "Tab", "Tab", "Tab", "Tab", "Tab", F6, "shift+Tab"],
    );
    // Terminal left without the re-entry stop.
    focus_fails(
        "re-entry missing",
        &["copy", "a", "term", "term"],
        &["copy", "a", "term"],
        &plan,
    );
    // r7 as observed: plain plan with the terminal before the toolbar; Tab and Shift+Tab stayed in
    // the PTY, so the toolbar never took focus.
    let r7 = [F6, "Tab", "Tab", "Tab", "Tab", "shift+Tab"];
    focus_fails("r7", &["a", "b", "term", "copy"], &["a", "b", "term"], &r7);
    // r11 as observed: the r10 plan (no wrap Tab) lost one stop; with the declaration the plan is
    // refused even if the focus had matched.
    let r10 = [F6, "Tab", "Tab", "Tab", F6, "shift+Tab"];
    focus_fails(
        "r10 plan under a declared wrap",
        &["copy", "a", "term", "term"],
        &["copy", "a", "term", "term"],
        &r10,
    );
}

// Planner decision r12. Would catch: the evaluator accepting the extra Tab without the page's
// declaration, at an index other than the last following stop, or with a split that does not match
// the expected stops; want itself stays each stop once + the terminal again.
#[test]
fn a11y_main_sweep_accepts_one_declared_wrap_tab_only() {
    const F6: &str = "ctrl+shift+F6";
    let order = ["copy", "a", "opener", "term", "term"];
    let plan = [F6, "Tab", "Tab", "Tab", "Tab", "Tab", F6, "shift+Tab"];
    let run = |wrap: Value, following: &[&str], preceding: &[&str]| {
        let (page, ledger) = terminal_main_wrap(&order, &order, &plan, wrap, following, preceding);
        a11y(&page, &ledger)
    };
    let pass = A11Y_CHECKS.map(|c| (c, true)).to_vec();
    // The real layout splits after the toolbar: index 1 here, index 0 in the other fixtures.
    assert_eq!(run(json!(1), &["copy", "a"], &["opener", "term"]), pass);
    assert_eq!(run(json!(0), &["copy"], &["a", "opener", "term"]), pass);
    let fails = |why: &str, r: Vec<(&'static str, bool)>| {
        assert!(!r[0].1, "{why}: {r:?}");
        assert!(r[1].1 && r[2].1, "{why}: only focus fails: {r:?}");
    };
    fails(
        "index inconsistent with the split",
        run(json!(0), &["copy", "a"], &["opener", "term"]),
    );
    fails(
        "index on the terminal",
        run(json!(3), &["copy", "a", "opener"], &["term"]),
    );
    fails(
        "split not the expected stops",
        run(json!(1), &["copy", "opener"], &["a", "term"]),
    );
    fails(
        "nothing follows the marker",
        run(json!(0), &[], &["copy", "a", "opener", "term"]),
    );
    // Two extra Tabs (plan and keys seen agree): refused.
    let (page, ledger) = terminal_main_wrap(
        &order,
        &order,
        &[
            F6,
            "Tab",
            "Tab",
            "Tab",
            "Tab",
            "Tab",
            "Tab",
            F6,
            "shift+Tab",
        ],
        json!(1),
        &["copy", "a"],
        &["opener", "term"],
    );
    fails("two extra Tabs", a11y(&page, &ledger));
    // Extra Tab without declaration: harness error naming the missing field, never a verdict.
    for k in ["wrap_after", "split"] {
        let (mut page, ledger) = terminal_main_wrap(
            &order,
            &order,
            &plan,
            json!(1),
            &["copy", "a"],
            &["opener", "term"],
        );
        page["main"].as_object_mut().unwrap().remove(k);
        let e = a11y_checks(A11Y_PHASE, &page, &ledger, &exp()).unwrap_err();
        assert!(e.contains(k), "{e}");
    }
    // Would catch: the main declaration applied to the dialog sweep (its plain plan refused).
    let (mut page, ledger) = a11y_world();
    page["dialog"]["wrap_after"] = json!(0);
    page["dialog"]["split"] = json!({"following":["d"],"preceding":["e","f"]});
    assert_eq!(a11y(&page, &ledger), pass);
}

#[test]
fn a11y_negatives() {
    let only = |i: usize, tweak: &dyn Fn(&mut Value)| {
        let (mut page, ledger) = a11y_world();
        tweak(&mut page);
        let r = a11y(&page, &ledger);
        assert!(!r[i].1, "{r:?}");
    };
    // Focus lands without any visible indicator.
    only(0, &|p| {
        let base = p["main"]["focus"][1]["baseline"].clone();
        p["main"]["focus"][1]["style"] = base;
    });
    // :focus-visible not matched (mouse-like focus).
    only(0, &|p| {
        p["dialog"]["focus"][0]["focus_visible"] = json!(false)
    });
    // Synthetic (untrusted) Tab.
    only(0, &|p| p["main"]["keys_seen"][0]["trusted"] = json!(false));
    // Sweep skipped a control.
    only(0, &|p| {
        p["main"]["focus"][1] = p["main"]["expected"][2].clone()
    });
    // Disabled element reachable by Tab.
    only(0, &|p| p["main"]["focus"][2]["disabled"] = json!(true));
    // Icon-only control without accessible name.
    only(1, &|p| {
        p["main"]["focus"][0]["name"] = json!("×");
        p["main"]["focus"][0]["name_source"] = json!("text");
    });
    // Status conveyed by color only.
    only(2, &|p| {
        p["states"][1]["carrier_text"] = json!("local · LOCAL")
    });
    // No status observed at all is not a pass.
    only(2, &|p| p["states"] = json!([]));
}

#[test]
fn a11y_dialog_order_follows_recorded_start_focus() {
    // Would catch: an evaluator that ignores dialog_start_focus and hard-codes index 0 or the
    // old outside-focus premise. Start on the middle and on the last field both pass.
    for start in [1, 2] {
        let (mut page, ledger) = a11y_world();
        let ids = ["d", "e", "f"];
        page["dialog"] = modal_sweep(&ids, start);
        page["dialog_start_focus"] = ctl(ids[start], &format!("Campo {}", ids[start]));
        assert_eq!(
            a11y(&page, &ledger),
            A11Y_CHECKS.map(|c| (c, true)).to_vec(),
            "start {start}"
        );
    }
}

#[test]
fn a11y_modal_dialog_negatives() {
    let only = |i: usize, why: &str, tweak: &dyn Fn(&mut Value)| {
        let (mut page, ledger) = a11y_world();
        tweak(&mut page);
        let r = a11y(&page, &ledger);
        assert!(!r[i].1, "{why}: {r:?}");
        assert!(
            r.iter().enumerate().all(|(j, c)| j == i || c.1),
            "{why}: only {i} fails: {r:?}"
        );
    };
    only(0, "no initial focus (body focused after open)", &|p| {
        p["dialog_start_focus"] = Value::Null
    });
    // Old premise: focus stays on the opener outside the modal, then first..last, last-1.
    only(0, "initial focus outside the modal", &|p| {
        p["dialog_start_focus"] = ctl("opener", "Nova conexão SSH");
        let ids = ["d", "e", "f", "e"];
        p["dialog"]["focus"] = json!(ids.map(|i| ctl(i, &format!("Campo {i}"))));
    });
    only(
        0,
        "recorded start is not where the Tab stream started",
        &|p| p["dialog_start_focus"] = ctl("e", "Campo e"),
    );
    only(0, "last Tab escapes the modal instead of wrapping", &|p| {
        p["dialog"]["focus"][2] = ctl("a", "Controle a")
    });
    only(
        0,
        "shift+Tab moves forward instead of wrapping back",
        &|p| p["dialog"]["focus"][3] = ctl("e", "Campo e"),
    );
    only(0, "Tab skipped a dialog control", &|p| {
        p["dialog"]["focus"][0] = ctl("f", "Campo f")
    });
    only(0, "disabled initial focus", &|p| {
        p["dialog_start_focus"]["disabled"] = json!(true)
    });
    only(0, "non-modal dialog (rest of the window reachable)", &|p| {
        p["dialog_modal"]["modal"] = json!(false)
    });
    only(0, "form without dialog role", &|p| {
        p["dialog_modal"]["role"] = json!("")
    });
    only(1, "unnamed modal", &|p| {
        p["dialog_modal"]["name"] = json!("");
        p["dialog_modal"]["name_source"] = json!("none");
    });
    only(1, "unnamed initial focus", &|p| {
        p["dialog_start_focus"]["name"] = json!(" ");
        p["dialog_start_focus"]["name_source"] = json!("none");
    });
    // Fields the page must always write: absent is a harness error, never a verdict.
    for k in ["dialog_start_focus", "dialog_modal"] {
        let (mut page, ledger) = a11y_world();
        page.as_object_mut().unwrap().remove(k);
        let e = a11y_checks(A11Y_PHASE, &page, &ledger, &exp()).unwrap_err();
        assert!(e.contains(k), "{e}");
    }
}

#[test]
fn a11y_close_dialog_negatives() {
    let only = |i: usize, why: &str, tweak: &dyn Fn(&mut Value)| {
        let (mut page, ledger) = a11y_world();
        tweak(&mut page);
        let r = a11y(&page, &ledger);
        assert!(!r[i].1, "{why}: {r:?}");
        assert!(
            r.iter().enumerate().all(|(j, c)| j == i || c.1),
            "{why}: only {i} fails: {r:?}"
        );
    };
    only(0, "Escape never reached the page", &|p| {
        p["dialog_close"]["keys_seen"] = json!([])
    });
    only(0, "synthetic (untrusted) Escape", &|p| {
        p["dialog_close"]["keys_seen"][0]["trusted"] = json!(false)
    });
    only(0, "another key seen instead of Escape", &|p| {
        p["dialog_close"]["keys_seen"] = json!([key("Enter", false)])
    });
    only(0, "dialog still open after Escape", &|p| {
        p["dialog_close"]["closed"] = json!(false)
    });
    only(0, "focus restored to another control", &|p| {
        p["dialog_close"]["focus_after"] = ctl("a", "Controle a")
    });
    only(0, "focus left on a dialog field", &|p| {
        p["dialog_close"]["focus_after"] = ctl("d", "Campo d")
    });
    only(0, "focus lost to the host/body", &|p| {
        p["dialog_close"]["focus_after"] = Value::Null
    });
    only(0, "opener not recorded", &|p| {
        p["dialog_opener"] = Value::Null
    });
    only(0, "pane switched before the close observation", &|p| {
        p["dialog_close"]["identity"]["pane_id"] = json!("w1-p9")
    });
    // Missing page field or parent step is a harness error, never a verdict.
    let (mut page, ledger) = a11y_world();
    page.as_object_mut().unwrap().remove("dialog_close");
    let e = a11y_checks(A11Y_PHASE, &page, &ledger, &exp()).unwrap_err();
    assert!(e.contains("dialog_close"), "{e}");
    let (page, mut ledger) = a11y_world();
    ledger.steps.remove("a11y-close-dialog");
    let e = a11y_checks(A11Y_PHASE, &page, &ledger, &exp()).unwrap_err();
    assert!(e.contains("a11y-close-dialog"), "{e}");
}

#[test]
fn parent_close_dialog_refused_before_dialog_sweep() {
    let (page, _) = a11y_world();
    let (mut view, mut ledger) = (fake(), Ledger::default());
    for (step, detail) in [
        ("a11y-sweep-main", a11y_detail(&page, "main", main_plan(3))),
        ("a11y-open-dialog", flat(&id(), json!({"keys":["Return"]}))),
    ] {
        parent_step(step, &detail, &exp(), &mut view, &mut no_pty(), &mut ledger).unwrap();
    }
    let pressed = view.calls.len();
    let close = flat(&id(), json!({"keys":["Escape"]}));
    let e = parent_step(
        "a11y-close-dialog",
        &close,
        &exp(),
        &mut view,
        &mut no_pty(),
        &mut ledger,
    )
    .unwrap_err();
    assert!(e.contains("a11y-sweep-dialog"), "{e}");
    assert_eq!(view.calls.len(), pressed, "nothing pressed");
}

// ---- parent adapters (parent_step + ViewEnv) over a fake private compositor/keyboard/PTY.

/// Fake private world: sway state changes only when `applies`; grim returns the actual mode
/// (or `stale_screen`) with sharp ink only inside INK; every call is logged.
struct FakeView {
    socket: String,
    mode: (u32, u32, u32),
    applies: bool,
    swaymsg_fails: bool,
    stale_screen: Option<(usize, usize)>,
    wtype_fails_at: Option<usize>,
    /// Window x on the output and the pid the tree reports for it.
    window_x: u32,
    tree_pid: u32,
    calls: Vec<Vec<String>>,
}

/// Ink region (physical px) inside the canvas crop of every stage × DPR, but outside the
/// scale2 canvas rect taken WITHOUT the DPR (594×342).
const INK: (usize, usize, usize, usize) = (700, 400, 1060, 600);

fn fake() -> FakeView {
    FakeView {
        socket: SOCK.into(),
        mode: (1280, 720, 1),
        applies: true,
        swaymsg_fails: false,
        stale_screen: None,
        wtype_fails_at: None,
        window_x: 0,
        tree_pid: WINDOW_PID,
        calls: Vec::new(),
    }
}

fn ran(argv: &[String], ok: bool, stdout: Vec<u8>, stderr: &str) -> Run {
    Run {
        argv: argv.to_vec(),
        status: if ok {
            "exit status: 0"
        } else {
            "exit status: 1"
        }
        .into(),
        success: ok,
        stdout,
        stderr: stderr.into(),
    }
}

impl ViewWorld for FakeView {
    fn socket(&self) -> String {
        self.socket.clone()
    }
    fn swaymsg(&mut self, args: &[String]) -> Result<Run, String> {
        self.calls.push(args.to_vec());
        if self.swaymsg_fails {
            return Ok(ran(args, false, vec![], "Error: output HEADLESS-1 unknown"));
        }
        if args[2..] == ["-t", "get_tree", "-r"] {
            // Window (logical) covers the output from window_x; the page client is 37 px shorter
            // (stage_view), i.e. a 37 px GTK header.
            let (w, h, s) = self.mode;
            let (lw, lh) = (w / s, h / s);
            let r =
                |x: u32, y: u32, w: u32, h: u32| json!({"x": x, "y": y, "width": w, "height": h});
            let tree = json!({"type": "root", "nodes": [{"type": "output", "name": "HEADLESS-1",
                "rect": r(0, 0, lw, lh), "nodes": [{"type": "workspace", "nodes": [
                    {"type": "con", "pid": self.tree_pid, "rect": r(self.window_x, 0, lw, lh),
                     "window_rect": r(0, 0, lw, lh), "nodes": [], "floating_nodes": []}]}]}]});
            return Ok(ran(args, true, tree.to_string().into_bytes(), ""));
        }
        if args[2..] == ["-t", "get_outputs", "-r"] {
            let (w, h, s) = self.mode;
            let (outputs, _, _) = stage_view(w, h, s, 0, 0);
            return Ok(ran(args, true, outputs.to_string().into_bytes(), ""));
        }
        let res: Vec<u32> = args[5].split('x').map(|n| n.parse().unwrap()).collect();
        if self.applies {
            self.mode = (res[0], res[1], args[7].parse().unwrap());
        }
        Ok(ran(args, true, b"[{\"success\": true}]".to_vec(), ""))
    }
    fn grim_ppm(&mut self) -> Result<Run, String> {
        let argv = ["grim", "-t", "ppm", "-o", "HEADLESS-1", "-"].map(str::to_owned);
        self.calls.push(argv.to_vec());
        let (w, h) = self
            .stale_screen
            .unwrap_or((self.mode.0 as usize, self.mode.1 as usize));
        let img = ppm(w, h, |x, y| {
            let inside = (INK.0..INK.2).contains(&x) && (INK.1..INK.3).contains(&y);
            if inside && glyph1(x, y) {
                230
            } else {
                10
            }
        });
        Ok(ran(&argv, true, img, ""))
    }
    /// Fake time: nothing waits and nothing is recorded (FocusView records its delays).
    fn sleep_ms(&mut self, _ms: u64) {}
    fn wtype(&mut self, args: &[String]) -> Result<(), String> {
        let n = self.calls.len();
        self.calls.push(args.to_vec());
        if self.wtype_fails_at == Some(n) {
            return Err(format!("wtype {args:?}: exit status: 1"));
        }
        Ok(())
    }
}

fn flat(identity: &Value, extra: Value) -> Value {
    let mut d = identity.clone();
    d.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    d
}

/// The PTY's actual `stty size` text for the fake's current mode (what a resized pane reports).
fn stty(view: &FakeView) -> String {
    let (w, h, s) = view.mode;
    let (_, _, pty) = stage_view(w, h, s, 0, 0);
    format!("{} {}\n", pty[0], pty[1])
}

struct Driven {
    page: Value,
    ledger: Ledger,
    results: Vec<Result<Value, String>>,
    view: FakeView,
}

/// Runs the seven resize steps as the page would (detail = identity + stage + view) and builds
/// the page report the evaluator reads.
fn drive_resize(mut view: FakeView, stty_of: impl Fn(&FakeView, &str) -> String) -> Driven {
    let (mut ledger, mut stages, mut results) = (Ledger::default(), Vec::new(), Vec::new());
    for (i, st) in STAGES.iter().enumerate() {
        let (_, mut page, _) = stage_view(st.width, st.height, st.scale, 200 * i as u64, i as u64);
        if st.name != "start" {
            let detail = flat(&id(), json!({"stage": st.name, "view": page.clone()}));
            let step = format!("resize-apply-{}", st.name);
            let mut noop = |_: &str| Err::<String, String>("not asked".into());
            results.push(parent_step(
                &step,
                &detail,
                &exp(),
                &mut view,
                &mut noop,
                &mut ledger,
            ));
        }
        let detail = flat(&id(), json!({"stage": st.name, "view": page.clone()}));
        let step = format!("resize-observe-{}", st.name);
        let snapshot = stty_of(&view, st.name);
        let mut asked = Vec::new();
        let mut pty = |pane: &str| {
            asked.push(pane.to_owned());
            Ok(snapshot.clone())
        };
        results.push(parent_step(
            &step,
            &detail,
            &exp(),
            &mut view,
            &mut pty,
            &mut ledger,
        ));
        assert_eq!(
            asked,
            ["w1-p1"],
            "PTY size is read once for the confirmed pane"
        );
        page["stage"] = json!(st.name);
        stages.push(page);
    }
    Driven {
        page: json!({"stages": stages, "error": null}),
        ledger,
        results,
        view,
    }
}

fn no_pty() -> impl FnMut(&str) -> Result<String, String> {
    |_: &str| Err("PTY must not be read".into())
}

#[test]
fn parent_resize_drives_private_output_and_records_actual_observations() {
    let d = drive_resize(fake(), |v, _| stty(v));
    for r in &d.results {
        assert!(r.is_ok(), "{r:?}");
    }
    // Only the private socket, the literal command per stage, get_outputs after each command.
    let sway: Vec<&Vec<String>> = d.view.calls.iter().filter(|c| c[0] == "-s").collect();
    assert!(sway.iter().all(|c| c[1] == SOCK));
    let commands: Vec<Vec<String>> = sway
        .iter()
        .filter(|c| c[2] == "output")
        .map(|c| c[2..].to_vec())
        .collect();
    let want: Vec<Vec<String>> = STAGES.iter().filter_map(output_command).collect();
    assert_eq!(commands, want);
    let scale2 = &d.ledger.steps["resize-observe-scale2"];
    assert_eq!(scale2["pty_stty"], "19 65\n");
    assert_eq!(scale2["pty_size"], json!([19, 65]));
    // The frame geometry the page received is recorded next to stty: surface vs pane inner_rect.
    assert_eq!(scale2["surface_cols"], 66);
    assert_eq!(scale2["surface_rows"], 19);
    assert_eq!(scale2["pane_cols"], 65);
    assert_eq!(scale2["pane_rows"], 19);
    assert_eq!(scale2["screen"]["width"], 1600, "physical PPM size");
    assert!(scale2["screen"]["ink_blocks"].as_u64().unwrap() >= MIN_INK_BLOCKS);
    assert_eq!(scale2["screen"]["crop_origin"]["source"], "sway get_tree");
    assert_eq!(scale2["screen"]["crop_origin"]["header"], 37.0);
    assert_eq!(
        resize_checks(RESIZE_PHASE, &d.page, &d.ledger, &exp()).unwrap(),
        RESIZE_CHECKS.map(|c| (c, true)).to_vec()
    );
}

#[test]
fn parent_resize_crop_uses_canvas_rect_times_dpr() {
    // Wrong behavior: cropping the scale2 screenshot at the CSS rect (594×342) sees no ink, so
    // the crispness of the actual glyphs would never be measured.
    let d = drive_resize(fake(), |v, _| stty(v));
    let s = &d.ledger.steps["resize-observe-scale2"]["screen"];
    assert!(s["ink_blocks"].as_u64().unwrap() >= MIN_INK_BLOCKS, "{s}");
    let crop = &s["crop"];
    let covers = |c: &Value| {
        let (x, y) = (c["x"].as_u64().unwrap(), c["y"].as_u64().unwrap());
        x + c["width"].as_u64().unwrap() >= INK.2 as u64
            && y + c["height"].as_u64().unwrap() >= INK.3 as u64
    };
    assert!(covers(crop), "{crop}");
}

#[test]
fn parent_resize_crop_is_the_observed_client_origin_and_header_times_output_scale() {
    // Wrong behavior (r2): crop origin assumed at output (0,0), ignoring the window position and
    // the GTK header, or scaled by 1 instead of the actual DPR.
    let mut moved = fake();
    moved.window_x = 12;
    let d = drive_resize(moved, |v, _| stty(v));
    for (step, dpr) in [("resize-observe-start", 1), ("resize-observe-scale2", 2)] {
        let s = &d.ledger.steps[step]["screen"];
        assert_eq!(s["crop"]["x"], 12 * dpr, "{step} {s}");
        assert_eq!(s["crop"]["y"], 37 * dpr, "{step} {s}");
        assert_eq!(s["crop_origin"]["x"], 12.0, "{step} {s}");
        assert!(s["ink_blocks"].as_u64().unwrap() >= MIN_INK_BLOCKS, "{s}");
    }
    assert!(d.results.iter().all(Result::is_ok), "{:?}", d.results);
}

#[test]
fn parent_resize_crop_without_observed_window_is_an_error_not_a_metric() {
    // Wrong behavior: falling back to (0,0) when the tree has no view of the window pid, or when
    // no window pid is known.
    let mut other = fake();
    other.tree_pid = 99;
    let d = drive_resize(other, |v, _| stty(v));
    let e = d.results[0].as_ref().unwrap_err();
    assert!(e.contains("window pid"), "{e}");
    assert!(d.ledger.steps["resize-observe-start"]["screen"]["edge_p95"].is_null());
    let mut view = fake();
    let (_, page, _) = stage_view(1280, 720, 1, 0, 0);
    let detail = flat(&id(), json!({"stage": "start", "view": page}));
    let no_pid = ViewExpectations::new("local", "/tmp/hd7L-4242", SOCK).unwrap();
    let mut ledger = Ledger::default();
    let mut pty = |_: &str| Ok::<String, String>("34 120\n".into());
    let r = parent_step(
        "resize-observe-start",
        &detail,
        &no_pid,
        &mut view,
        &mut pty,
        &mut ledger,
    );
    assert!(r.is_err_and(|e| e.contains("window pid")));
}

#[test]
fn parent_resize_crop_requires_page_dpr_equal_to_output_scale() {
    // Wrong behavior: image pixels computed with a page DPR the compositor does not have.
    let mut view = fake();
    view.mode = (1600, 900, 2);
    let (_, mut page, _) = stage_view(1600, 900, 2, 0, 0);
    page["dpr"] = json!(1.0);
    let detail = flat(&id(), json!({"stage": "start", "view": page}));
    let mut ledger = Ledger::default();
    let mut pty = |_: &str| Ok::<String, String>("19 66\n".into());
    let r = parent_step(
        "resize-observe-start",
        &detail,
        &exp(),
        &mut view,
        &mut pty,
        &mut ledger,
    );
    assert!(r.is_err_and(|e| e.contains("scale")));
    assert!(ledger.steps["resize-observe-start"]["screen"]["edge_p95"].is_null());
}

#[test]
fn parent_resize_does_not_fabricate_dimensions_or_pty_rows() {
    // Compositor ignored the command: actual outputs (still 1280x720) are recorded, geometry false.
    let mut ignored = fake();
    ignored.applies = false;
    let d = drive_resize(ignored, |v, _| stty(v));
    let grow = &d.ledger.steps["resize-observe-grow"];
    assert_eq!(grow["outputs"][0]["current_mode"]["width"], 1280);
    assert_eq!(grow["screen"]["width"], 1280);
    // The page's grow client (1600 wide) does not fit the actual 1280-wide window: the crop
    // geometry is refused before any metric (formerly caught later as a crop outside the image).
    assert!(
        d.results[2]
            .as_ref()
            .is_err_and(|e| e.contains("differs from the observed window width")),
        "{:?}",
        d.results[2]
    );
    assert!(grow["screen"]["edge_p95"].is_null());
    let r = resize_checks(RESIZE_PHASE, &d.page, &d.ledger, &exp());
    assert!(!matches!(&r, Ok(v) if v[0].1), "{r:?}");
    // grim still delivers the old physical size: recorded as seen, crisp false.
    let mut stale = fake();
    stale.stale_screen = Some((1280, 720));
    let d = drive_resize(stale, |v, _| stty(v));
    assert_eq!(
        d.ledger.steps["resize-observe-grow"]["screen"]["width"],
        1280
    );
    assert_eq!(
        d.ledger.steps["resize-observe-grow"]["outputs"][0]["current_mode"]["width"],
        1600
    );
    let r = resize_checks(RESIZE_PHASE, &d.page, &d.ledger, &exp());
    assert!(!matches!(&r, Ok(v) if v[1].1), "{r:?}");
    // PTY never resized: the callback's actual text is what is recorded, geometry false.
    let d = drive_resize(fake(), |_, _| "34 120\n".into());
    assert_eq!(
        d.ledger.steps["resize-observe-grow"]["pty_size"],
        json!([34, 120])
    );
    let r = resize_checks(RESIZE_PHASE, &d.page, &d.ledger, &exp()).unwrap();
    assert!(!r[0].1, "{r:?}");
    // Unparseable stty text is an error, never a guessed size.
    let d = drive_resize(fake(), |_, _| {
        "stty: 'standard input': Inappropriate ioctl".into()
    });
    let e = d.results[0].as_ref().unwrap_err();
    assert!(e.contains("stty"), "{e}");
    assert!(d.ledger.steps["resize-observe-start"]["pty_size"].is_null());
}

#[test]
fn parent_resize_failed_process_is_recorded_once_and_not_replayed() {
    let mut view = fake();
    let mut ledger = Ledger::default();
    let (_, page, _) = stage_view(1280, 720, 1, 0, 0);
    let start = flat(&id(), json!({"stage":"start","view":page}));
    let mut pty = |_: &str| Ok::<_, String>("34 120\n".to_owned());
    parent_step(
        "resize-observe-start",
        &start,
        &exp(),
        &mut view,
        &mut pty,
        &mut ledger,
    )
    .unwrap();
    view.swaymsg_fails = true;
    let grow = flat(&id(), json!({"stage":"grow","view":page}));
    let e = parent_step(
        "resize-apply-grow",
        &grow,
        &exp(),
        &mut view,
        &mut no_pty(),
        &mut ledger,
    )
    .unwrap_err();
    assert!(
        e.contains("HEADLESS-1 unknown"),
        "actual stderr surfaced: {e}"
    );
    let a = &ledger.steps["resize-apply-grow"];
    assert_eq!(a["run"]["success"], false);
    assert_eq!(a["run"]["status"], "exit status: 1");
    assert!(a["error"].is_string() && a["outputs"].is_null(), "{a}");
    // Wrong behavior: retrying the same apply re-commands the compositor.
    let calls = view.calls.len();
    view.swaymsg_fails = false;
    let e = parent_step(
        "resize-apply-grow",
        &grow,
        &exp(),
        &mut view,
        &mut no_pty(),
        &mut ledger,
    )
    .unwrap_err();
    assert!(e.contains("twice"), "{e}");
    assert_eq!(view.calls.len(), calls);
}

#[test]
fn parent_step_refuses_before_acting() {
    let (_, page, _) = stage_view(1600, 900, 1, 0, 0);
    let grow = flat(&id(), json!({"stage":"grow","view":page}));
    let refused = |view: &mut FakeView, ledger: &mut Ledger, step: &str, detail: &Value| {
        let before = (view.calls.len(), ledger.steps.len());
        let r = parent_step(step, detail, &exp(), view, &mut no_pty(), ledger);
        assert!(r.is_err(), "{step}: {r:?}");
        assert_eq!(
            (view.calls.len(), ledger.steps.len()),
            before,
            "{step} acted"
        );
        r.unwrap_err()
    };
    let mut ledger = Ledger::default();
    // Missing previous step: apply-grow before observe-start.
    let e = refused(&mut fake(), &mut ledger, "resize-apply-grow", &grow);
    assert!(e.contains("resize-observe-start"), "{e}");
    // Unknown step.
    refused(&mut fake(), &mut ledger, "resize-apply-huge", &grow);
    // Each refusal below uses an otherwise admissible observe-start detail.
    let start = flat(
        &id(),
        json!({"stage":"start","view":stage_view(1280, 720, 1, 0, 0).1}),
    );
    // Wrong socket (user compositor).
    let mut user = fake();
    user.socket = "/run/user/1000/sway-ipc.1000.5.sock".into();
    let e = refused(&mut user, &mut ledger, "resize-observe-start", &start);
    assert!(e.contains("private compositor"), "{e}");
    // Other host.
    let mut other = start.clone();
    other["endpoint"] = json!("ssh:devbox");
    refused(&mut fake(), &mut ledger, "resize-observe-start", &other);
    // Incomplete identity.
    let mut partial = start.clone();
    partial["generation"] = json!("");
    refused(&mut fake(), &mut ledger, "resize-observe-start", &partial);
    // Stage of the detail differs from the step.
    refused(&mut fake(), &mut ledger, "resize-observe-start", &grow);
    let mut view = fake();
    let mut pty = |_: &str| Ok::<_, String>("34 120\n".to_owned());
    parent_step(
        "resize-observe-start",
        &start,
        &exp(),
        &mut view,
        &mut pty,
        &mut ledger,
    )
    .unwrap();
    refused(&mut view, &mut ledger, "resize-apply-grow", &start);
    // Stale pane after the first step.
    let mut stale = grow.clone();
    stale["pane_id"] = json!("w1-p9");
    refused(&mut view, &mut ledger, "resize-apply-grow", &stale);
}

fn a11y_detail(page: &Value, sweep: &str, keys: Value) -> Value {
    let mut extra = json!({"expected": page[sweep]["expected"], "keys": keys});
    if sweep == "main" {
        extra["wrap_after"] = page[sweep]["wrap_after"].clone();
        extra["split"] = page[sweep]["split"].clone();
    }
    flat(&id(), extra)
}

fn plan(n: usize) -> Value {
    let mut k = vec![json!("Tab"); n];
    k.push(json!("shift+Tab"));
    json!(k)
}

#[test]
fn parent_a11y_presses_validated_plan_through_wtype_and_evaluates() {
    let (page, _) = a11y_world();
    let (mut view, mut ledger) = (fake(), Ledger::default());
    let steps = [
        ("a11y-sweep-main", a11y_detail(&page, "main", main_plan(3))),
        (
            "a11y-open-dialog",
            flat(
                &id(),
                json!({"opener":"Nova conexão SSH","keys":["Return"]}),
            ),
        ),
        ("a11y-sweep-dialog", a11y_detail(&page, "dialog", plan(3))),
        (
            "a11y-close-dialog",
            flat(
                &id(),
                json!({"opener":"Nova conexão SSH","keys":["Escape"]}),
            ),
        ),
    ];
    for (step, detail) in &steps {
        let a = parent_step(step, detail, &exp(), &mut view, &mut no_pty(), &mut ledger).unwrap();
        assert_eq!(a["keys"], detail["keys"], "{step}");
    }
    let mut pressed: Vec<Vec<String>> = steps
        .iter()
        .flat_map(|(_, d)| d["keys"].as_array().unwrap().clone())
        .map(|k| wtype_args(k.as_str().unwrap()).unwrap())
        .collect();
    // After the Escape only: three compositor focus samples (decision r7), nothing else.
    let tree = swaymsg_args(SOCK, &["-t", "get_tree", "-r"].map(str::to_owned));
    pressed.extend([tree.clone(), tree.clone(), tree]);
    assert_eq!(view.calls, pressed);
    assert_eq!(
        a11y(&page, &ledger),
        A11Y_CHECKS.map(|c| (c, true)).to_vec()
    );
}

#[test]
fn parent_a11y_refuses_bad_plans_and_records_failed_key() {
    let (page, _) = a11y_world();
    let (mut view, mut ledger) = (fake(), Ledger::default());
    // Plan with one Tab more than the expected stops (+ wrap), the r10 plan without the wrap Tab
    // and the r8/r9 wrap plan: nothing pressed, nothing recorded.
    let f6 = json!("ctrl+shift+F6");
    let wrap = json!([f6, "Tab", "Tab", "Tab", "Tab", f6, "Tab", "shift+Tab"]);
    let r10 = json!([f6, "Tab", "Tab", "Tab", f6, "shift+Tab"]);
    for bad in [main_plan(4), r10, wrap] {
        let e = parent_step(
            "a11y-sweep-main",
            &a11y_detail(&page, "main", bad),
            &exp(),
            &mut view,
            &mut no_pty(),
            &mut ledger,
        )
        .unwrap_err();
        assert!(e.contains("does not match"), "{e}");
        assert!(view.calls.is_empty() && ledger.steps.is_empty());
    }
    // r12: the valid plan with the wrap undeclared, declared at another index, or declared with a
    // split that is not the expected stops: refused before pressing anything.
    for (why, tweak) in [
        (
            "undeclared wrap_after",
            &(|d: &mut Value| {
                d.as_object_mut().unwrap().remove("wrap_after");
            }) as &dyn Fn(&mut Value),
        ),
        ("undeclared split", &|d: &mut Value| {
            d.as_object_mut().unwrap().remove("split");
        }),
        ("index not the last following stop", &|d: &mut Value| {
            d["wrap_after"] = json!(1)
        }),
        ("split not the expected stops", &|d: &mut Value| {
            d["split"] = json!({"following":["a","b"],"preceding":["c"]});
            d["wrap_after"] = json!(1);
            d["expected"] = json!([
                ctl("b", "Controle b"),
                ctl("a", "Controle a"),
                ctl("c", "Controle c"),
                ctl("c", "Controle c")
            ]);
        }),
    ] {
        let mut d = a11y_detail(&page, "main", main_plan(3));
        tweak(&mut d);
        assert!(
            parent_step(
                "a11y-sweep-main",
                &d,
                &exp(),
                &mut view,
                &mut no_pty(),
                &mut ledger
            )
            .is_err(),
            "{why}"
        );
        assert!(view.calls.is_empty() && ledger.steps.is_empty(), "{why}");
    }
    // Dialog before the main sweep is a missing step.
    let open = flat(&id(), json!({"keys":["Return"]}));
    assert!(parent_step(
        "a11y-open-dialog",
        &open,
        &exp(),
        &mut view,
        &mut no_pty(),
        &mut ledger
    )
    .is_err());
    assert!(view.calls.is_empty());
    // wtype fails on the second key: error recorded with the actual count pressed.
    view.wtype_fails_at = Some(1);
    let e = parent_step(
        "a11y-sweep-main",
        &a11y_detail(&page, "main", main_plan(3)),
        &exp(),
        &mut view,
        &mut no_pty(),
        &mut ledger,
    )
    .unwrap_err();
    assert!(e.contains("exit status: 1"), "{e}");
    let a = &ledger.steps["a11y-sweep-main"];
    assert_eq!(a["pressed"], 1);
    assert!(a["error"].is_string());
    assert_eq!(view.calls.len(), 2, "stops at the failing key");
}

fn pointer_base(runtime: &str) -> display::Launch {
    display::Launch {
        program: "/usr/bin/wtype".into(),
        args: vec![],
        env: vec![
            ("PATH".into(), "/usr/bin:/bin".into()),
            ("XDG_RUNTIME_DIR".into(), runtime.into()),
            ("WAYLAND_DISPLAY".into(), format!("{runtime}/wayland-1")),
            ("LD_LIBRARY_PATH".into(), "/home/user/lib".into()),
            (
                "SWAYSOCK".into(),
                "/run/user/1000/sway-ipc.1000.1.sock".into(),
            ),
            ("DISPLAY".into(), ":0".into()),
        ],
    }
}

#[test]
fn view_env_launches_only_the_private_compositor() {
    let prefix = std::path::Path::new("/p/native-input/prefix");
    let pointer =
        mouse_flow::PointerEnv::new(&pointer_base("/tmp/hd7L-4242"), prefix, 1000, 77).unwrap();
    let env = ViewEnv::new(&pointer, &exp()).unwrap();
    assert_eq!(env.socket(), SOCK);
    let words = output_command(&STAGES[2]).unwrap();
    let apply = env.swaymsg(&swaymsg_args(SOCK, &words)).unwrap();
    assert_eq!(apply.program, prefix.join("usr/bin/swaymsg"));
    assert_eq!(apply.args, swaymsg_args(SOCK, &words));
    assert_eq!(
        apply.var("LD_LIBRARY_PATH"),
        Some("/p/native-input/prefix/usr/lib")
    );
    let grim = env.grim();
    assert_eq!(grim.program, std::path::PathBuf::from("/usr/bin/grim"));
    assert_eq!(grim.args, ["-t", "ppm", "-o", "HEADLESS-1", "-"]);
    assert_eq!(
        grim.var("LD_LIBRARY_PATH"),
        None,
        "system grim, not the sway prefix libs"
    );
    for l in [&apply, &grim] {
        assert_eq!(l.var("XDG_RUNTIME_DIR"), Some("/tmp/hd7L-4242"));
        assert!(
            l.var("SWAYSOCK").is_none() && l.var("DISPLAY").is_none(),
            "user session leaked"
        );
    }
    // Wrong behavior: swaymsg aimed at another socket than the private one.
    let user = swaymsg_args("/run/user/1000/sway-ipc.1000.5.sock", &words);
    assert!(env.swaymsg(&user).is_err());
    assert!(
        env.swaymsg(&words).is_err(),
        "without -s SOCK swaymsg would use SWAYSOCK"
    );
    // Pointer env of another sway instance does not match the expected socket.
    let other =
        mouse_flow::PointerEnv::new(&pointer_base("/tmp/hd7L-4242"), prefix, 1000, 78).unwrap();
    assert!(ViewEnv::new(&other, &exp()).is_err());
}

// ------------------------------------------------ terminal focus carrier (Planner decision r7)

/// The IME textarea (`textarea.ime-target`) as the page records it in GUI r6: the textarea itself
/// keeps outline none (style == baseline) while its `.terminal` container carries the ring.
fn ime_stop(id: &str) -> Value {
    let none = json!({"outline_style":"none","outline_width":"3px","outline_color":"rgba(0, 0, 0, 0)",
        "box_shadow":"none","border_color":"rgba(0, 0, 0, 0)","background_color":"rgba(0, 0, 0, 0)"});
    json!({"id":id,"tag":"textarea","classes":["ime-target","svelte-kfm3zr"],"role":"",
        "name":"Terminal Herdr","name_source":"aria-label","disabled":false,"tabindex":0,
        "focus_visible":true,"style":none.clone(),"baseline":none,
        "container_style":{"outline_style":"solid","outline_width":"2px","outline_color":"rgb(97, 175, 239)",
            "box_shadow":"none","border_color":"rgb(231, 233, 238)","background_color":"rgb(16, 20, 24)",
            "outline_offset":"-2px","classes":"terminal svelte-kfm3zr focus-visible"},
        "container_baseline":{"outline_style":"none","outline_width":"0px","outline_color":"rgb(231, 233, 238)",
            "box_shadow":"none","border_color":"rgb(231, 233, 238)","background_color":"rgb(16, 20, 24)"}})
}

/// a11y world whose main sweep has the terminal stop as its middle control `b`.
fn a11y_world_with(stop: Value) -> (Value, Ledger) {
    let (mut page, ledger) = a11y_world();
    for list in ["expected", "focus"] {
        for f in page["main"][list].as_array_mut().unwrap() {
            if f["id"] == "b" {
                *f = stop.clone();
            }
        }
    }
    (page, ledger)
}

#[test]
fn a11y_terminal_container_ring_counts_for_the_ime_target_only() {
    // Would catch: the r6 evaluator reading only the textarea's own style (fails the ringed stop).
    let (page, ledger) = a11y_world_with(ime_stop("b"));
    assert_eq!(
        a11y(&page, &ledger),
        A11Y_CHECKS.map(|c| (c, true)).to_vec()
    );
    // Container box-shadow different from the container baseline also counts (outline none).
    let mut shadow = ime_stop("b");
    shadow["container_style"]["outline_style"] = json!("none");
    shadow["container_style"]["box_shadow"] = json!("rgb(97, 175, 239) 0px 0px 0px 2px inset");
    let (page, ledger) = a11y_world_with(shadow);
    assert_eq!(
        a11y(&page, &ledger),
        A11Y_CHECKS.map(|c| (c, true)).to_vec()
    );
}

#[test]
fn a11y_terminal_container_negatives() {
    let only_focus = |why: &str, stop: Value| {
        let (page, ledger) = a11y_world_with(stop);
        let r = a11y(&page, &ledger);
        assert_eq!(
            r,
            vec![
                (A11Y_CHECKS[0], false),
                (A11Y_CHECKS[1], true),
                (A11Y_CHECKS[2], true)
            ],
            "{why}"
        );
    };
    let tweak = |f: &dyn Fn(&mut Value)| {
        let mut s = ime_stop("b");
        f(&mut s);
        s
    };
    only_focus(
        "container without outline (equal to its baseline)",
        tweak(&|s| s["container_style"] = s["container_baseline"].clone()),
    );
    only_focus(
        "no container recorded at all",
        tweak(&|s| s["container_style"] = Value::Null),
    );
    only_focus(
        "container outline of width 0",
        tweak(&|s| s["container_style"]["outline_width"] = json!("0px")),
    );
    only_focus(
        "container outline transparent",
        tweak(&|s| s["container_style"]["outline_color"] = json!("rgba(97, 175, 239, 0)")),
    );
    only_focus(
        "container outline not solid",
        tweak(&|s| s["container_style"]["outline_style"] = json!("dashed")),
    );
    only_focus(
        "container ring while :focus-visible is not matched",
        tweak(&|s| s["focus_visible"] = json!(false)),
    );
    only_focus(
        "container box-shadow change without a recorded container baseline",
        tweak(&|s| {
            s["container_style"]["outline_style"] = json!("none");
            s["container_style"]["box_shadow"] = json!("rgb(97, 175, 239) 0px 0px 0px 2px inset");
            s.as_object_mut().unwrap().remove("container_baseline");
        }),
    );
    // Any other control: a container_style ring never stands in for its own indicator.
    only_focus(
        "input with a container outline and no own indicator",
        tweak(&|s| {
            s["tag"] = json!("input");
            s["classes"] = json!([]);
        }),
    );
    only_focus(
        "input carrying the ime-target class is still not the IME textarea",
        tweak(&|s| s["tag"] = json!("input")),
    );
    only_focus(
        "textarea without the ime-target class",
        tweak(&|s| s["classes"] = json!(["notes"])),
    );
    only_focus(
        "classes absent (not proven to be the IME textarea)",
        tweak(&|s| {
            s.as_object_mut().unwrap().remove("classes");
        }),
    );
}

// ------------------------------------------------ compositor focus after Escape (decision r7)

/// Sway tree with the window (`tree_pid`, app_id herdr-desktop) and an fcitx5 view; `focus` names
/// which node sway marks focused: "window", "fcitx5" or "workspace".
fn focus_tree(tree_pid: u32, focus: &str) -> Value {
    json!({"type":"root","id":1,"focused":false,"nodes":[{"type":"output","name":"HEADLESS-1","id":3,
        "focused":false,"rect":{"x":0,"y":0,"width":1280,"height":720},"nodes":[{"type":"workspace",
        "id":4,"name":"1","focused":focus == "workspace","nodes":[
            {"type":"con","id":7,"pid":tree_pid,"app_id":"herdr-desktop","name":"Herdr Desktop",
             "focused":focus == "window","nodes":[],"floating_nodes":[],
             "rect":{"x":0,"y":0,"width":1280,"height":720},"window_rect":{"x":0,"y":0,"width":1280,"height":720}}],
        "floating_nodes":[{"type":"floating_con","id":9,"pid":9191,"app_id":"org.fcitx.fcitx5",
             "name":"Fcitx5","focused":focus == "fcitx5","nodes":[],"floating_nodes":[]}]}]}]})
}

struct FocusView {
    inner: FakeView,
    focus: Vec<&'static str>,
    sleeps: Vec<u64>,
}

impl ViewWorld for FocusView {
    fn socket(&self) -> String {
        self.inner.socket()
    }
    fn swaymsg(&mut self, args: &[String]) -> Result<Run, String> {
        if args[2..] == ["-t", "get_tree", "-r"] && !self.inner.swaymsg_fails {
            self.inner.calls.push(args.to_vec());
            let focus = if self.focus.is_empty() {
                "workspace"
            } else {
                self.focus.remove(0)
            };
            let tree = focus_tree(self.inner.tree_pid, focus);
            return Ok(ran(args, true, tree.to_string().into_bytes(), ""));
        }
        self.inner.swaymsg(args)
    }
    fn grim_ppm(&mut self) -> Result<Run, String> {
        self.inner.grim_ppm()
    }
    fn wtype(&mut self, args: &[String]) -> Result<(), String> {
        self.inner.wtype(args)
    }
    fn sleep_ms(&mut self, ms: u64) {
        self.inner.calls.push(vec![format!("sleep {ms}")]);
        self.sleeps.push(ms);
    }
}

fn drive_to_close(view: &mut FocusView, ledger: &mut Ledger) -> Result<Value, String> {
    let (page, _) = a11y_world();
    for (step, detail) in [
        ("a11y-sweep-main", a11y_detail(&page, "main", main_plan(3))),
        ("a11y-open-dialog", flat(&id(), json!({"keys":["Return"]}))),
        ("a11y-sweep-dialog", a11y_detail(&page, "dialog", plan(3))),
    ] {
        parent_step(step, &detail, &exp(), view, &mut no_pty(), ledger).unwrap();
        assert!(
            ledger.steps[step].get("compositor_focus").is_none(),
            "{step}: only the close step samples the compositor"
        );
    }
    assert!(view.sleeps.is_empty(), "no delay before the Escape step");
    let close = flat(&id(), json!({"keys":["Escape"]}));
    parent_step(
        "a11y-close-dialog",
        &close,
        &exp(),
        view,
        &mut no_pty(),
        ledger,
    )
}

#[test]
fn parent_close_dialog_samples_compositor_focus_after_the_key() {
    // Would catch: sampling before Escape, all samples at once, or a hard-coded "focused" verdict.
    let mut view = FocusView {
        inner: fake(),
        focus: vec!["window", "fcitx5", "workspace"],
        sleeps: Vec::new(),
    };
    let mut ledger = Ledger::default();
    let answer = drive_to_close(&mut view, &mut ledger).unwrap();
    let tree = swaymsg_args(SOCK, &["-t", "get_tree", "-r"].map(str::to_owned));
    let escape = wtype_args("Escape").unwrap();
    let at = view.inner.calls.iter().position(|c| *c == escape).unwrap();
    assert_eq!(
        view.inner.calls[at..],
        [
            escape.clone(),
            tree.clone(),
            vec![format!("sleep {}", view.sleeps[0])],
            tree.clone(),
            vec![format!("sleep {}", view.sleeps[1])],
            tree.clone(),
        ]
    );
    assert!(
        view.sleeps[0] > 0 && view.sleeps[0] <= 50,
        "{:?}",
        view.sleeps
    );
    assert!(
        view.sleeps[1] >= 100 && view.sleeps[0] + view.sleeps[1] <= 200,
        "{:?}",
        view.sleeps
    );
    let samples = ledger.steps["a11y-close-dialog"]["compositor_focus"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(answer["compositor_focus"], json!(samples));
    let at_ms: Vec<Value> = samples.iter().map(|s| s["at_ms"].clone()).collect();
    assert_eq!(at_ms, [json!(0), json!(50), json!(200)]);
    for s in &samples {
        assert!(s["elapsed_ms"].is_number(), "{s}");
        assert_eq!(s["run"]["argv"], json!(tree), "{s}");
        assert_eq!(s["run"]["success"], true, "{s}");
    }
    let focused: Vec<(Value, Value, Value, Value)> = samples
        .iter()
        .map(|s| {
            let f = &s["focused"];
            (
                f["type"].clone(),
                f["pid"].clone(),
                f["app_id"].clone(),
                s["window_focused"].clone(),
            )
        })
        .collect();
    assert_eq!(
        focused,
        [
            (
                json!("con"),
                json!(WINDOW_PID),
                json!("herdr-desktop"),
                json!(true)
            ),
            (
                json!("floating_con"),
                json!(9191),
                json!("org.fcitx.fcitx5"),
                json!(false)
            ),
            (json!("workspace"), Value::Null, Value::Null, json!(false)),
        ]
    );
}

#[test]
fn parent_close_dialog_focus_sampling_records_failures_without_failing_the_key() {
    // Would catch: a failed get_tree turning into a fabricated sample, or undoing a delivered key.
    let mut inner = fake();
    inner.swaymsg_fails = true;
    let mut view = FocusView {
        inner,
        focus: Vec::new(),
        sleeps: Vec::new(),
    };
    let mut ledger = Ledger::default();
    let answer = drive_to_close(&mut view, &mut ledger).unwrap();
    assert_eq!(answer["pressed"], 1);
    let samples = answer["compositor_focus"].as_array().unwrap();
    assert_eq!(samples.len(), 3);
    for s in samples {
        assert!(
            s["error"].as_str().is_some_and(|e| e.contains("get_tree")),
            "{s}"
        );
        assert!(
            s.get("focused").is_none() && s.get("window_focused").is_none(),
            "{s}"
        );
    }
    // Key never delivered: nothing sampled.
    let mut view = FocusView {
        inner: fake(),
        focus: vec!["window"; 3],
        sleeps: Vec::new(),
    };
    let mut ledger = Ledger::default();
    let (page, _) = a11y_world();
    for (step, detail) in [
        ("a11y-sweep-main", a11y_detail(&page, "main", main_plan(3))),
        ("a11y-open-dialog", flat(&id(), json!({"keys":["Return"]}))),
        ("a11y-sweep-dialog", a11y_detail(&page, "dialog", plan(3))),
    ] {
        parent_step(step, &detail, &exp(), &mut view, &mut no_pty(), &mut ledger).unwrap();
    }
    view.inner.wtype_fails_at = Some(view.inner.calls.len());
    let close = flat(&id(), json!({"keys":["Escape"]}));
    assert!(parent_step(
        "a11y-close-dialog",
        &close,
        &exp(),
        &mut view,
        &mut no_pty(),
        &mut ledger
    )
    .is_err());
    assert!(ledger.steps["a11y-close-dialog"]
        .get("compositor_focus")
        .is_none());
    assert!(view.sleeps.is_empty());
}
