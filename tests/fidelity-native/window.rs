//! The composed product window under the native harness.
//!
//! Mounts the real product: `herdr_desktop::configure` (its own `setup` → `install`, window
//! lifecycle and close release, untouched), the window declared in `tauri.conf.json` and the
//! product `handler()`. The harness adds, only in this test binary:
//!
//! - an invoke wrapper that sends `harness_report` to the harness and every other command to the
//!   product handler (no second product registry, no product command shadowed);
//! - a test plugin whose `js_init_script` injects `window.__HERDR_HARNESS__` (feature `fidelity`)
//!   before page scripts, and whose setup arms the phase timeout. The product `.setup` is not
//!   replaced.
//!
//! `src/main.ts` then mounts `src/features/fidelity/preview.svelte` (which mounts `<App/>` with
//! its real bridges) and runs `src/features/fidelity/e2e.ts`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use herdr_client::bootstrap::BootstrapConfig;
use herdr_desktop::connections::ssh_options::IsolatedSshConfig;
use herdr_desktop::{DesktopConfig, DEFAULT_GEOMETRY};
use tauri::{Manager, Wry};

use super::session;

/// WebKit settings API arms of the resource-bench window (memory diagnostic only).
#[path = "webkit_policy.rs"]
pub mod webkit_policy;

/// Feature name the page resolves (`src/features/fidelity/`).
pub const FEATURE: &str = "fidelity";
/// The only commands answered by the harness instead of the product.
/// `harness_await` is the explicitly identified step helper: the page waits for the parent's
/// acknowledgement of a named step (native keys sent, fixture written) before observing.
/// `harness_window_visible` hides/shows this window so the product's own visibility path runs
/// (spec 009): the product grants the WebView only close/minimize/toggle-maximize/start-dragging
/// (`src-tauri/capabilities/default.json`), so the hide lives here, in the test binary, and is
/// never a product capability.
pub const HARNESS_COMMANDS: [&str; 3] =
    ["harness_report", "harness_await", "harness_window_visible"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Harness,
    Product,
}

pub fn route(command: &str) -> Route {
    if HARNESS_COMMANDS.contains(&command) {
        Route::Harness
    } else {
        Route::Product
    }
}

/// Script injected before the page loads (same shape `src/harness/native.ts` validates).
pub fn selection_script(phase: &str, params: &serde_json::Value) -> String {
    let selection = serde_json::json!({ "feature": FEATURE, "phase": phase, "params": params });
    format!("window.__HERDR_HARNESS__ = Object.freeze({selection});")
}

/// Where the page's reports go.
pub struct ReportSink {
    path: PathBuf,
    finished: AtomicBool,
}

fn write_atomic(path: &Path, value: &serde_json::Value) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn append_jsonl(path: &Path, value: &serde_json::Value) -> Result<(), String> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    writeln!(file, "{value}").map_err(|e| e.to_string())
}

/// Valid step names of [`harness_await`] (file names under the result dir).
pub fn valid_step(step: &str) -> bool {
    !step.is_empty()
        && step.len() <= 64
        && step
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Wait budget of [`harness_await`]: the page's `timeoutMs`, not a 120 s clip (memory windows are 1800 s).
pub fn harness_await_wait(timeout_ms: u64) -> Duration {
    Duration::from_millis(timeout_ms)
}

/// Page asks the parent to act on `step` and waits for `<result>.ack-<step>` (its JSON body is
/// returned). The request, with the page's `detail` (e.g. the confirmed pane), is recorded as
/// `<result>.want-<step>`.
#[tauri::command]
pub async fn harness_await(
    state: tauri::State<'_, ReportSink>,
    step: String,
    detail: serde_json::Value,
    timeout_ms: u64,
) -> Result<serde_json::Value, String> {
    if !valid_step(&step) {
        return Err(format!("invalid step {step:?}"));
    }
    let want = state.path.with_extension(format!("want-{step}"));
    let ack = state.path.with_extension(format!("ack-{step}"));
    let tmp = want.with_extension("tmp");
    std::fs::write(&tmp, detail.to_string()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &want).map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now() + harness_await_wait(timeout_ms);
    loop {
        if let Ok(raw) = std::fs::read_to_string(&ack) {
            if let Ok(value) = serde_json::from_str(&raw) {
                return Ok(value);
            }
        }
        if std::time::Instant::now() > deadline {
            return Err(format!("parent did not acknowledge step {step}"));
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
}

/// Hides or shows the real window and answers with the visibility the window reports back.
///
/// This is the state `src/shell/interest.ts` reacts to: an unmapped window makes WebKit set
/// `document.visibilityState = "hidden"` and Tauri report `is_visible() == false`, and the product
/// then drops the surface lease (`setInterest(false)`), so the engine sends no frame. The page
/// still has to observe the state itself; this answer is not proof for it.
#[tauri::command]
pub fn harness_window_visible(app: tauri::AppHandle, visible: bool) -> Result<bool, String> {
    let window = app
        .get_webview_window("main")
        .ok_or("harness_window_visible: no main window")?;
    if visible {
        window.show()
    } else {
        window.hide()
    }
    .map_err(|e| e.to_string())?;
    window.is_visible().map_err(|e| e.to_string())
}

/// Page → harness report; `done` ends the window process (exit 1 when `error` is set).
#[tauri::command]
pub fn harness_report(
    app: tauri::AppHandle,
    state: tauri::State<'_, ReportSink>,
    report: serde_json::Value,
    done: bool,
) -> Result<(), String> {
    write_atomic(&state.path, &report)?;
    append_jsonl(&state.path.with_extension("jsonl"), &report)?;
    if done && !state.finished.swap(true, Ordering::AcqRel) {
        let failed = report.get("error").is_some_and(|e| !e.is_null());
        eprintln!("fidelity harness: phase reported done (error: {failed})");
        app.exit(if failed { 1 } else { 0 });
    }
    Ok(())
}

fn harness_handler() -> impl Fn(tauri::ipc::Invoke<Wry>) -> bool + Send + Sync + 'static {
    tauri::generate_handler![harness_report, harness_await, harness_window_visible]
}

pub struct WindowPhase {
    pub phase: String,
    pub params: serde_json::Value,
    pub report_path: PathBuf,
    pub timeout: Duration,
    pub config: DesktopConfig,
}

fn absolute(env: &dyn Fn(&str) -> Option<String>, key: &str) -> Result<PathBuf, String> {
    let value = env(key)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| format!("{key} must be set by the flow"))?;
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(format!("{key} must be absolute"));
    }
    Ok(path)
}

/// Desktop configuration of a window phase, only from variables the flow set explicitly.
///
/// The window runs with a private HOME/XDG tree, so the engine config dir (where the disposable
/// session's sockets live) is passed explicitly instead of derived. Preferences and the engine
/// state dir used for profile import are private to the run. No session → error, never the
/// empty state or the default session.
pub fn desktop_config(env: &dyn Fn(&str) -> Option<String>) -> Result<DesktopConfig, String> {
    let session_raw = env("HERDR_DESKTOP_E2E_SESSION")
        .ok_or("HERDR_DESKTOP_E2E_SESSION must be set by the flow")?;
    let session = session::disposable(&session_raw)?;
    let config_dir = absolute(env, "HERDR_DESKTOP_E2E_HERDR_CONFIG_DIR")?;
    let prefs_dir = absolute(env, "HERDR_DESKTOP_E2E_PREFS_DIR")?;
    let state_dir = absolute(env, "HERDR_DESKTOP_E2E_STATE_DIR")?;
    let herdr_bin = absolute(env, "HERDR_DESKTOP_HERDR_BIN")?;
    for (key, dir) in [("PREFS", &prefs_dir), ("STATE", &state_dir)] {
        if dir.starts_with(&config_dir) || config_dir.starts_with(dir) {
            return Err(format!(
                "HERDR_DESKTOP_E2E_{key}_DIR overlaps the engine config dir"
            ));
        }
    }
    let surface_trace = match env("HERDR_DESKTOP_E2E_SURFACE_TRACE") {
        Some(_) => Some(absolute(env, "HERDR_DESKTOP_E2E_SURFACE_TRACE")?),
        None => None,
    };
    let isolated_ssh = match (
        env("HERDR_DESKTOP_E2E_SSH_IDENTITY"),
        env("HERDR_DESKTOP_E2E_SSH_KNOWN_HOSTS"),
    ) {
        (None, None) => None,
        (Some(_), Some(_)) => Some(IsolatedSshConfig {
            identity_file: absolute(env, "HERDR_DESKTOP_E2E_SSH_IDENTITY")?,
            user_known_hosts_file: absolute(env, "HERDR_DESKTOP_E2E_SSH_KNOWN_HOSTS")?,
        }),
        _ => return Err("SSH identity and known hosts must be set together".into()),
    };
    Ok(DesktopConfig {
        bootstrap: BootstrapConfig {
            config_dir,
            session: Some(session),
            // The 007 harness owns its sessions: the window never starts one.
            auto_start: false,
            surface_trace,
            herdr_bin,
        },
        prefs_dir: Some(prefs_dir),
        herdr_state_dir: state_dir,
        isolated_ssh,
        geometry: DEFAULT_GEOMETRY,
    })
}

/// Runs the composed window until the page reports `done` (exit 0/1) or `timeout` (exit 3).
pub fn run_app_window(context: tauri::Context<Wry>, window: WindowPhase) {
    let WindowPhase {
        phase,
        params,
        report_path,
        timeout,
        config,
    } = window;
    let timeout_path = report_path.clone();
    // Bounded WebKit settings diagnostic: only `resource-bench` with a closed `webkit_policy`;
    // without the key (every standard native phase) no hook is armed.
    let policy = webkit_policy::selection(&phase, &params)
        .unwrap_or_else(|e| panic!("harness window refused params: {e}"));
    let mut builder = tauri::plugin::Builder::<Wry, ()>::new("fidelity-harness")
        .js_init_script(selection_script(&phase, &params));
    if let Some(policy) = policy {
        let record = webkit_policy::record_path(&report_path);
        let started = std::time::Instant::now();
        let mut count = 0usize;
        builder = builder.on_webview_ready(move |webview| {
            count += 1;
            let (record, label, n) = (record.clone(), webview.label().to_owned(), count);
            let elapsed = started.elapsed().as_millis();
            let fallback = record.clone();
            let applied = webview.with_webview(move |platform| {
                let value =
                    webkit_policy::apply(&platform.inner(), policy, &label, n, elapsed);
                if let Err(e) = write_atomic(&record, &value) {
                    eprintln!("fidelity harness: webkit policy record: {e}");
                }
            });
            if let Err(e) = applied {
                let _ = write_atomic(
                    &fallback,
                    &serde_json::json!({ "variant": policy.name(), "errors": [format!("with_webview: {e}")] }),
                );
            }
        });
    }
    let plugin = builder
        .setup(move |app, _api| {
            let handle = app.clone();
            std::thread::Builder::new()
                .name("fidelity-harness-timeout".into())
                .spawn(move || {
                    std::thread::sleep(timeout);
                    let state = handle.state::<ReportSink>();
                    if !state.finished.swap(true, Ordering::AcqRel) {
                        let previous = std::fs::read_to_string(&timeout_path).ok();
                        let _ = write_atomic(
                            &timeout_path,
                            &serde_json::json!({
                                "error": format!("harness timeout after {timeout:?}"),
                                "last_report": previous,
                            }),
                        );
                        eprintln!("fidelity harness: timeout after {timeout:?}");
                        handle.exit(3);
                    }
                })?;
            Ok(())
        })
        .build();
    let product = herdr_desktop::handler::<Wry>();
    let harness = harness_handler();
    let result = herdr_desktop::configure(tauri::Builder::default(), config)
        .any_thread()
        .manage(ReportSink {
            path: report_path,
            finished: AtomicBool::new(false),
        })
        .plugin(plugin)
        .invoke_handler(move |invoke| match route(invoke.message.command()) {
            Route::Harness => harness(invoke),
            Route::Product => product(invoke),
        })
        .run(context);
    if let Err(error) = result {
        panic!("composed harness window failed: {error}");
    }
}
