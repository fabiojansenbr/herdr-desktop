//! Smallest local adapter for the web view viewport origin inside HEADLESS-1 (live latency seam).
//! Predicted from the private sway `get_tree` window of the measured pid plus the page's client
//! size (GTK CSD header = window content height − client height × DPR), then confirmed by locating
//! the seq-0 marker edges in a real capture. No WebKit screenX/Y. Root may replace this with the
//! shared geometry module after merge.

use serde_json::Value;

use super::marker::{self, PageObservation, RgbView, ON_RGB, TOLERANCE};

/// Window content rect in output pixels (scale 1 only).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContentRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

const SEARCH_PX: i64 = 16;
pub const AGREEMENT_PX: f64 = 1.0;

fn find<'a>(
    node: &'a Value,
    pid: u32,
    scale: Option<f64>,
    out: &mut Vec<(&'a Value, Option<f64>)>,
) {
    let scale = if node["type"] == "output" {
        node["scale"].as_f64()
    } else {
        scale
    };
    if node["pid"].as_u64() == Some(u64::from(pid)) {
        out.push((node, scale));
    }
    for key in ["nodes", "floating_nodes"] {
        for child in node[key].as_array().into_iter().flatten() {
            find(child, pid, scale, out);
        }
    }
}

/// Pid of the unique focused container that owns a window. A focused workspace with no
/// window, or several focused pids, is not compositor focus on the measured window.
pub fn focused_window_pid(tree: &Value) -> Result<u32, String> {
    fn walk(node: &Value, out: &mut Vec<u32>) {
        if node["focused"] == true {
            if let Some(pid) = node["pid"].as_u64() {
                out.push(pid as u32);
            }
        }
        for key in ["nodes", "floating_nodes"] {
            for child in node[key].as_array().into_iter().flatten() {
                walk(child, out);
            }
        }
    }
    let mut found = Vec::new();
    walk(tree, &mut found);
    match found[..] {
        [pid] => Ok(pid),
        _ => Err(format!(
            "expected exactly one focused window pid, got {}",
            found.len()
        )),
    }
}

/// `rect + window_rect` of the single tree node owned by `pid`, on a scale-1 output.
pub fn content_rect(tree: &Value, pid: u32) -> Result<ContentRect, String> {
    let mut found = Vec::new();
    find(tree, pid, None, &mut found);
    let [(node, scale)] = found[..] else {
        return Err(format!(
            "expected one tree node with pid {pid}, found {}",
            found.len()
        ));
    };
    if scale != Some(1.0) {
        return Err(format!(
            "output scale {scale:?} is not 1 (logical != capture pixels)"
        ));
    }
    let n = |v: &Value, k: &str| v[k].as_f64().ok_or_else(|| format!("tree {k} missing"));
    let (r, w) = (&node["rect"], &node["window_rect"]);
    Ok(ContentRect {
        x: n(r, "x")? + n(w, "x")?,
        y: n(r, "y")? + n(w, "y")?,
        width: n(w, "width")?,
        height: n(w, "height")?,
    })
}

/// Web view origin: header only above the client (no side/bottom decoration accepted).
pub fn offset_from_client(
    c: ContentRect,
    client_w_css: f64,
    client_h_css: f64,
    dpr: f64,
) -> Result<(f64, f64), String> {
    let (w, h) = (client_w_css * dpr, client_h_css * dpr);
    if !(dpr.is_finite() && dpr > 0.0) || (w - c.width).abs() > AGREEMENT_PX {
        return Err(format!(
            "client width {w} px != window content width {} px",
            c.width
        ));
    }
    let header = c.height - h;
    if !(0.0..c.height).contains(&header) {
        return Err(format!(
            "client height {h} px does not fit window content {} px",
            c.height
        ));
    }
    Ok((c.x, c.y + header))
}

fn on(img: RgbView, x: i64, y: i64) -> bool {
    if x < 0 || y < 0 || x as usize >= img.width || y as usize >= img.height {
        return false;
    }
    let i = (y as usize * img.width + x as usize) * 3;
    (0..3).all(|k| img.rgb[i + k].abs_diff(ON_RGB[k]) <= TOLERANCE)
}

/// Locates the left/top edges of marker cell 0 (bit 1) near the prediction, derives the offset
/// they imply, and requires the seq-0 marker to decode there.
pub fn locate_seq0(
    img: RgbView,
    page: &PageObservation,
    predicted: (f64, f64),
) -> Result<(f64, f64), String> {
    let cell_x = (page.canvas_left_css + marker::COL as f64 * page.cell_w_css) * page.dpr;
    let cell_y = (page.canvas_top_css + marker::ROW as f64 * page.cell_h_css) * page.dpr;
    let (px, py) = ((predicted.0 + cell_x) as i64, (predicted.1 + cell_y) as i64);
    let yc = py + (page.cell_h_css * page.dpr / 2.0) as i64;
    let x0 = (px - SEARCH_PX..=px + SEARCH_PX)
        .find(|&x| on(img, x, yc) && !on(img, x - 1, yc))
        .ok_or("seq-0 marker left edge not found near prediction")?;
    let xc = x0 + (page.cell_w_css * page.dpr / 2.0) as i64;
    let y0 = (py - SEARCH_PX..=py + SEARCH_PX)
        .find(|&y| on(img, xc, y) && !on(img, xc, y - 1))
        .ok_or("seq-0 marker top edge not found near prediction")?;
    let located = (x0 as f64 - cell_x, y0 as f64 - cell_y);
    let grid = marker::grid(page, Some(located))?;
    match marker::decode(img, &grid) {
        Ok(0) => Ok(located),
        other => Err(format!(
            "located marker at {located:?} is not seq 0: {other:?}"
        )),
    }
}

/// Predicted and located offsets must agree within 1 px; the prediction is kept.
pub fn confirm_offset(predicted: (f64, f64), located: (f64, f64)) -> Result<(f64, f64), String> {
    if (predicted.0 - located.0).abs() <= AGREEMENT_PX
        && (predicted.1 - located.1).abs() <= AGREEMENT_PX
    {
        Ok(predicted)
    } else {
        Err(format!(
            "viewport offset predicted {predicted:?} vs seq-0 marker {located:?}"
        ))
    }
}
