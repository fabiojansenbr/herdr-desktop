//! `mouse-scroll-links` phase of the single native flow of spec 007 (AC-007-02): parent-side
//! steps and pure evaluators. Prepared module; NOT executed by `e2e_fidelity_flow` until the root
//! links it (see `.local/orchestration/mouse-contract.md`). `plan.rs` keeps the phase Pending.
//!
//! Split of authority:
//! - The page (`src/features/fidelity/mouse-flow.ts`) only computes output points of literal
//!   pane-local cells ([`CLICK_CELL`], [`WHEEL_CELL`], [`SCROLL_CELL`], [`LINK_CELL`]) from the
//!   painted terminal geometry, records the pointer/wheel DOM events on the real terminal target
//!   and reads the link toolbar.
//! - The parent answers the `harness_await` steps ([`STEPS`]) with [`parent_step`]: pointer on
//!   the private compositor ([`Pointer`], `zwlr_virtual_pointer_v1` on the private Wayland socket
//!   of [`PointerEnv`]; Ctrl from the existing virtual keyboard), raw PTY bytes of an alt-screen
//!   mouse app ([`MouseApp`]), engine scroll state and viewport text ([`EngineView`]), fixtures
//!   shown in the confirmed pane ([`PaneFixture`]) and the private URI recorder ([`LinkRecorder`],
//!   installed by [`LinkHandler`]).
//! - [`checks`] compares the parent's own answers with literal expectations (SGR reports, numbered
//!   scrollback, nonce URI); nothing expected is derived from what the page relays. A missing
//!   step or field is an error, never a `true`.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// Pointer injection on the private display only (`zwlr_virtual_pointer_v1`); compiled here so the
/// include sites of this module do not need to declare it (same pattern as `remote::session`).
#[cfg(target_os = "linux")]
#[allow(dead_code)]
#[path = "pointer_virtual.rs"]
pub mod pointer_virtual;

use super::corpus::hex;
use super::display::{validate_runtime_dir, validate_wayland_socket, Launch, FORBIDDEN_INHERITED};
use super::paste_flow::{PaneFixture, ParentLedger, PtyCapture};

pub const PHASE: &str = "mouse-scroll-links";
/// Must equal the checks of `plan::FLOW` for [`PHASE`].
pub const CHECKS: [&str; 3] = [
    "alt_screen_mouse_reports_once",
    "scrollback_navigates",
    "link_open_effect_recorded_once",
];
/// `harness_await` steps of the phase, in order.
pub const STEPS: [&str; 3] = ["alt-screen-mouse", "scrollback-wheel", "link-ctrl-click"];
/// Private headless output (`.local/native-input/sway.conf`: HEADLESS-1 1280x720).
pub const OUTPUT: (f64, f64) = (1280.0, 720.0);

/// Pane-local cells `(row, col)`, 0-based.
pub const CLICK_CELL: (u16, u16) = (2, 5);
pub const WHEEL_CELL: (u16, u16) = (4, 9);
pub const SCROLL_CELL: (u16, u16) = (3, 4);
pub const LINK_CELL: (u16, u16) = (0, 3);

/// Alt screen + button-event mouse (1000, no motion reports) + SGR encoding (1006).
pub const MOUSE_APP_ENABLE: &str = "\\033[?1049h\\033[?1000h\\033[?1006h";
pub const MOUSE_APP_RESTORE: &str = "\\033[?1006l\\033[?1000l\\033[?1049l";
/// One left click at [`CLICK_CELL`] then one wheel-up notch at [`WHEEL_CELL`], SGR 1-based
/// `col;row`; the engine sends one report per wheel event (`server/pane_input.rs:apply_scroll`).
pub const MOUSE_REPORTS: &str = "\x1b[<0;6;3M\x1b[<0;6;3m\x1b[<64;10;5M";

/// Numbered scrollback fixture `hd007-sb-001..=hd007-sb-200`.
pub const SCROLL_LINES: u32 = 200;
pub const SCROLL_PREFIX: &str = "hd007-sb-";
pub const WHEEL_NOTCHES: u8 = 3;

/// Inert link: `.invalid` never resolves (RFC 6761); the nonce distinguishes runs.
pub const LINK_BASE: &str = "https://hd007-link.invalid/open?nonce=";
pub const LINK_TEXT_PREFIX: &str = "hd007-link-";
pub const RECORDER: &str = "hd007-link-recorder";

/// Native pointer on the private compositor (binding: [`PrivatePointer`]).
pub trait Pointer {
    /// IPC socket of the compositor that receives the pointer commands.
    fn socket(&self) -> String;
    fn click(&mut self, at: Point, ctrl: bool) -> Result<(), String>;
    fn wheel_up(&mut self, at: Point, notches: u8) -> Result<(), String>;
    fn seat_commands(&mut self) -> Vec<Value> {
        Vec::new()
    }
}

/// Ledger entry of one pointer action: how it was injected (`method`), its arguments and its
/// result. The real driver and the contract doubles record the same entry.
pub fn pointer_ledger_entry(
    action: &str,
    argv: Vec<String>,
    args: Value,
    outcome: &Result<(), String>,
) -> Value {
    let (exit, result) = match outcome {
        Ok(()) => (0, "ok".to_owned()),
        Err(e) => (-1, format!("error: {e}")),
    };
    json!({
        "method": "virtual_pointer",
        "action": action,
        "args": args,
        "argv": argv,
        "exit": exit,
        "status": result.clone(),
        "result": result,
        "stdout": "",
    })
}

/// Alt-screen mouse app with raw capture (binding: `herdr pane run <pane>` [`mouse_app_command`]
/// plus the file capture of `live.rs` `Capture`: `len/since(hex)/alive_pids/stop`).
pub trait MouseApp: PtyCapture {
    fn start_mouse_app(&mut self, pane: &str) -> Result<(), String>;
}

/// Engine view of one pane (binding: `herdr pane get <pane>` → `scroll`, and
/// `herdr pane read <pane> --source visible`; one read each, no polling).
pub trait EngineView {
    /// `{offset_from_bottom, max_offset_from_bottom, viewport_rows}` as the engine reports it.
    fn scroll(&mut self, pane: &str) -> Result<Value, String>;
    fn visible(&mut self, pane: &str) -> Result<String, String>;
}

/// URIs received by the private handler (binding: [`LinkHandler::parse_log`] of its log file).
pub trait LinkRecorder {
    fn log_path(&self) -> String;
    fn entries(&self) -> Result<Vec<String>, String>;
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    /// Integer output coordinates for `seat cursor set` (0 would mean "do not update").
    pub fn output(v: &Value) -> Result<Self, String> {
        let (x, y) = (v["x"].as_f64(), v["y"].as_f64());
        match (x, y) {
            (Some(x), Some(y)) if x >= 1.0 && y >= 1.0 && x < OUTPUT.0 && y < OUTPUT.1 => {
                Ok(Self { x, y })
            }
            _ => Err(format!("point {v} is not inside the private output")),
        }
    }
}

/// Shell command run in the pane: enable modes, capture raw reports, restore on stop.
pub fn mouse_app_command(capture: &Path) -> Result<String, String> {
    let path = capture.to_string_lossy();
    if !capture.is_absolute() || path.contains(['\'', '\n']) {
        return Err(format!("capture path {path} cannot be quoted"));
    }
    Ok(format!(
        "printf '{MOUSE_APP_ENABLE}'; stty raw -echo; cat > '{path}'; stty sane; printf '{MOUSE_APP_RESTORE}'"
    ))
}

pub fn scroll_fixture() -> String {
    (1..=SCROLL_LINES)
        .map(|n| format!("{SCROLL_PREFIX}{n:03}\n"))
        .collect()
}

pub fn link_fixture(uri: &str, nonce: &str) -> String {
    format!("\x1b]8;;{uri}\x1b\\{LINK_TEXT_PREFIX}{nonce}\x1b]8;;\x1b\\\n")
}

/// Literal values fixed by the parent before the window starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expectations {
    pub endpoint: String,
    pub sway_socket: String,
    pub nonce: String,
    pub link_uri: String,
    pub recorder_log: String,
}

impl Expectations {
    pub fn new(
        endpoint: &str,
        runtime_dir: &str,
        sway_socket: &str,
        handler: &LinkHandler,
        nonce: &str,
    ) -> Result<Self, String> {
        if endpoint.is_empty() {
            return Err("endpoint is required".into());
        }
        if !runtime_dir.starts_with("/tmp/hd7L-")
            || !sway_socket.starts_with(&format!("{runtime_dir}/sway-ipc."))
        {
            return Err(format!("{sway_socket} is not the private compositor"));
        }
        if !(6..=32).contains(&nonce.len())
            || !nonce
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        {
            return Err(format!("nonce {nonce:?} must be 6..=32 [a-z0-9]"));
        }
        Ok(Self {
            endpoint: endpoint.into(),
            sway_socket: sway_socket.into(),
            nonce: nonce.into(),
            link_uri: format!("{LINK_BASE}{nonce}"),
            recorder_log: handler.log.to_string_lossy().into_owned(),
        })
    }
}

const IDENTITY: [&str; 4] = ["pane_id", "generation", "boot_prefix", "endpoint"];

fn identity(v: &Value, what: &str) -> Result<Value, String> {
    let mut out = serde_json::Map::new();
    for key in IDENTITY {
        let s = v[key]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("{what} without {key}"))?;
        out.insert(key.into(), json!(s));
    }
    Ok(Value::Object(out))
}

fn points(detail: &Value, step: &str, names: &[&str]) -> Result<Vec<Point>, String> {
    names
        .iter()
        .map(|n| Point::output(&detail["points"][*n]).map_err(|e| format!("{step}: {n}: {e}")))
        .collect()
}

/// Answers one step for the pane the page confirmed. Refused before any pointer/app/fixture when
/// the step is unknown, the identity is incomplete, the host is another one or a point falls
/// outside the private output.
pub fn parent_step<W: Pointer + MouseApp + EngineView + PaneFixture + LinkRecorder>(
    step: &str,
    detail: &Value,
    e: &Expectations,
    world: &mut W,
) -> Result<Value, String> {
    if !STEPS.contains(&step) {
        return Err(format!("{step} is not a {PHASE} step"));
    }
    let id = identity(detail, step)?;
    if id["endpoint"] != e.endpoint.as_str() {
        return Err(format!(
            "{step}: page host {} is not {}",
            id["endpoint"], e.endpoint
        ));
    }
    let pane = id["pane_id"].as_str().unwrap_or_default().to_owned();
    let mut answer = json!({
        "step": step, "pane_id": pane, "identity": id, "pointer_socket": world.socket(),
    });
    match step {
        "alt-screen-mouse" => {
            let p = points(detail, step, &["click", "wheel"])?;
            world.start_mouse_app(&pane)?;
            let before = world.len();
            world.click(p[0], false)?;
            world.wheel_up(p[1], 1)?;
            answer["observed_hex"] = json!(world.since(before));
            answer["capture_pids_stopped"] = json!(world.stop());
        }
        "scrollback-wheel" => {
            let p = points(detail, step, &["scroll"])?;
            let text = scroll_fixture();
            world.show(&pane, &text)?;
            answer["shown_hex"] = json!(hex(text.as_bytes()));
            answer["scroll_before"] = world.scroll(&pane)?;
            answer["visible_before"] = json!(world.visible(&pane)?);
            world.wheel_up(p[0], WHEEL_NOTCHES)?;
            answer["scroll_after"] = world.scroll(&pane)?;
            answer["visible_after"] = json!(world.visible(&pane)?);
        }
        _ => {
            let p = points(detail, step, &["link"])?;
            answer["recorder_log"] = json!(world.log_path());
            answer["entries_before"] = json!(world.entries()?);
            let text = link_fixture(&e.link_uri, &e.nonce);
            world.show(&pane, &text)?;
            answer["shown_hex"] = json!(hex(text.as_bytes()));
            world.click(p[0], true)?;
            answer["entries_after"] = json!(world.entries()?);
        }
    }
    answer["seat_commands"] = json!(world.seat_commands());
    Ok(answer)
}

fn field<'a>(v: &'a Value, key: &str, what: &str) -> Result<&'a Value, String> {
    v.get(key)
        .filter(|f| !f.is_null())
        .ok_or_else(|| format!("{what} without {key}"))
}

fn step<'a>(ledger: &'a ParentLedger, name: &str) -> Result<&'a Value, String> {
    let answer = ledger
        .steps
        .get(name)
        .ok_or_else(|| format!("{name}: parent never observed this step"))?;
    if answer["step"] != name {
        return Err(format!("{name}: ledger entry of step {}", answer["step"]));
    }
    Ok(answer)
}

/// Page DOM events of one step: all trusted, and exactly the listed `(type, button, ctrl)`
/// sequence; wheel entries (`button` -1) must scroll up.
fn trusted_sequence(
    events: &Value,
    what: &str,
    want: &[(&str, i64, bool)],
) -> Result<bool, String> {
    let list = events
        .as_array()
        .ok_or_else(|| format!("page without {what}"))?;
    Ok(list.len() == want.len()
        && list.iter().zip(want).all(|(ev, (kind, button, ctrl))| {
            ev["trusted"] == true
                && ev["type"] == *kind
                && ev["ctrl"] == *ctrl
                && if *kind == "wheel" {
                    ev["deltaY"].as_f64().is_some_and(|d| d < 0.0)
                } else {
                    ev["button"] == *button
                }
        }))
}

fn scroll_state(v: &Value, what: &str) -> Result<(u64, u64, u64), String> {
    let n = |k: &str| {
        v[k].as_u64()
            .ok_or_else(|| format!("{what} without numeric {k}"))
    };
    Ok((
        n("offset_from_bottom")?,
        n("max_offset_from_bottom")?,
        n("viewport_rows")?,
    ))
}

/// Number of the first fixture line visible in the viewport text.
pub fn first_numbered(text: &str) -> Option<u32> {
    text.lines().find_map(|l| {
        let n = l.trim_end().strip_prefix(SCROLL_PREFIX)?;
        (n.len() == 3).then(|| n.parse().ok()).flatten()
    })
}

/// Named checks of [`PHASE`] from the page report and the parent's own answers.
pub fn checks(
    phase: &str,
    page: &Value,
    ledger: &ParentLedger,
    e: &Expectations,
) -> Result<Vec<(&'static str, bool)>, String> {
    if phase != PHASE {
        return Err(format!("{phase} is not {PHASE}"));
    }
    if !page["error"].is_null() {
        return Err(format!("{phase}: page error {}", page["error"]));
    }
    let before = identity(field(page, "identity_before", "page")?, "identity_before")?;
    let after = identity(field(page, "identity_after", "page")?, "identity_after")?;
    let identity_ok = before == after && before["endpoint"] == e.endpoint.as_str();
    let cell = |c: (u16, u16)| json!([c.0, c.1]);
    let cells_ok = field(page, "cells", "page")?
        == &json!({ "click": cell(CLICK_CELL), "wheel": cell(WHEEL_CELL),
                    "scroll": cell(SCROLL_CELL), "link": cell(LINK_CELL) });
    let parent_ok = |answer: &Value| -> Result<bool, String> {
        let id = identity(
            field(answer, "identity", "parent answer")?,
            "parent identity",
        )?;
        Ok(identity_ok
            && cells_ok
            && id == before
            && answer["pane_id"] == before["pane_id"]
            && field(answer, "pointer_socket", "parent answer")? == e.sway_socket.as_str())
    };

    let mouse = step(ledger, "alt-screen-mouse")?;
    let mouse_ok = parent_ok(mouse)?
        && field(mouse, "observed_hex", "alt-screen-mouse")?
            == hex(MOUSE_REPORTS.as_bytes()).as_str()
        && field(mouse, "capture_pids_stopped", "alt-screen-mouse")?
            .as_array()
            .is_some_and(|p| !p.is_empty())
        && trusted_sequence(
            field(page, "mouse_events", "page")?,
            "mouse_events",
            &[
                ("pointerdown", 0, false),
                ("pointerup", 0, false),
                ("wheel", -1, false),
            ],
        )?;

    let scroll = step(ledger, "scrollback-wheel")?;
    let (off0, max0, rows0) = scroll_state(
        field(scroll, "scroll_before", "scrollback-wheel")?,
        "scroll_before",
    )?;
    let (off1, max1, rows1) = scroll_state(
        field(scroll, "scroll_after", "scrollback-wheel")?,
        "scroll_after",
    )?;
    let text = |k: &str| -> Result<String, String> {
        Ok(field(scroll, k, "scrollback-wheel")?
            .as_str()
            .unwrap_or_default()
            .to_owned())
    };
    let (vis0, vis1) = (text("visible_before")?, text("visible_after")?);
    let last = format!("{SCROLL_PREFIX}{SCROLL_LINES:03}");
    let wheel_events = field(page, "scroll_events", "page")?
        .as_array()
        .ok_or("page without scroll_events")?;
    let scroll_ok = parent_ok(scroll)?
        && field(scroll, "shown_hex", "scrollback-wheel")?
            == hex(scroll_fixture().as_bytes()).as_str()
        && off0 == 0
        && rows0 > 0
        && rows0 == rows1
        && max0 == max1
        && max0 + rows0 > u64::from(SCROLL_LINES)
        && off1 > off0
        && off1 <= max1
        && vis0.lines().any(|l| l.trim_end() == last)
        && !vis1.lines().any(|l| l.trim_end() == last)
        && match (first_numbered(&vis0), first_numbered(&vis1)) {
            (Some(a), Some(b)) => u64::from(b) + (off1 - off0) == u64::from(a),
            _ => false,
        }
        && !wheel_events.is_empty()
        && wheel_events.iter().all(|ev| {
            ev["type"] == "wheel"
                && ev["trusted"] == true
                && ev["deltaY"].as_f64().is_some_and(|d| d < 0.0)
        });

    let link = step(ledger, "link-ctrl-click")?;
    let link_ok = parent_ok(link)?
        && field(link, "recorder_log", "link-ctrl-click")? == e.recorder_log.as_str()
        && field(link, "shown_hex", "link-ctrl-click")?
            == hex(link_fixture(&e.link_uri, &e.nonce).as_bytes()).as_str()
        && field(link, "entries_before", "link-ctrl-click")? == &json!([])
        && field(link, "entries_after", "link-ctrl-click")? == &json!([e.link_uri])
        && page["link_uri"] == e.link_uri.as_str()
        && trusted_sequence(
            field(page, "link_events", "page")?,
            "link_events",
            &[("pointerdown", 0, true), ("pointerup", 0, true)],
        )?;

    Ok(vec![
        ("alt_screen_mouse_reports_once", mouse_ok),
        ("scrollback_navigates", scroll_ok),
        ("link_open_effect_recorded_once", link_ok),
    ])
}

/// `swaymsg` of the private sway prefix against the private compositor only, with the
/// environment derived from the private `wtype` [`Launch`] (`PrivateDisplay::wtype`). The Wayland
/// socket of the virtual pointer comes from this same private environment, never the user's.
#[derive(Debug, Clone)]
pub struct PointerEnv {
    env: Vec<(String, String)>,
    swaymsg: PathBuf,
    socket: String,
    wayland: PathBuf,
}

impl PointerEnv {
    pub fn new(
        base: &Launch,
        sway_prefix: &Path,
        user_uid: u32,
        sway_pid: u32,
    ) -> Result<Self, String> {
        let runtime = base
            .var("XDG_RUNTIME_DIR")
            .ok_or("pointer env without XDG_RUNTIME_DIR")?
            .to_owned();
        validate_runtime_dir(Path::new(&runtime), user_uid)?;
        if !runtime.starts_with("/tmp/hd7L-") {
            return Err(format!("{runtime} is not a private run dir"));
        }
        let wayland = base
            .var("WAYLAND_DISPLAY")
            .filter(|d| !d.is_empty())
            .ok_or("pointer env without WAYLAND_DISPLAY (the private compositor socket)")?;
        let wayland = PathBuf::from(wayland);
        validate_wayland_socket(&wayland, Path::new(&runtime))?;
        let socket = format!("{runtime}/sway-ipc.{user_uid}.{sway_pid}.sock");
        let mut env: Vec<(String, String)> = base
            .env
            .iter()
            .filter(|(k, _)| !FORBIDDEN_INHERITED.contains(&k.as_str()) && k != "LD_LIBRARY_PATH")
            .cloned()
            .collect();
        env.push((
            "LD_LIBRARY_PATH".into(),
            sway_prefix.join("usr/lib").to_string_lossy().into_owned(),
        ));
        Ok(Self {
            env,
            swaymsg: sway_prefix.join("usr/bin/swaymsg"),
            socket,
            wayland,
        })
    }

    #[cfg(test)]
    pub fn dummy() -> Self {
        Self {
            env: Vec::new(),
            swaymsg: PathBuf::from("/bin/true"),
            socket: "/tmp/dummy.sock".into(),
            wayland: PathBuf::from("/tmp/dummy-wayland-1"),
        }
    }

    pub fn socket(&self) -> &str {
        &self.socket
    }

    /// Absolute socket of the private headless compositor; refused when absent (never a fallback
    /// to the user's display: it can only come from the private launch environment).
    pub fn wayland_socket(&self) -> Result<PathBuf, String> {
        if !self.wayland.exists() {
            return Err(format!(
                "private Wayland socket {} is absent",
                self.wayland.display()
            ));
        }
        Ok(self.wayland.clone())
    }

    fn swaymsg(&self, command: &[String]) -> Launch {
        let mut args = vec![
            "-s".to_owned(),
            self.socket.clone(),
            "seat".into(),
            "seat0".into(),
            "cursor".into(),
        ];
        args.extend(command.iter().cloned());
        Launch {
            program: self.swaymsg.clone(),
            args,
            env: self.env.clone(),
        }
    }

    pub fn cursor_set(&self, at: Point) -> Launch {
        self.swaymsg(&[
            "set".into(),
            format!("{}", at.x.round()),
            format!("{}", at.y.round()),
        ])
    }

    /// `swaymsg -t get_tree -r` on the private compositor: observed window/client geometry.
    pub fn get_tree(&self) -> Launch {
        Launch {
            program: self.swaymsg.clone(),
            args: ["-s", &self.socket, "-t", "get_tree", "-r"]
                .map(str::to_owned)
                .to_vec(),
            env: self.env.clone(),
        }
    }

    /// `button1` press/release; `button4` press is one wheel-up axis event.
    pub fn button(&self, press: bool, button: u8) -> Launch {
        let action = if press { "press" } else { "release" };
        self.swaymsg(&[action.into(), format!("button{button}")])
    }

    /// Holds Ctrl on the private seat (virtual keyboard) for `ms` while the click happens.
    pub fn ctrl_hold(&self, ms: u32) -> Launch {
        Launch {
            program: PathBuf::from("/usr/bin/wtype"),
            args: ["-M", "ctrl", "-s", &ms.to_string(), "-m", "ctrl"]
                .map(str::to_owned)
                .to_vec(),
            env: self
                .env
                .iter()
                .filter(|(k, _)| k != "LD_LIBRARY_PATH")
                .cloned()
                .collect(),
        }
    }
}

/// Private XDG URI handler: files under the window's private HOME and the extra window env.
/// The recorder only appends `pid<TAB>argc<TAB>uri` to its log and exits 0 (so neither xdg-open
/// nor `open` falls back to a browser).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkHandler {
    pub recorder: PathBuf,
    pub log: PathBuf,
    pub desktop: PathBuf,
    pub mimeapps: PathBuf,
}

impl LinkHandler {
    /// `home` is the private HOME of `PrivateDisplay` (XDG config/data below it).
    pub fn new(home: &Path, user_home: &Path, user_uid: u32) -> Result<Self, String> {
        let s = home.to_string_lossy();
        if !home.is_absolute()
            || home == Path::new("/")
            || home.starts_with(format!("/run/user/{user_uid}"))
            || user_home.starts_with(home)
            || s.contains(|c: char| c.is_whitespace() || "'\"\\%$`".contains(c))
        {
            return Err(format!("{s} is not a private HOME"));
        }
        Ok(Self {
            recorder: home.join("bin").join(RECORDER),
            log: home.join("state").join(format!("{RECORDER}.log")),
            desktop: home
                .join("data/applications")
                .join(format!("{RECORDER}.desktop")),
            mimeapps: home.join("config/mimeapps.list"),
        })
    }

    /// `(path, content, mode)` to write before the window starts.
    pub fn files(&self) -> Vec<(PathBuf, String, u32)> {
        let (rec, log) = (self.recorder.display(), self.log.display());
        vec![
            (
                self.recorder.clone(),
                format!("#!/bin/sh\n# hd007: records the URI only; never opens it.\nprintf '%s\\t%s\\t%s\\n' \"$$\" \"$#\" \"$1\" >> '{log}'\nexit 0\n"),
                0o700,
            ),
            (
                self.desktop.clone(),
                format!("[Desktop Entry]\nType=Application\nName=hd007 link recorder\nExec={rec} %u\nNoDisplay=true\nMimeType=x-scheme-handler/http;x-scheme-handler/https;\n"),
                0o600,
            ),
            (
                self.mimeapps.clone(),
                format!("[Default Applications]\nx-scheme-handler/http={RECORDER}.desktop\nx-scheme-handler/https={RECORDER}.desktop\n"),
                0o600,
            ),
        ]
    }

    /// Extra window variables (`Supervisor::start_window` `extra`): generic xdg-open and the
    /// recorder as `$BROWSER` fallback.
    pub fn window_env(&self) -> Vec<(String, String)> {
        vec![
            ("XDG_CURRENT_DESKTOP".into(), "X-Generic".into()),
            (
                "BROWSER".into(),
                self.recorder.to_string_lossy().into_owned(),
            ),
        ]
    }

    /// One entry per recorder run; a run with argc ≠ 1 is kept visibly different.
    pub fn parse_log(text: &str) -> Vec<String> {
        text.lines()
            .filter(|l| !l.is_empty())
            .map(|l| match l.splitn(3, '\t').collect::<Vec<_>>()[..] {
                [_, "1", uri] => uri.to_owned(),
                _ => format!("malformed:{l}"),
            })
            .collect()
    }
}

#[cfg(target_os = "linux")]
pub use private::{PrivateLinkRecorder, PrivatePointer};

#[cfg(target_os = "linux")]
mod private {
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Child, Stdio};
    use std::time::Duration;

    use super::super::paste_flow::cleanup_plan;
    use super::super::supervisor::{starttime, Tracked};
    use super::pointer_virtual::{VirtualPointerClient, BTN_LEFT};
    use super::{
        pointer_ledger_entry, LinkHandler, LinkRecorder, Point, Pointer, PointerEnv, Value, OUTPUT,
    };

    const SETTLE: Duration = Duration::from_millis(400);
    /// One vertical wheel notch up: `wl_pointer` axis value and discrete steps (wlr convention).
    const AXIS_NOTCH: f64 = -15.0;
    const AXIS_NOTCH_DISCRETE: i32 = -1;

    /// Integer point of the private output; refused (with no action at all) when the requested
    /// point is outside it, before the device is touched.
    fn output_integer(at: Point) -> Result<(u32, u32), String> {
        let out = Point::output(&serde_json::json!({ "x": at.x, "y": at.y }))?;
        let (x, y) = (out.x.round(), out.y.round());
        if !(x >= 1.0 && y >= 1.0 && x < OUTPUT.0 && y < OUTPUT.1) {
            return Err(format!("point ({x}, {y}) is not inside the private output"));
        }
        Ok((x as u32, y as u32))
    }

    /// Injects pointer motion/buttons/axis with `zwlr_virtual_pointer_v1` on the private display;
    /// the Ctrl-holding wtype is tracked by PID+starttime and cleaned up after the click, on
    /// [`PrivatePointer::cleanup`] and on drop. Every injected action lands in `seat_commands`
    /// with its `method`, arguments and result.
    pub struct PrivatePointer {
        env: PointerEnv,
        pointer: Option<VirtualPointerClient>,
        held: Vec<(Tracked, Child)>,
        pub log: Vec<String>,
        pub seat_commands: Vec<serde_json::Value>,
    }

    impl PrivatePointer {
        pub fn new(env: PointerEnv) -> Result<Self, String> {
            let socket = env.wayland_socket()?;
            let pointer = VirtualPointerClient::connect(&socket)
                .map_err(|e| format!("virtual pointer on {}: {e}", socket.display()))?;
            Ok(Self {
                env,
                pointer: Some(pointer),
                held: Vec::new(),
                log: vec![format!("virtual pointer on {}", socket.display())],
                seat_commands: Vec::new(),
            })
        }

        #[cfg(test)]
        pub fn with_commands(commands: Vec<Value>) -> Self {
            Self {
                env: PointerEnv::dummy(),
                pointer: None,
                held: Vec::new(),
                log: Vec::new(),
                seat_commands: commands,
            }
        }

        fn device(&mut self) -> Result<&mut VirtualPointerClient, String> {
            self.pointer
                .as_mut()
                .ok_or_else(|| "virtual pointer not initialized".to_owned())
        }

        fn record(
            &mut self,
            action: &str,
            argv: Vec<String>,
            args: Value,
            outcome: &Result<(), String>,
        ) {
            self.seat_commands
                .push(pointer_ledger_entry(action, argv, args, outcome));
        }

        /// Absolute motion to one output point, with the private output as the coordinate space.
        fn motion_absolute(&mut self, x: u32, y: u32) -> Result<(), String> {
            let (x_extent, y_extent) = (OUTPUT.0 as u32, OUTPUT.1 as u32);
            let args =
                serde_json::json!({ "x": x, "y": y, "x_extent": x_extent, "y_extent": y_extent });
            let argv = vec![
                "motion_absolute".to_owned(),
                x.to_string(),
                y.to_string(),
                x_extent.to_string(),
                y_extent.to_string(),
            ];
            let outcome = self
                .device()
                .and_then(|p| p.motion_absolute(x, y, x_extent, y_extent));
            self.record("motion_absolute", argv, args, &outcome);
            outcome.map_err(|e| format!("virtual pointer motion_absolute({x}, {y}): {e}"))
        }

        fn button(&mut self, state: &str) -> Result<(), String> {
            let args = serde_json::json!({ "button": BTN_LEFT, "state": state });
            let argv = vec!["button".to_owned(), BTN_LEFT.to_string(), state.to_owned()];
            let outcome = self.device().and_then(|p| {
                if state == "pressed" {
                    p.button_press(BTN_LEFT)
                } else {
                    p.button_release(BTN_LEFT)
                }
            });
            self.record("button", argv, args, &outcome);
            outcome.map_err(|e| format!("virtual pointer button({BTN_LEFT}, {state}): {e}"))
        }

        fn axis_vertical(&mut self) -> Result<(), String> {
            let args = serde_json::json!({
                "axis": "vertical", "value": AXIS_NOTCH, "discrete": AXIS_NOTCH_DISCRETE,
            });
            let argv = vec![
                "axis".to_owned(),
                "vertical".to_owned(),
                format!("{AXIS_NOTCH}"),
                AXIS_NOTCH_DISCRETE.to_string(),
            ];
            let outcome = self
                .device()
                .and_then(|p| p.axis_vertical(AXIS_NOTCH, Some(AXIS_NOTCH_DISCRETE)));
            self.record("axis", argv, args, &outcome);
            outcome.map_err(|e| format!("virtual pointer axis vertical: {e}"))
        }

        /// Holds Ctrl on the private seat (the existing virtual keyboard) while the click happens.
        fn hold_ctrl(&mut self) -> Result<(), String> {
            let child = self
                .env
                .ctrl_hold(1500)
                .command()
                .stdin(Stdio::null())
                .spawn()
                .map_err(|e| format!("wtype ctrl: {e}"))?;
            let pid = child.id();
            let st = starttime(pid).ok_or("wtype ctrl exited immediately")?;
            self.log
                .push(format!("spawn wtype-ctrl pid={pid} starttime={st}"));
            self.held.push((
                Tracked {
                    name: "wtype-ctrl".into(),
                    pid,
                    starttime: st,
                },
                child,
            ));
            std::thread::sleep(SETTLE);
            Ok(())
        }

        /// Raw `get_tree` of the private compositor (read-only observation of window geometry).
        pub fn tree(&mut self) -> Result<serde_json::Value, String> {
            let out = self
                .env
                .get_tree()
                .command()
                .stdin(Stdio::null())
                .output()
                .map_err(|e| format!("swaymsg get_tree: {e}"))?;
            self.log.push(format!("swaymsg get_tree -> {}", out.status));
            if !out.status.success() {
                return Err(format!(
                    "swaymsg get_tree: {} {}",
                    out.status,
                    String::from_utf8_lossy(&out.stderr)
                ));
            }
            serde_json::from_slice(&out.stdout).map_err(|e| format!("get_tree json: {e}"))
        }

        pub fn cleanup(&mut self) -> Vec<String> {
            let tracked: Vec<(u32, u64)> = self
                .held
                .iter()
                .map(|(t, _)| (t.pid, t.starttime))
                .collect();
            let live = cleanup_plan(&tracked, starttime);
            for (t, mut child) in self.held.drain(..) {
                if live.contains(&t.pid) {
                    let _ = child.kill();
                }
                let status = child.wait().map(|s| s.to_string()).unwrap_or_default();
                self.log
                    .push(format!("stop {} pid={} -> {status}", t.name, t.pid));
            }
            self.log.clone()
        }
    }

    impl Drop for PrivatePointer {
        fn drop(&mut self) {
            self.cleanup();
        }
    }

    impl Pointer for PrivatePointer {
        fn socket(&self) -> String {
            self.env.socket().to_owned()
        }

        fn click(&mut self, at: Point, ctrl: bool) -> Result<(), String> {
            let (x, y) = output_integer(at)?;
            if ctrl {
                self.hold_ctrl()?;
            }
            let result = self
                .motion_absolute(x, y)
                .and_then(|_| {
                    std::thread::sleep(SETTLE);
                    self.button("pressed")
                })
                .and_then(|_| {
                    std::thread::sleep(SETTLE);
                    self.button("released")
                });
            std::thread::sleep(SETTLE);
            self.cleanup();
            result
        }

        fn wheel_up(&mut self, at: Point, notches: u8) -> Result<(), String> {
            let (x, y) = output_integer(at)?;
            self.motion_absolute(x, y)?;
            std::thread::sleep(SETTLE);
            for _ in 0..notches {
                self.axis_vertical()?;
                std::thread::sleep(Duration::from_millis(150));
            }
            std::thread::sleep(SETTLE);
            Ok(())
        }

        fn seat_commands(&mut self) -> Vec<Value> {
            std::mem::take(&mut self.seat_commands)
        }
    }

    /// Installs the private handler files and reads its log (no process of its own).
    pub struct PrivateLinkRecorder {
        pub handler: LinkHandler,
    }

    impl PrivateLinkRecorder {
        pub fn from_handler(handler: LinkHandler) -> Self {
            Self { handler }
        }

        pub fn install(handler: LinkHandler) -> Result<Self, String> {
            for (path, content, mode) in handler.files() {
                let dir = path.parent().ok_or("handler file without parent")?;
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                std::fs::write(&path, content).map_err(|e| format!("{}: {e}", path.display()))?;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
                    .map_err(|e| format!("{}: {e}", path.display()))?;
            }
            Ok(Self { handler })
        }
    }

    impl LinkRecorder for PrivateLinkRecorder {
        fn log_path(&self) -> String {
            self.handler.log.to_string_lossy().into_owned()
        }

        fn entries(&self) -> Result<Vec<String>, String> {
            std::thread::sleep(Duration::from_millis(1500));
            match std::fs::read_to_string(&self.handler.log) {
                Ok(text) => Ok(LinkHandler::parse_log(&text)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
                Err(e) => Err(format!("{}: {e}", self.handler.log.display())),
            }
        }
    }
}
