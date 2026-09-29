//! `resize-dpi` and `a11y-navigation` phases of the spec 007 native flow (AC-007-02): literal
//! stages/steps, parent key/command builders and pure evaluators. Prepared module; NOT executed
//! by `e2e_fidelity_flow` until the owner links it (`.local/orchestration/view-contract.md`).
//! `plan.rs` keeps both phases Pending.
//!
//! The parent commands the PRIVATE sway output ([`output_command`] on [`ViewExpectations::socket`])
//! and presses native keys ([`wtype_args`]); the page only observes actual viewport, DPR, canvas,
//! paint counters and focus/DOM. A missing step or field is an error, never a `true`.

use serde_json::{json, Value};

use super::display::{Launch, FORBIDDEN_INHERITED};
// `super::geometry` (client origin from the private sway tree) is used by the crop.
use super::mouse_flow::{Point, PointerEnv};
pub use super::paste_flow::ParentLedger as Ledger;

pub const RESIZE_PHASE: &str = "resize-dpi";
pub const RESIZE_CHECKS: [&str; 2] = ["resize_geometry_confirmed", "scale_change_repaints_crisp"];
pub const A11Y_PHASE: &str = "a11y-navigation";
pub const A11Y_CHECKS: [&str; 3] = [
    "focus_visible_on_keyboard_navigation",
    "controls_have_accessible_names",
    "state_not_color_only",
];
pub const OUTPUT_NAME: &str = "HEADLESS-1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stage {
    pub name: &'static str,
    /// Physical mode.
    pub width: u32,
    pub height: u32,
    /// Integer output scale.
    pub scale: u32,
}

const fn stage(name: &'static str, width: u32, height: u32, scale: u32) -> Stage {
    Stage {
        name,
        width,
        height,
        scale,
    }
}

/// Start (as configured by `.local/native-input/sway.conf`, no command), then three commands.
pub const STAGES: [Stage; 4] = [
    stage("start", 1280, 720, 1),
    stage("grow", 1600, 900, 1),
    stage("scale2", 1600, 900, 2),
    stage("restore", 1280, 720, 1),
];
pub const RESIZE_STEPS: [&str; 7] = [
    "resize-observe-start",
    "resize-apply-grow",
    "resize-observe-grow",
    "resize-apply-scale2",
    "resize-observe-scale2",
    "resize-apply-restore",
    "resize-observe-restore",
];
pub const A11Y_STEPS: [&str; 4] = [
    "a11y-sweep-main",
    "a11y-open-dialog",
    "a11y-sweep-dialog",
    "a11y-close-dialog",
];
/// Provisional crispness thresholds (synthetic PPM only; calibrate natively, see contract).
pub const MAX_DOUBLED_FRACTION: f64 = 0.5;
pub const MIN_INK_BLOCKS: u64 = 50;
pub const MIN_EDGE_RATIO: f64 = 0.8;
pub const MAX_KEYS: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewExpectations {
    pub endpoint: String,
    pub socket: String,
    /// Pid of the composed window in the private sway tree (crop origin); `None` = no crop.
    pub window_pid: Option<u32>,
}

impl ViewExpectations {
    pub fn new(endpoint: &str, runtime_dir: &str, socket: &str) -> Result<Self, String> {
        if endpoint.is_empty() {
            return Err("endpoint is required".into());
        }
        if !runtime_dir.starts_with("/tmp/hd7L-")
            || !socket.starts_with(&format!("{runtime_dir}/sway-ipc."))
        {
            return Err(format!("{socket} is not the private compositor"));
        }
        Ok(Self {
            endpoint: endpoint.into(),
            socket: socket.into(),
            window_pid: None,
        })
    }

    pub fn with_window(mut self, pid: u32) -> Self {
        self.window_pid = Some(pid);
        self
    }
}

/// `swaymsg` words after `-s SOCK`; `None` for the start stage (observed, not commanded).
pub fn output_command(s: &Stage) -> Option<Vec<String>> {
    (s.name != "start").then(|| {
        let res = format!("{}x{}", s.width, s.height);
        [
            "output",
            OUTPUT_NAME,
            "resolution",
            &res,
            "scale",
            &s.scale.to_string(),
        ]
        .map(str::to_owned)
        .to_vec()
    })
}

pub fn swaymsg_args(socket: &str, words: &[String]) -> Vec<String> {
    let mut args = vec!["-s".to_owned(), socket.to_owned()];
    args.extend(words.iter().cloned());
    args
}

/// `wtype` arguments of one allowed key token (for `Supervisor::wtype`).
pub fn wtype_args(token: &str) -> Result<Vec<String>, String> {
    let words: &[&str] = match token {
        "Tab" => &["-k", "Tab"],
        "shift+Tab" => &["-M", "shift", "-k", "Tab", "-m", "shift"],
        "ctrl+shift+F6" => &[
            "-M", "ctrl", "-M", "shift", "-k", "F6", "-m", "shift", "-m", "ctrl",
        ],
        "Return" => &["-k", "Return"],
        "Escape" => &["-k", "Escape"],
        _ => return Err(format!("key {token:?} is not allowed")),
    };
    Ok(words.iter().map(|w| (*w).to_owned()).collect())
}

/// Exit chord of the terminal (TerminalView isExitChord), as a parent key token.
pub const EXIT_CHORD: &str = "ctrl+shift+F6";

/// Planner decision r12 (GUI r11): the main sweep crosses the document wrap between the last stop
/// following the exit marker and the first preceding one, where WebKitGTK spends two Tabs (the
/// first reaches `body` without focusin). The page declares `wrap_after` (index in `expected` of
/// the last following stop) and the `split` it derives from (`following`/`preceding` ids, in
/// `orderAfterMarker` order). Returns the index only when the split is exactly the distinct stops
/// of `expected` in order (all but the final re-entry), both sides are non-empty and `wrap_after`
/// is the last following stop. A missing declaration is an error naming the field.
pub fn main_wrap(detail: &Value, expected: &[Value]) -> Result<usize, String> {
    let declared = field(detail, "wrap_after", "a11y-sweep-main")?;
    let split = field(detail, "split", "a11y-sweep-main")?;
    let side = |k: &str| -> Result<Vec<&str>, String> {
        field(split, k, "split")?
            .as_array()
            .ok_or(format!("split.{k} is not a list"))?
            .iter()
            .map(|v| v.as_str().ok_or(format!("split.{k}: {v} is not an id")))
            .collect()
    };
    let (following, preceding) = (side("following")?, side("preceding")?);
    let stops: Vec<Option<&str>> = expected
        .iter()
        .take(expected.len().saturating_sub(1))
        .map(|c| c["id"].as_str())
        .collect();
    let listed: Vec<Option<&str>> = following
        .iter()
        .chain(&preceding)
        .map(|i| Some(*i))
        .collect();
    if following.is_empty() || preceding.is_empty() || listed != stops {
        return Err(format!(
            "a11y-sweep-main: split {split} is not the expected stops split around the marker"
        ));
    }
    let wrap = following.len() - 1;
    (declared.as_u64() == Some(wrap as u64))
        .then_some(wrap)
        .ok_or(format!(
            "a11y-sweep-main: wrap_after {declared} is not the last following stop {wrap}"
        ))
}

/// Parent-side validation of the page's key plan before pressing anything. `wrap_after` is the
/// main sweep's checked declaration ([`main_wrap`]); every other step takes `None`.
pub fn validate_key_plan(
    step: &str,
    plan: &Value,
    expected: usize,
    wrap_after: Option<usize>,
) -> Result<Vec<String>, String> {
    let keys: Vec<String> = plan
        .as_array()
        .ok_or(format!("{step}: key plan is not a list"))?
        .iter()
        .map(|k| {
            k.as_str()
                .map(str::to_owned)
                .ok_or(format!("{step}: key {k} is not text"))
        })
        .collect::<Result<_, _>>()?;
    for k in &keys {
        wtype_args(k)?;
    }
    let ok = match (step, wrap_after) {
        ("a11y-open-dialog", None) => keys == ["Return"],
        ("a11y-close-dialog", None) => keys == ["Escape"],
        // Planner decision r10: `expected` = N stops (terminal last) + the terminal again; keys =
        // chord, Tab per stop, chord once on the terminal, Shift+Tab back into it. No Tab after
        // the terminal. r12: exactly one extra Tab for the declared wrap, which must follow a stop
        // before the terminal (w + 2 <= N); undeclared, it is refused.
        ("a11y-sweep-main", Some(w)) => {
            keys.len() <= MAX_KEYS
                && expected >= 3
                && w + 3 <= expected
                && keys.len() == expected + 3
                && keys[0] == EXIT_CHORD
                && keys[1..=expected].iter().all(|k| k == "Tab")
                && keys[expected + 1] == EXIT_CHORD
                && keys[expected + 2] == "shift+Tab"
        }
        ("a11y-sweep-main", None) => false,
        ("a11y-sweep-dialog", None) => {
            keys.len() <= MAX_KEYS
                && expected >= 2
                && keys.iter().filter(|k| *k == "Tab").count() == expected
                && keys.iter().filter(|k| *k == "shift+Tab").count() == 1
                && keys.last().is_some_and(|k| k == "shift+Tab")
                && !keys.contains(&"Return".to_owned())
                && !keys.contains(&"Escape".to_owned())
        }
        ("a11y-open-dialog" | "a11y-close-dialog" | "a11y-sweep-dialog", Some(_)) => false,
        _ => return Err(format!("{step} is not a {A11Y_PHASE} step")),
    };
    ok.then_some(keys).ok_or(format!(
        "{step}: key plan {plan} does not match {expected} controls"
    ))
}

// ---------------------------------------------------------------- screenshots (grim -t ppm)

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Crop {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Crisp {
    pub ink_blocks: u64,
    pub doubled_fraction: f64,
    pub edge_p95: f64,
}

/// Binary PPM `P6`, maxval 255.
pub fn parse_ppm(bytes: &[u8]) -> Result<Image, String> {
    let mut fields = Vec::new();
    let mut i = 0;
    while fields.len() < 4 {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if start == i {
            return Err("truncated PPM header".into());
        }
        fields.push(String::from_utf8_lossy(&bytes[start..i]).into_owned());
    }
    let n = |s: &str| s.parse::<usize>().map_err(|_| format!("PPM field {s}"));
    let (width, height) = (n(&fields[1])?, n(&fields[2])?);
    if fields[0] != "P6" || fields[3] != "255" {
        return Err(format!(
            "unsupported PPM {} maxval {}",
            fields[0], fields[3]
        ));
    }
    let rgb = bytes.get(i + 1..).unwrap_or_default().to_vec();
    if rgb.len() != width * height * 3 {
        return Err(format!("PPM {width}x{height} with {} bytes", rgb.len()));
    }
    Ok(Image { width, height, rgb })
}

fn luma(img: &Image, x: usize, y: usize) -> i32 {
    let p = &img.rgb[(y * img.width + x) * 3..];
    (i32::from(p[0]) * 299 + i32::from(p[1]) * 587 + i32::from(p[2]) * 114) / 1000
}

/// Ink 2×2 blocks (even-aligned), fraction of ink blocks whose 4 pixels are identical (nearest
/// doubling) and 95th percentile of horizontal luma steps touching ink (soft edges are low).
pub fn crispness(img: &Image, c: Crop) -> Result<Crisp, String> {
    if c.width < 4 || c.height < 4 || c.x + c.width > img.width || c.y + c.height > img.height {
        return Err(format!("crop {c:?} outside {}x{}", img.width, img.height));
    }
    let mut hist = [0u64; 256];
    for y in c.y..c.y + c.height {
        for x in c.x..c.x + c.width {
            hist[luma(img, x, y).clamp(0, 255) as usize] += 1;
        }
    }
    let bg = (0..256).max_by_key(|v| hist[*v]).unwrap_or(0) as i32;
    let ink = |x, y| (luma(img, x, y) - bg).abs() > 32;
    let (mut blocks, mut doubled, mut steps) = (0u64, 0u64, Vec::new());
    let x0 = c.x + c.x % 2;
    let y0 = c.y + c.y % 2;
    for by in (y0..c.y + c.height - 1).step_by(2) {
        for bx in (x0..c.x + c.width - 1).step_by(2) {
            let px = [(bx, by), (bx + 1, by), (bx, by + 1), (bx + 1, by + 1)];
            if px.iter().any(|&(x, y)| ink(x, y)) {
                blocks += 1;
                let at = |(x, y): (usize, usize)| &img.rgb[(y * img.width + x) * 3..][..3];
                if px.iter().all(|&p| at(p) == at(px[0])) {
                    doubled += 1;
                }
            }
        }
    }
    for y in c.y..c.y + c.height {
        for x in c.x..c.x + c.width - 1 {
            if ink(x, y) || ink(x + 1, y) {
                steps.push((luma(img, x, y) - luma(img, x + 1, y)).abs());
            }
        }
    }
    steps.sort_unstable();
    let edge_p95 = steps.get(steps.len() * 95 / 100).copied().unwrap_or(0) as f64;
    let doubled_fraction = if blocks == 0 {
        1.0
    } else {
        doubled as f64 / blocks as f64
    };
    Ok(Crisp {
        ink_blocks: blocks,
        doubled_fraction,
        edge_p95,
    })
}

// ---------------------------------------------------------------- evaluators

fn field<'a>(v: &'a Value, key: &str, what: &str) -> Result<&'a Value, String> {
    v.get(key)
        .filter(|f| !f.is_null())
        .ok_or_else(|| format!("{what} without {key}"))
}

fn num(v: &Value, key: &str, what: &str) -> Result<f64, String> {
    field(v, key, what)?
        .as_f64()
        .ok_or_else(|| format!("{what} without numeric {key}"))
}

const IDENTITY: [&str; 4] = ["pane_id", "generation", "boot_prefix", "endpoint"];

fn identity(v: &Value, what: &str) -> Result<Vec<String>, String> {
    IDENTITY
        .iter()
        .map(|k| {
            v[*k]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| format!("{what} without {k}"))
        })
        .collect()
}

fn answer<'a>(ledger: &'a Ledger, name: &str) -> Result<&'a Value, String> {
    let a = ledger
        .steps
        .get(name)
        .ok_or_else(|| format!("{name}: parent never observed this step"))?;
    if a["step"] != name {
        return Err(format!("{name}: ledger entry of step {}", a["step"]));
    }
    Ok(a)
}

fn start(phase: &str, want: &str, page: &Value) -> Result<(), String> {
    if phase != want {
        return Err(format!("{phase} is not {want}"));
    }
    if !page["error"].is_null() {
        return Err(format!("{phase}: page error {}", page["error"]));
    }
    Ok(())
}

fn px(v: &Value, key: &str) -> Result<f64, String> {
    field(v, key, "canvas")?
        .as_str()
        .and_then(|s| s.strip_suffix("px"))
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("canvas {key} is not NNNpx"))
}

/// `inner_rect` of `pane` among the page's frame metadata panes; absent is an error.
fn pane_inner_rect<'a>(page: &'a Value, pane: &str, what: &str) -> Result<&'a Value, String> {
    field(page, "panes", what)?
        .as_array()
        .and_then(|ps| ps.iter().find(|p| p["pane_id"] == pane))
        .map(|p| &p["inner_rect"])
        .filter(|r| r.is_object())
        .ok_or_else(|| format!("{what}: frame metadata without inner_rect of pane {pane}"))
}

struct Seen {
    rows: u64,
    cols: u64,
    painted: f64,
    full: f64,
    edge: f64,
}

/// Named checks of [`RESIZE_PHASE`].
pub fn resize_checks(
    phase: &str,
    page: &Value,
    ledger: &Ledger,
    e: &ViewExpectations,
) -> Result<Vec<(&'static str, bool)>, String> {
    start(phase, RESIZE_PHASE, page)?;
    for step in RESIZE_STEPS {
        answer(ledger, step)?;
    }
    let stages = field(page, "stages", "page")?
        .as_array()
        .ok_or("page stages is not a list")?;
    let names_ok = stages.len() == STAGES.len()
        && stages
            .iter()
            .zip(&STAGES)
            .all(|(p, s)| p["stage"] == s.name);
    if !names_ok {
        return Ok(vec![(RESIZE_CHECKS[0], false), (RESIZE_CHECKS[1], false)]);
    }
    let id0 = identity(field(&stages[0], "identity", "stage start")?, "stage start")?;
    let (mut geo, mut crisp) = (id0[3] == e.endpoint, true);
    let mut seen: Vec<Seen> = Vec::new();
    let mut offset = None;
    for (p, s) in stages.iter().zip(&STAGES) {
        let what = format!("stage {}", s.name);
        let obs = answer(ledger, &format!("resize-observe-{}", s.name))?;
        geo &= identity(field(p, "identity", &what)?, &what)? == id0
            && identity(field(obs, "identity", &what)?, &what)? == id0
            && obs["socket"] == e.socket.as_str();
        if let Some(cmd) = output_command(s) {
            let apply = answer(ledger, &format!("resize-apply-{}", s.name))?;
            geo &= apply["command"] == serde_json::json!(cmd)
                && apply["socket"] == e.socket.as_str()
                && identity(field(apply, "identity", &what)?, &what)? == id0;
        }
        let out = field(obs, "outputs", &what)?
            .as_array()
            .and_then(|o| o.iter().find(|o| o["name"] == OUTPUT_NAME))
            .ok_or_else(|| format!("{what}: outputs without {OUTPUT_NAME}"))?;
        let (lw, lh) = (f64::from(s.width / s.scale), f64::from(s.height / s.scale));
        let scale = f64::from(s.scale);
        geo &= out["active"] == true
            && num(&out["current_mode"], "width", &what)? == f64::from(s.width)
            && num(&out["current_mode"], "height", &what)? == f64::from(s.height)
            && num(out, "scale", &what)? == scale
            && num(&out["rect"], "width", &what)? == lw
            && num(&out["rect"], "height", &what)? == lh;
        let d = (
            lw - num(p, "inner_width", &what)?,
            lh - num(p, "inner_height", &what)?,
        );
        let o = *offset.get_or_insert(d);
        geo &= d == o && (0.0..64.0).contains(&d.0) && (0.0..64.0).contains(&d.1);
        let dpr = num(p, "dpr", &what)?;
        let canvas = field(p, "canvas", &what)?;
        let (cw, ch) = (px(canvas, "css_width")?, px(canvas, "css_height")?);
        let transform = field(canvas, "transform", &what)?
            .as_array()
            .ok_or("transform")?;
        let t: Vec<f64> = transform.iter().filter_map(Value::as_f64).collect();
        let rect = field(canvas, "rect", &what)?;
        let m = field(p, "metrics", &what)?;
        let (cell_w, cell_h) = (num(m, "cellWidth", &what)?, num(m, "cellHeight", &what)?);
        let cols = (num(rect, "width", &what)? / cell_w).floor().max(2.0);
        let rows = (num(rect, "height", &what)? / cell_h).floor().max(1.0);
        // The engine composes the tab: the PTY has the pane inner_rect the frame metadata carried
        // (one column narrower than the surface with the stable gutter), never floor(canvas/cell).
        let surface = field(p, "surface", &what)?;
        let (s_cols, s_rows) = (num(surface, "cols", &what)?, num(surface, "rows", &what)?);
        let inner = pane_inner_rect(p, &id0[0], &what)?;
        let (px_, py_) = (num(inner, "x", &what)?, num(inner, "y", &what)?);
        let (p_cols, p_rows) = (num(inner, "width", &what)?, num(inner, "height", &what)?);
        let pty = field(obs, "pty_size", &what)?;
        geo &= s_cols == cols
            && s_rows == rows
            && p_cols >= 1.0
            && p_rows >= 1.0
            && px_ + p_cols <= s_cols
            && py_ + p_rows <= s_rows;
        geo &= dpr == scale
            && num(canvas, "width", &what)? == (cw * dpr).round()
            && num(canvas, "height", &what)? == (ch * dpr).round()
            && t == [dpr, 0.0, 0.0, dpr, 0.0, 0.0]
            && (num(rect, "width", &what)? - cw).abs() <= 0.5
            && (num(rect, "height", &what)? - ch).abs() <= 0.5
            && pty == &serde_json::json!([p_rows as u64, p_cols as u64])
            && cw == cols * cell_w
            && ch == rows * cell_h;
        let probe = field(p, "probe", &what)?;
        let screen = field(obs, "screen", &what)?;
        let now = Seen {
            rows: rows as u64,
            cols: cols as u64,
            painted: num(probe, "painted_rows", &what)?,
            full: num(probe, "full_frames", &what)?,
            edge: num(screen, "edge_p95", &what)?,
        };
        crisp &= num(screen, "width", &what)? == f64::from(s.width)
            && num(screen, "height", &what)? == f64::from(s.height);
        if let Some(prev) = seen.last() {
            crisp &= now.painted - prev.painted >= now.rows as f64 && now.full - prev.full >= 1.0;
        }
        if s.scale == 2 {
            crisp &= num(screen, "doubled_fraction", &what)? <= MAX_DOUBLED_FRACTION
                && num(screen, "ink_blocks", &what)? >= MIN_INK_BLOCKS as f64
                && now.edge >= MIN_EDGE_RATIO * seen[0].edge;
        }
        seen.push(now);
    }
    let bigger = |a: &Seen, b: &Seen| a.rows > b.rows && a.cols > b.cols;
    geo &= bigger(&seen[1], &seen[0])
        && bigger(&seen[1], &seen[2])
        && seen[3].rows == seen[0].rows
        && seen[3].cols == seen[0].cols;
    Ok(vec![(RESIZE_CHECKS[0], geo), (RESIZE_CHECKS[1], crisp)])
}

/// Expected non-modifier keydown `(key, shift, ctrl)` of one parent key token.
fn keydown(token: &str) -> (&'static str, bool, bool) {
    match token {
        "shift+Tab" => ("Tab", true, false),
        "ctrl+shift+F6" => ("F6", true, true),
        "Return" => ("Enter", false, false),
        "Escape" => ("Escape", false, false),
        _ => ("Tab", false, false),
    }
}

fn keys_match(seen: &Value, parent: &Value, what: &str) -> Result<bool, String> {
    let seen = seen
        .as_array()
        .ok_or_else(|| format!("{what}: keys_seen is not a list"))?;
    let parent = parent
        .as_array()
        .ok_or_else(|| format!("{what}: parent keys"))?;
    let modifier =
        |k: &Value| ["Shift", "Control", "Alt", "Meta"].contains(&k["key"].as_str().unwrap_or(""));
    let real: Vec<&Value> = seen.iter().filter(|k| !modifier(k)).collect();
    Ok(seen.iter().all(|k| k["trusted"] == true)
        && real.len() == parent.len()
        && real.iter().zip(parent).all(|(k, p)| {
            let (key, shift, ctrl) = keydown(p.as_str().unwrap_or(""));
            k["key"] == key && k["shift"] == shift && k["ctrl"] == ctrl
        }))
}

/// Visible outline: style (`solid` only when `solid`), width > 0 px, opaque non-transparent color.
fn outline_visible(s: &Value, solid: bool) -> bool {
    let width = s["outline_width"]
        .as_str()
        .and_then(|w| w.strip_suffix("px"))
        .and_then(|w| w.parse::<f64>().ok());
    let color = s["outline_color"].as_str().unwrap_or("");
    let style = s["outline_style"].as_str().unwrap_or("none");
    (if solid {
        style == "solid"
    } else {
        style != "none"
    }) && width.is_some_and(|w| w > 0.0)
        && !color.is_empty()
        && color != "transparent"
        && !color.replace(' ', "").ends_with(",0)")
}

/// Visible outline, or box-shadow/border/background different from the recorded baseline.
fn indicator(s: &Value, b: &Value, solid: bool) -> bool {
    let changed = |k: &str| s[k].is_string() && b[k].is_string() && s[k] != b[k];
    outline_visible(s, solid)
        || changed("box_shadow")
        || changed("border_color")
        || changed("background_color")
}

/// The terminal IME target (`textarea.ime-target`): the only control whose focus ring may be
/// drawn by its `.terminal` container (Planner decision r7).
fn ime_target(f: &Value) -> bool {
    f["tag"] == "textarea"
        && f["classes"]
            .as_array()
            .is_some_and(|c| c.iter().any(|k| k == "ime-target"))
}

fn indicated(f: &Value) -> bool {
    f["focus_visible"] == true
        && (indicator(&f["style"], &f["baseline"], false)
            || ime_target(f)
                && f["container_style"].is_object()
                && indicator(&f["container_style"], &f["container_baseline"], true))
}

fn named(text: &Value) -> bool {
    text.as_str()
        .is_some_and(|t| t.chars().any(char::is_alphanumeric))
}

/// Index in the dialog's expected controls of the focus the page recorded right after the native
/// Return (product initial focus; the harness never focuses inside the dialog). `None` when
/// nothing or something outside the modal was focused; absent field is an error.
fn dialog_start(page: &Value, ids: &[&Value]) -> Result<Option<usize>, String> {
    let start = page
        .get("dialog_start_focus")
        .ok_or("page without dialog_start_focus")?;
    Ok(start
        .get("id")
        .and_then(|id| ids.iter().position(|w| *w == id))
        .filter(|_| ids.len() >= 2))
}

/// Named checks of [`A11Y_PHASE`].
pub fn a11y_checks(
    phase: &str,
    page: &Value,
    ledger: &Ledger,
    e: &ViewExpectations,
) -> Result<Vec<(&'static str, bool)>, String> {
    start(phase, A11Y_PHASE, page)?;
    let (mut focus_ok, mut names_ok) = (true, true);
    let mut id0 = None;
    for (sweep, step) in [("main", A11Y_STEPS[0]), ("dialog", A11Y_STEPS[2])] {
        let p = field(page, sweep, "page")?;
        let a = answer(ledger, step)?;
        let id = identity(field(p, "identity", sweep)?, sweep)?;
        let first = id0.get_or_insert_with(|| id.clone()).clone();
        focus_ok &= id == first
            && id[3] == e.endpoint
            && identity(field(a, "identity", step)?, step)? == first;
        let expected = field(p, "expected", sweep)?.as_array().ok_or("expected")?;
        let focus = field(p, "focus", sweep)?.as_array().ok_or("focus")?;
        let keys = field(a, "keys", step)?;
        // r12: the main sweep must declare its wrap (absent = harness error); an inconsistent
        // declaration fails the sweep. The dialog sweep has none.
        let wrap = if sweep == "main" {
            field(p, "wrap_after", sweep)?;
            field(p, "split", sweep)?;
            main_wrap(p, expected).ok()
        } else {
            None
        };
        focus_ok &= (sweep != "main" || wrap.is_some())
            && validate_key_plan(step, keys, expected.len(), wrap).is_ok()
            && keys_match(field(p, "keys_seen", sweep)?, keys, sweep)?;
        let ids: Vec<&Value> = expected.iter().map(|x| &x["id"]).collect();
        let want = match sweep {
            // r10 plan: from the exit marker every stop once (first..terminal), the chord leaves
            // the terminal and Shift+Tab re-enters it. The page must list exactly that: distinct
            // stops, then the last one again; otherwise (e.g. the r8/r9 wrap to the first stop)
            // nothing is wanted and the sweep fails.
            "main" => {
                let n = ids.len();
                let once = n >= 3 && (1..n - 1).all(|i| !ids[..i].contains(&ids[i]));
                if once && ids[n - 1] == ids[n - 2] {
                    let mut w = ids[..n - 1].to_vec();
                    w.push(ids[n - 2]);
                    w
                } else {
                    focus_ok = false;
                    Vec::new()
                }
            }
            // Modal: from the recorded initial focus, N Tabs wrap back to it, shift+Tab to start-1.
            _ => match dialog_start(page, &ids)? {
                Some(s) => {
                    let n = ids.len();
                    let mut w: Vec<&Value> = (1..=n).map(|k| ids[(s + k) % n]).collect();
                    w.push(ids[(s + n - 1) % n]);
                    w
                }
                None => {
                    focus_ok = false;
                    Vec::new()
                }
            },
        };
        focus_ok &= focus.len() == want.len()
            && focus.iter().zip(&want).all(|(f, w)| &&f["id"] == w)
            && focus.iter().all(|f| {
                f["disabled"] == false
                    && f["tabindex"].as_i64().is_some_and(|t| t >= 0)
                    && indicated(f)
            });
        names_ok &= focus
            .iter()
            .chain(expected)
            .all(|f| named(&f["name"]) && f["name_source"].as_str().is_some_and(|s| s != "none"));
    }
    let start = page
        .get("dialog_start_focus")
        .ok_or("page without dialog_start_focus")?;
    let modal = field(page, "dialog_modal", "page")?;
    focus_ok &= modal["role"] == "dialog"
        && modal["modal"] == true
        && (start.is_null()
            || start["disabled"] == false && start["tabindex"].as_i64().is_some_and(|t| t >= 0));
    names_ok &= named(&modal["name"])
        && modal["name_source"].as_str().is_some_and(|s| s != "none")
        && (start.is_null()
            || named(&start["name"]) && start["name_source"].as_str().is_some_and(|s| s != "none"));
    let open = answer(ledger, A11Y_STEPS[1])?;
    focus_ok &= keys_match(
        field(page, "dialog_open_keys", "page")?,
        field(open, "keys", A11Y_STEPS[1])?,
        "dialog",
    )?;
    // Native Escape closes the modal and returns focus to the recorded opener, same identity.
    let close = field(page, "dialog_close", "page")?;
    let close_answer = answer(ledger, A11Y_STEPS[3])?;
    let opener = &page["dialog_opener"]["id"];
    focus_ok &= keys_match(
        field(close, "keys_seen", "dialog_close")?,
        field(close_answer, "keys", A11Y_STEPS[3])?,
        "dialog_close",
    )? && close["closed"] == true
        && opener.is_string()
        && &close["focus_after"]["id"] == opener
        && identity(field(close, "identity", "dialog_close")?, "dialog_close")?
            == id0.clone().unwrap_or_default()
        && identity(
            field(close_answer, "identity", A11Y_STEPS[3])?,
            A11Y_STEPS[3],
        )? == id0.clone().unwrap_or_default();
    let states = field(page, "states", "page")?.as_array().ok_or("states")?;
    let states_ok = !states.is_empty()
        && states.iter().all(|s| {
            let token = s["status"].as_str().unwrap_or("").to_lowercase();
            named(&s["own_text"])
                && s["own_text"]
                    .as_str()
                    .is_some_and(|t| t.chars().any(char::is_alphabetic))
                || (!token.is_empty()
                    && s["carrier_text"]
                        .as_str()
                        .is_some_and(|c| c.to_lowercase().contains(&token)))
        });
    Ok(vec![
        (A11Y_CHECKS[0], focus_ok),
        (A11Y_CHECKS[1], names_ok),
        (A11Y_CHECKS[2], states_ok),
    ])
}

// ---------------------------------------------------------------- parent adapters

/// Actual result of one private process run (argv, exit status, output), never fabricated.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Run {
    pub argv: Vec<String>,
    pub status: String,
    pub success: bool,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

/// What the parent touches natively. `Err` only when the process could not be run at all.
pub trait ViewWorld {
    /// Private sway IPC socket the world commands.
    fn socket(&self) -> String;
    /// `swaymsg ARGS` (ARGS = [`swaymsg_args`] on the private socket).
    fn swaymsg(&mut self, args: &[String]) -> Result<Run, String>;
    /// `grim -t ppm -o HEADLESS-1 -` of the private output.
    fn grim_ppm(&mut self) -> Result<Run, String>;
    /// One key token's [`wtype_args`] through `Supervisor::wtype`.
    fn wtype(&mut self, args: &[String]) -> Result<(), String>;
    /// Waits `ms` of real time (compositor focus samples after the close Escape).
    fn sleep_ms(&mut self, ms: u64);
}

/// `stty size` text of the pane PTY, supplied by the live adapter (pane id → text).
pub type PtySize<'a> = dyn FnMut(&str) -> Result<String, String> + 'a;

impl Run {
    /// Audit record; `stdout` kept as text only when it is small text (not a screenshot).
    fn record(&self, with_stdout: bool) -> Value {
        let mut r = json!({"argv": self.argv, "status": self.status, "success": self.success,
            "stdout_bytes": self.stdout.len(), "stderr": self.stderr});
        if with_stdout {
            r["stdout"] = json!(String::from_utf8_lossy(&self.stdout));
        }
        r
    }

    fn checked(&self, what: &str) -> Result<(), String> {
        if self.success {
            Ok(())
        } else {
            Err(format!(
                "{what} {:?}: {}: {}",
                self.argv,
                self.status,
                self.stderr.trim()
            ))
        }
    }
}

/// Crop origin source: client origin observed in the private sway tree (window + GTK header,
/// [`super::geometry::client_geometry`]); crop px = (origin + view.canvas.rect) x output scale.
pub const CROP_ORIGIN: &str = "sway get_tree";

enum Action {
    Apply(Stage, Vec<String>),
    Observe(Stage),
    Keys(Vec<String>),
}

fn sequence(step: &str) -> Result<&'static [&'static str], String> {
    if RESIZE_STEPS.contains(&step) {
        Ok(&RESIZE_STEPS)
    } else if A11Y_STEPS.contains(&step) {
        Ok(&A11Y_STEPS)
    } else {
        Err(format!("{step} is not a {RESIZE_PHASE}/{A11Y_PHASE} step"))
    }
}

/// Everything refused before touching the world: unknown, duplicate or out-of-order step,
/// incomplete/other-host/stale identity, wrong socket, stage or key plan not of this step.
fn admit<W: ViewWorld>(
    step: &str,
    detail: &Value,
    e: &ViewExpectations,
    world: &W,
    ledger: &Ledger,
) -> Result<(Value, Action), String> {
    let seq = sequence(step)?;
    if ledger.steps.contains_key(step) {
        return Err(format!("{step}: answered twice"));
    }
    let at = seq.iter().position(|s| *s == step).unwrap_or(0);
    if let Some(prev) = at.checked_sub(1).map(|p| seq[p]) {
        if !ledger.steps.contains_key(prev) {
            return Err(format!("{step}: previous step {prev} was never answered"));
        }
    }
    let id = identity(detail, step)?;
    if id[3] != e.endpoint {
        return Err(format!("{step}: page host {} is not {}", id[3], e.endpoint));
    }
    if at > 0 && identity(&ledger.steps[seq[0]]["identity"], seq[0])? != id {
        return Err(format!("{step}: identity {id:?} differs from {}", seq[0]));
    }
    let socket = world.socket();
    if socket != e.socket {
        return Err(format!("{step}: {socket} is not the private compositor"));
    }
    let id: serde_json::Map<String, Value> = IDENTITY
        .iter()
        .zip(id)
        .map(|(k, v)| ((*k).into(), json!(v)))
        .collect();
    let stage = |name: &str| -> Result<Stage, String> {
        let s = STAGES.iter().find(|s| s.name == name).copied();
        let s = s.ok_or_else(|| format!("{step}: no stage {name}"))?;
        if detail["stage"] != s.name {
            return Err(format!(
                "{step}: page stage {} is not {}",
                detail["stage"], s.name
            ));
        }
        Ok(s)
    };
    let action = if let Some(name) = step.strip_prefix("resize-apply-") {
        let s = stage(name)?;
        Action::Apply(
            s,
            output_command(&s).ok_or("start is observed, not applied")?,
        )
    } else if let Some(name) = step.strip_prefix("resize-observe-") {
        Action::Observe(stage(name)?)
    } else {
        let (expected, wrap) = match step {
            "a11y-open-dialog" | "a11y-close-dialog" => (0, None),
            _ => {
                let list = field(detail, "expected", step)?
                    .as_array()
                    .ok_or_else(|| format!("{step}: expected is not a list"))?;
                let wrap = if step == A11Y_STEPS[0] {
                    Some(main_wrap(detail, list)?)
                } else {
                    None
                };
                (list.len(), wrap)
            }
        };
        Action::Keys(validate_key_plan(
            step,
            field(detail, "keys", step)?,
            expected,
            wrap,
        )?)
    };
    let answer = json!({"step": step, "identity": Value::Object(id), "socket": socket});
    Ok((answer, action))
}

fn outputs<W: ViewWorld>(world: &mut W, socket: &str, answer: &mut Value) -> Result<(), String> {
    let words = ["-t", "get_outputs", "-r"].map(str::to_owned);
    let run = world.swaymsg(&swaymsg_args(socket, &words))?;
    answer["outputs_run"] = run.record(!run.success);
    run.checked("swaymsg")?;
    let parsed: Value = serde_json::from_slice(&run.stdout)
        .map_err(|err| format!("get_outputs is not JSON: {err}"))?;
    if !parsed.is_array() {
        return Err(format!("get_outputs is not a list: {parsed}"));
    }
    answer["outputs"] = parsed;
    Ok(())
}

/// `stty size` → `[rows, cols]`; anything else is an error.
pub fn parse_stty_size(text: &str) -> Result<[u64; 2], String> {
    let n: Vec<u64> = text
        .split_whitespace()
        .map(str::parse)
        .collect::<Result<_, _>>()
        .map_err(|_| format!("stty size text {text:?}"))?;
    match n[..] {
        [rows, cols] if rows > 0 && cols > 0 => Ok([rows, cols]),
        _ => Err(format!("stty size text {text:?}")),
    }
}

fn screen<W: ViewWorld>(
    world: &mut W,
    detail: &Value,
    e: &ViewExpectations,
    answer: &mut Value,
) -> Result<(), String> {
    let run = world.grim_ppm()?;
    answer["grim_run"] = run.record(false);
    run.checked("grim")?;
    let img = parse_ppm(&run.stdout)?;
    answer["screen"] = json!({"width": img.width, "height": img.height});
    let view = field(detail, "view", "observe detail")?;
    let dpr = num(view, "dpr", "view")?;
    // Actual output of this observation (outputs ran first): scale and physical mode.
    let output = answer["outputs"]
        .as_array()
        .and_then(|o| o.iter().find(|o| o["name"] == OUTPUT_NAME))
        .ok_or_else(|| format!("no {OUTPUT_NAME} in the observed outputs"))?
        .clone();
    let scale = num(&output, "scale", "output")?;
    if dpr != scale {
        return Err(format!(
            "page dpr {dpr} is not the observed output scale {scale}"
        ));
    }
    let mode = field(&output, "current_mode", "output")?;
    let (mw, mh) = (num(mode, "width", "mode")?, num(mode, "height", "mode")?);
    if img.width as f64 != mw || img.height as f64 != mh {
        return Err(format!(
            "screenshot {}x{} is not the observed mode {mw}x{mh}",
            img.width, img.height
        ));
    }
    let pid = e
        .window_pid
        .ok_or("crop needs the window pid (no observed window pid)")?;
    let tree_run = world.swaymsg(&swaymsg_args(
        &e.socket,
        &["-t", "get_tree", "-r"].map(str::to_owned),
    ))?;
    answer["tree_run"] = tree_run.record(false);
    tree_run.checked("swaymsg get_tree")?;
    let tree: Value = serde_json::from_slice(&tree_run.stdout)
        .map_err(|err| format!("get_tree is not JSON: {err}"))?;
    let client = json!({
        "coordinate_space": super::geometry::CLIENT_SPACE,
        "width": num(view, "inner_width", "view")?,
        "height": num(view, "inner_height", "view")?,
        "dpr": dpr,
    });
    let g = super::geometry::client_geometry(&tree, pid, &client)?;
    answer["screen"]["crop_origin"] = json!({"source": CROP_ORIGIN, "x": g.x, "y": g.y,
        "header": g.header, "dpr": dpr, "observation": g.observation});
    let rect = field(field(view, "canvas", "view")?, "rect", "canvas")?;
    let px = |v: f64, what: &str| -> Result<usize, String> {
        let v = v * dpr;
        (v >= 0.0)
            .then_some(v.round() as usize)
            .ok_or_else(|| format!("canvas {what} {v} is negative on the output"))
    };
    let crop = Crop {
        x: px(g.x + num(rect, "left", "canvas rect")?, "left")?,
        y: px(g.y + num(rect, "top", "canvas rect")?, "top")?,
        width: px(num(rect, "width", "canvas rect")?, "width")?,
        height: px(num(rect, "height", "canvas rect")?, "height")?,
    };
    answer["screen"]["crop"] =
        json!({"x": crop.x, "y": crop.y, "width": crop.width, "height": crop.height, "dpr": dpr});
    let c = crispness(&img, crop)?;
    answer["screen"]["ink_blocks"] = json!(c.ink_blocks);
    answer["screen"]["doubled_fraction"] = json!(c.doubled_fraction);
    answer["screen"]["edge_p95"] = json!(c.edge_p95);
    Ok(())
}

/// Offsets (ms after the close Escape returned) of the compositor focus samples.
pub const FOCUS_SAMPLE_MS: [u64; 3] = [0, 50, 200];

/// Node sway marks `focused` (depth-first over nodes and floating_nodes), if any.
fn sway_focused(node: &Value) -> Option<&Value> {
    if node["focused"] == true {
        return Some(node);
    }
    ["nodes", "floating_nodes"]
        .iter()
        .flat_map(|k| node[*k].as_array().into_iter().flatten())
        .find_map(sway_focused)
}

/// Actual compositor focus right after the close Escape, +50 ms and +200 ms: `swaymsg get_tree`
/// on the private socket, focused node id/type/pid/app_id/name/shell and whether it is the window
/// pid. A failed or unparsable run is recorded as `error`, never as a focus value.
fn focus_samples<W: ViewWorld>(world: &mut W, socket: &str, e: &ViewExpectations) -> Vec<Value> {
    let start = std::time::Instant::now();
    let args = swaymsg_args(socket, &["-t", "get_tree", "-r"].map(str::to_owned));
    let mut reached = 0;
    FOCUS_SAMPLE_MS
        .iter()
        .map(|&at| {
            // Remaining time to the offset: never re-sleep a delay already waited.
            let waited = (start.elapsed().as_millis() as u64).max(reached);
            if at > waited {
                world.sleep_ms(at - waited);
            }
            reached = at;
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            let mut sample = json!({"at_ms": at, "elapsed_ms": elapsed});
            let parsed = world.swaymsg(&args).and_then(|run| {
                sample["run"] = run.record(false);
                run.checked("swaymsg get_tree")?;
                serde_json::from_slice::<Value>(&run.stdout)
                    .map_err(|err| format!("get_tree is not JSON: {err}"))
            });
            match parsed {
                Ok(tree) => {
                    let f = sway_focused(&tree).cloned().unwrap_or(Value::Null);
                    sample["window_focused"] = json!(e
                        .window_pid
                        .is_some_and(|pid| f["pid"].as_u64() == Some(u64::from(pid))));
                    sample["focused"] = if f.is_null() {
                        Value::Null
                    } else {
                        json!({"id": f["id"], "type": f["type"], "pid": f["pid"],
                            "app_id": f["app_id"], "name": f["name"], "shell": f["shell"]})
                    };
                }
                Err(err) => sample["error"] = json!(format!("get_tree: {err}")),
            }
            sample
        })
        .collect()
}

/// Answers one parent step once, in phase order, for the pane the page confirmed. Refusals
/// (see `admit`) touch nothing and record nothing; once admitted, the actual commands, outputs
/// and errors are recorded in `ledger` (even when failing, so a step is never replayed).
/// `pty_size` is the live adapter's `stty size` reader of the pane (called on observe only).
pub fn parent_step<W: ViewWorld>(
    step: &str,
    detail: &Value,
    e: &ViewExpectations,
    world: &mut W,
    pty_size: &mut PtySize<'_>,
    ledger: &mut Ledger,
) -> Result<Value, String> {
    let (mut answer, action) = admit(step, detail, e, world, ledger)?;
    let socket = e.socket.clone();
    let result = match action {
        Action::Apply(_, words) => {
            answer["command"] = json!(words);
            world
                .swaymsg(&swaymsg_args(&socket, &words))
                .and_then(|run| {
                    answer["run"] = run.record(true);
                    run.checked("swaymsg")
                })
                .and_then(|()| outputs(world, &socket, &mut answer))
        }
        Action::Observe(_) => {
            let pane = answer["identity"]["pane_id"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            let errors: Vec<String> = [
                outputs(world, &socket, &mut answer),
                {
                    // Frame geometry the page received, next to stty (null when not received).
                    let view = &detail["view"];
                    let inner = view["panes"]
                        .as_array()
                        .and_then(|ps| ps.iter().find(|p| p["pane_id"] == pane.as_str()))
                        .map_or(&Value::Null, |p| &p["inner_rect"]);
                    answer["surface_cols"] = view["surface"]["cols"].clone();
                    answer["surface_rows"] = view["surface"]["rows"].clone();
                    answer["pane_cols"] = inner["width"].clone();
                    answer["pane_rows"] = inner["height"].clone();
                    Ok(())
                },
                pty_size(&pane).and_then(|text| {
                    answer["pty_stty"] = json!(text);
                    parse_stty_size(&text).map(|size| answer["pty_size"] = json!(size))
                }),
                screen(world, detail, e, &mut answer),
            ]
            .into_iter()
            .filter_map(Result::err)
            .collect();
            if errors.is_empty() {
                Ok(())
            } else {
                Err(errors.join("; "))
            }
        }
        Action::Keys(keys) => {
            answer["keys"] = json!(keys);
            let mut pressed = 0;
            let r = keys.iter().try_for_each(|k| {
                world.wtype(&wtype_args(k)?)?;
                pressed += 1;
                Ok::<(), String>(())
            });
            answer["pressed"] = json!(pressed);
            if r.is_ok() && step == A11Y_STEPS[3] {
                answer["compositor_focus"] = json!(focus_samples(world, &socket, e));
            }
            r
        }
    };
    if let Err(err) = &result {
        answer["error"] = json!(format!("{step}: {err}"));
    }
    ledger.record(step, answer.clone())?;
    result
        .map(|()| answer)
        .map_err(|err| format!("{step}: {err}"))
}

/// Launches of the private compositor only, derived from `mouse_flow::PointerEnv` (its explicit
/// `FORBIDDEN_INHERITED` stripping and swaymsg prefix + `LD_LIBRARY_PATH`); grim takes the
/// pointer's wtype env (no prefix libs).
#[derive(Debug, Clone)]
pub struct ViewEnv {
    socket: String,
    swaymsg: Launch,
    grim: Launch,
}

impl ViewEnv {
    pub fn new(pointer: &PointerEnv, e: &ViewExpectations) -> Result<Self, String> {
        if pointer.socket() != e.socket {
            return Err(format!("{} is not {}", pointer.socket(), e.socket));
        }
        let mut swaymsg = pointer.cursor_set(Point { x: 0.0, y: 0.0 });
        if swaymsg.args.get(..2) != Some(&["-s".to_owned(), e.socket.clone()][..]) {
            return Err(format!("pointer swaymsg {:?} is not private", swaymsg.args));
        }
        swaymsg.args.clear();
        let mut grim = pointer.ctrl_hold(0);
        grim.program = "/usr/bin/grim".into();
        grim.args = ["-t", "ppm", "-o", OUTPUT_NAME, "-"]
            .map(str::to_owned)
            .to_vec();
        for l in [&swaymsg, &grim] {
            if let Some(k) = FORBIDDEN_INHERITED.iter().find(|k| l.var(k).is_some()) {
                return Err(format!("{} inherits {k}", l.program.display()));
            }
        }
        Ok(Self {
            socket: e.socket.clone(),
            swaymsg,
            grim,
        })
    }

    pub fn socket(&self) -> &str {
        &self.socket
    }

    /// Refuses any argv not starting with `-s <private socket>`.
    pub fn swaymsg(&self, args: &[String]) -> Result<Launch, String> {
        if args.len() < 3 || args[0] != "-s" || args[1] != self.socket {
            return Err(format!("swaymsg {args:?} is not on {}", self.socket));
        }
        Ok(Launch {
            args: args.to_vec(),
            ..self.swaymsg.clone()
        })
    }

    pub fn grim(&self) -> Launch {
        self.grim.clone()
    }
}

#[cfg(target_os = "linux")]
pub use private::PrivateView;

#[cfg(target_os = "linux")]
mod private {
    use std::process::Stdio;

    use super::super::display::Launch;
    use super::super::supervisor::Supervisor;
    use super::{Run, ViewEnv, ViewWorld};

    /// swaymsg/grim of [`ViewEnv`] run synchronously per step (no process left behind); keys go
    /// through the one `Supervisor::wtype` (its own log).
    pub struct PrivateView<'a> {
        env: ViewEnv,
        supervisor: &'a mut Supervisor,
        pub log: Vec<String>,
    }

    impl<'a> PrivateView<'a> {
        pub fn new(env: ViewEnv, supervisor: &'a mut Supervisor) -> Self {
            Self {
                env,
                supervisor,
                log: Vec::new(),
            }
        }

        fn run(&mut self, launch: Launch) -> Result<Run, String> {
            let name = launch.program.display().to_string();
            let out = launch
                .command()
                .stdin(Stdio::null())
                .output()
                .map_err(|e| format!("{name}: {e}"))?;
            let run = Run {
                argv: launch.args.clone(),
                status: out.status.to_string(),
                success: out.status.success(),
                stdout: out.stdout,
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            };
            self.log.push(format!(
                "{name} {:?} -> {} stdout={}B stderr={:?}",
                run.argv,
                run.status,
                run.stdout.len(),
                run.stderr.trim()
            ));
            Ok(run)
        }
    }

    impl ViewWorld for PrivateView<'_> {
        fn socket(&self) -> String {
            self.env.socket().to_owned()
        }

        fn swaymsg(&mut self, args: &[String]) -> Result<Run, String> {
            let launch = self.env.swaymsg(args)?;
            self.run(launch)
        }

        fn grim_ppm(&mut self) -> Result<Run, String> {
            let launch = self.env.grim();
            self.run(launch)
        }

        fn wtype(&mut self, args: &[String]) -> Result<(), String> {
            self.supervisor.wtype(args)
        }

        fn sleep_ms(&mut self, ms: u64) {
            std::thread::sleep(std::time::Duration::from_millis(ms));
        }
    }
}
