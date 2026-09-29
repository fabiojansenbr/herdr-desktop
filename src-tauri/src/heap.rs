//! Linux/glibc heap policy of the desktop host (spec 009, TASK-009-02).
//!
//! Measured cause (evidencias/009/base1-control.split.txt): in the idle snapshot of a 1-pane
//! window the `anon` category is 110.6 MiB of PSS and **all** of it is exclusive to the tree, so
//! it is the only large category a product change can move. Inside the window process, one
//! anonymous mapping of 63 072 kB holds 30 356 kB resident and fully `Private_Dirty`: the shape
//! of a glibc per-thread arena (the window runs 53 threads), which glibc grows and keeps.
//!
//! Three knobs, applied together as one arm because they address the same arena:
//!
//! - `M_ARENA_MAX`: how many arenas glibc may create. Fewer arenas means the same live bytes are
//!   packed into fewer 64 MiB heaps instead of leaving a partially used heap per thread.
//! - `M_TRIM_THRESHOLD`: an explicit threshold also disables glibc's dynamic adjustment, which
//!   otherwise raises the threshold after the first large `free` and stops returning pages.
//! - one `malloc_trim` after start-up, which returns the free pages of *every* arena to the OS.
//!
//! Nothing here is periodic: the trim runs once, so hidden panes keep their zero periodic
//! repaint and idle CPU (spec 007) is untouched. Everything is a no-op outside Linux/glibc.

use std::time::Duration;

/// Arenas glibc may create. 1 would serialize every thread on the main heap; 2 keeps a second
/// arena for the frame/IPC threads while removing the per-thread heaps.
pub const ARENA_MAX: i32 = 2;
/// Bytes of free top-of-heap glibc keeps before returning pages (glibc's own initial default).
/// Setting it explicitly is what pins the dynamic adjustment.
pub const TRIM_THRESHOLD: i32 = 128 * 1024;
/// Delay of the single start-up trim: after the window loaded and the first frames were painted,
/// and before any measurement window of the resource bench starts.
pub const STARTUP_TRIM_AFTER: Duration = Duration::from_secs(2);

/// The same policy for the processes WebKitGTK spawns from this one (`WebKitWebProcess`,
/// `WebKitNetworkProcess`): `mallopt` governs only this process, and those two carry 50.4 and
/// 13.5 MiB of exclusive anonymous memory of their own (evidencias/009/ab2-heap.split.txt).
/// glibc reads these at allocator start-up, so a child inherits them; this process already
/// initialized its own allocator and is unaffected.
pub const CHILD_ENV: [(&str, &str); 2] = [
    ("MALLOC_ARENA_MAX", "2"),
    ("MALLOC_TRIM_THRESHOLD_", "131072"),
];

/// What the host actually applied, recorded so a no-op is visible instead of assumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Applied {
    pub arena_max: bool,
    pub trim_threshold: bool,
}

impl Applied {
    pub fn all(&self) -> bool {
        self.arena_max && self.trim_threshold
    }
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
mod sys {
    /// `M_TRIM_THRESHOLD` of glibc `<malloc.h>`.
    const M_TRIM_THRESHOLD: i32 = -1;
    /// `M_ARENA_MAX` of glibc `<malloc.h>`.
    const M_ARENA_MAX: i32 = -8;

    unsafe extern "C" {
        /// glibc `mallopt`: 1 on success, 0 on failure.
        fn mallopt(param: i32, value: i32) -> i32;
        /// glibc `malloc_trim`: 1 if memory was released to the OS, 0 otherwise.
        fn malloc_trim(pad: usize) -> i32;
    }

    pub fn arena_max(value: i32) -> bool {
        unsafe { mallopt(M_ARENA_MAX, value) == 1 }
    }

    pub fn trim_threshold(value: i32) -> bool {
        unsafe { mallopt(M_TRIM_THRESHOLD, value) == 1 }
    }

    pub fn trim() -> bool {
        unsafe { malloc_trim(0) == 1 }
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
mod sys {
    pub fn arena_max(_value: i32) -> bool {
        false
    }
    pub fn trim_threshold(_value: i32) -> bool {
        false
    }
    pub fn trim() -> bool {
        false
    }
}

/// Applies the allocator policy. Call before the host spawns its threads: `mallopt` only governs
/// arenas created after it.
pub fn configure() -> Applied {
    Applied {
        arena_max: sys::arena_max(ARENA_MAX),
        trim_threshold: sys::trim_threshold(TRIM_THRESHOLD),
    }
}

/// Exports [`CHILD_ENV`] so the processes spawned later inherit the policy. A variable already
/// present in the environment is kept: the operator's value always wins, and the names that were
/// set by this call are returned.
pub fn export_child_policy() -> Vec<&'static str> {
    let mut exported = Vec::new();
    for (key, value) in CHILD_ENV {
        if std::env::var_os(key).is_none() {
            std::env::set_var(key, value);
            exported.push(key);
        }
    }
    exported
}

/// Returns the free pages of every arena to the OS. `true` when the allocator released memory.
pub fn trim_now() -> bool {
    sys::trim()
}

/// The body of the start-up trim, with its clock and allocator injected: sleeps once and trims
/// once, never in a loop.
pub fn run_startup_trim(
    after: Duration,
    sleep: impl FnOnce(Duration),
    trim: impl FnOnce() -> bool,
) -> bool {
    sleep(after);
    trim()
}

/// Spawns the single start-up trim on its own thread. A failure to spawn is not fatal: the host
/// keeps running with the untrimmed heap.
pub fn spawn_startup_trim(after: Duration) {
    let _ = std::thread::Builder::new()
        .name("herdr-desktop-trim".into())
        .spawn(move || {
            run_startup_trim(after, std::thread::sleep, trim_now);
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn the_startup_trim_sleeps_once_and_trims_once() {
        let slept = AtomicUsize::new(0);
        let trimmed = AtomicUsize::new(0);
        let released = run_startup_trim(
            Duration::from_millis(7),
            |d| {
                assert_eq!(d, Duration::from_millis(7));
                slept.fetch_add(1, Ordering::Relaxed);
            },
            || {
                trimmed.fetch_add(1, Ordering::Relaxed);
                true
            },
        );
        assert!(released);
        assert_eq!(slept.load(Ordering::Relaxed), 1);
        assert_eq!(trimmed.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn the_startup_trim_sleeps_before_it_trims() {
        let order = std::cell::RefCell::new(Vec::new());
        run_startup_trim(
            STARTUP_TRIM_AFTER,
            |_| order.borrow_mut().push("sleep"),
            || {
                order.borrow_mut().push("trim");
                false
            },
        );
        assert_eq!(*order.borrow(), ["sleep", "trim"]);
    }

    #[test]
    fn the_policy_values_are_the_measured_ones_and_nothing_is_periodic() {
        assert_eq!(ARENA_MAX, 2);
        assert_eq!(TRIM_THRESHOLD, 128 * 1024);
        assert_eq!(STARTUP_TRIM_AFTER, Duration::from_secs(2));
        let source = include_str!("heap.rs");
        let code: String = source
            .split("#[cfg(test)]")
            .next()
            .expect("module body")
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for periodic in ["loop {", "while ", "Instant::now", "set_interval"] {
            assert!(
                !code.contains(periodic),
                "the heap policy must stay one-shot, found {periodic:?}"
            );
        }
    }

    /// `set_var` is process-wide; these cases must not run beside each other.
    static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn the_child_policy_is_exported_for_the_webkit_processes() {
        let _guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let saved: Vec<_> = CHILD_ENV
            .iter()
            .map(|(k, _)| (*k, std::env::var_os(k)))
            .collect();
        for (key, _) in CHILD_ENV {
            std::env::remove_var(key);
        }
        assert_eq!(
            export_child_policy(),
            CHILD_ENV.map(|(k, _)| k).to_vec(),
            "every name must be exported when the environment has none"
        );
        for (key, value) in CHILD_ENV {
            assert_eq!(std::env::var(key).as_deref(), Ok(value));
        }
        assert!(
            export_child_policy().is_empty(),
            "a second call must not re-export what is already set"
        );
        for (key, value) in saved {
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
    }

    #[test]
    fn an_operator_value_is_never_overwritten() {
        let _guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let (key, _) = CHILD_ENV[0];
        let saved = std::env::var_os(key);
        std::env::set_var(key, "17");
        assert!(!export_child_policy().contains(&key));
        assert_eq!(std::env::var(key).as_deref(), Ok("17"));
        match saved {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }

    #[test]
    fn the_host_applies_the_policy_before_its_threads_and_trims_once_from_setup() {
        let lib = include_str!("lib.rs");
        let configure = lib
            .split("pub fn configure<R: Runtime>")
            .nth(1)
            .expect("configure");
        assert!(
            configure.contains("heap::configure()"),
            "configure must apply the allocator policy before the builder chain"
        );
        assert!(
            configure.contains("heap::export_child_policy()"),
            "configure must export the policy before WebKitGTK spawns its processes"
        );
        let install = lib
            .split("pub fn install<R: Runtime>")
            .nth(1)
            .expect("install");
        assert!(
            install.contains("heap::spawn_startup_trim(heap::STARTUP_TRIM_AFTER)"),
            "setup must schedule the single start-up trim"
        );
    }

    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    #[test]
    fn glibc_accepts_the_policy_and_the_trim_is_callable() {
        let applied = configure();
        assert!(applied.all(), "glibc refused the policy: {applied:?}");
        // `malloc_trim` answers whether it released pages; either answer is valid here, only the
        // call must be sound.
        let _ = trim_now();
    }

    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    #[test]
    fn the_policy_is_a_no_op_outside_linux_glibc() {
        assert_eq!(configure(), Applied::default());
        assert!(!trim_now());
    }
}
