//! Client (CSS px) → private output geometry of the composed window, observed from the private
//! sway `get_tree` only. Shared by the pointer path (`live::client_offset`) and the screenshot
//! crop (`view_flow`). The page declares `coordinate_space: "client"`; WebKitGTK's
//! window.screenX/Y is not the compositor position (r2: 20,20 with the window at 0,0), so it is
//! kept as audit data only and never added.

use serde_json::{json, Value};

pub const CLIENT_SPACE: &str = "client";

#[derive(Debug, Clone, PartialEq)]
pub struct ClientGeometry {
    /// Output-local logical origin of the client (window origin + GTK header).
    pub x: f64,
    pub y: f64,
    pub header: f64,
    pub dpr: f64,
    pub observation: Value,
}

pub fn client_geometry(
    tree: &Value,
    window_pid: u32,
    client: &Value,
) -> Result<ClientGeometry, String> {
    fn views<'a>(
        node: &'a Value,
        output: Option<&'a Value>,
        pid: u32,
        out: &mut Vec<(&'a Value, Option<&'a Value>)>,
    ) {
        let output = if node["type"] == "output" {
            Some(node)
        } else {
            output
        };
        if node["pid"].as_u64() == Some(u64::from(pid)) {
            out.push((node, output));
        }
        for key in ["nodes", "floating_nodes"] {
            for child in node[key].as_array().into_iter().flatten() {
                views(child, output, pid, out);
            }
        }
    }
    if client["coordinate_space"] != CLIENT_SPACE {
        return Err(format!(
            "page coordinate_space {} is not {CLIENT_SPACE:?}",
            client["coordinate_space"]
        ));
    }
    let mut found = Vec::new();
    views(tree, None, window_pid, &mut found);
    let [(view, output)] = found[..] else {
        return Err(format!(
            "sway tree has {} views of window pid {window_pid} (exactly one required)",
            found.len()
        ));
    };
    let num = |v: &Value, what: &str| {
        v.as_f64()
            .ok_or_else(|| format!("sway/page geometry without numeric {what}"))
    };
    let rect = |node: &Value, key: &str| -> Result<[f64; 4], String> {
        let r = &node[key];
        Ok([
            num(&r["x"], key)?,
            num(&r["y"], key)?,
            num(&r["width"], key)?,
            num(&r["height"], key)?,
        ])
    };
    let (con, win) = (rect(view, "rect")?, rect(view, "window_rect")?);
    // Output rect is optional in fixtures without it only when absent; present = subtracted.
    let out_origin = match output.filter(|o| o["rect"].is_object()) {
        Some(o) => {
            let r = rect(o, "rect")?;
            [r[0], r[1]]
        }
        None => [0.0, 0.0],
    };
    let (width, height, dpr) = (
        num(&client["width"], "client width")?,
        num(&client["height"], "client height")?,
        num(&client["dpr"], "client dpr")?,
    );
    if dpr.is_nan() || dpr <= 0.0 {
        return Err(format!("client devicePixelRatio {dpr} is not positive"));
    }
    if width != win[2] {
        return Err(format!(
            "client width {width} differs from the observed window width {} (side decoration not observable)",
            win[2]
        ));
    }
    if !(1.0..=win[3]).contains(&height) {
        return Err(format!(
            "client height {height} does not fit the observed window height {}",
            win[3]
        ));
    }
    // The GTK header bar is the only client-side decoration above the WebView.
    let header = win[3] - height;
    let (x, y) = (
        con[0] + win[0] - out_origin[0],
        con[1] + win[1] + header - out_origin[1],
    );
    Ok(ClientGeometry {
        x,
        y,
        header,
        dpr,
        observation: json!({
            "window_pid": window_pid,
            "app_id": view["app_id"],
            "rect": view["rect"],
            "window_rect": view["window_rect"],
            "geometry": view["geometry"],
            "output_origin": { "x": out_origin[0], "y": out_origin[1] },
            "client": client,
            "screen_audit": { "screen_x": client["screen_x"], "screen_y": client["screen_y"] },
            "header_height": header,
            "offset": { "x": x, "y": y },
        }),
    })
}
