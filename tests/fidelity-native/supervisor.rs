//! Supervisor of the private display run of `e2e_fidelity_flow` (Linux only).
//!
//! One private session bus (`dbus-daemon --session`, address inside the private runtime dir)
//! is shared by sway, fcitx5 and the composed window. Every process is spawned from an empty
//! environment ([`super::display::Launch`]), recorded with its `/proc/<pid>/stat` starttime and
//! is the only thing [`Supervisor::cleanup`] may signal. Leftovers are searched by the private
//! `XDG_RUNTIME_DIR` in `/proc/*/environ` and reported (a leftover fails the run).

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use super::display::{Launch, PrivateDisplay};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tracked {
    pub name: String,
    pub pid: u32,
    pub starttime: u64,
}

/// Field 22 of `/proc/<pid>/stat` (after the parenthesised comm, which may contain spaces).
pub fn starttime(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_starttime(&stat)
}

pub fn parse_starttime(stat: &str) -> Option<u64> {
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(19)?.parse().ok()
}

/// A tracked PID still names the same process (PID reuse is not mistaken for ours).
pub fn same_process(tracked: &Tracked, current_starttime: Option<u64>) -> bool {
    current_starttime == Some(tracked.starttime)
}

/// Whether a process environment (`/proc/<pid>/environ`, NUL separated) belongs to the run.
pub fn environ_in_runtime(environ: &[u8], runtime: &Path) -> bool {
    let needle = format!("XDG_RUNTIME_DIR={}", runtime.display());
    environ.split(|b| *b == 0).any(|kv| kv == needle.as_bytes())
}

pub struct Supervisor {
    pub display: PrivateDisplay,
    pub out: PathBuf,
    pub bus: String,
    pub wayland: PathBuf,
    pub tracked: Vec<Tracked>,
    children: Vec<(String, Child)>,
    log: Vec<String>,
}

fn wait_until(
    what: &str,
    timeout: Duration,
    mut probe: impl FnMut() -> bool,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if probe() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(format!("timed out waiting for {what}"))
}

impl Supervisor {
    /// Starts the private bus and sway; returns once the Wayland socket exists.
    pub fn start(display: PrivateDisplay, out: PathBuf) -> Result<Self, String> {
        std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        for sub in ["config", "data", "cache", "state"] {
            std::fs::create_dir_all(display.home.join(sub)).map_err(|e| e.to_string())?;
        }
        let mut sup = Self {
            display,
            out,
            bus: String::new(),
            wayland: PathBuf::new(),
            tracked: Vec::new(),
            children: Vec::new(),
            log: Vec::new(),
        };
        if let Err(e) = sup.start_bus().and_then(|_| sup.start_sway()) {
            sup.cleanup();
            return Err(e);
        }
        Ok(sup)
    }

    /// Creates an unstarted supervisor struct for testing harness contracts.
    pub fn for_test(display: PrivateDisplay, out: PathBuf, bus: String, wayland: PathBuf) -> Self {
        Self {
            display,
            out,
            bus,
            wayland,
            tracked: Vec::new(),
            children: Vec::new(),
            log: Vec::new(),
        }
    }

    fn record(&mut self, name: &str, child: Child) -> Result<u32, String> {
        let pid = child.id();
        let st = starttime(pid).ok_or_else(|| format!("{name} exited immediately"))?;
        self.tracked.push(Tracked {
            name: name.into(),
            pid,
            starttime: st,
        });
        self.log
            .push(format!("spawn {name} pid={pid} starttime={st}"));
        self.children.push((name.into(), child));
        Ok(pid)
    }

    fn with_bus(&self, mut launch: Launch) -> Launch {
        launch
            .env
            .push(("DBUS_SESSION_BUS_ADDRESS".into(), self.bus.clone()));
        launch
    }

    fn logged(&self, launch: &Launch, name: &str) -> Result<Command, String> {
        let mut command = launch.command();
        let file = |suffix: &str| {
            std::fs::File::create(self.out.join(format!("{name}.{suffix}")))
                .map_err(|e| e.to_string())
        };
        command
            .stdin(Stdio::null())
            .stdout(file("out.log")?)
            .stderr(file("err.log")?);
        Ok(command)
    }

    fn start_bus(&mut self) -> Result<(), String> {
        let socket = self.display.runtime_dir.join("bus");
        let mut child = Command::new("/usr/bin/dbus-daemon")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("XDG_RUNTIME_DIR", &self.display.runtime_dir)
            .env("HOME", &self.display.home)
            .args([
                "--session".to_owned(),
                "--nofork".to_owned(),
                "--print-address=1".to_owned(),
                format!("--address=unix:path={}", socket.display()),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("dbus-daemon: {e}"))?;
        let mut line = String::new();
        let stdout = child.stdout.take().ok_or("dbus-daemon stdout")?;
        BufReader::new(stdout)
            .read_line(&mut line)
            .map_err(|e| e.to_string())?;
        self.record("dbus-daemon", child)?;
        let address = line.trim().to_owned();
        if !address.contains(&*self.display.runtime_dir.to_string_lossy()) {
            return Err(format!("private bus address outside runtime: {address}"));
        }
        self.bus = address;
        Ok(())
    }

    fn start_sway(&mut self) -> Result<(), String> {
        let launch = self.with_bus(self.display.sway());
        let child = self
            .logged(&launch, "sway")?
            .spawn()
            .map_err(|e| format!("sway: {e}"))?;
        self.record("sway", child)?;
        let runtime = self.display.runtime_dir.clone();
        let mut found = None;
        wait_until("sway wayland socket", Duration::from_secs(15), || {
            found = std::fs::read_dir(&runtime).ok().and_then(|entries| {
                entries.flatten().map(|e| e.path()).find(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("wayland-") && !n.ends_with(".lock"))
                })
            });
            found.is_some()
        })?;
        self.wayland = found.expect("socket");
        Ok(())
    }

    /// Private fcitx5 with the prepared Pinyin profile copied into the private XDG config.
    pub fn start_fcitx5(&mut self) -> Result<(), String> {
        let src = &self.display.resources.fcitx5_config;
        let dst = self.display.home.join("config/fcitx5");
        std::fs::create_dir_all(dst.join("conf")).map_err(|e| e.to_string())?;
        for (from, to) in [
            ("profile", "profile"),
            ("keyboard.conf", "conf/keyboard.conf"),
            ("conf/pinyin.conf", "conf/pinyin.conf"),
        ] {
            std::fs::copy(src.join(from), dst.join(to)).map_err(|e| format!("{from}: {e}"))?;
        }
        let mut launch = self.with_bus(self.display.fcitx5());
        launch
            .env
            .push(("WAYLAND_DISPLAY".into(), self.wayland.display().to_string()));
        let child = self
            .logged(&launch, "fcitx5")?
            .spawn()
            .map_err(|e| format!("fcitx5: {e}"))?;
        self.record("fcitx5", child)?;
        std::thread::sleep(Duration::from_secs(2));
        Ok(())
    }

    /// The composed window: this test binary selecting exactly `fidelity_window_phase`.
    pub fn start_window(&mut self, exe: &Path, extra: &[(&str, &str)]) -> Result<u32, String> {
        let mut launch = self.display.window(exe, &self.wayland, true, extra)?;
        launch.args = [
            "fidelity_window_phase",
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ]
        .map(str::to_owned)
        .to_vec();
        launch.env.push((
            "HERDR_DESKTOP_E2E_RUNTIME".into(),
            self.display.runtime_dir.display().to_string(),
        ));
        let launch = self.with_bus(launch);
        let child = self
            .logged(&launch, "window")?
            .spawn()
            .map_err(|e| format!("window: {e}"))?;
        self.record("window", child)
    }

    /// Exit status of a tracked child once it ended (`None` while running).
    pub fn try_exit(&mut self, name: &str) -> Option<std::process::ExitStatus> {
        self.children
            .iter_mut()
            .find(|(n, _)| n == name)
            .and_then(|(_, c)| c.try_wait().ok().flatten())
    }

    /// Real key events on the private compositor; returns wtype's exit status.
    pub fn wtype(&mut self, keys: &[String]) -> Result<(), String> {
        let launch = self.with_bus(self.display.wtype(&self.wayland, keys)?);
        let status = launch
            .command()
            .stdin(Stdio::null())
            .status()
            .map_err(|e| format!("wtype: {e}"))?;
        self.log.push(format!("wtype {keys:?} -> {status}"));
        if status.success() {
            Ok(())
        } else {
            Err(format!("wtype {keys:?}: {status}"))
        }
    }

    /// Screenshot of the private output only.
    pub fn grim(&mut self, name: &str) -> Result<PathBuf, String> {
        let path = self.out.join(format!("{name}.png"));
        let launch = Launch {
            program: PathBuf::from("/usr/bin/grim"),
            args: vec![path.display().to_string()],
            env: {
                let mut env = self.display.wtype(&self.wayland, &[])?.env;
                env.push(("DBUS_SESSION_BUS_ADDRESS".into(), self.bus.clone()));
                env
            },
        };
        let status = launch.command().status().map_err(|e| e.to_string())?;
        self.log
            .push(format!("grim {} -> {status}", path.display()));
        status
            .success()
            .then_some(path)
            .ok_or_else(|| format!("grim {name}: {status}"))
    }

    /// Instrumented wtype typing 'a' with HD_LATENCY_LOG, bounded by deadline and killed on timeout.
    pub fn latency_wtype(&mut self, bin: &Path, log: &Path) -> Result<(), String> {
        if log.exists() {
            return Err(format!("latency log {} already exists", log.display()));
        }
        let launch = latency_wtype_launch(&self.display, &self.wayland, &self.bus, bin, log)?;
        let mut child = launch
            .command()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("latency_wtype: {e}"))?;
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut exited = false;
        while Instant::now() < deadline {
            if let Ok(Some(status)) = child.try_wait() {
                exited = true;
                if !status.success() {
                    return Err(format!("latency_wtype: {status}"));
                }
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if !exited {
            let _ = child.kill();
            let _ = child.wait();
            return Err("latency_wtype timed out waiting for process to exit".into());
        }
        if !log.exists() {
            return Err(format!("latency log {} was not created", log.display()));
        }
        self.log.push("latency_wtype -> ok".to_string());
        Ok(())
    }

    /// Screenshots the private output in PPM format to stdout, bracketed by monotonic clock readings.
    pub fn grim_ppm(&mut self) -> Result<(Vec<u8>, i64, i64), String> {
        let launch = grim_ppm_launch(&self.display, &self.wayland, &self.bus)?;
        let start_ns = monotonic_now_ns();
        let output = launch
            .command()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| format!("grim ppm: {e}"))?;
        let end_ns = monotonic_now_ns();
        if !output.status.success() {
            return Err(format!(
                "grim ppm: {} {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok((output.stdout, start_ns, end_ns))
    }

    fn swaymsg(&self, extra: &[&str]) -> Result<std::process::Output, String> {
        let sway_pid = self
            .tracked
            .iter()
            .find(|t| t.name == "sway")
            .map(|t| t.pid)
            .ok_or("sway is not tracked")?;
        let uid = unsafe { getuid() };
        let socket = sway_ipc_socket(&self.display.runtime_dir, uid, sway_pid)?;
        if !socket.exists() {
            return Err(format!(
                "sway ipc socket {} does not exist",
                socket.display()
            ));
        }
        let swaymsg = self.display.resources.sway_prefix.join("usr/bin/swaymsg");
        Command::new(&swaymsg)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env(
                "LD_LIBRARY_PATH",
                self.display.resources.sway_prefix.join("usr/lib"),
            )
            .arg("-s")
            .arg(&socket)
            .args(extra)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("swaymsg: {e}"))
    }

    /// Queries the private sway compositor for its node tree via `swaymsg -t get_tree -r`.
    pub fn get_tree(&mut self) -> Result<serde_json::Value, String> {
        let output = self.swaymsg(&["-t", "get_tree", "-r"])?;
        if !output.status.success() {
            return Err(format!(
                "swaymsg get_tree: {} {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        serde_json::from_slice(&output.stdout).map_err(|e| format!("swaymsg get_tree json: {e}"))
    }

    /// Gives compositor keyboard focus to the measured window (`swaymsg [pid=N] focus`).
    pub fn focus_window(&mut self, pid: u32) -> Result<(), String> {
        if pid == 0 {
            return Err("focus_window: pid 0".into());
        }
        let [msg] = focus_window_args(pid);
        let output = self.swaymsg(&[msg.as_str()])?;
        if !output.status.success() {
            return Err(format!(
                "swaymsg {msg}: {} {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        self.log.push(format!("swaymsg {msg} -> ok"));
        Ok(())
    }

    /// Unix sockets of the tracked processes (`ss -xpn`), for the isolation evidence.
    pub fn isolation(&self) -> String {
        let output = Command::new("ss")
            .args(["-xpn"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        let pids: Vec<String> = self
            .tracked
            .iter()
            .map(|t| format!("pid={},", t.pid))
            .collect();
        output
            .lines()
            .filter(|l| pids.iter().any(|p| l.contains(p.as_str())))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Terminates only tracked processes (starttime checked), then scans for leftovers of the
    /// private runtime. Returns the log lines; leftover lines start with `LEFTOVER`.
    pub fn cleanup(&mut self) -> Vec<String> {
        for signal in ["-TERM", "-KILL"] {
            for tracked in self.tracked.iter().rev() {
                if same_process(tracked, starttime(tracked.pid)) {
                    let _ = Command::new("kill")
                        .args([signal, &tracked.pid.to_string()])
                        .status();
                    self.log.push(format!(
                        "kill {signal} {} pid={}",
                        tracked.name, tracked.pid
                    ));
                }
            }
            let _ = wait_until("tracked exit", Duration::from_secs(3), || {
                for (_, child) in self.children.iter_mut() {
                    let _ = child.try_wait();
                }
                self.tracked
                    .iter()
                    .all(|t| !same_process(t, starttime(t.pid)))
            });
        }
        for (_, child) in self.children.iter_mut() {
            let _ = child.try_wait();
        }
        let runtime = self.display.runtime_dir.clone();
        let mut leftovers = Vec::new();
        if let Ok(entries) = std::fs::read_dir("/proc") {
            for entry in entries.flatten() {
                let Some(pid) = entry
                    .file_name()
                    .to_str()
                    .and_then(|s| s.parse::<u32>().ok())
                else {
                    continue;
                };
                if pid == std::process::id() {
                    continue;
                }
                if let Ok(environ) = std::fs::read(entry.path().join("environ")) {
                    if environ_in_runtime(&environ, &runtime) {
                        let cmd = std::fs::read(entry.path().join("cmdline")).unwrap_or_default();
                        leftovers.push(format!(
                            "LEFTOVER pid={pid} {}",
                            String::from_utf8_lossy(&cmd).replace('\0', " ")
                        ));
                    }
                }
            }
        }
        if leftovers.is_empty() {
            self.log.push(format!(
                "no process left with XDG_RUNTIME_DIR={}",
                runtime.display()
            ));
        }
        self.log.extend(leftovers);
        let _ = std::fs::remove_dir_all(&runtime);
        self.log
            .push(format!("runtime dir removed: {}", !runtime.exists()));
        self.log.clone()
    }
}

#[repr(C)]
struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
}

extern "C" {
    fn clock_gettime(clock: i32, ts: *mut Timespec) -> i32;
    fn getuid() -> u32;
}

pub fn monotonic_now_ns() -> i64 {
    let mut ts = Timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let rc = unsafe { clock_gettime(1, &mut ts) };
    assert_eq!(rc, 0, "clock_gettime");
    ts.tv_sec * 1_000_000_000 + ts.tv_nsec
}

/// Criteria + command that focuses the window with `pid` on the private compositor.
pub fn focus_window_args(pid: u32) -> [String; 1] {
    [format!("[pid={pid}] focus")]
}

/// Builds the private sway-ipc socket path inside `runtime_dir` and validates it.
pub fn sway_ipc_socket(runtime_dir: &Path, uid: u32, sway_pid: u32) -> Result<PathBuf, String> {
    if !runtime_dir.is_absolute() {
        return Err(format!(
            "runtime dir {} is not absolute",
            runtime_dir.display()
        ));
    }
    let name = format!("sway-ipc.{uid}.{sway_pid}.sock");
    let socket = runtime_dir.join(name);
    if socket.parent() != Some(runtime_dir) {
        return Err(format!("socket {} is not in runtime dir", socket.display()));
    }
    Ok(socket)
}

/// Prepares launch for latency instrumented wtype.
pub fn latency_wtype_launch(
    display: &PrivateDisplay,
    wayland: &Path,
    bus: &str,
    bin: &Path,
    log: &Path,
) -> Result<Launch, String> {
    let mut launch = display.wtype(wayland, &["a".to_owned()])?;
    launch.program = bin.to_path_buf();
    launch
        .env
        .push(("DBUS_SESSION_BUS_ADDRESS".into(), bus.to_string()));
    launch
        .env
        .push(("HD_LATENCY_LOG".into(), log.display().to_string()));
    Ok(launch)
}

/// Prepares launch for grim PPM screenshot to stdout.
pub fn grim_ppm_launch(
    display: &PrivateDisplay,
    wayland: &Path,
    bus: &str,
) -> Result<Launch, String> {
    let mut env = display.wtype(wayland, &[])?.env;
    env.push(("DBUS_SESSION_BUS_ADDRESS".into(), bus.to_string()));
    Ok(Launch {
        program: PathBuf::from("/usr/bin/grim"),
        args: vec![
            "-t".into(),
            "ppm".into(),
            "-o".into(),
            "HEADLESS-1".into(),
            "-".into(),
        ],
        env,
    })
}
