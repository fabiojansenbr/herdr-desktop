//! Native feature harness shared by the feature E2E suites (002 onwards).
//!
//! Included by an integration test with
//! `#[path = "../../scripts/feature-harness/native.rs"] mod native_harness;` so a feature
//! can prove its backend in real processes without touching the final composition
//! (`src-tauri/src/lib.rs`, owned by 007).
//!
//! A "GUI restart" is modelled as a fresh child process of the same test binary: the parent
//! test re-executes itself selecting one phase test, passes the phase name and a result
//! path through the environment and reads the JSON the child wrote. Nothing survives
//! between phases except what the feature persisted on disk and the engine itself.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Environment variable carrying the phase name to the child process.
pub const PHASE_ENV: &str = "HERDR_DESKTOP_HARNESS_PHASE";
/// Environment variable carrying the path where the child writes its JSON result.
pub const RESULT_ENV: &str = "HERDR_DESKTOP_HARNESS_RESULT";

/// Reads a variable the gate must set. Missing variables fail the test; they never skip it.
pub fn required(name: &str) -> String {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("{name} must be set by the gate (scripts/check-spec.mjs)"))
}

/// Result of one phase run in its own process.
#[derive(Debug)]
pub struct PhaseRun {
    pub pid: u32,
    pub result: serde_json::Value,
    pub stderr: String,
}

/// Runs `phase_test` (an ignored test of the current binary) in a new process with the
/// given phase name and extra environment. Panics with the child's output if it fails or
/// does not write a JSON result.
pub fn run_phase(phase_test: &str, phase: &str, work_dir: &Path, env: &[(&str, &str)]) -> PhaseRun {
    let exe = std::env::current_exe().expect("current test executable");
    let result_path: PathBuf = work_dir.join(format!("phase-{phase}.json"));
    let _ = std::fs::remove_file(&result_path);
    let mut command = Command::new(exe);
    command
        .args([
            phase_test,
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(PHASE_ENV, phase)
        .env(RESULT_ENV, &result_path);
    for (key, value) in env {
        command.env(key, value);
    }
    let child = command.spawn().expect("spawn phase process");
    let pid = child.id();
    let output = child.wait_with_output().expect("wait phase process");
    let stderr = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "phase {phase} failed ({}):\n{stderr}",
        output.status
    );
    let raw = std::fs::read_to_string(&result_path)
        .unwrap_or_else(|e| panic!("phase {phase} wrote no result ({e}):\n{stderr}"));
    let result = serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("phase {phase} result is not JSON ({e}): {raw}"));
    PhaseRun {
        pid,
        result,
        stderr,
    }
}

/// Inside a phase process: the phase name, or a panic when run outside the harness.
pub fn current_phase() -> String {
    required(PHASE_ENV)
}

/// Inside a phase process: writes the JSON result for the parent.
pub fn write_phase_result(value: &serde_json::Value) {
    let path = required(RESULT_ENV);
    std::fs::write(&path, serde_json::to_vec_pretty(value).expect("phase JSON"))
        .expect("write phase result");
}

/// True while `pid` names a live process (Linux `/proc`).
#[cfg(target_os = "linux")]
pub fn process_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}
