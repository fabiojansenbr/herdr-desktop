//! Parent side of the private display run: one composed window running the listed phases, the
//! parent answering `harness_await` steps (native keys/IME on the private compositor, PTY bytes
//! measured from a capture fixture the parent runs in the disposable pane).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};

use super::corpus::{self, Corpus, IME_ITEMS, NATIVE_KEY_ITEMS};
use super::display::{PrivateDisplay, Resources};
use super::mouse_flow::{self, Point};
use super::paste_flow;
use super::ssh_flow::{self, HostObserver, ParentLedger};
use super::supervisor::Supervisor;
use super::view_flow::{self, ViewWorld};
use super::visual_agents;
use super::visual_center;
use super::visual_files;
use super::visual_frame;
use super::visual_projects;

pub struct FlowEnv {
    /// Session of the private Local host (the fixture's, never a default session).
    pub session: String,
    /// Complete engine environment of that Local host (its namespace); nothing is inherited.
    pub herdr_env: Vec<(String, String)>,
    /// Boot id of that Local host: the pane the page confirmed must belong to this boot (the SSH
    /// hosts share the pane id `w1:p1`).
    pub local_boot: String,
    pub pane: String,
    pub session_dir: PathBuf,
    pub herdr_bin: PathBuf,
    pub herdr_config_dir: PathBuf,
    pub resources_root: PathBuf,
    pub evidence: PathBuf,
    pub corpus: Corpus,
    pub user: String,
    pub uid: u32,
}

#[cfg(test)]
impl FlowEnv {
    pub fn dummy() -> Self {
        Self {
            session: "hd007-dummy".into(),
            herdr_env: Vec::new(),
            local_boot: "boot-dummy".into(),
            pane: "w1:p1".into(),
            session_dir: PathBuf::from("/tmp"),
            herdr_bin: PathBuf::from("/bin/true"),
            herdr_config_dir: PathBuf::from("/tmp"),
            resources_root: PathBuf::from("/tmp"),
            evidence: PathBuf::from("/tmp"),
            corpus: Corpus {
                version: 1,
                cjk_standalone_final_hex: String::new(),
                items: Vec::new(),
            },
            user: "test".into(),
            uid: 1000,
        }
    }
}

#[derive(Debug, Default)]
pub struct LiveRun {
    pub reports: BTreeMap<String, Value>,
    pub final_report: Option<Value>,
    pub window_exit: Option<String>,
    pub isolation: String,
    pub supervisor_log: Vec<String>,
    pub errors: Vec<String>,
    /// Parent answers to the SSH steps (engine snapshots); the only engine source of the evaluator.
    pub ledger: ParentLedger,
    /// Parent answers to the paste-selection and mouse-scroll-links steps (distinct step names).
    pub pointer_ledger: paste_flow::ParentLedger,
    /// Literals fixed by the parent for those phases; `None` = setup failed (never a pass).
    pub paste_expectations: Option<paste_flow::Expectations>,
    pub mouse_expectations: Option<mouse_flow::Expectations>,
    /// resize-dpi / a11y-navigation parent answers (`view_flow::parent_step` records them once).
    pub view_ledger: view_flow::Ledger,
    pub view_expectations: Option<view_flow::ViewExpectations>,
    /// visual-frame parent answers (spec 010): output commands, engine data and PTY bytes.
    pub visual_ledger: visual_frame::Ledger,
    /// visual-agents parent answers (spec 014): engine reads, the reported state change and the
    /// confirmed focus of the agent pane.
    pub agents_ledger: visual_agents::Ledger,
    /// visual-projects parent answers (spec 011): screenshots, the confirmed pane and the engine
    /// agent list after the phase starts its agent.
    pub visual_projects_ledger: visual_projects::Ledger,
    /// visual-files parent answers (spec 015): dictated buffer, engine agent, PTY bytes, remote snapshots.
    pub visual_files_ledger: visual_files::Ledger,
    /// visual-center parent answers (spec 013): engine snapshots and the four stage commands.
    pub center_ledger: visual_center::Ledger,
}

/// SSH phases linked to the run: the parent's own host observer, the page params and the isolated
/// SSH client files the window uses (never the user's keys or known_hosts).
pub struct SshLink<'a> {
    pub observer: &'a dyn HostObserver,
    pub page_params: Value,
    pub identity: PathBuf,
    pub known_hosts: PathBuf,
}

pub fn uid() -> u32 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("Uid:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse().ok())
        })
        .expect("uid from /proc/self/status")
}

/// Output offset of the page's client (CSS px) origin, observed from the private sway tree.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientOffset {
    pub x: f64,
    pub y: f64,
    pub observation: Value,
}

pub fn client_offset(
    tree: &Value,
    window_pid: u32,
    client: &Value,
) -> Result<ClientOffset, String> {
    let g = super::geometry::client_geometry(tree, window_pid, client)?;
    // Pointer coordinates are layout px: CSS px equal them only at scale 1 (the mouse phase runs
    // before resize-dpi changes the scale).
    if g.dpr != 1.0 {
        return Err(format!("client devicePixelRatio {} is not 1", g.dpr));
    }
    Ok(ClientOffset {
        x: g.x,
        y: g.y,
        observation: g.observation,
    })
}

/// Output point of a page client point; refused outside the private output.
pub fn to_output(p: Point, offset: &ClientOffset) -> Result<Point, String> {
    let at = Point {
        x: p.x + offset.x,
        y: p.y + offset.y,
    };
    Point::output(&json!({ "x": at.x, "y": at.y }))
}

/// `scroll` of `herdr pane get <pane>` (engine JSON), never defaulted.
pub fn pane_scroll(raw: &str) -> Result<Value, String> {
    let v: Value = serde_json::from_str(raw).map_err(|e| format!("pane get: {e}"))?;
    let scroll = &v["result"]["pane"]["scroll"];
    if scroll.is_object() {
        Ok(scroll.clone())
    } else {
        Err(format!("pane get without scroll: {v}"))
    }
}

/// Pane of the page identity when its boot is the Local host boot (the SSH hosts share `w1:p1`).
pub fn local_target(local_boot: &str, want: &Value) -> Result<String, String> {
    let pane = want["pane_id"]
        .as_str()
        .filter(|p| !p.is_empty())
        .ok_or("page did not report its confirmed pane")?;
    let boot_prefix = want["boot_prefix"].as_str().unwrap_or("");
    if boot_prefix.is_empty() || !local_boot.starts_with(boot_prefix) {
        return Err(format!(
            "precondition: page confirmed boot {boot_prefix:?} ({}), not the Local host boot {local_boot}",
            want["endpoint"]
        ));
    }
    Ok(pane.to_owned())
}

/// Handshake file of the window under the results dir: `report.json`, `report.jsonl`,
/// `report.tmp`, `report.want-<step>` and `report.ack-<step>` (the names `window.rs` writes).
pub fn is_handshake_file(name: &str) -> bool {
    matches!(name, "report.json" | "report.jsonl" | "report.tmp")
        || name.starts_with("report.want-")
        || name.starts_with("report.ack-")
}

fn written_since(path: &Path, run_started: SystemTime) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .is_ok_and(|mtime| mtime >= run_started)
}

/// Removes, at the start of a run, every handshake file of `results` last written before
/// `run_started` (an earlier run's want/ack/report, even tracked evidence). Other files are kept;
/// a file that cannot be removed is an error (the run must not start over it).
pub fn clear_stale_handshake(
    results: &Path,
    run_started: SystemTime,
) -> Result<Vec<PathBuf>, String> {
    let entries = match std::fs::read_dir(results) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("stale handshake: read {}: {e}", results.display())),
    };
    let mut removed = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let is_file = entry.file_type().is_ok_and(|t| t.is_file());
        if !is_file || !entry.file_name().to_str().is_some_and(is_handshake_file) {
            continue;
        }
        if written_since(&path, run_started) {
            continue;
        }
        std::fs::remove_file(&path)
            .map_err(|e| format!("stale handshake: remove {}: {e}", path.display()))?;
        removed.push(path);
    }
    Ok(removed)
}

/// JSON body of `<result>.want-<step>` only when this run wrote it (mtime at or after
/// `run_started`); an older or torn file is not a request.
pub fn fresh_want(result_path: &Path, step: &str, run_started: SystemTime) -> Option<Value> {
    let want = result_path.with_extension(format!("want-{step}"));
    if !written_since(&want, run_started) {
        return None;
    }
    std::fs::read_to_string(&want)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
}

/// Steps the page requested during this run (`want-<step>` with valid JSON, written at or after
/// `run_started`) and the parent has not answered yet; each is marked handled when returned, so
/// it is answered exactly once.
pub fn due_steps(
    result_path: &Path,
    steps: &[&str],
    handled: &mut Vec<String>,
    run_started: SystemTime,
) -> Vec<(String, Value)> {
    let mut due = Vec::new();
    for step in steps {
        if handled.iter().any(|h| h == step) {
            continue;
        }
        let Some(detail) = fresh_want(result_path, step, run_started) else {
            continue;
        };
        handled.push((*step).to_owned());
        due.push(((*step).to_owned(), detail));
    }
    due
}

/// Prefix of the refusal of a native want whose `boot_prefix` is not this run's engine boot.
pub const FOREIGN_BOOT_WANT: &str = "foreign-boot want";

/// Answers a native-keys/native-ime want: a want of another engine boot (or without a boot) is
/// refused with [`FOREIGN_BOOT_WANT`], without acting and without an ack. Otherwise `act` runs and
/// its outcome is acked; an action error is acked as `{"error"}` and recorded in `errors`.
pub fn answer_native_step(
    result_path: &Path,
    local_boot: &str,
    step: &str,
    detail: &Value,
    errors: &mut Vec<String>,
    act: impl FnOnce(&Value) -> Result<Value, String>,
) -> Result<Value, String> {
    let boot_prefix = detail["boot_prefix"].as_str().unwrap_or("");
    if boot_prefix.is_empty() || !local_boot.starts_with(boot_prefix) {
        return Err(format!(
            "{FOREIGN_BOOT_WANT}: want-{step} names boot {boot_prefix:?}, not the engine boot {local_boot} of this run; not answered"
        ));
    }
    let outcome = act(detail).unwrap_or_else(|e| {
        errors.push(format!("{step}: {e}"));
        json!({ "error": e })
    });
    write_ack(result_path, step, &outcome);
    Ok(outcome)
}

/// Whole ack (write + rename) the page's `harness_await` reads.
pub fn write_ack(result_path: &Path, step: &str, outcome: &Value) {
    let ack = result_path.with_extension(format!("ack-{step}"));
    let tmp = ack.with_extension("tmp");
    let _ = std::fs::write(&tmp, outcome.to_string()).and_then(|_| std::fs::rename(&tmp, &ack));
}

fn herdr(env: &FlowEnv, args: &[&str]) -> Result<String, String> {
    if env.herdr_env.is_empty() {
        return Err("herdr: the Local host environment is required (no inherited env)".into());
    }
    let output = Command::new(&env.herdr_bin)
        .env_clear()
        .envs(env.herdr_env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .arg("--session")
        .arg(&env.session)
        .args(args)
        .output()
        .map_err(|e| format!("herdr {args:?}: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(format!(
            "herdr {args:?}: {} {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

/// PIDs of processes whose stdout is `path` (the capture `cat` this run started in the pane).
pub fn capture_pids(path: &Path) -> Vec<u32> {
    let mut pids = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|s| s.parse::<u32>().ok())
            else {
                continue;
            };
            if std::fs::read_link(entry.path().join("fd/1"))
                .ok()
                .as_deref()
                == Some(path)
            {
                pids.push(pid);
            }
        }
    }
    pids
}

/// Raw PTY capture in the disposable pane: every byte the terminal delivers lands in `path`.
struct Capture {
    path: PathBuf,
}

impl Capture {
    fn start(env: &FlowEnv, pane: &str, path: PathBuf) -> Result<Self, String> {
        let command = format!("stty raw -echo; cat > '{}'; stty sane", path.display());
        Self::start_command(env, pane, path, &command)
    }

    /// Runs `command` (which must write the pane's raw input to `path`) and waits for its `cat`.
    fn start_command(
        env: &FlowEnv,
        pane: &str,
        path: PathBuf,
        command: &str,
    ) -> Result<Self, String> {
        std::fs::write(&path, b"").map_err(|e| e.to_string())?;
        herdr(env, &["pane", "run", pane, command])?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while capture_pids(&path).is_empty() {
            if Instant::now() > deadline {
                return Err(format!("capture cat for {} never started", path.display()));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        std::thread::sleep(Duration::from_millis(300));
        Ok(Self { path })
    }

    fn len(&self) -> usize {
        std::fs::metadata(&self.path)
            .map(|m| m.len() as usize)
            .unwrap_or(0)
    }

    fn since(&self, offset: usize) -> String {
        let bytes = std::fs::read(&self.path).unwrap_or_default();
        corpus::hex(bytes.get(offset..).unwrap_or_default())
    }

    fn stop(&self) -> Vec<u32> {
        let pids = capture_pids(&self.path);
        for pid in &pids {
            let _ = Command::new("kill").arg(pid.to_string()).status();
        }
        pids
    }
}

fn item<'a>(corpus: &'a Corpus, id: &str) -> Result<&'a corpus::Item, String> {
    corpus
        .items
        .iter()
        .find(|i| i.id == id)
        .ok_or_else(|| format!("corpus item {id} missing"))
}

const SETTLE: Duration = Duration::from_millis(900);

/// Epoch milliseconds (the page reports `performance.timeOrigin + now()` on the same clock).
fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

/// Pane the page confirmed; must be a pane of the disposable session (engine `pane list`).
fn confirmed_pane(env: &FlowEnv, want: &Value) -> Result<(String, Value), String> {
    let pane = local_target(&env.local_boot, want)?;
    let list: Value = serde_json::from_str(&herdr(env, &["pane", "list"])?)
        .map_err(|e| format!("pane list: {e}"))?;
    let known = list["result"]["panes"]
        .as_array()
        .is_some_and(|panes| panes.iter().any(|p| p["pane_id"] == pane.as_str()));
    if !known {
        return Err(format!(
            "pane {pane} is not in session {}: {list}",
            env.session
        ));
    }
    Ok((pane, list))
}

fn native_keys(sup: &mut Supervisor, env: &FlowEnv, pane: &str) -> Result<Value, String> {
    let capture = Capture::start(env, pane, env.evidence.join("pty-native-keys.bin"))?;
    let mut items = Vec::new();
    for id in NATIVE_KEY_ITEMS {
        let spec = item(&env.corpus, id)?;
        let before = capture.len();
        sup.wtype(&spec.keys)?;
        std::thread::sleep(SETTLE);
        items.push(json!({
            "id": id,
            "category": spec.category,
            "expected_hex": spec.commits.iter().map(|c| c.pty_hex.as_str()).collect::<String>(),
            "observed_hex": capture.since(before),
        }));
    }
    let _ = sup.grim("native-keys");
    let stopped = capture.stop();
    Ok(json!({ "items": items, "capture_pids_stopped": stopped }))
}

fn native_ime(sup: &mut Supervisor, env: &FlowEnv, pane: &str) -> Result<Value, String> {
    sup.start_fcitx5()?;
    let capture = Capture::start(env, pane, env.evidence.join("pty-native-ime.bin"))?;
    let mut items = Vec::new();
    for (n, id) in IME_ITEMS.into_iter().enumerate() {
        let spec = item(&env.corpus, id)?;
        // Same sequence as the prepared CJK recipe: Return between items (its bytes are recorded
        // separately). Candidate order may still differ between clean profiles; the evaluator
        // accepts only `corpus::NATIVE_IME_VARIANTS`.
        let mut separator_hex = Value::Null;
        if n > 0 {
            let before = capture.len();
            sup.wtype(&["-k", "Return"].map(str::to_owned))?;
            std::thread::sleep(SETTLE);
            separator_hex = json!(capture.since(before));
        }
        let before = capture.len();
        let preedit_start = now_ms();
        sup.wtype(&spec.keys)?;
        std::thread::sleep(SETTLE);
        let preedit_hex = capture.since(before);
        let preedit_png = sup.grim(&format!("native-ime-{n}-preedit")).ok();
        let preedit_end = now_ms();
        let before_commit = capture.len();
        let commit_start = now_ms();
        sup.wtype(&spec.confirm)?;
        std::thread::sleep(SETTLE);
        let commit_hex = capture.since(before_commit);
        let commit_end = now_ms();
        items.push(json!({
            "id": id,
            "keys": spec.keys,
            "confirm": spec.confirm,
            "preedit_hex": preedit_hex,
            "separator_before_hex": separator_hex,
            "preedit_window_ms": [preedit_start, preedit_end],
            "commit_window_ms": [commit_start, commit_end],
            "commit_hex": commit_hex,
            "preedit_png": preedit_png,
        }));
    }
    // Leave the input method (fcitx5 trigger Ctrl+Space) and type plain keys again.
    sup.wtype(&["-M", "ctrl", "-k", "space", "-m", "ctrl"].map(str::to_owned))?;
    std::thread::sleep(SETTLE);
    let before = capture.len();
    sup.wtype(&["-d", "20", "ok"].map(str::to_owned))?;
    std::thread::sleep(SETTLE);
    let after =
        json!({ "text": "ok", "expected_hex": "6f6b", "observed_hex": capture.since(before) });
    let _ = sup.grim("native-ime-after");
    let stopped = capture.stop();
    Ok(json!({ "items": items, "after_ime": after, "capture_pids_stopped": stopped }))
}

/// Status text of the Local host the page must show for the paste/mouse phases (observed in the
/// agent-ready native run: `status-item[1]`).
pub const LOCAL_ENDPOINT: &str = "Este computador · Local";

/// Private clipboard/pointer/recorder of one run, created only when their phase is driven.
#[derive(Default)]
struct PointerTools {
    clipboard: Option<paste_flow::PrivateClipboard>,
    pointer: Option<mouse_flow::PrivatePointer>,
    recorder: Option<mouse_flow::PrivateLinkRecorder>,
    window_pid: Option<u32>,
    view: Option<mouse_flow::PointerEnv>,
}

fn stop_capture(capture: &mut Option<Capture>) -> Vec<u32> {
    capture.take().map(|c| c.stop()).unwrap_or_default()
}

fn show_fixture(env: &FlowEnv, pane: &str, text: &str) -> Result<(), String> {
    let path = env.evidence.join(format!(
        "fixture-{}-{}.txt",
        pane.replace(|c: char| !c.is_ascii_alphanumeric(), "-"),
        now_ms() as u64
    ));
    if path.to_string_lossy().contains(['\'', '\n']) {
        return Err(format!("fixture path {} cannot be quoted", path.display()));
    }
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    herdr(
        env,
        &[
            "pane",
            "run",
            pane,
            &format!("clear; cat '{}'", path.display()),
        ],
    )?;
    std::thread::sleep(SETTLE);
    Ok(())
}

struct PasteWorld<'a> {
    sup: &'a mut Supervisor,
    env: &'a FlowEnv,
    clipboard: &'a mut paste_flow::PrivateClipboard,
    capture: Option<Capture>,
}

impl paste_flow::Keys for PasteWorld<'_> {
    fn press(&mut self, keys: &[String]) -> Result<(), String> {
        self.sup.wtype(keys)?;
        std::thread::sleep(SETTLE);
        Ok(())
    }
}

macro_rules! capture_impl {
    ($world:ident) => {
        impl paste_flow::PtyCapture for $world<'_> {
            fn len(&self) -> usize {
                self.capture.as_ref().map(Capture::len).unwrap_or(0)
            }
            fn since(&self, offset: usize) -> String {
                self.capture
                    .as_ref()
                    .map(|c| c.since(offset))
                    .unwrap_or_default()
            }
            fn alive_pids(&self) -> Vec<u32> {
                self.capture
                    .as_ref()
                    .map(|c| capture_pids(&c.path))
                    .unwrap_or_default()
            }
            fn stop(&mut self) -> Vec<u32> {
                stop_capture(&mut self.capture)
            }
        }
        impl paste_flow::PaneFixture for $world<'_> {
            fn show(&mut self, pane: &str, text: &str) -> Result<(), String> {
                show_fixture(self.env, pane, text)
            }
        }
    };
}
capture_impl!(PasteWorld);
capture_impl!(MouseWorld);

impl paste_flow::Clipboard for PasteWorld<'_> {
    fn display(&self) -> (String, String) {
        self.clipboard.display()
    }
    fn set(&mut self, text: &str) -> Result<(), String> {
        self.clipboard.set(text)
    }
    fn read(&mut self) -> Result<Vec<u8>, String> {
        self.clipboard.read()
    }
}

struct MouseWorld<'a> {
    pub env: &'a FlowEnv,
    pub pointer: &'a mut mouse_flow::PrivatePointer,
    pub recorder: &'a mouse_flow::PrivateLinkRecorder,
    pub offset: ClientOffset,
    pub capture: Option<Capture>,
}

impl mouse_flow::Pointer for MouseWorld<'_> {
    fn socket(&self) -> String {
        self.pointer.socket()
    }
    fn click(&mut self, at: Point, ctrl: bool) -> Result<(), String> {
        let at = to_output(at, &self.offset)?;
        self.pointer.click(at, ctrl)
    }
    fn wheel_up(&mut self, at: Point, notches: u8) -> Result<(), String> {
        let at = to_output(at, &self.offset)?;
        self.pointer.wheel_up(at, notches)
    }
    fn seat_commands(&mut self) -> Vec<Value> {
        self.pointer.seat_commands()
    }
}

impl mouse_flow::MouseApp for MouseWorld<'_> {
    fn start_mouse_app(&mut self, pane: &str) -> Result<(), String> {
        let path = self.env.evidence.join("pty-mouse-app.bin");
        let command = mouse_flow::mouse_app_command(&path)?;
        self.capture = Some(Capture::start_command(self.env, pane, path, &command)?);
        Ok(())
    }
}

impl mouse_flow::EngineView for MouseWorld<'_> {
    fn scroll(&mut self, pane: &str) -> Result<Value, String> {
        pane_scroll(&herdr(self.env, &["pane", "get", pane])?)
    }
    fn visible(&mut self, pane: &str) -> Result<String, String> {
        herdr(self.env, &["pane", "read", pane, "--source", "visible"])
    }
}

impl mouse_flow::LinkRecorder for MouseWorld<'_> {
    fn log_path(&self) -> String {
        self.recorder.log_path()
    }
    fn entries(&self) -> Result<Vec<String>, String> {
        self.recorder.entries()
    }
}

#[cfg(test)]
pub fn mouse_world_seat_commands(pointer: &mut mouse_flow::PrivatePointer) -> Vec<Value> {
    use mouse_flow::Pointer;
    let env = FlowEnv::dummy();
    let handler = mouse_flow::LinkHandler::new(
        Path::new("/tmp/hd007-live-test"),
        Path::new("/home/u"),
        1000,
    )
    .unwrap();
    let recorder = mouse_flow::PrivateLinkRecorder::from_handler(handler);
    let mut world = MouseWorld {
        env: &env,
        pointer,
        recorder: &recorder,
        offset: ClientOffset {
            x: 0.0,
            y: 0.0,
            observation: json!({}),
        },
        capture: None,
    };
    world.seat_commands()
}

/// One paste-selection step: Local pane confirmed by the engine before any key or clipboard use.
fn paste_step(
    sup: &mut Supervisor,
    env: &FlowEnv,
    tools: &mut PointerTools,
    exp: Option<&paste_flow::Expectations>,
    step: &str,
    detail: &Value,
) -> Result<Value, String> {
    let exp = exp.ok_or("paste expectations unavailable (setup failed)")?;
    let clipboard = tools
        .clipboard
        .as_mut()
        .ok_or("private clipboard unavailable (setup failed)")?;
    let (pane, list) = confirmed_pane(env, detail)?;
    let capture = if step == "paste-native" {
        Some(Capture::start(
            env,
            &pane,
            env.evidence.join("pty-paste-native.bin"),
        )?)
    } else {
        None
    };
    let mut world = PasteWorld {
        sup,
        env,
        clipboard,
        capture,
    };
    let result = paste_flow::parent_step(step, detail, exp, &mut world);
    let leftover = stop_capture(&mut world.capture);
    result.map(|mut answer| {
        answer["engine_pane_list"] = list;
        if !leftover.is_empty() {
            answer["capture_pids_stopped_after_step"] = json!(leftover);
        }
        answer
    })
}

/// One mouse-scroll-links step: Local pane confirmed, then the window geometry observed on the
/// private compositor (raw tree kept as evidence) before any pointer action.
fn mouse_step(
    env: &FlowEnv,
    tools: &mut PointerTools,
    exp: Option<&mouse_flow::Expectations>,
    step: &str,
    detail: &Value,
) -> Result<Value, String> {
    let exp = exp.ok_or("mouse expectations unavailable (setup failed)")?;
    let window_pid = tools.window_pid.ok_or("window pid unknown")?;
    let (Some(pointer), Some(recorder)) = (tools.pointer.as_mut(), tools.recorder.as_ref()) else {
        return Err("private pointer/link recorder unavailable (setup failed)".into());
    };
    let (_, list) = confirmed_pane(env, detail)?;
    let tree = pointer.tree()?;
    let _ = std::fs::write(
        env.evidence.join(format!("sway-tree-{step}.json")),
        serde_json::to_vec_pretty(&tree).unwrap_or_default(),
    );
    let offset = client_offset(&tree, window_pid, &detail["client"])?;
    let geometry = offset.observation.clone();
    let mut world = MouseWorld {
        env,
        pointer,
        recorder,
        offset,
        capture: None,
    };
    let result = mouse_flow::parent_step(step, detail, exp, &mut world);
    let leftover = stop_capture(&mut world.capture);
    result.map(|mut answer| {
        answer["pointer_geometry"] = geometry;
        answer["engine_pane_list"] = list;
        if !leftover.is_empty() {
            answer["capture_pids_stopped_after_step"] = json!(leftover);
        }
        answer
    })
}

/// Private view world that keeps the raw screenshot and tree of each step as evidence.
struct KeptView<'a, 'b> {
    inner: view_flow::PrivateView<'a>,
    evidence: &'b Path,
    step: &'b str,
}

impl view_flow::ViewWorld for KeptView<'_, '_> {
    fn socket(&self) -> String {
        self.inner.socket()
    }
    fn swaymsg(&mut self, args: &[String]) -> Result<view_flow::Run, String> {
        let run = self.inner.swaymsg(args)?;
        let kind = args.iter().skip_while(|a| *a != "-t").nth(1);
        if let Some(kind) = kind {
            // Repeated queries of one step (close-dialog focus samples) keep every raw answer.
            let path = (1..)
                .map(|n| match n {
                    1 => self.evidence.join(format!("{kind}-{}.json", self.step)),
                    n => self.evidence.join(format!("{kind}-{}-{n}.json", self.step)),
                })
                .find(|p| !p.exists())
                .expect("unbounded names");
            let _ = std::fs::write(path, &run.stdout);
        }
        Ok(run)
    }
    fn grim_ppm(&mut self) -> Result<view_flow::Run, String> {
        let run = self.inner.grim_ppm()?;
        let _ = std::fs::write(
            self.evidence.join(format!("screen-{}.ppm", self.step)),
            &run.stdout,
        );
        Ok(run)
    }
    fn wtype(&mut self, args: &[String]) -> Result<(), String> {
        self.inner.wtype(args)
    }
    fn sleep_ms(&mut self, ms: u64) {
        self.inner.sleep_ms(ms)
    }
}

/// `stty size` of the pane's actual PTY: written by the pane shell to a private evidence file.
fn live_stty_size(env: &FlowEnv, pane: &str, step: &str) -> Result<String, String> {
    let path = env.evidence.join(format!("stty-{step}.txt"));
    let _ = std::fs::remove_file(&path);
    herdr(
        env,
        &[
            "pane",
            "run",
            pane,
            &format!("stty size > {}", path.display()),
        ],
    )?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        if text.ends_with('\n') {
            return Ok(text);
        }
        if Instant::now() > deadline {
            return Err(format!(
                "stty size never written to {} ({text:?})",
                path.display()
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// One resize-dpi / a11y-navigation step: Local pane confirmed by the engine before any output
/// command, screenshot or key; then the prepared parent step on the private compositor only.
fn view_step(
    sup: &mut Supervisor,
    env: &FlowEnv,
    tools: &PointerTools,
    exp: Option<&view_flow::ViewExpectations>,
    ledger: &mut view_flow::Ledger,
    step: &str,
    detail: &Value,
) -> Result<Value, String> {
    let exp = exp.ok_or("view expectations unavailable (setup failed)")?;
    let pointer = tools
        .view
        .as_ref()
        .ok_or("private compositor env unavailable (setup failed)")?;
    let (pane, list) = confirmed_pane(env, detail)?;
    if detail["pane_id"] != pane.as_str() {
        return Err(format!(
            "{step}: page pane {} is not {pane}",
            detail["pane_id"]
        ));
    }
    let world = view_flow::PrivateView::new(view_flow::ViewEnv::new(pointer, exp)?, sup);
    let mut kept = KeptView {
        inner: world,
        evidence: &env.evidence,
        step,
    };
    let mut pty = |p: &str| live_stty_size(env, p, step);
    view_flow::parent_step(step, detail, exp, &mut kept, &mut pty, ledger).map(|mut answer| {
        answer["engine_pane_list"] = list;
        answer
    })
}

/// `swaymsg` on the private compositor only (the same private launch as resize-dpi).
fn private_swaymsg(
    sup: &mut Supervisor,
    tools: &PointerTools,
    exp: &view_flow::ViewExpectations,
    words: &[String],
) -> Result<Vec<u8>, String> {
    let pointer = tools
        .view
        .as_ref()
        .ok_or("private compositor env unavailable (setup failed)")?;
    let mut world = view_flow::PrivateView::new(view_flow::ViewEnv::new(pointer, exp)?, sup);
    let run = world.swaymsg(&view_flow::swaymsg_args(&exp.socket, words))?;
    if run.success {
        Ok(run.stdout)
    } else {
        Err(format!("swaymsg {words:?}: {} {}", run.status, run.stderr))
    }
}

/// Git branch of `cwd` as git reports it (the engine derives the workspace branch from `HEAD`);
/// `symbolic-ref` also names the branch of the fixture repository, which has no commit.
fn git_branch(cwd: &str) -> Option<String> {
    let out = Command::new("/usr/bin/git")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(["-C", cwd, "symbolic-ref", "--short", "HEAD"])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// One visual-frame step (spec 010): the Local pane confirmed by the engine first, then the output
/// command, engine reads, native key or PTY measurement of that step only.
fn visual_step(
    sup: &mut Supervisor,
    env: &FlowEnv,
    tools: &PointerTools,
    exp: Option<&view_flow::ViewExpectations>,
    capture: &mut Option<Capture>,
    step: &str,
    detail: &Value,
) -> Result<Value, String> {
    let exp = exp.ok_or("view expectations unavailable (setup failed)")?;
    let (pane, list) = confirmed_pane(env, detail)?;
    if detail["pane_id"] != pane.as_str() {
        return Err(format!(
            "{step}: page pane {} is not {pane}",
            detail["pane_id"]
        ));
    }
    let mut answer = json!({ "step": step, "pane_id": pane });
    match step {
        "visual-viewport" => {
            let raw = private_swaymsg(sup, tools, exp, &["-t".into(), "get_outputs".into()])?;
            let outputs: Value =
                serde_json::from_slice(&raw).map_err(|e| format!("get_outputs: {e}"))?;
            let size = |k: &str| {
                detail[k]
                    .as_f64()
                    .ok_or(format!("{step}: page sent no {k}"))
            };
            let words = visual_frame::viewport_command(
                &outputs,
                size("inner_width")?,
                size("inner_height")?,
            )?;
            private_swaymsg(sup, tools, exp, &words)?;
            answer["command"] = json!(words);
            answer["outputs_before"] = outputs;
            // The fixture's deterministic agent (`pi`, remote.rs) in another pane of the session,
            // so the palette's Agentes list has an engine agent to match; the measured PTY stays
            // the confirmed pane's.
            let host = visual_frame::agent_host_pane(&list, &pane)?;
            herdr(env, &["pane", "run", &host, "pi"])?;
            let deadline = Instant::now() + Duration::from_millis(visual_frame::AGENT_WAIT_MS);
            let want = format!("agent:{host}");
            let agents = loop {
                let raw: Value = serde_json::from_str(&herdr(env, &["agent", "list"])?)
                    .map_err(|e| format!("agent list: {e}"))?;
                if visual_frame::engine_agent_ids(&raw)?.contains(&want) {
                    break raw;
                }
                if Instant::now() > deadline {
                    return Err(format!(
                        "{step}: the engine never listed the agent started in {host}: {raw}"
                    ));
                }
                std::thread::sleep(Duration::from_millis(250));
            };
            answer["agent_pane"] = json!(host);
            answer["agent_list_after_start"] = agents;
        }
        "visual-engine" => {
            answer["screenshot"] = json!(sup.grim("visual-frame-1440x900")?);
            let version = Command::new(&env.herdr_bin)
                .env_clear()
                .envs(env.herdr_env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
                .arg("--version")
                .output()
                .map_err(|e| format!("herdr --version: {e}"))?;
            let tabs: Value = serde_json::from_str(&herdr(env, &["tab", "list"])?)
                .map_err(|e| format!("tab list: {e}"))?;
            let cwd = list["result"]["panes"]
                .as_array()
                .and_then(|p| p.iter().find(|x| x["pane_id"] == pane.as_str()))
                .and_then(|p| p["cwd"].as_str())
                .unwrap_or_default()
                .to_owned();
            let branch = git_branch(&cwd);
            answer["engine"] = visual_frame::engine_expectations(
                &String::from_utf8_lossy(&version.stdout),
                &tabs,
                &list,
                &pane,
                branch.as_deref(),
            )?;
            answer["engine_tab_list"] = tabs;
            answer["pane_cwd"] = json!(cwd);
            *capture = Some(Capture::start(
                env,
                &pane,
                env.evidence.join("pty-visual-frame.bin"),
            )?);
        }
        "visual-ctrlk-outside" | "visual-escape" | "visual-ctrlk-terminal" => {
            let token = if step == "visual-escape" {
                "Escape"
            } else {
                "ctrl+k"
            };
            let before = capture
                .as_ref()
                .ok_or("PTY capture not started (visual-engine failed)")?
                .len();
            sup.wtype(&visual_frame::wtype_args(token)?)?;
            std::thread::sleep(SETTLE);
            answer["keys"] = json!(token);
            answer["pty_hex"] = json!(capture.as_ref().map(|c| c.since(before)));
            if step == "visual-ctrlk-outside" {
                answer["screenshot"] = json!(sup.grim("visual-frame-palette")?);
                // Sources of the palette lists, read by the parent with the palette open: the
                // engine's agent list and the window's project catalog (projects.json).
                answer["engine_agent_list"] =
                    serde_json::from_str::<Value>(&herdr(env, &["agent", "list"])?)
                        .map_err(|e| format!("agent list: {e}"))?;
                let catalog = env.session_dir.join("gui/prefs").join("projects.json");
                answer["project_catalog"] = serde_json::from_slice::<Value>(
                    &std::fs::read(&catalog).map_err(|e| format!("{}: {e}", catalog.display()))?,
                )
                .map_err(|e| format!("{}: {e}", catalog.display()))?;
                answer["project_catalog_path"] = json!(catalog.display().to_string());
            }
            if step == "visual-ctrlk-terminal" {
                answer["capture_pids_stopped"] = json!(stop_capture(capture));
            }
        }
        "visual-restore" => {
            let (w, h) = visual_frame::RESTORE_MODE;
            let words = visual_frame::mode_command(w, h);
            private_swaymsg(sup, tools, exp, &words)?;
            answer["command"] = json!(words);
        }
        other => return Err(format!("{other} is not a visual-frame step")),
    }
    answer["engine_pane_list"] = list;
    Ok(answer)
}

fn visual_projects_step(
    sup: &mut Supervisor,
    env: &FlowEnv,
    step: &str,
    detail: &Value,
) -> Result<Value, String> {
    // The page's confirmed Local pane, validated against the engine before any action: this phase
    // starts the deterministic agent in it, so its workspace is the one the AC-011-01 dots must
    // show (a missing or foreign pane is refused, never silently skipped).
    let (pane, list) = confirmed_pane(env, detail)?;
    let mut answer = json!({ "step": step, "pane_id": pane });
    match step {
        "visual-projects-sidebar" => {
            // AC-011-01: start the deterministic agent `pi` in the active project's pane; only the
            // agents of that workspace may produce dots on the active project row.
            herdr(env, &["pane", "run", &pane, "pi"])?;
            let deadline = Instant::now() + Duration::from_millis(visual_frame::AGENT_WAIT_MS);
            let want = format!("agent:{pane}");
            let agents = loop {
                let raw: Value = serde_json::from_str(&herdr(env, &["agent", "list"])?)
                    .map_err(|e| format!("agent list: {e}"))?;
                if visual_frame::engine_agent_ids(&raw)?.contains(&want) {
                    break raw;
                }
                if Instant::now() > deadline {
                    return Err(format!(
                        "{step}: the engine never listed the agent started in {pane}: {raw}"
                    ));
                }
                std::thread::sleep(Duration::from_millis(100));
            };
            answer["agent_pane"] = json!(pane);
            answer["agent_list_after_start"] = agents;
            std::thread::sleep(Duration::from_millis(500));
            answer["screenshot"] = json!(sup.grim("visual-projects-sidebar")?);
        }
        "visual-projects-dialog" => {
            answer["screenshot"] = json!(sup.grim("visual-projects-dialog")?);
        }
        "visual-projects-close" => {
            sup.wtype(&["-k".into(), "Escape".into()])?;
            std::thread::sleep(SETTLE);
            answer["keys"] = json!("Escape");
        }
        other => return Err(format!("{other} is not a visual-projects step")),
    }
    answer["engine_pane_list"] = list;
    Ok(answer)
}

/// One visual-agents step (spec 014): the parent drives the engine from outside the window —
/// it leaves a nonce on the fixture agent's screen line, reports that agent waiting for the
/// user, presses the native Enter on the focused card and reads the engine back.
fn agents_step(
    sup: &mut Supervisor,
    env: &FlowEnv,
    step: &str,
    detail: &Value,
) -> Result<Value, String> {
    let (pane, _list) = confirmed_pane(env, detail)?;
    let agent_list = |env: &FlowEnv| -> Result<Value, String> {
        serde_json::from_str::<Value>(&herdr(env, &["agent", "list"])?)
            .map_err(|e| format!("agent list: {e}"))
    };
    // `herdr agent read` prints the snapshot text itself (cli::print_read_response), so the
    // answer wraps that text in the API shape the evaluator reads.
    let detection = |env: &FlowEnv, target: &str| -> Result<Value, String> {
        let text = herdr(
            env,
            &[
                "agent",
                "read",
                target,
                "--source",
                "detection",
                "--lines",
                "1",
            ],
        )?;
        Ok(json!({ "result": { "read": {
            "pane_id": target, "source": "detection", "text": text,
        } } }))
    };
    let mut answer = json!({ "step": step, "pane_id": pane });
    match step {
        "visual-agents-arm" => {
            let list = agent_list(env)?;
            let target = visual_agents::agent_pane(&list, &pane)?;
            // Terminal echo of the fixture agent's shell puts the nonce on the last screen line
            // without submitting it, so the line the card shows can only come from the engine.
            herdr(env, &["pane", "send-text", &target, visual_agents::NONCE])?;
            let deadline = Instant::now() + Duration::from_millis(visual_agents::SCREEN_WAIT_MS);
            let read = loop {
                let read = detection(env, &target)?;
                if visual_agents::last_snapshot_line(&read)
                    .is_some_and(|line| line.contains(visual_agents::NONCE))
                {
                    break read;
                }
                if Instant::now() > deadline {
                    return Err(format!(
                        "{step}: the detection snapshot of {target} never showed the nonce: {read}"
                    ));
                }
                std::thread::sleep(Duration::from_millis(200));
            };
            let catalog = env.session_dir.join("gui/prefs").join("projects.json");
            answer["agent_pane"] = json!(target);
            answer["nonce"] = json!(visual_agents::NONCE);
            answer["detection"] = read;
            answer["agent_list"] = agent_list(env)?;
            answer["project_catalog"] = serde_json::from_slice::<Value>(
                &std::fs::read(&catalog).map_err(|e| format!("{}: {e}", catalog.display()))?,
            )
            .map_err(|e| format!("{}: {e}", catalog.display()))?;
        }
        "visual-agents-block" => {
            let target = detail["pane_agent"]
                .as_str()
                .ok_or(format!("{step}: page sent no pane_agent"))?;
            answer["reported_at_ms"] = json!(now_ms());
            herdr(
                env,
                &[
                    "pane",
                    "report-agent",
                    target,
                    "--source",
                    visual_agents::REPORT_SOURCE,
                    "--agent",
                    visual_agents::REPORT_AGENT,
                    "--state",
                    "blocked",
                ],
            )?;
            // The change is complete when the command returns; the check measures from
            // `reported_at_ms` (before it), so the deadline is never credited to this call.
            answer["reported_done_ms"] = json!(now_ms());
            answer["agent_list"] = agent_list(env)?;
            answer["tab_list"] = serde_json::from_str::<Value>(&herdr(env, &["tab", "list"])?)
                .map_err(|e| format!("tab list: {e}"))?;
            // Workspaces carry the project UUID the window tagged them with
            // (project_store::PROJECT_TOKEN), which names the project of each agent.
            answer["workspace_list"] =
                serde_json::from_str::<Value>(&herdr(env, &["workspace", "list"])?)
                    .map_err(|e| format!("workspace list: {e}"))?;
            answer["detection"] = detection(env, target)?;
        }
        "visual-agents-focus" => {
            let target = detail["pane_agent"]
                .as_str()
                .ok_or(format!("{step}: page sent no pane_agent"))?;
            answer["screenshot"] = json!(sup.grim("visual-agents-panel")?);
            sup.wtype(&visual_agents::wtype_args("Return")?)?;
            let deadline = Instant::now() + Duration::from_millis(visual_agents::FOCUS_WAIT_MS);
            let focused = loop {
                let got: Value = serde_json::from_str(&herdr(env, &["pane", "get", target])?)
                    .map_err(|e| format!("pane get: {e}"))?;
                if got["result"]["pane"]["focused"] == true {
                    break got;
                }
                if Instant::now() > deadline {
                    return Err(format!("{step}: the engine never focused {target}: {got}"));
                }
                std::thread::sleep(Duration::from_millis(200));
            };
            let list = agent_list(env)?;
            answer["focused_pane"] = json!(target);
            answer["pane_get"] = focused;
            answer["agent_status_after"] =
                visual_agents::engine_agent(&list, target)?["agent_status"].clone();
            // The key must not have reached the agent: its pending screen line is untouched.
            answer["detection_after"] =
                json!(visual_agents::last_snapshot_line(&detection(env, target)?));
            answer["agent_list"] = list;
        }
        "visual-agents-collapse" => {
            answer["screenshot"] = json!(sup.grim("visual-agents-collapsed")?);
            let target = detail["pane_agent"]
                .as_str()
                .ok_or(format!("{step}: page sent no pane_agent"))?;
            // The fixture agent goes back to the state it reported itself.
            herdr(
                env,
                &[
                    "pane",
                    "report-agent",
                    target,
                    "--source",
                    visual_agents::REPORT_SOURCE,
                    "--agent",
                    visual_agents::REPORT_AGENT,
                    "--state",
                    "idle",
                ],
            )?;
            answer["agent_list"] = agent_list(env)?;
        }
        other => return Err(format!("{other} is not a visual-agents step")),
    }
    Ok(answer)
}

/// One visual-files step (spec 015): the Local pane confirmed by the engine first, then the output
/// command, the dictated buffer, the engine reads, the PTY measurement of the dock's corpus, the
/// marker echo or the remote snapshots the page compares.
// Each phase driver mirrors `visual_step`: the same world (supervisor, env, tools, expectations,
// link, capture slot) plus the step and its detail; folding them into a struct would hide them.
#[allow(clippy::too_many_arguments)]
fn visual_files_step(
    sup: &mut Supervisor,
    env: &FlowEnv,
    tools: &PointerTools,
    exp: Option<&view_flow::ViewExpectations>,
    ssh: Option<&SshLink>,
    capture: &mut Option<Capture>,
    step: &str,
    detail: &Value,
) -> Result<Value, String> {
    let exp = exp.ok_or("view expectations unavailable (setup failed)")?;
    let (pane, list) = confirmed_pane(env, detail)?;
    let mut answer = json!({ "step": step, "pane_id": pane });
    if let Some(want) = detail["pane_id"].as_str().filter(|w| !w.is_empty()) {
        if want != pane.as_str() {
            return Err(format!("{step}: page pane {want} is not {pane}"));
        }
    }
    match step {
        "visual-files-viewport" => {
            let raw = private_swaymsg(sup, tools, exp, &["-t".into(), "get_outputs".into()])?;
            let outputs: Value =
                serde_json::from_slice(&raw).map_err(|e| format!("get_outputs: {e}"))?;
            let size = |k: &str| {
                detail[k]
                    .as_f64()
                    .ok_or(format!("{step}: page sent no {k}"))
            };
            let words = visual_frame::viewport_command(
                &outputs,
                size("inner_width")?,
                size("inner_height")?,
            )?;
            private_swaymsg(sup, tools, exp, &words)?;
            answer["command"] = json!(words);
        }
        "visual-files-local" => {
            let path = env
                .session_dir
                .join("work")
                .join(visual_files::FIXTURE_FILE);
            let original =
                std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            if original != visual_files::ORIGINAL {
                return Err(format!(
                    "{step}: {} does not hold the fixture the phase expects: {original:?}",
                    path.display()
                ));
            }
            answer["file"] = json!(visual_files::FIXTURE_FILE);
            answer["root"] = json!(env.session_dir.join("work").display().to_string());
            answer["original"] = json!(original);
            answer["buffer"] = json!(visual_files::BUFFER);
            answer["removed"] = json!(visual_files::only_in(&original, visual_files::BUFFER));
            answer["added"] = json!(visual_files::only_in(visual_files::BUFFER, &original));
        }
        "visual-files-engine" => {
            let agents: Value = serde_json::from_str(&herdr(env, &["agent", "list"])?)
                .map_err(|e| format!("agent list: {e}"))?;
            let (agent, label) =
                visual_files::expected_dock(&agents, visual_files::LOCAL_PROJECT_LABEL)?;
            answer["agent"] = json!(agent);
            answer["expected_label"] = json!(label);
            answer["engine_agent_list"] = agents;
            answer["pane_cwd"] = json!(list["result"]["panes"]
                .as_array()
                .and_then(|panes| panes.iter().find(|p| p["pane_id"] == pane.as_str()))
                .and_then(|p| p["cwd"].as_str())
                .unwrap_or_default());
            answer["screenshot"] = json!(sup.grim("visual-files-local")?);
            if let Ok((ppm, _, _)) = sup.grim_ppm() {
                let path = env.evidence.join("screen-visual-files-local.ppm");
                let _ = std::fs::write(&path, ppm);
                answer["ppm"] = json!(path.display().to_string());
            }
        }
        "visual-files-keys" => {
            if detail["focused"] != true {
                return Err(format!(
                    "{step}: the dock's terminal was not focused by the page"
                ));
            }
            *capture = Some(Capture::start(
                env,
                &pane,
                env.evidence.join("pty-visual-files-keys.bin"),
            )?);
            let mut items = Vec::new();
            for id in corpus::NATIVE_KEY_ITEMS {
                let spec = item(&env.corpus, id)?;
                let before = capture.as_ref().expect("capture started").len();
                sup.wtype(&spec.keys)?;
                std::thread::sleep(SETTLE);
                let observed = capture.as_ref().expect("capture started").since(before);
                items.push(json!({
                    "id": id,
                    "expected_hex": spec.commits.iter().map(|c| c.pty_hex.as_str()).collect::<String>(),
                    "observed_hex": observed,
                }));
            }
            answer["items"] = json!(items);
            answer["capture_pids_stopped"] = json!(stop_capture(capture));
        }
        "visual-files-frames" => {
            let marker = "visual-files-marker-015";
            // The capture's shell runs `stty sane` after its `cat` was stopped; the echo follows it.
            std::thread::sleep(Duration::from_millis(500));
            herdr(env, &["pane", "run", &pane, &format!("echo {marker}")])?;
            std::thread::sleep(SETTLE);
            answer["marker"] = json!(marker);
            answer["pane_read"] = json!(herdr(env, &["pane", "read", &pane])?);
        }
        "visual-files-remote-before" | "visual-files-remote-change" => {
            let link = ssh.ok_or("SSH fixture link unavailable (setup failed)")?;
            let root = link.page_params["ssh"]["root"]
                .as_str()
                .filter(|r| r.starts_with('/'))
                .ok_or("ssh page params without an absolute root")?;
            let label = link.page_params["ssh_profile"]["label"]
                .as_str()
                .ok_or("ssh page params without the profile label")?;
            let content = if step == "visual-files-remote-before" {
                visual_files::REMOTE_A
            } else {
                visual_files::REMOTE_B
            };
            let path = Path::new(root).join(visual_files::REMOTE_FILE);
            std::fs::write(&path, content).map_err(|e| format!("{}: {e}", path.display()))?;
            answer["root"] = json!(root);
            answer["path"] = json!(path.display().to_string());
            answer["banner_label"] = json!(label);
            if step == "visual-files-remote-before" {
                answer["content_a"] = json!(content);
            } else {
                answer["content_a"] = json!(visual_files::REMOTE_A);
                answer["content_b"] = json!(content);
            }
        }
        "visual-files-evidence" => {
            answer["screenshot"] = json!(sup.grim("visual-files-remote")?);
            if let Ok((ppm, _, _)) = sup.grim_ppm() {
                let path = env.evidence.join("screen-visual-files-remote.ppm");
                let _ = std::fs::write(&path, ppm);
                answer["ppm"] = json!(path.display().to_string());
            }
        }
        "visual-files-restore" => {
            let (w, h) = visual_files::RESTORE_MODE;
            let words = visual_frame::mode_command(w, h);
            private_swaymsg(sup, tools, exp, &words)?;
            answer["command"] = json!(words);
        }
        other => return Err(format!("{other} is not a visual-files step")),
    }
    answer["engine_pane_list"] = list;
    Ok(answer)
}

/// One visual-center step (spec 013): the engine snapshots the checks compare against, the output
/// command of a stage, and the screenshots kept as evidence. The parent never reads the page's DOM.
fn center_step(
    sup: &mut Supervisor,
    env: &FlowEnv,
    tools: &PointerTools,
    exp: Option<&view_flow::ViewExpectations>,
    step: &str,
    detail: &Value,
) -> Result<Value, String> {
    let exp = exp.ok_or("view expectations unavailable (setup failed)")?;
    let (pane, list) = confirmed_pane(env, detail)?;
    let mut answer = json!({ "step": step, "pane_id": pane });
    let engine = |answer: &mut Value, env: &FlowEnv| -> Result<(), String> {
        answer["engine_tab_list"] = serde_json::from_str::<Value>(&herdr(env, &["tab", "list"])?)
            .map_err(|e| format!("tab list: {e}"))?;
        answer["engine_agent_list"] =
            serde_json::from_str::<Value>(&herdr(env, &["agent", "list"])?)
                .map_err(|e| format!("agent list: {e}"))?;
        Ok(())
    };
    match step {
        "center-engine-before" => {
            engine(&mut answer, env)?;
            // Projects the window knows (its own catalog) and the branch git reports for the cwd of
            // the focused pane: the header must show one of those roots and that branch.
            let catalog = env.session_dir.join("gui/prefs").join("projects.json");
            answer["project_catalog"] = serde_json::from_slice::<Value>(
                &std::fs::read(&catalog).map_err(|e| format!("{}: {e}", catalog.display()))?,
            )
            .map_err(|e| format!("{}: {e}", catalog.display()))?;
            let cwd = visual_center::focused_cwd(&list)
                .ok_or_else(|| format!("{step}: no focused pane with a cwd in {list}"))?;
            answer["focused_cwd"] = json!(cwd);
            answer["branch"] = json!(git_branch(&cwd));
            answer["screenshot"] = json!(sup.grim("visual-center-before")?);
        }
        "center-actions" => {
            engine(&mut answer, env)?;
            answer["screenshot"] = json!(sup.grim("visual-center-actions")?);
            // Restores the layout the page zoomed: `--mode off` only changes something when the
            // window's expand button really zoomed that pane in the engine.
            let target = detail["actions"]["focus_target"]
                .as_str()
                .ok_or_else(|| format!("{step}: page sent no focus_target"))?;
            let words = visual_center::zoom_off_command(target);
            let argv: Vec<&str> = words.iter().map(String::as_str).collect();
            answer["zoom_off_command"] = json!(words);
            answer["zoom_off"] = serde_json::from_str::<Value>(&herdr(env, &argv)?)
                .map_err(|e| format!("pane zoom off: {e}"))?;
            answer["engine_pane_list_after_unzoom"] =
                serde_json::from_str::<Value>(&herdr(env, &["pane", "list"])?)
                    .map_err(|e| format!("pane list: {e}"))?;
        }
        _ => {
            let stage = detail["stage"]
                .as_str()
                .ok_or_else(|| format!("{step}: page sent no stage"))?;
            let spec = view_flow::STAGES
                .iter()
                .find(|s| s.name == stage)
                .ok_or_else(|| format!("{step}: {stage} is not a stage of the flow"))?;
            if step.starts_with("center-apply-") {
                let words = view_flow::output_command(spec)
                    .ok_or_else(|| format!("{step}: stage {stage} has no output command"))?;
                private_swaymsg(sup, tools, exp, &words)?;
                answer["command"] = json!(words);
            } else {
                answer["screenshot"] = json!(sup.grim(&format!("visual-center-{stage}"))?);
            }
        }
    }
    answer["engine_pane_list"] = list;
    Ok(answer)
}

fn read_jsonl(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Runs the private display, the window and the parent steps; always cleans up.
pub fn run(
    env: &FlowEnv,
    exe: &Path,
    phases: &[&str],
    fixture_file: &str,
    ssh: Option<&SshLink>,
) -> LiveRun {
    let mut live = LiveRun::default();
    // Every handshake file older than this instant belongs to another run.
    let run_started = SystemTime::now();
    let runtime = PathBuf::from(format!("/tmp/hd7L-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&runtime);
    if let Err(e) = std::fs::create_dir(&runtime).and_then(|_| {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700))
    }) {
        live.errors.push(format!("runtime dir: {e}"));
        return live;
    }
    let gui = env.session_dir.join("gui");
    let (home, prefs, state) = (gui.join("home"), gui.join("prefs"), gui.join("state"));
    let results = env.evidence.join("window");
    for dir in [&prefs, &state, &results] {
        let _ = std::fs::create_dir_all(dir);
    }
    match clear_stale_handshake(&results, run_started) {
        Ok(removed) => live.supervisor_log.extend(
            removed
                .iter()
                .map(|p| format!("stale handshake removed: {}", p.display())),
        ),
        Err(e) => {
            live.errors.push(e);
            let _ = std::fs::remove_dir_all(&runtime);
            return live;
        }
    }
    let display = match PrivateDisplay::new(
        runtime.clone(),
        home,
        Resources::under(&env.resources_root),
        env.user.clone(),
        env.uid,
    ) {
        Ok(d) => d,
        Err(e) => {
            live.errors.push(e);
            let _ = std::fs::remove_dir_all(&runtime);
            return live;
        }
    };
    let missing: Vec<_> = display
        .resources
        .required_files()
        .into_iter()
        .filter(|p| !p.exists())
        .collect();
    if !missing.is_empty() {
        live.errors.push(format!("missing resources: {missing:?}"));
        let _ = std::fs::remove_dir_all(&runtime);
        return live;
    }
    let mut sup = match Supervisor::start(display, env.evidence.join("processes")) {
        Ok(s) => s,
        Err(e) => {
            live.errors.push(format!("supervisor: {e}"));
            return live;
        }
    };
    let mut tools = PointerTools::default();
    let (paste_exp, mouse_exp, link_env) =
        pointer_setup(env, &sup, &runtime, phases, &mut tools, &mut live.errors);
    live.paste_expectations = paste_exp;
    live.mouse_expectations = mouse_exp;
    live.view_expectations = view_setup(&sup, &runtime, phases, &mut tools, &mut live.errors);
    let result_path = results.join("report.json");
    let work = env.session_dir.join("work");
    let mut params = json!({
        "phases": phases,
        // Paint/Full-frame counters of the mounted terminal (resize-dpi requires them).
        "terminal_probe": true,
        "project": {
            "label": "Fidelidade Local",
            "endpoint": "local",
            "session": env.session,
            "root": work.display().to_string(),
            "file": fixture_file,
        },
    });
    if let Some(link) = ssh {
        params["ssh_flow"] = link.page_params.clone();
    }
    let paths = |p: &Path| p.display().to_string();
    let mut extra = vec![
        ("HERDR_DESKTOP_HARNESS_PHASE", "flow".to_owned()),
        ("HERDR_DESKTOP_HARNESS_RESULT", paths(&result_path)),
        ("HERDR_DESKTOP_E2E_USER_UID", env.uid.to_string()),
        ("HERDR_DESKTOP_E2E_PARAMS", params.to_string()),
        ("HERDR_DESKTOP_E2E_SESSION", env.session.clone()),
        (
            "HERDR_DESKTOP_E2E_HERDR_CONFIG_DIR",
            paths(&env.herdr_config_dir),
        ),
        ("HERDR_DESKTOP_E2E_PREFS_DIR", paths(&prefs)),
        ("HERDR_DESKTOP_E2E_STATE_DIR", paths(&state)),
        ("HERDR_DESKTOP_HERDR_BIN", paths(&env.herdr_bin)),
    ];
    if let Some(link) = ssh {
        extra.push(("HERDR_DESKTOP_E2E_SSH_IDENTITY", paths(&link.identity)));
        extra.push((
            "HERDR_DESKTOP_E2E_SSH_KNOWN_HOSTS",
            paths(&link.known_hosts),
        ));
    }
    let mut extra: Vec<(&str, &str)> = extra.iter().map(|(k, v)| (*k, v.as_str())).collect();
    extra.extend(link_env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    let paste_steps: &[&str] = if phases.contains(&paste_flow::PHASE) {
        &paste_flow::STEPS
    } else {
        &[]
    };
    let mouse_steps: &[&str] = if phases.contains(&mouse_flow::PHASE) {
        &mouse_flow::STEPS
    } else {
        &[]
    };
    let started = sup.start_window(exe, &extra);
    if let Err(e) = started {
        live.errors.push(format!("window: {e}"));
    } else {
        tools.window_pid = started.ok();
        if let (Some(pid), Some(e)) = (tools.window_pid, live.view_expectations.take()) {
            live.view_expectations = Some(e.with_window(pid));
        }
        let view_steps: Vec<&str> = [
            (view_flow::RESIZE_PHASE, &view_flow::RESIZE_STEPS[..]),
            (view_flow::A11Y_PHASE, &view_flow::A11Y_STEPS[..]),
        ]
        .into_iter()
        .filter(|(phase, _)| phases.contains(phase))
        .flat_map(|(_, steps)| steps.iter().copied())
        .collect();
        let visual_steps: &[&str] = if phases.contains(&visual_frame::PHASE) {
            &visual_frame::STEPS
        } else {
            &[]
        };
        let agents_steps: &[&str] = if phases.contains(&visual_agents::PHASE) {
            &visual_agents::STEPS
        } else {
            &[]
        };
        let files_steps: &[&str] = if phases.contains(&visual_files::PHASE) {
            &visual_files::STEPS
        } else {
            &[]
        };
        let center_steps: &[&str] = if phases.contains(&visual_center::PHASE) {
            &visual_center::STEPS
        } else {
            &[]
        };
        let projects_steps: &[&str] = if phases.contains(&visual_projects::PHASE) {
            &visual_projects::STEPS
        } else {
            &[]
        };
        let mut visual_capture: Option<Capture> = None;
        let mut files_capture: Option<Capture> = None;
        let jsonl = result_path.with_extension("jsonl");
        let deadline = Instant::now() + Duration::from_secs(900);
        let mut seen = 0;
        let mut handled: Vec<String> = Vec::new();
        loop {
            if let Some(status) = sup.try_exit("window") {
                live.window_exit = Some(status.to_string());
                break;
            }
            if Instant::now() > deadline {
                live.errors
                    .push("window did not finish within 900 s".into());
                break;
            }
            let reports = read_jsonl(&jsonl);
            for report in reports.iter().skip(seen) {
                let name = report["phase"].as_str().unwrap_or("").to_owned();
                if live.isolation.is_empty() {
                    live.isolation = sup.isolation();
                }
                if report["done"] == false && !name.is_empty() {
                    let _ = sup.grim(&format!("after-{name}"));
                }
            }
            seen = reports.len();
            for step in ["native-keys", "native-ime"] {
                if handled.iter().any(|h| h == step) {
                    continue;
                }
                let Some(detail) = fresh_want(&result_path, step, run_started) else {
                    continue;
                };
                handled.push(step.into());
                let answered = answer_native_step(
                    &result_path,
                    &env.local_boot,
                    step,
                    &detail,
                    &mut live.errors,
                    |detail| {
                        let (pane, list) = confirmed_pane(env, detail)?;
                        let mut value = match step {
                            "native-keys" => native_keys(&mut sup, env, &pane),
                            _ => native_ime(&mut sup, env, &pane),
                        }?;
                        value["pane_id"] = json!(pane);
                        value["engine_pane_list"] = list;
                        Ok(value)
                    },
                );
                if let Err(refused) = answered {
                    live.errors.push(format!("{step}: {refused}"));
                }
            }
            for step in ssh.map(|_| ssh_flow::STEPS).unwrap_or_default() {
                let Some(detail) = fresh_want(&result_path, step, run_started) else {
                    continue;
                };
                if handled.iter().any(|h| h == step) {
                    continue;
                }
                handled.push(step.into());
                let observer = ssh.expect("steps only with a link").observer;
                // The parent's own snapshots; recorded before the page may continue.
                let outcome = ssh_flow::parent_step(step, &detail, observer)
                    .and_then(|answer| live.ledger.record(step, answer.clone()).map(|_| answer))
                    .unwrap_or_else(|e| {
                        live.errors.push(format!("{step}: {e}"));
                        json!({ "error": e })
                    });
                let ack = result_path.with_extension(format!("ack-{step}"));
                let tmp = ack.with_extension("tmp");
                let _ = std::fs::write(&tmp, outcome.to_string())
                    .and_then(|_| std::fs::rename(&tmp, &ack));
            }
            for (step, detail) in due_steps(&result_path, paste_steps, &mut handled, run_started) {
                let outcome = paste_step(
                    &mut sup,
                    env,
                    &mut tools,
                    live.paste_expectations.as_ref(),
                    &step,
                    &detail,
                );
                answer_linked(&mut live, &result_path, &step, outcome);
            }
            for (step, detail) in due_steps(&result_path, mouse_steps, &mut handled, run_started) {
                let outcome = mouse_step(
                    env,
                    &mut tools,
                    live.mouse_expectations.as_ref(),
                    &step,
                    &detail,
                );
                answer_linked(&mut live, &result_path, &step, outcome);
            }
            for (step, detail) in due_steps(&result_path, &view_steps, &mut handled, run_started) {
                let outcome = view_step(
                    &mut sup,
                    env,
                    &tools,
                    live.view_expectations.as_ref(),
                    &mut live.view_ledger,
                    &step,
                    &detail,
                )
                .unwrap_or_else(|e| {
                    live.errors.push(format!("{step}: {e}"));
                    json!({ "error": e })
                });
                write_ack(&result_path, &step, &outcome);
            }
            for (step, detail) in due_steps(&result_path, center_steps, &mut handled, run_started) {
                let outcome = center_step(
                    &mut sup,
                    env,
                    &tools,
                    live.view_expectations.as_ref(),
                    &step,
                    &detail,
                )
                .and_then(|answer| {
                    live.center_ledger
                        .record(&step, answer.clone())
                        .map(|_| answer)
                })
                .unwrap_or_else(|e| {
                    live.errors.push(format!("{step}: {e}"));
                    json!({ "error": e })
                });
                write_ack(&result_path, &step, &outcome);
            }
            for (step, detail) in due_steps(&result_path, visual_steps, &mut handled, run_started) {
                let outcome = visual_step(
                    &mut sup,
                    env,
                    &tools,
                    live.view_expectations.as_ref(),
                    &mut visual_capture,
                    &step,
                    &detail,
                )
                .and_then(|answer| {
                    live.visual_ledger
                        .record(&step, answer.clone())
                        .map(|_| answer)
                })
                .unwrap_or_else(|e| {
                    live.errors.push(format!("{step}: {e}"));
                    json!({ "error": e })
                });
                write_ack(&result_path, &step, &outcome);
            }
            for (step, detail) in due_steps(&result_path, projects_steps, &mut handled, run_started)
            {
                let outcome = visual_projects_step(&mut sup, env, &step, &detail)
                    .and_then(|answer| {
                        live.visual_projects_ledger
                            .record(&step, answer.clone())
                            .map(|_| answer)
                    })
                    .unwrap_or_else(|e| {
                        live.errors.push(format!("{step}: {e}"));
                        json!({ "error": e })
                    });
                write_ack(&result_path, &step, &outcome);
            }
            for (step, detail) in due_steps(&result_path, agents_steps, &mut handled, run_started) {
                let outcome = agents_step(&mut sup, env, &step, &detail)
                    .and_then(|answer| {
                        live.agents_ledger
                            .record(&step, answer.clone())
                            .map(|_| answer)
                    })
                    .unwrap_or_else(|e| {
                        live.errors.push(format!("{step}: {e}"));
                        json!({ "error": e })
                    });
                write_ack(&result_path, &step, &outcome);
            }
            for (step, detail) in due_steps(&result_path, files_steps, &mut handled, run_started) {
                let outcome = visual_files_step(
                    &mut sup,
                    env,
                    &tools,
                    live.view_expectations.as_ref(),
                    ssh,
                    &mut files_capture,
                    &step,
                    &detail,
                )
                .and_then(|answer| {
                    live.visual_files_ledger
                        .record(&step, answer.clone())
                        .map(|_| answer)
                })
                .unwrap_or_else(|e| {
                    live.errors.push(format!("{step}: {e}"));
                    json!({ "error": e })
                });
                write_ack(&result_path, &step, &outcome);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let files_leftover = stop_capture(&mut files_capture);
        if !files_leftover.is_empty() {
            live.supervisor_log.push(format!(
                "visual-files capture stopped at the end: {files_leftover:?}"
            ));
        }
        let leftover = stop_capture(&mut visual_capture);
        if !leftover.is_empty() {
            live.supervisor_log.push(format!(
                "visual-frame capture stopped at the end: {leftover:?}"
            ));
        }
        for report in read_jsonl(&jsonl) {
            match report["phase"].as_str() {
                Some("flow") => live.final_report = Some(report),
                Some(name) => {
                    live.reports.insert(name.to_owned(), report);
                }
                None => {}
            }
        }
    }
    live.supervisor_log.extend(pointer_cleanup(env, &mut tools));
    live.supervisor_log.extend(sup.cleanup());
    live
}

/// Records the parent's answer (a step answered twice is an error) and acks the page.
fn answer_linked(
    live: &mut LiveRun,
    result_path: &Path,
    step: &str,
    outcome: Result<Value, String>,
) {
    let outcome = outcome
        .and_then(|answer| {
            live.pointer_ledger
                .record(step, answer.clone())
                .map(|_| answer)
        })
        .unwrap_or_else(|e| {
            live.errors.push(format!("{step}: {e}"));
            json!({ "error": e })
        });
    write_ack(result_path, step, &outcome);
}

type LinkEnv = Vec<(String, String)>;

/// Private clipboard (paste-selection) and pointer + link recorder (mouse-scroll-links) on the
/// private display only; literals fixed before the window starts. A failure is recorded and the
/// phase then has no expectations (never a pass).
fn pointer_setup(
    env: &FlowEnv,
    sup: &Supervisor,
    runtime: &Path,
    phases: &[&str],
    tools: &mut PointerTools,
    errors: &mut Vec<String>,
) -> (
    Option<paste_flow::Expectations>,
    Option<mouse_flow::Expectations>,
    LinkEnv,
) {
    let nonce = format!("{:x}", (now_ms() * 1000.0) as u64);
    let runtime_s = runtime.to_string_lossy().into_owned();
    let mut paste = None;
    if phases.contains(&paste_flow::PHASE) {
        let made = sup
            .display
            .wtype(&sup.wayland, &[])
            .and_then(|base| paste_flow::ClipboardEnv::new(&base, &sup.bus, env.uid))
            .and_then(|clip_env| {
                let exp = paste_flow::Expectations::new(
                    &env.corpus,
                    &runtime_s,
                    &sup.wayland.to_string_lossy(),
                    LOCAL_ENDPOINT,
                    &format!("{}{nonce}", paste_flow::SENTINEL_PREFIX),
                )?;
                Ok((paste_flow::PrivateClipboard::new(clip_env), exp))
            });
        match made {
            Ok((clipboard, exp)) => {
                tools.clipboard = Some(clipboard);
                paste = Some(exp);
            }
            Err(e) => errors.push(format!("{}: setup: {e}", paste_flow::PHASE)),
        }
    }
    let mut mouse = None;
    let mut link_env = Vec::new();
    if phases.contains(&mouse_flow::PHASE) {
        let sway_pid = sup.tracked.iter().find(|t| t.name == "sway").map(|t| t.pid);
        let made = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| "HOME of the user is unknown".to_owned())
            .and_then(|user_home| {
                mouse_flow::LinkHandler::new(&sup.display.home, &user_home, env.uid)
            })
            .and_then(|handler| {
                let sway_pid = sway_pid.ok_or("private sway pid not tracked")?;
                let base = sup.display.wtype(&sup.wayland, &[])?;
                let pointer_env = mouse_flow::PointerEnv::new(
                    &base,
                    &sup.display.resources.sway_prefix,
                    env.uid,
                    sway_pid,
                )?;
                let exp = mouse_flow::Expectations::new(
                    LOCAL_ENDPOINT,
                    &runtime_s,
                    pointer_env.socket(),
                    &handler,
                    &nonce,
                )?;
                let window_env = handler.window_env();
                let recorder = mouse_flow::PrivateLinkRecorder::install(handler)?;
                Ok((
                    mouse_flow::PrivatePointer::new(pointer_env)?,
                    recorder,
                    exp,
                    window_env,
                ))
            });
        match made {
            Ok((pointer, recorder, exp, window_env)) => {
                tools.pointer = Some(pointer);
                tools.recorder = Some(recorder);
                mouse = Some(exp);
                link_env = window_env;
            }
            Err(e) => errors.push(format!("{}: setup: {e}", mouse_flow::PHASE)),
        }
    }
    (paste, mouse, link_env)
}

/// Private compositor env of resize-dpi / a11y-navigation (same derivation as the pointer).
fn view_setup(
    sup: &Supervisor,
    runtime: &Path,
    phases: &[&str],
    tools: &mut PointerTools,
    errors: &mut Vec<String>,
) -> Option<view_flow::ViewExpectations> {
    let wanted: Vec<&str> = [
        view_flow::RESIZE_PHASE,
        view_flow::A11Y_PHASE,
        visual_frame::PHASE,
        visual_files::PHASE,
        visual_center::PHASE,
    ]
    .into_iter()
    .filter(|p| phases.contains(p))
    .collect();
    if wanted.is_empty() {
        return None;
    }
    let sway_pid = sup.tracked.iter().find(|t| t.name == "sway").map(|t| t.pid);
    let made = sway_pid
        .ok_or_else(|| "private sway pid not tracked".to_owned())
        .and_then(|sway_pid| {
            let base = sup.display.wtype(&sup.wayland, &[])?;
            let env = mouse_flow::PointerEnv::new(
                &base,
                &sup.display.resources.sway_prefix,
                uid(),
                sway_pid,
            )?;
            let e = view_flow::ViewExpectations::new(
                LOCAL_ENDPOINT,
                &runtime.to_string_lossy(),
                env.socket(),
            )?;
            Ok((env, e))
        });
    match made {
        Ok((env, e)) => {
            tools.view = Some(env);
            Some(e)
        }
        Err(e) => {
            for phase in wanted {
                errors.push(format!("{phase}: setup: {e}"));
            }
            None
        }
    }
}

/// Stops the tracked clipboard/pointer children (PID+starttime) and keeps the recorder log.
fn pointer_cleanup(env: &FlowEnv, tools: &mut PointerTools) -> Vec<String> {
    let mut log = Vec::new();
    if let Some(clipboard) = tools.clipboard.as_mut() {
        log.extend(
            clipboard
                .cleanup()
                .into_iter()
                .map(|l| format!("clipboard: {l}")),
        );
    }
    if let Some(pointer) = tools.pointer.as_mut() {
        log.extend(
            pointer
                .cleanup()
                .into_iter()
                .map(|l| format!("pointer: {l}")),
        );
    }
    if let Some(recorder) = tools.recorder.as_ref() {
        let copy = env.evidence.join("link-recorder.log");
        match std::fs::copy(&recorder.handler.log, &copy) {
            Ok(_) => log.push(format!("recorder log kept at {}", copy.display())),
            Err(e) => log.push(format!(
                "recorder log {}: {e}",
                recorder.handler.log.display()
            )),
        }
    }
    log
}
