// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::atomic::{AtomicBool, Ordering};

use herdr_client::bootstrap::{bootstrap_from_env, process_env, FATAL_LINE};

/// Set once the window host is running; panics before that are start-up failures.
static RUNNING: AtomicBool = AtomicBool::new(false);

fn main() {
    // A panic never prints a payload or a backtrace: only the literal line and a code.
    std::panic::set_hook(Box::new(|_info| {
        if RUNNING.load(Ordering::Acquire) {
            eprintln!("herdr-desktop: erro interno (unexpected_panic)");
        } else {
            eprintln!("{FATAL_LINE} (unexpected_panic)");
        }
    }));

    let config = match bootstrap_from_env(&process_env) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{}", error.fatal_line());
            std::process::exit(error.exit_code());
        }
    };

    RUNNING.store(true, Ordering::Release);
    if herdr_desktop::run(config).is_err() {
        RUNNING.store(false, Ordering::Release);
        eprintln!("{FATAL_LINE} (window_host_failed)");
        std::process::exit(2);
    }
}
