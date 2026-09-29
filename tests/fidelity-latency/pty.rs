//! Raw log parsers (instrumented wtype, PTY helper) and a minimal PTY used to prove the helper
//! without a display. libc symbols come from the C library std links; no new dependency.

use super::clock::CLOCK_NAME;
use super::record::Press;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperEvent {
    pub byte: u8,
    pub count: u64,
    pub seq: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WtypeLog {
    pub press: Vec<Press>,
    pub released_ns: Vec<i64>,
    pub res_ns: i64,
}

fn fields<'a>(line: &'a str, prefix: &str, keys: &[&str]) -> Result<Vec<&'a str>, String> {
    let rest = line
        .strip_prefix(prefix)
        .ok_or(format!("unexpected line {line:?}"))?;
    let parts: Vec<&str> = rest.split(' ').collect();
    if parts.len() != keys.len() {
        return Err(format!("malformed line {line:?}"));
    }
    parts
        .iter()
        .zip(keys)
        .map(|(p, k)| {
            p.strip_prefix(k)
                .and_then(|v| v.strip_prefix('='))
                .ok_or(format!("missing {k} in {line:?}"))
        })
        .collect()
}

fn num<T: std::str::FromStr>(v: &str, line: &str) -> Result<T, String> {
    v.parse()
        .map_err(|_| format!("bad number {v:?} in {line:?}"))
}

pub fn parse_wtype_log(text: &str) -> Result<WtypeLog, String> {
    let keys = ["index", "keycode", "clock", "res_ns", "ns"];
    let mut log = WtypeLog {
        press: vec![],
        released_ns: vec![],
        res_ns: 0,
    };
    for line in text.lines() {
        let (event, f) = if let Ok(f) = fields(line, "hd-latency v1 press ", &keys) {
            ("press", f)
        } else {
            ("released", fields(line, "hd-latency v1 released ", &keys)?)
        };
        if f[2] != CLOCK_NAME {
            return Err(format!("foreign clock {:?}", f[2]));
        }
        let res: i64 = num(f[3], line)?;
        let ns: i64 = num(f[4], line)?;
        if res <= 0 || ns < 0 || (log.res_ns != 0 && log.res_ns != res) {
            return Err(format!("bad clock values in {line:?}"));
        }
        log.res_ns = res;
        if event == "press" {
            log.press.push(Press {
                clock: f[2].to_string(),
                index: num(f[0], line)?,
                keycode: num(f[1], line)?,
                ns,
            });
        } else {
            log.released_ns.push(ns);
        }
    }
    Ok(log)
}

/// Every received byte (expected or not); `ready` lines are skipped.
pub fn parse_helper_log(text: &str) -> Result<Vec<HelperEvent>, String> {
    let mut out = vec![];
    for line in text.lines() {
        if line.starts_with("hd-pty v1 ready ") {
            continue;
        }
        let f = fields(
            line,
            "hd-pty v1 recv ",
            &["byte", "count", "seq", "emitted", "ns"],
        )?;
        let byte = f[0]
            .strip_prefix("0x")
            .and_then(|h| u8::from_str_radix(h, 16).ok())
            .ok_or(format!("bad byte in {line:?}"))?;
        num::<i64>(f[4], line)?;
        out.push(HelperEvent {
            byte,
            count: num(f[1], line)?,
            seq: num(f[2], line)?,
        });
    }
    Ok(out)
}

#[repr(C)]
struct Winsize {
    row: u16,
    col: u16,
    xpixel: u16,
    ypixel: u16,
}

#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

extern "C" {
    fn posix_openpt(flags: i32) -> i32;
    fn grantpt(fd: i32) -> i32;
    fn unlockpt(fd: i32) -> i32;
    fn ptsname_r(fd: i32, buf: *mut u8, len: usize) -> i32;
    fn ioctl(fd: i32, request: u64, ...) -> i32;
    fn poll(fds: *mut PollFd, nfds: u64, timeout_ms: i32) -> i32;
    fn setsid() -> i32;
}

const O_RDWR: i32 = 2;
const O_NOCTTY: i32 = 0o400;
const TIOCSWINSZ: u64 = 0x5414;
const TIOCSCTTY: u64 = 0x540E;
const POLLIN: i16 = 1;

pub struct Pty {
    master: File,
    slave: Option<File>,
}

impl Pty {
    pub fn open(cols: u16, rows: u16) -> Result<Self, String> {
        // SAFETY: plain libc calls on a fd we own; buffers are sized and NUL-terminated.
        unsafe {
            let fd = posix_openpt(O_RDWR | O_NOCTTY);
            if fd < 0 || grantpt(fd) != 0 || unlockpt(fd) != 0 {
                return Err("posix_openpt".into());
            }
            let master = File::from_raw_fd(fd);
            let mut name = [0u8; 128];
            if ptsname_r(fd, name.as_mut_ptr(), name.len()) != 0 {
                return Err("ptsname_r".into());
            }
            let end = name.iter().position(|b| *b == 0).ok_or("ptsname")?;
            let path = std::str::from_utf8(&name[..end]).map_err(|e| e.to_string())?;
            let ws = Winsize {
                row: rows,
                col: cols,
                xpixel: 0,
                ypixel: 0,
            };
            if ioctl(fd, TIOCSWINSZ, &ws as *const Winsize) != 0 {
                return Err("TIOCSWINSZ".into());
            }
            let slave = OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(O_NOCTTY)
                .open(path)
                .map_err(|e| format!("open {path}: {e}"))?;
            Ok(Self {
                master,
                slave: Some(slave),
            })
        }
    }

    /// Child gets the slave as stdin/stdout/stderr and as controlling terminal.
    pub fn spawn(&mut self, cmd: &mut Command) -> Result<Child, String> {
        let slave = self.slave.take().ok_or("already spawned")?;
        let io = |f: &File| f.try_clone().map(Stdio::from).map_err(|e| e.to_string());
        cmd.stdin(io(&slave)?)
            .stdout(io(&slave)?)
            .stderr(io(&slave)?);
        // SAFETY: only async-signal-safe calls between fork and exec.
        unsafe {
            cmd.pre_exec(|| {
                if setsid() < 0 || ioctl(0, TIOCSCTTY, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = cmd.spawn().map_err(|e| e.to_string());
        drop(slave);
        child
    }

    pub fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.master.write_all(bytes).map_err(|e| e.to_string())
    }

    /// Exactly `n` bytes within `timeout_ms`, else Err with what arrived.
    pub fn read_exact_timeout(&mut self, n: usize, timeout_ms: i32) -> Result<Vec<u8>, String> {
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms as u64);
        let mut out = Vec::with_capacity(n);
        while out.len() < n {
            let left = deadline
                .saturating_duration_since(std::time::Instant::now())
                .as_millis() as i32;
            let mut p = PollFd {
                fd: self.master.as_raw_fd(),
                events: POLLIN,
                revents: 0,
            };
            // SAFETY: one valid pollfd.
            if left == 0 || unsafe { poll(&mut p, 1, left) } <= 0 {
                return Err(format!(
                    "timeout after {} bytes: {:?}",
                    out.len(),
                    String::from_utf8_lossy(&out)
                ));
            }
            let mut buf = vec![0u8; n - out.len()];
            let got = self.master.read(&mut buf).map_err(|e| e.to_string())?;
            if got == 0 {
                return Err("eof".into());
            }
            out.extend_from_slice(&buf[..got]);
        }
        Ok(out)
    }
}

use std::os::unix::fs::OpenOptionsExt;
