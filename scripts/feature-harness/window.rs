//! Native window host for feature E2E phases (spec 002 onwards).
//!
//! A phase process opens a real Tauri window (WebView + the built frontend in `dist/`)
//! with the feature's real backend commands registered, injects the harness selection
//! (`window.__HERDR_HARNESS__ = { feature, phase, params }`) and lets the feature scenario
//! (`src/features/<feature>/e2e.ts`) drive the mounted component through the DOM. The page
//! reports through the `harness_report` command; a report with `done: true` closes the
//! window and ends the process (exit 0, or 1 when the report carries `error`).
//!
//! The final window composition (`src-tauri/src/lib.rs`) is not used or modified: each
//! feature test builds its own handler with `tauri::generate_handler!`, including
//! `harness_report`, and passes it to [`run_feature_window`].

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder, Wry};

/// Where the page's reports are written (set per phase by the parent test).
pub struct HarnessReportPath {
    path: PathBuf,
    finished: AtomicBool,
}

fn write_atomic(path: &std::path::Path, value: &serde_json::Value) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    std::fs::write(
        &tmp,
        serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// Page → host report. `done` closes the window and ends the phase process.
#[tauri::command]
pub fn harness_report(
    app: tauri::AppHandle,
    state: tauri::State<'_, HarnessReportPath>,
    report: serde_json::Value,
    done: bool,
) -> Result<(), String> {
    write_atomic(&state.path, &report)?;
    if done && !state.finished.swap(true, Ordering::AcqRel) {
        let code = if report.get("error").is_some_and(|e| !e.is_null()) {
            1
        } else {
            0
        };
        eprintln!("harness: phase reported done (exit {code})");
        app.exit(code);
    }
    Ok(())
}

/// Opens the harness window and runs the event loop until the page reports `done` or
/// `timeout` elapses (then a failure report is written and the process exits with 3).
pub fn run_feature_window(
    context: tauri::Context<Wry>,
    builder: tauri::Builder<Wry>,
    feature: &str,
    phase: &str,
    params: serde_json::Value,
    report_path: PathBuf,
    timeout: Duration,
) {
    let mut context = context;
    // The window is created below with the harness selection injected before any script runs.
    context.config_mut().app.windows.clear();
    let selection = serde_json::json!({ "feature": feature, "phase": phase, "params": params });
    let script = format!("window.__HERDR_HARNESS__ = Object.freeze({selection});");
    let title = format!("Herdr Desktop — harness {feature}/{phase}");
    let timeout_path = report_path.clone();
    let result = builder
        .any_thread()
        .manage(HarnessReportPath {
            path: report_path,
            finished: AtomicBool::new(false),
        })
        .setup(move |app| {
            WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title(title)
                .inner_size(1000.0, 760.0)
                .focused(false)
                .initialization_script(script)
                .build()?;
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(timeout);
                let state = handle.state::<HarnessReportPath>();
                if !state.finished.swap(true, Ordering::AcqRel) {
                    let previous = std::fs::read_to_string(&timeout_path).ok();
                    let _ = write_atomic(
                        &timeout_path,
                        &serde_json::json!({
                            "error": format!("harness timeout after {timeout:?}"),
                            "last_report": previous,
                        }),
                    );
                    eprintln!("harness: timeout after {timeout:?}");
                    handle.exit(3);
                }
            });
            Ok(())
        })
        .run(context);
    if let Err(error) = result {
        panic!("harness window failed: {error}");
    }
}
