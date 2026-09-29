//! Pure mapping from what a live run really read (engine JSON, /proc, the generator's counter
//! file, the page's probe snapshots) to [`super::verdict`] inputs. No field is filled from the
//! expectation: a missing reading stays missing and makes the validator reject the run.

use serde_json::{json, Value};

use super::verdict::{CounterReading, Geometry};

/// Renderer variables (`WEBKIT_DISABLE_DMABUF_RENDERER` and the WebKit/GDK renderer variant keys) of
/// a NUL-separated `/proc/<pid>/environ` of an owned window; every other variable is dropped.
pub fn renderer_env_of(environ: &[u8]) -> std::collections::BTreeMap<String, String> {
    let fixed = [super::plan::RENDERER_FIXED.0];
    let keys: Vec<&str> = fixed
        .into_iter()
        .chain(super::plan::RENDERER_VARIANT_KEYS)
        .collect();
    environ
        .split(|b| *b == 0)
        .filter_map(|entry| {
            let entry = std::str::from_utf8(entry).ok()?;
            let (k, v) = entry.split_once('=')?;
            keys.contains(&k).then(|| (k.to_owned(), v.to_owned()))
        })
        .collect()
}

/// WebKit processes (`comm` starting with `WebKit`) that are not inside any owned window tree:
/// `(pid, starttime, comm)` only, for labelling host interference. Nothing else is read.
pub fn foreign_webkit(
    procs: &[(u32, u64, &str)],
    owned_roots: &[u32],
    ppid_of: &dyn Fn(u32) -> Option<u32>,
) -> Vec<(u32, u64, String)> {
    procs
        .iter()
        .filter(|(pid, _, comm)| {
            comm.starts_with("WebKit")
                && !owned_roots.contains(pid)
                && !ancestors(*pid, ppid_of)
                    .iter()
                    .any(|a| owned_roots.contains(a))
        })
        .map(|(pid, st, comm)| (*pid, *st, (*comm).to_owned()))
        .collect()
}

/// Parent pid from `/proc/<pid>/stat` (field 4, after the parenthesised comm).
pub fn parse_ppid(stat: &str) -> Option<u32> {
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

/// Parent chain (nearest first) until pid 1/0, bounded and cycle-safe.
pub fn ancestors(pid: u32, ppid_of: &dyn Fn(u32) -> Option<u32>) -> Vec<u32> {
    let mut chain = Vec::new();
    let mut current = pid;
    while let Some(parent) = ppid_of(current) {
        if parent == 0 || chain.contains(&parent) || chain.len() >= 64 {
            break;
        }
        chain.push(parent);
        if parent == 1 {
            break;
        }
        current = parent;
    }
    chain
}

/// A generator belongs to this run only when its cmdline carries the run's unique token.
pub fn generator_owned(cmdline: &[u8], token: &str) -> bool {
    !token.is_empty()
        && cmdline
            .split(|b| *b == 0)
            .any(|arg| String::from_utf8_lossy(arg).contains(token))
}

/// Counter file written by the generator: `<bytes> <monotonic_ns> <pid>`.
pub fn parse_counter(raw: &str) -> Option<(CounterReading, u32)> {
    let mut it = raw.split_whitespace();
    let bytes = it.next()?.parse().ok()?;
    let monotonic_ns = it.next()?.parse().ok()?;
    let pid = it.next()?.parse().ok()?;
    it.next().is_none().then_some((
        CounterReading {
            bytes,
            monotonic_ns,
        },
        pid,
    ))
}

/// Panes of `herdr pane list` (API JSON `result.panes`), None when the shape is not the API's.
pub fn pane_list_ids(list: &Value) -> Option<Vec<String>> {
    list["result"]["panes"].as_array().map(|panes| {
        panes
            .iter()
            .filter_map(|p| p["pane_id"].as_str().map(str::to_owned))
            .collect()
    })
}

/// `shell_pid` of `pane.process_info`, only when the answer is about `pane_id`.
pub fn engine_shell_pid(process_info: &Value, pane_id: &str) -> Option<u32> {
    let info = &process_info["result"]["process_info"];
    (info["pane_id"] == pane_id)
        .then(|| {
            info["shell_pid"]
                .as_u64()
                .and_then(|p| u32::try_from(p).ok())
        })
        .flatten()
}

/// Cols/rows the engine laid out for `pane_id` (`pane.layout` rect).
pub fn engine_pane_size(layout: &Value, pane_id: &str) -> Option<(u16, u16)> {
    let panes = layout["result"]["layout"]["panes"].as_array()?;
    let pane = panes.iter().find(|p| p["pane_id"] == pane_id)?;
    let size = |k: &str| pane["rect"][k].as_u64().and_then(|v| u16::try_from(v).ok());
    Some((size("width")?, size("height")?))
}

const PROBE_KEYS: [&str; 4] = [
    "raf_requests",
    "raf_callbacks",
    "paint_calls",
    "painted_rows",
];

/// after − before of the page's probe snapshots; keys missing or going backwards are dropped,
/// so the validator reports the probe as missing instead of seeing a fabricated zero.
pub fn probe_delta(before: &Value, after: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for key in PROBE_KEYS {
        if let (Some(a), Some(b)) = (before[key].as_u64(), after[key].as_u64()) {
            if b >= a {
                out.insert(key.into(), json!(b - a));
            }
        }
    }
    Value::Object(out)
}

/// Geometry confirmed by the product: engine cols/rows of the pane, canvas backing store in
/// device px divided by them, devicePixelRatio and the canvas context font as painted.
pub fn geometry(page: &Value, engine_size: Option<(u16, u16)>) -> Result<Geometry, String> {
    let (cols, rows) = engine_size.ok_or("engine did not report the pane size")?;
    let int = |k: &str| {
        page[k]
            .as_u64()
            .ok_or_else(|| format!("page geometry field {k} missing"))
    };
    let (width, height) = (int("canvas_width")?, int("canvas_height")?);
    if cols == 0 || rows == 0 || width % cols as u64 != 0 || height % rows as u64 != 0 {
        return Err(format!(
            "canvas {width}x{height} px is not a whole cell grid of {cols}x{rows}"
        ));
    }
    let dpr = page["device_pixel_ratio"]
        .as_f64()
        .filter(|v| v.is_finite() && *v > 0.0)
        .ok_or("page devicePixelRatio missing")?;
    let raw_font = page["font"]
        .as_str()
        .filter(|f| !f.is_empty())
        .ok_or("page canvas font missing")?;
    // `ctx.font` keeps the last painted cell's style ("bold 14px …"): keep from the size on.
    let size_at = raw_font
        .match_indices(|c: char| c.is_ascii_digit())
        .map(|(i, _)| i)
        .find(|&i| {
            (i == 0 || raw_font.as_bytes()[i - 1] == b' ')
                && raw_font[i..]
                    .split_whitespace()
                    .next()
                    .and_then(|t| t.strip_suffix("px"))
                    .is_some_and(|n| n.parse::<f64>().is_ok())
        })
        .ok_or_else(|| format!("canvas font {raw_font:?} has no px size"))?;
    let font = &raw_font[size_at..];
    let narrow = |v: u64| u16::try_from(v).map_err(|e| e.to_string());
    Ok(Geometry {
        cols,
        rows,
        cell_width_px: narrow(width / cols as u64)?,
        cell_height_px: narrow(height / rows as u64)?,
        scale_milli: narrow((dpr * 1000.0).round() as u64)?,
        font: font.to_owned(),
    })
}

/// Observed (warmup, measured) seconds of one collector run: warmup = elapsed time of the first
/// non-warmup sample, measured = actual duration − that. Never the requested values. Errors on
/// malformed samples, warmup after measurement, no measured sample or a non-finite duration.
pub fn collector_window(samples_jsonl: &str, actual_duration_s: f64) -> Result<(f64, f64), String> {
    if !actual_duration_s.is_finite() {
        return Err(format!("actual duration {actual_duration_s} is not finite"));
    }
    let mut first_measured = None;
    for (n, line) in samples_jsonl
        .lines()
        .filter(|l| !l.trim().is_empty())
        .enumerate()
    {
        let v: Value = serde_json::from_str(line).map_err(|e| format!("sample {n}: {e}"))?;
        let (Some(t), Some(warm)) = (v["elapsed_monotonic_s"].as_f64(), v["is_warmup"].as_bool())
        else {
            return Err(format!("sample {n} lacks elapsed_monotonic_s/is_warmup"));
        };
        match (warm, first_measured) {
            (true, Some(_)) => return Err(format!("warmup sample {n} after measurement")),
            (false, None) => first_measured = Some(t),
            _ => {}
        }
    }
    let warmup = first_measured.ok_or("no measured sample")?;
    Ok((warmup, actual_duration_s - warmup))
}
