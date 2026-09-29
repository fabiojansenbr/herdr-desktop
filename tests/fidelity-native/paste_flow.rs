//! `paste-selection` phase of the single native flow of spec 007 (AC-007-02): parent-side steps
//! and pure evaluators. Prepared module; NOT executed by `e2e_fidelity_flow` until the root links
//! it (see `.local/orchestration/paste-contract.md`). `plan.rs` keeps the phase Pending.
//!
//! Split of authority:
//! - The page (`src/features/fidelity/paste-flow.ts`) acts only through the real terminal target
//!   (`textarea.ime-target`) and its toolbar, and reports raw DOM observations: the confirmed
//!   identity before/after, trusted `paste`/copy-chord events, the dragged cells and the status.
//! - The parent answers the `harness_await` steps ([`STEPS`]) with [`parent_step`]: native keys on
//!   the private display ([`Keys`]), raw PTY bytes of the capture `cat` ([`PtyCapture`]), the
//!   private clipboard ([`Clipboard`], wl-copy/wl-paste under [`ClipboardEnv`] only) and the
//!   selection fixture shown in the confirmed pane ([`PaneFixture`]). It keeps its own answers
//!   ([`ParentLedger`]).
//! - [`checks`] compares those answers with literal expectations ([`Expectations`], the existing
//!   corpus item `paste-multiline` and [`SELECTION_TEXT`]); nothing is derived from what the page
//!   relays. A missing step or field is an error, never a `true`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::corpus::{hex, Corpus};
use super::display::{validate_runtime_dir, Launch};

pub const PHASE: &str = "paste-selection";
/// Must equal the checks of `plan::FLOW` for [`PHASE`].
pub const CHECKS: [&str; 2] = [
    "multiline_paste_once",
    "selection_copied_to_private_clipboard",
];
/// `harness_await` steps of the phase, in order.
pub const STEPS: [&str; 3] = ["paste-native", "selection-fixture", "selection-copy"];
/// Existing corpus item (Ctrl+Shift+V, multiline + Unicode text).
pub const PASTE_ITEM: &str = "paste-multiline";
/// Printed at the top of the confirmed pane; rows 0..=1 are selected, row 2 must stay out.
pub const SELECTION_LINES: [&str; 3] = [
    "hd007 seleção 你好世界",
    "segunda 表格 ação 2",
    "hd007 fora da seleção",
];
/// Literal clipboard text expected after copying rows 0..=1 (lines joined by LF, no trailing LF).
pub const SELECTION_TEXT: &str = "hd007 seleção 你好世界\nsegunda 表格 ação 2";
/// Last cell of row 1: "segunda " 8 + 表格 4 (double width) + " ação 2" 7 = 19 cells.
pub const SELECTION_END_COL: u16 = 18;
/// Native copy chord of the terminal (`isCopyChord`: Ctrl+Shift+C).
pub const COPY_KEYS: [&str; 10] = [
    "-M", "ctrl", "-M", "shift", "-k", "c", "-m", "shift", "-m", "ctrl",
];
pub const SENTINEL_PREFIX: &str = "hd007-clipboard-sentinel-";
/// Status of the terminal toolbar (`TerminalView.svelte`).
pub const STATUS_SELECTED: &str = "Seleção: 2 linhas";
pub const STATUS_COPIED: &str = "Seleção: 2 linhas Seleção copiada";
const MIME: &str = "text/plain;charset=utf-8";

/// Native keys on the private display (binding: `Supervisor::wtype` + settle).
pub trait Keys {
    fn press(&mut self, keys: &[String]) -> Result<(), String>;
}

/// Raw capture `cat` already running in the confirmed pane (binding: `live.rs` `Capture`).
pub trait PtyCapture {
    fn len(&self) -> usize;
    fn since(&self, offset: usize) -> String;
    fn alive_pids(&self) -> Vec<u32>;
    fn stop(&mut self) -> Vec<u32>;
}

/// Private clipboard (binding: [`PrivateClipboard`]).
pub trait Clipboard {
    /// `(XDG_RUNTIME_DIR, WAYLAND_DISPLAY)` the clipboard tools run under.
    fn display(&self) -> (String, String);
    fn set(&mut self, text: &str) -> Result<(), String>;
    fn read(&mut self) -> Result<Vec<u8>, String>;
}

/// Shows text at the top of a pane (binding: `herdr pane run <pane> "clear; cat <file>"`).
pub trait PaneFixture {
    fn show(&mut self, pane: &str, text: &str) -> Result<(), String>;
}

/// Literal values fixed by the parent before the window starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expectations {
    pub paste_text: String,
    pub paste_keys: Vec<String>,
    pub runtime_dir: String,
    pub wayland_display: String,
    pub endpoint: String,
    pub sentinel: String,
}

impl Expectations {
    pub fn new(
        corpus: &Corpus,
        runtime_dir: &str,
        wayland_display: &str,
        endpoint: &str,
        sentinel: &str,
    ) -> Result<Self, String> {
        let item = corpus
            .items
            .iter()
            .find(|i| i.id == PASTE_ITEM)
            .ok_or("corpus item paste-multiline missing")?;
        let paste_text = item.clipboard.clone().unwrap_or_default();
        if item.driver != "private-clipboard"
            || paste_text.lines().count() < 2
            || paste_text.is_ascii()
            || item.keys.is_empty()
        {
            return Err(format!(
                "corpus item {PASTE_ITEM} is not a multiline Unicode paste"
            ));
        }
        if !runtime_dir.starts_with("/tmp/hd7L-")
            || !wayland_display.starts_with(&format!("{runtime_dir}/"))
        {
            return Err(format!(
                "{runtime_dir}/{wayland_display} is not the private display"
            ));
        }
        if endpoint.is_empty()
            || !sentinel.starts_with(SENTINEL_PREFIX)
            || sentinel.len() == SENTINEL_PREFIX.len()
        {
            return Err("endpoint and a sentinel nonce are required".into());
        }
        Ok(Self {
            paste_text,
            paste_keys: item.keys.clone(),
            runtime_dir: runtime_dir.into(),
            wayland_display: wayland_display.into(),
            endpoint: endpoint.into(),
            sentinel: sentinel.into(),
        })
    }

    fn fixture_text() -> String {
        format!("{}\n", SELECTION_LINES.join("\n"))
    }
}

/// Answers the parent gave, kept by the parent (the page's relayed copies are ignored).
#[derive(Debug, Clone, Default)]
pub struct ParentLedger {
    pub steps: BTreeMap<String, Value>,
}

impl ParentLedger {
    /// Records one answer; a step answered twice is an error (no replayed observation).
    pub fn record(&mut self, step: &str, answer: Value) -> Result<(), String> {
        if self.steps.contains_key(step) {
            return Err(format!("{step}: answered twice"));
        }
        self.steps.insert(step.to_owned(), answer);
        Ok(())
    }

    fn step(&self, step: &str) -> Result<&Value, String> {
        let answer = self
            .steps
            .get(step)
            .ok_or_else(|| format!("{step}: parent never observed this step"))?;
        if answer["step"] != step {
            return Err(format!("{step}: ledger entry of step {}", answer["step"]));
        }
        Ok(answer)
    }
}

const IDENTITY: [&str; 4] = ["pane_id", "generation", "boot_prefix", "endpoint"];

/// The identity the page confirmed (status diagnostics), all four fields non-empty.
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

fn keys(list: &[impl AsRef<str>]) -> Vec<String> {
    list.iter().map(|k| k.as_ref().to_owned()).collect()
}

/// Answers one step for the pane the page confirmed. Refused before any key/tool when the step
/// is unknown, the identity is incomplete or it names another host than the expected one.
pub fn parent_step<W: Keys + PtyCapture + Clipboard + PaneFixture>(
    step: &str,
    detail: &Value,
    e: &Expectations,
    world: &mut W,
) -> Result<Value, String> {
    if !STEPS.contains(&step) {
        return Err(format!("{step} is not a paste-selection step"));
    }
    let id = identity(detail, step)?;
    if id["endpoint"] != e.endpoint.as_str() {
        return Err(format!(
            "{step}: page host {} is not {}",
            id["endpoint"], e.endpoint
        ));
    }
    let pane = id["pane_id"].as_str().unwrap_or_default().to_owned();
    let (runtime, wayland) = world.display();
    let mut answer = json!({
        "step": step, "pane_id": pane, "identity": id,
        "clipboard_display": { "runtime_dir": runtime, "wayland_display": wayland },
    });
    match step {
        "paste-native" => {
            world.set(&e.paste_text)?;
            answer["clipboard_set_hex"] = json!(hex(e.paste_text.as_bytes()));
            answer["clipboard_readback_hex"] = json!(world.read().map(|b| hex(&b)).ok());
            let before = world.len();
            world.press(&e.paste_keys)?;
            answer["keys"] = json!(e.paste_keys);
            answer["observed_hex"] = json!(world.since(before));
            answer["capture_alive_after"] = json!(!world.alive_pids().is_empty());
            answer["capture_pids_stopped"] = json!(world.stop());
        }
        "selection-fixture" => {
            let text = Expectations::fixture_text();
            world.show(&pane, &text)?;
            answer["shown_hex"] = json!(hex(text.as_bytes()));
        }
        _ => {
            world.set(&e.sentinel)?;
            answer["sentinel_readback_hex"] = json!(world.read().map(|b| hex(&b)).ok());
            world.press(&keys(&COPY_KEYS))?;
            answer["keys"] = json!(COPY_KEYS);
            match world.read() {
                Ok(bytes) => answer["clipboard_after_hex"] = json!(hex(&bytes)),
                Err(error) => {
                    answer["clipboard_after_hex"] = Value::Null;
                    answer["clipboard_after_error"] = json!(error);
                }
            }
        }
    }
    Ok(answer)
}

fn field<'a>(v: &'a Value, key: &str, what: &str) -> Result<&'a Value, String> {
    v.get(key)
        .filter(|f| !f.is_null())
        .ok_or_else(|| format!("{what} without {key}"))
}

fn one_trusted(
    events: &Value,
    what: &str,
    matches: impl Fn(&Value) -> bool,
) -> Result<bool, String> {
    let list = events
        .as_array()
        .ok_or_else(|| format!("page without {what}"))?;
    Ok(list.len() == 1 && list.iter().all(|ev| ev["trusted"] == true && matches(ev)))
}

fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
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
    let display = json!({ "runtime_dir": e.runtime_dir, "wayland_display": e.wayland_display });
    let parent_ok = |answer: &Value| -> Result<bool, String> {
        let id = identity(
            field(answer, "identity", "parent answer")?,
            "parent identity",
        )?;
        Ok(id == before
            && answer["pane_id"] == before["pane_id"]
            && field(answer, "clipboard_display", "parent answer")? == &display)
    };

    let paste = ledger.step("paste-native")?;
    let paste_hex = hex(e.paste_text.as_bytes());
    let paste_ok = identity_ok
        && parent_ok(paste)?
        && field(paste, "clipboard_set_hex", "paste-native")? == paste_hex.as_str()
        && paste["clipboard_readback_hex"] == paste_hex.as_str()
        && field(paste, "observed_hex", "paste-native")? == paste_hex.as_str()
        // The raw capture `cat` was still ours after the paste (a shell would have consumed it).
        && field(paste, "capture_pids_stopped", "paste-native")?
            .as_array()
            .is_some_and(|p| !p.is_empty())
        && field(page, "paste_events", "page")?
            .as_array()
            .is_some_and(|evs| evs.is_empty())
        && one_trusted(
            field(page, "paste_keydowns", "page")?,
            "paste_keydowns",
            |ev| {
                ev["type"] == "keydown"
                    && ev["ctrl"] == true
                    && ev["shift"] == true
                    && ev["key"]
                        .as_str()
                        .is_some_and(|k| k.eq_ignore_ascii_case("v"))
            },
        )?;

    let fixture = ledger.step("selection-fixture")?;
    let copy = ledger.step("selection-copy")?;
    let status_before = field(page, "status_before_copy", "page")?
        .as_str()
        .unwrap_or_default();
    let status_after = field(page, "status_after_copy", "page")?
        .as_str()
        .unwrap_or_default();
    let drag = field(page, "drag", "page")?;
    let selection_ok = identity_ok
        && parent_ok(fixture)?
        && parent_ok(copy)?
        && field(fixture, "shown_hex", "selection-fixture")?
            == hex(Expectations::fixture_text().as_bytes()).as_str()
        && e.sentinel != SELECTION_TEXT
        && copy["sentinel_readback_hex"] == hex(e.sentinel.as_bytes()).as_str()
        && copy
            .get("clipboard_after_hex")
            .ok_or("selection-copy without clipboard_after_hex")?
            == hex(SELECTION_TEXT.as_bytes()).as_str()
        && *drag == json!({ "anchor": [0, 0], "cursor": [1, SELECTION_END_COL] })
        && squash(status_before) == STATUS_SELECTED
        && squash(status_after) == STATUS_COPIED
        && one_trusted(
            field(page, "copy_keydowns", "page")?,
            "copy_keydowns",
            |ev| {
                ev["type"] == "keydown"
                    && ev["key"]
                        .as_str()
                        .is_some_and(|k| k.eq_ignore_ascii_case("c"))
                    && ev["ctrl"] == true
                    && ev["shift"] == true
            },
        )?;

    Ok(vec![
        ("multiline_paste_once", paste_ok),
        ("selection_copied_to_private_clipboard", selection_ok),
    ])
}

/// Clipboard tools under the private display only: environment derived from the private
/// `wtype` [`Launch`] (`PrivateDisplay::wtype`), plus the private session bus.
#[derive(Debug, Clone)]
pub struct ClipboardEnv {
    env: Vec<(String, String)>,
    runtime_dir: String,
    wayland_display: String,
}

impl ClipboardEnv {
    pub fn new(base: &Launch, bus: &str, user_uid: u32) -> Result<Self, String> {
        let runtime = base
            .var("XDG_RUNTIME_DIR")
            .ok_or("clipboard env without XDG_RUNTIME_DIR")?
            .to_owned();
        validate_runtime_dir(Path::new(&runtime), user_uid)?;
        if !runtime.starts_with("/tmp/hd7L-") {
            return Err(format!("{runtime} is not a private run dir"));
        }
        let wayland = base
            .var("WAYLAND_DISPLAY")
            .ok_or("clipboard env without WAYLAND_DISPLAY")?
            .to_owned();
        if !wayland.starts_with(&format!("{runtime}/")) {
            return Err(format!("WAYLAND_DISPLAY {wayland} outside {runtime}"));
        }
        // dbus-daemon prints `unix:path=<runtime>/bus,guid=<hex>` (observed in native r1).
        let private_bus = format!("unix:path={runtime}/bus");
        let guid_ok = |g: &str| !g.is_empty() && g.bytes().all(|b| b.is_ascii_hexdigit());
        let accepted = bus == private_bus
            || bus
                .strip_prefix(&format!("{private_bus},guid="))
                .is_some_and(guid_ok);
        if !accepted {
            return Err(format!("bus {bus} is not the private bus"));
        }
        let mut env: Vec<(String, String)> = base
            .env
            .iter()
            .filter(|(k, _)| !matches!(k.as_str(), "DISPLAY" | "DBUS_SESSION_BUS_ADDRESS"))
            .cloned()
            .collect();
        env.push(("DBUS_SESSION_BUS_ADDRESS".into(), bus.into()));
        Ok(Self {
            env,
            runtime_dir: runtime,
            wayland_display: wayland,
        })
    }

    /// `wl-copy --foreground`: the owner stays our tracked child until replaced or cleaned up.
    pub fn copy_foreground(&self) -> Launch {
        self.launch("/usr/bin/wl-copy", &["--foreground", "--type", MIME])
    }

    pub fn paste(&self) -> Launch {
        self.launch("/usr/bin/wl-paste", &["--no-newline", "--type", MIME])
    }

    fn launch(&self, program: &str, args: &[&str]) -> Launch {
        Launch {
            program: PathBuf::from(program),
            args: keys(args),
            env: self.env.clone(),
        }
    }
}

/// PIDs that may be signalled: tracked `(pid, starttime)` still naming the same process.
pub fn cleanup_plan(tracked: &[(u32, u64)], now: impl Fn(u32) -> Option<u64>) -> Vec<u32> {
    tracked
        .iter()
        .filter(|(pid, st)| now(*pid) == Some(*st))
        .map(|(pid, _)| *pid)
        .collect()
}

#[cfg(target_os = "linux")]
pub use private::PrivateClipboard;

#[cfg(target_os = "linux")]
mod private {
    use std::io::Write;
    use std::process::{Child, Stdio};
    use std::time::{Duration, Instant};

    use super::super::supervisor::{starttime, Tracked};
    use super::{cleanup_plan, Clipboard, ClipboardEnv};

    /// Runs wl-copy/wl-paste only when a step calls it; every child is tracked by PID+starttime
    /// and cleaned up on replacement, on [`PrivateClipboard::cleanup`] and on drop (partial
    /// failure included).
    pub struct PrivateClipboard {
        env: ClipboardEnv,
        owners: Vec<(Tracked, Child)>,
        pub log: Vec<String>,
    }

    impl PrivateClipboard {
        pub fn new(env: ClipboardEnv) -> Self {
            Self {
                env,
                owners: Vec::new(),
                log: Vec::new(),
            }
        }

        fn track(&mut self, name: &str, child: Child) -> Result<Tracked, String> {
            let pid = child.id();
            let st = starttime(pid).ok_or_else(|| format!("{name} exited immediately"))?;
            let tracked = Tracked {
                name: name.into(),
                pid,
                starttime: st,
            };
            self.log
                .push(format!("spawn {name} pid={pid} starttime={st}"));
            self.owners.push((tracked.clone(), child));
            Ok(tracked)
        }

        pub fn cleanup(&mut self) -> Vec<String> {
            let tracked: Vec<(u32, u64)> = self
                .owners
                .iter()
                .map(|(t, _)| (t.pid, t.starttime))
                .collect();
            let live = cleanup_plan(&tracked, starttime);
            for (t, mut child) in self.owners.drain(..) {
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

    impl Drop for PrivateClipboard {
        fn drop(&mut self) {
            self.cleanup();
        }
    }

    impl Clipboard for PrivateClipboard {
        fn display(&self) -> (String, String) {
            (
                self.env.runtime_dir.clone(),
                self.env.wayland_display.clone(),
            )
        }

        fn set(&mut self, text: &str) -> Result<(), String> {
            self.cleanup();
            let mut child = self
                .env
                .copy_foreground()
                .command()
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| format!("wl-copy: {e}"))?;
            let stdin = child.stdin.take();
            self.track("wl-copy", child)?;
            let mut stdin = stdin.ok_or("wl-copy without stdin")?;
            stdin
                .write_all(text.as_bytes())
                .map_err(|e| format!("wl-copy stdin: {e}"))?;
            drop(stdin);
            std::thread::sleep(Duration::from_millis(300));
            Ok(())
        }

        fn read(&mut self) -> Result<Vec<u8>, String> {
            let child = self
                .env
                .paste()
                .command()
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| format!("wl-paste: {e}"))?;
            let pid = child.id();
            let st = starttime(pid);
            let deadline = Instant::now() + Duration::from_secs(5);
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(child.wait_with_output());
            });
            loop {
                if let Ok(out) = rx.try_recv() {
                    let out = out.map_err(|e| format!("wl-paste: {e}"))?;
                    self.log
                        .push(format!("wl-paste pid={pid} -> {}", out.status));
                    return if out.status.success() {
                        Ok(out.stdout)
                    } else {
                        Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
                    };
                }
                if Instant::now() > deadline {
                    if st.is_some() && starttime(pid) == st {
                        let _ = std::process::Command::new("kill")
                            .arg(pid.to_string())
                            .status();
                    }
                    return Err(format!("wl-paste pid={pid} timed out"));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}
