//! Private headless display for the native phases (keyboard, IME, clipboard, pointer).
//!
//! Same isolation as `docs/PREFLIGHT-NATIVE-INPUT.md` and `docs/PREFLIGHT-CJK-INPUT.md`, adapted
//! to the product window: every process starts from an empty environment (`env_clear`) with a
//! private HOME/XDG tree and runtime dir, under its own `dbus-run-session`; `WAYLAND_DISPLAY` is
//! the absolute path of the private sway socket, so it can never resolve to the user's display.
//! The prepared resources are used read-only; their own runners are not executed (they hardcode
//! the main checkout: `prep-cjk/inner.sh` BASE_PREFIX, `native-input/run.sh` RT under
//! `<main>/.local/nirt`, `probe-webkit/runner.py` NATIVE_INPUT_DIR).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

/// AF_UNIX `sun_path` limit including the terminator.
pub const SUN_PATH_MAX: usize = 108;

/// Variables that would reach the user's session if inherited.
pub const FORBIDDEN_INHERITED: [&str; 5] = [
    "DISPLAY",
    "SWAYSOCK",
    "HYPRLAND_INSTANCE_SIGNATURE",
    "I3SOCK",
    "WAYLAND_SOCKET",
];

/// Read-only resources prepared outside the worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resources {
    /// `.local/native-input/prefix` (sway 1.12 / wlroots 0.20 extracted, verified packages).
    pub sway_prefix: PathBuf,
    /// `.local/native-input/sway.conf` (no include/exec/bar).
    pub sway_config: PathBuf,
    /// `.local/prep-cjk/prefix` (fcitx5-chinese-addons, libime, opencc, marisa).
    pub cjk_prefix: PathBuf,
    /// `.local/prep-cjk/fcitx5` (profile with pinyin, keyboard.conf, conf/pinyin.conf).
    pub fcitx5_config: PathBuf,
}

impl Resources {
    pub fn under(local: &Path) -> Self {
        Self {
            sway_prefix: local.join("native-input/prefix"),
            sway_config: local.join("native-input/sway.conf"),
            cjk_prefix: local.join("prep-cjk/prefix"),
            fcitx5_config: local.join("prep-cjk/fcitx5"),
        }
    }

    /// Files the flow needs; missing ones are reported, never skipped.
    pub fn required_files(&self) -> Vec<PathBuf> {
        vec![
            self.sway_prefix.join("usr/bin/sway"),
            self.sway_config.clone(),
            self.cjk_prefix.join("usr/lib/fcitx5/libpinyin.so"),
            self.fcitx5_config.join("profile"),
            self.fcitx5_config.join("keyboard.conf"),
            self.fcitx5_config.join("conf/pinyin.conf"),
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivateDisplay {
    /// Private `XDG_RUNTIME_DIR` (mode 0700, short path).
    pub runtime_dir: PathBuf,
    /// Private HOME; XDG config/data/cache/state live below it.
    pub home: PathBuf,
    pub resources: Resources,
    pub user: String,
}

/// A process to spawn with exactly `env` (nothing inherited).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl Launch {
    pub fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.args).env_clear();
        for (k, v) in &self.env {
            command.env(k, v);
        }
        command
    }

    pub fn var(&self, key: &str) -> Option<&str> {
        self.env
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

impl PrivateDisplay {
    /// Validates the layout; `user_uid` identifies the user's runtime dir to refuse it.
    pub fn new(
        runtime_dir: PathBuf,
        home: PathBuf,
        resources: Resources,
        user: String,
        user_uid: u32,
    ) -> Result<Self, String> {
        validate_runtime_dir(&runtime_dir, user_uid)?;
        if !home.is_absolute() {
            return Err(format!("private HOME {} is not absolute", home.display()));
        }
        if home.starts_with(format!("/run/user/{user_uid}")) || home == Path::new("/") {
            return Err(format!("private HOME {} is not private", home.display()));
        }
        Ok(Self {
            runtime_dir,
            home,
            resources,
            user,
        })
    }

    fn base_env(&self) -> Vec<(String, String)> {
        let h = |sub: &str| self.home.join(sub).to_string_lossy().into_owned();
        vec![
            ("PATH".into(), "/usr/bin:/bin".into()),
            ("LANG".into(), "en_US.UTF-8".into()),
            ("USER".into(), self.user.clone()),
            ("HOME".into(), self.home.to_string_lossy().into_owned()),
            (
                "XDG_RUNTIME_DIR".into(),
                self.runtime_dir.to_string_lossy().into_owned(),
            ),
            ("XDG_CONFIG_HOME".into(), h("config")),
            ("XDG_DATA_HOME".into(), h("data")),
            ("XDG_CACHE_HOME".into(), h("cache")),
            ("XDG_STATE_HOME".into(), h("state")),
        ]
    }

    /// Wraps `inner` in its own session bus: `dbus-run-session -- program args`.
    pub fn with_private_bus(&self, inner: &Launch) -> Launch {
        let mut args = vec![
            "--".to_owned(),
            inner.program.to_string_lossy().into_owned(),
        ];
        args.extend(inner.args.iter().cloned());
        Launch {
            program: PathBuf::from("/usr/bin/dbus-run-session"),
            args,
            env: inner.env.clone(),
        }
    }

    pub fn sway(&self) -> Launch {
        let mut env = self.base_env();
        env.extend([
            ("WLR_BACKENDS".into(), "headless".into()),
            ("WLR_RENDERER".into(), "pixman".into()),
            ("WLR_HEADLESS_OUTPUTS".into(), "1".into()),
            (
                "LD_LIBRARY_PATH".into(),
                self.resources
                    .sway_prefix
                    .join("usr/lib")
                    .to_string_lossy()
                    .into_owned(),
            ),
        ]);
        Launch {
            program: self.resources.sway_prefix.join("usr/bin/sway"),
            args: vec![
                "-c".into(),
                self.resources.sway_config.to_string_lossy().into_owned(),
            ],
            env,
        }
    }

    /// Private fcitx5 with the Pinyin addon (config copied into the private XDG config first).
    pub fn fcitx5(&self) -> Launch {
        let mut env = self.base_env();
        let cjk = &self.resources.cjk_prefix;
        env.extend([
            (
                "FCITX_ADDON_DIRS".into(),
                format!("{}:/usr/lib/fcitx5", cjk.join("usr/lib/fcitx5").display()),
            ),
            (
                "XDG_DATA_DIRS".into(),
                format!("{}:/usr/share", cjk.join("usr/share").display()),
            ),
            (
                "LD_LIBRARY_PATH".into(),
                format!(
                    "{}:{}",
                    cjk.join("usr/lib").display(),
                    self.resources.sway_prefix.join("usr/lib").display()
                ),
            ),
        ]);
        Launch {
            program: PathBuf::from("/usr/bin/fcitx5"),
            args: vec![
                "--disable=xcb,xim,kimpanel,notificationitem,clipboard,fcitx4frontend,ibusfrontend,dbusfrontend,notifications,virtualkeyboard".into(),
            ],
            env,
        }
    }

    /// The window phase process (this test binary) on the private display.
    pub fn window(
        &self,
        exe: &Path,
        wayland_socket: &Path,
        ime: bool,
        extra: &[(&str, &str)],
    ) -> Result<Launch, String> {
        validate_wayland_socket(wayland_socket, &self.runtime_dir)?;
        let mut env = self.base_env();
        env.extend([
            (
                "WAYLAND_DISPLAY".into(),
                wayland_socket.to_string_lossy().into_owned(),
            ),
            ("GDK_BACKEND".into(), "wayland".into()),
            ("GIO_USE_VFS".into(), "local".into()),
            ("WEBKIT_DISABLE_DMABUF_RENDERER".into(), "1".into()),
            // Spec 067 (AC-067-04): the private display fixes `LANG=en_US.UTF-8`, so the window
            // is told which language to open in; the visual checks compare the Portuguese words.
            ("HERDR_DESKTOP_LOCALE".into(), "pt".into()),
        ]);
        if ime {
            env.push(("GTK_IM_MODULE".into(), "wayland".into()));
        }
        for (k, v) in extra {
            if env.iter().any(|(existing, _)| existing == k) {
                return Err(format!("extra variable {k} overrides the private display"));
            }
            env.push(((*k).into(), (*v).into()));
        }
        Ok(Launch {
            program: exe.to_path_buf(),
            args: Vec::new(),
            env,
        })
    }

    /// Real key events through `zwp_virtual_keyboard_v1` on the private compositor only.
    /// `-s 250` waits for `wl_keyboard.enter` (first key was dropped without it).
    pub fn wtype(&self, wayland_socket: &Path, keys: &[String]) -> Result<Launch, String> {
        validate_wayland_socket(wayland_socket, &self.runtime_dir)?;
        let mut env = self.base_env();
        env.push((
            "WAYLAND_DISPLAY".into(),
            wayland_socket.to_string_lossy().into_owned(),
        ));
        let mut args = vec!["-s".to_owned(), "250".to_owned()];
        args.extend(keys.iter().cloned());
        Ok(Launch {
            program: PathBuf::from("/usr/bin/wtype"),
            args,
            env,
        })
    }
}

pub fn validate_runtime_dir(dir: &Path, user_uid: u32) -> Result<(), String> {
    if !dir.is_absolute() {
        return Err(format!("runtime dir {} is not absolute", dir.display()));
    }
    if dir.starts_with(format!("/run/user/{user_uid}")) {
        return Err(format!("runtime dir {} is the user's", dir.display()));
    }
    let socket = dir.join("wayland-1");
    if socket.as_os_str().len() >= SUN_PATH_MAX {
        return Err(format!(
            "runtime dir {} too long for AF_UNIX sockets",
            dir.display()
        ));
    }
    Ok(())
}

pub fn validate_wayland_socket(socket: &Path, runtime_dir: &Path) -> Result<(), String> {
    if !socket.is_absolute() {
        return Err(format!(
            "WAYLAND_DISPLAY {} is not absolute",
            socket.display()
        ));
    }
    if socket.parent() != Some(runtime_dir) {
        return Err(format!(
            "WAYLAND_DISPLAY {} is not in the private runtime dir {}",
            socket.display(),
            runtime_dir.display()
        ));
    }
    let name = socket.file_name().and_then(OsStr::to_str).unwrap_or("");
    if !name.starts_with("wayland-") {
        return Err(format!("{} is not a wayland socket name", socket.display()));
    }
    Ok(())
}

/// Guard of the window process itself: refuses to open a window unless it runs on the private
/// display (absolute socket in `HERDR_DESKTOP_E2E_RUNTIME`) with nothing from the user's session.
pub fn guard_window_env(env: &dyn Fn(&str) -> Option<String>, user_uid: u32) -> Result<(), String> {
    for key in FORBIDDEN_INHERITED {
        if env(key).is_some_and(|v| !v.is_empty()) {
            return Err(format!("refusing: {key} is set"));
        }
    }
    let runtime = env("XDG_RUNTIME_DIR").ok_or("refusing: XDG_RUNTIME_DIR unset")?;
    let runtime = PathBuf::from(runtime);
    validate_runtime_dir(&runtime, user_uid)?;
    let expected =
        env("HERDR_DESKTOP_E2E_RUNTIME").ok_or("refusing: HERDR_DESKTOP_E2E_RUNTIME unset")?;
    if runtime != Path::new(&expected) {
        return Err("refusing: XDG_RUNTIME_DIR is not the flow's private runtime".into());
    }
    let display = env("WAYLAND_DISPLAY").ok_or("refusing: WAYLAND_DISPLAY unset")?;
    validate_wayland_socket(Path::new(&display), &runtime)?;
    if env("DBUS_SESSION_BUS_ADDRESS").as_deref()
        == Some(format!("unix:path=/run/user/{user_uid}/bus").as_str())
    {
        return Err("refusing: user session bus".into());
    }
    Ok(())
}
