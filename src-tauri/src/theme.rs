//! Omarchy terminal theme (spec 020): read `colors.toml` + `ghostty.conf`/`alacritty.toml`
//! from a directory, fall back field-by-field, and poll for live changes.
//!
//! The product path is `~/.local/state/omarchy/current/theme`. Tests inject a temp dir and
//! never consult that location.

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use herdr_client::protocol::wire::{
    ClientHostAppearance, ClientHostColor, ClientHostDefaultColorKind, ClientHostThemeUpdate,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime};

/// Commands this module exposes (kept in sync with `lib.rs` by the registry test).
pub const COMMANDS: &[&str] = &["theme_current"];

/// Product poll interval when `notify` is not a dependency.
pub const POLL_INTERVAL: Duration = Duration::from_secs(1);

const SOURCE_OMARCHY: &str = "omarchy";
const SOURCE_DEFAULT: &str = "default";

/// Neutral tokens of `src/app.css` (spec 061).
const PEN_SURFACE: &str = "#0A0A0A";
const PEN_SURFACE_2: &str = "#171717";
const PEN_SURFACE_3: &str = "#242424";
const PEN_BORDER: &str = "#1C1C1C";
const PEN_TEXT: &str = "#EDEDED";
const PEN_TEXT_MUTED: &str = "#A3A3A3";
const PEN_TEXT_DIM: &str = "#6B6B6B";
const PEN_ACCENT: &str = "#D4D4D4";
const PEN_WORKING: &str = "#4ADE80";
const PEN_ATTENTION: &str = "#E9A23B";
const PEN_ERROR: &str = "#F87171";
const PEN_IDLE: &str = "#6B6B6B";

/// `DEFAULT_THEME` of `src/terminal/colors.ts` — terminal fallback.
const TERM_FG: &str = "#EDEDED";
const TERM_BG: &str = "#0F0F0F";
const TERM_CURSOR: &str = "#EDEDED";
const TERM_NAMED: [&str; 16] = [
    "#1b1f24", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#abb2bf",
    "#5c6370", "#f27983", "#a9d48a", "#f0cc8a", "#7cbdf5", "#d38fe6", "#6fc7d3", "#ffffff",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeDto {
    pub source: String,
    pub mode: String,
    pub background: String,
    pub foreground: String,
    pub cursor: String,
    pub selection: String,
    pub named: Vec<String>,
    pub surface: String,
    pub surface_2: String,
    pub surface_3: String,
    pub border: String,
    pub text: String,
    pub text_muted: String,
    pub text_dim: String,
    pub accent: String,
    pub working: String,
    pub attention: String,
    pub error: String,
    pub idle: String,
}

/// Fallback dto: neutral terminal and UI tokens. `source` is `"default"`.
pub fn default_theme() -> ThemeDto {
    ThemeDto {
        source: SOURCE_DEFAULT.to_owned(),
        mode: "dark".to_owned(),
        background: TERM_BG.to_owned(),
        foreground: TERM_FG.to_owned(),
        cursor: TERM_CURSOR.to_owned(),
        selection: "#2A2A2A".to_owned(),
        named: TERM_NAMED.iter().map(|s| (*s).to_owned()).collect(),
        surface: PEN_SURFACE.to_owned(),
        surface_2: PEN_SURFACE_2.to_owned(),
        surface_3: PEN_SURFACE_3.to_owned(),
        border: PEN_BORDER.to_owned(),
        text: PEN_TEXT.to_owned(),
        text_muted: PEN_TEXT_MUTED.to_owned(),
        text_dim: PEN_TEXT_DIM.to_owned(),
        accent: PEN_ACCENT.to_owned(),
        working: PEN_WORKING.to_owned(),
        attention: PEN_ATTENTION.to_owned(),
        error: PEN_ERROR.to_owned(),
        idle: PEN_IDLE.to_owned(),
    }
}

/// `~/.local/state/omarchy/current/theme` when that path exists (file or symlink).
pub fn omarchy_theme_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let path = PathBuf::from(home).join(".local/state/omarchy/current/theme");
    fs::symlink_metadata(&path).ok()?;
    Some(path)
}

/// Load from `dir`, or the default theme when `dir` is `None` or unreadable.
pub fn load_theme_from(dir: Option<&Path>) -> ThemeDto {
    load_theme_with_warnings(dir).0
}

/// Same as [`load_theme_from`], plus one warning string per ignored malformed file.
pub fn load_theme_with_warnings(dir: Option<&Path>) -> (ThemeDto, Vec<String>) {
    let Some(dir) = dir else {
        return (default_theme(), Vec::new());
    };
    if !dir_present(dir) {
        return (default_theme(), Vec::new());
    }
    let mut warnings = Vec::new();
    let mut partial = PartialTheme::default();
    let mut parsed_any = false;

    let colors = dir.join("colors.toml");
    match read_if_present(&colors) {
        FileRead::Missing => {}
        FileRead::Malformed(reason) => {
            let line = format!("{}: {reason}", colors.display());
            tracing::warn!(path = %colors.display(), "{reason}");
            warnings.push(line);
        }
        FileRead::Text(text) => {
            if parse_colors_toml(&text, &mut partial) {
                parsed_any = true;
            } else if has_non_comment_content(&text) {
                let reason = "malformed omarchy theme file ignored";
                tracing::warn!(path = %colors.display(), "{reason}");
                warnings.push(format!("{}: {reason}", colors.display()));
            }
        }
    }

    let ghostty = dir.join("ghostty.conf");
    let alacritty = dir.join("alacritty.toml");
    match read_if_present(&ghostty) {
        FileRead::Missing => match read_if_present(&alacritty) {
            FileRead::Missing => {}
            FileRead::Malformed(reason) => {
                tracing::warn!(path = %alacritty.display(), "{reason}");
                warnings.push(format!("{}: {reason}", alacritty.display()));
            }
            FileRead::Text(text) => {
                if parse_alacritty(&text, &mut partial) {
                    parsed_any = true;
                } else if has_non_comment_content(&text) {
                    let reason = "malformed omarchy theme file ignored";
                    tracing::warn!(path = %alacritty.display(), "{reason}");
                    warnings.push(format!("{}: {reason}", alacritty.display()));
                }
            }
        },
        FileRead::Malformed(reason) => {
            tracing::warn!(path = %ghostty.display(), "{reason}");
            warnings.push(format!("{}: {reason}", ghostty.display()));
        }
        FileRead::Text(text) => {
            if parse_ghostty(&text, &mut partial) {
                parsed_any = true;
            } else if has_non_comment_content(&text) {
                let reason = "malformed omarchy theme file ignored";
                tracing::warn!(path = %ghostty.display(), "{reason}");
                warnings.push(format!("{}: {reason}", ghostty.display()));
            }
        }
    }

    if !parsed_any {
        return (default_theme(), warnings);
    }
    (partial.into_dto(), warnings)
}

#[tauri::command]
pub fn theme_current() -> ThemeDto {
    load_theme_from(omarchy_theme_dir().as_deref())
}

/// Neutral defaults plus the same sixteen ANSI colors rendered by the desktop.
pub fn host_theme_updates(dir: Option<&Path>) -> Vec<ClientHostThemeUpdate> {
    let color = |hex: &str| {
        let [r, g, b] = rgb(hex).expect("loaded theme colors are six-digit hex");
        ClientHostColor { r, g, b }
    };
    vec![
        ClientHostThemeUpdate::DefaultColor {
            kind: ClientHostDefaultColorKind::Background,
            color: color(TERM_BG),
        },
        ClientHostThemeUpdate::DefaultColor {
            kind: ClientHostDefaultColorKind::Foreground,
            color: color(TERM_FG),
        },
        ClientHostThemeUpdate::PaletteColors(
            load_theme_from(dir)
                .named
                .iter()
                .enumerate()
                .map(|(index, hex)| (index as u8, color(hex)))
                .collect(),
        ),
        ClientHostThemeUpdate::Appearance(ClientHostAppearance::Dark),
    ]
}

/// `#rrggbb` as an opaque RGBA (alpha 255). None when the string is not a 6-digit hex colour.
pub fn opaque_rgba(hex: &str) -> Option<[u8; 4]> {
    let [r, g, b] = rgb(hex)?;
    Some([r, g, b, 255])
}

/// Native fill stays neutral even when the DTO reports an Omarchy theme.
pub fn native_background_rgba(_dto: &ThemeDto) -> [u8; 4] {
    opaque_rgba(TERM_BG).expect("neutral background is a six-digit hex color")
}

/// Sets the product window and WebView native background to the neutral fill (opaque).
pub fn apply_native_background<R: Runtime>(app: &AppHandle<R>, dto: &ThemeDto) {
    let color = tauri::window::Color::from(native_background_rgba(dto));
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_background_color(Some(color));
    }
}

/// Polls `dir` for mtime/symlink/content changes and emits `theme_changed`.
pub fn spawn_watch<R: Runtime>(app: AppHandle<R>, dir: PathBuf) {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("herdr-theme-watch".into())
        .spawn(move || {
            let mut watcher = ThemeWatcher::new(dir);
            loop {
                std::thread::sleep(POLL_INTERVAL);
                if let Some(dto) = watcher.poll() {
                    apply_native_background(&app, &dto);
                    let _ = app.emit("theme_changed", &dto);
                }
            }
        });
}

/// Last-emitted snapshot used so a single write yields exactly one event.
pub struct ThemeWatcher {
    dir: PathBuf,
    last_fp: String,
    last_dto: ThemeDto,
}

impl ThemeWatcher {
    pub fn new(dir: PathBuf) -> Self {
        let last_dto = load_theme_from(Some(&dir));
        let last_fp = fingerprint(&dir);
        Self {
            dir,
            last_fp,
            last_dto,
        }
    }

    /// Returns the new theme when the directory (or its symlink target) changed.
    pub fn poll(&mut self) -> Option<ThemeDto> {
        let fp = fingerprint(&self.dir);
        if fp == self.last_fp {
            return None;
        }
        self.last_fp = fp;
        let dto = load_theme_from(Some(&self.dir));
        if dto == self.last_dto {
            return None;
        }
        self.last_dto = dto.clone();
        Some(dto)
    }
}

fn dir_present(dir: &Path) -> bool {
    fs::symlink_metadata(dir).is_ok()
}

enum FileRead {
    Missing,
    Malformed(&'static str),
    Text(String),
}

fn read_if_present(path: &Path) -> FileRead {
    match fs::read_to_string(path) {
        Ok(text) => FileRead::Text(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => FileRead::Missing,
        Err(_) => FileRead::Malformed("omarchy theme file could not be read"),
    }
}

fn has_non_comment_content(text: &str) -> bool {
    text.lines().any(|line| !content_line(line).is_empty())
}

/// Whole-line comments only. Inline `#rrggbb` (ghostty, unquoted) is a colour, not a comment.
fn content_line(raw: &str) -> &str {
    let t = raw.trim();
    if t.is_empty() || t.starts_with('#') {
        ""
    } else {
        t
    }
}

#[derive(Default)]
struct PartialTheme {
    mode: Option<String>,
    background: Option<String>,
    foreground: Option<String>,
    cursor: Option<String>,
    selection: Option<String>,
    named: [Option<String>; 16],
    lighter_background: Option<String>,
    dark_foreground: Option<String>,
    light_foreground: Option<String>,
    bright_foreground: Option<String>,
    accent: Option<String>,
    muted: Option<String>,
    red: Option<String>,
    yellow: Option<String>,
    green: Option<String>,
}

impl PartialTheme {
    fn into_dto(self) -> ThemeDto {
        let fallback = default_theme();
        let mode = self
            .mode
            .filter(|m| m == "light" || m == "dark")
            .unwrap_or(fallback.mode);
        let background = self.background.unwrap_or(fallback.background);
        let foreground = self.foreground.unwrap_or(fallback.foreground);
        let cursor = self.cursor.unwrap_or(fallback.cursor);
        let selection = self.selection.clone().unwrap_or(fallback.selection);
        let named: Vec<String> = (0..16)
            .map(|i| {
                self.named[i]
                    .clone()
                    .unwrap_or_else(|| fallback.named[i].clone())
            })
            .collect();
        let surface_2 = self
            .lighter_background
            .clone()
            .unwrap_or(fallback.surface_2);
        let surface_3 = adjust_surface_3(&surface_2, &mode);
        let text = self
            .bright_foreground
            .clone()
            .or_else(|| named.get(15).cloned())
            .unwrap_or(fallback.text);
        let text_muted = self.light_foreground.unwrap_or(fallback.text_muted);
        let text_dim = self.dark_foreground.unwrap_or(fallback.text_dim);
        let idle = self
            .muted
            .clone()
            .or_else(|| named.get(8).cloned())
            .unwrap_or(fallback.idle);
        ThemeDto {
            source: SOURCE_OMARCHY.to_owned(),
            mode,
            surface: background.clone(),
            surface_2,
            surface_3,
            border: self.selection.unwrap_or(fallback.border),
            text,
            text_muted,
            text_dim,
            accent: self.accent.unwrap_or(fallback.accent),
            working: self.green.unwrap_or(fallback.working),
            attention: self.yellow.unwrap_or(fallback.attention),
            error: self.red.unwrap_or(fallback.error),
            idle,
            background,
            foreground,
            cursor,
            selection,
            named,
        }
    }
}

fn parse_colors_toml(text: &str, dst: &mut PartialTheme) -> bool {
    let mut any = false;
    for raw in text.lines() {
        let line = content_line(raw);
        if line.is_empty() || line.starts_with('[') {
            continue;
        }
        let Some((key, value)) = split_kv(line) else {
            continue;
        };
        match key {
            "mode" => {
                let mode = unquote(value).to_ascii_lowercase();
                if mode == "light" || mode == "dark" {
                    dst.mode = Some(mode);
                    any = true;
                }
            }
            "background" => set_color(&mut dst.background, value, &mut any),
            "foreground" => set_color(&mut dst.foreground, value, &mut any),
            "selection" => set_color(&mut dst.selection, value, &mut any),
            "lighter_background" => set_color(&mut dst.lighter_background, value, &mut any),
            "dark_foreground" => set_color(&mut dst.dark_foreground, value, &mut any),
            "light_foreground" => set_color(&mut dst.light_foreground, value, &mut any),
            "bright_foreground" => set_color(&mut dst.bright_foreground, value, &mut any),
            "accent" => set_color(&mut dst.accent, value, &mut any),
            "muted" => set_color(&mut dst.muted, value, &mut any),
            "red" => set_color(&mut dst.red, value, &mut any),
            "yellow" => set_color(&mut dst.yellow, value, &mut any),
            "green" => set_color(&mut dst.green, value, &mut any),
            _ => {}
        }
    }
    any
}

fn parse_ghostty(text: &str, dst: &mut PartialTheme) -> bool {
    let mut any = false;
    for raw in text.lines() {
        let line = content_line(raw);
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("palette") {
            let rest = rest.trim().trim_start_matches('=').trim();
            if let Some((idx, hex)) = rest.split_once('=') {
                if let Ok(i) = idx.trim().parse::<usize>() {
                    if i < 16 {
                        if let Some(color) = parse_hex(hex) {
                            dst.named[i] = Some(color);
                            any = true;
                        }
                    }
                }
            }
            continue;
        }
        let Some((key, value)) = split_kv(line) else {
            continue;
        };
        match key {
            "background" => set_color(&mut dst.background, value, &mut any),
            "foreground" => set_color(&mut dst.foreground, value, &mut any),
            "cursor-color" => set_color(&mut dst.cursor, value, &mut any),
            "selection-background" => set_color(&mut dst.selection, value, &mut any),
            _ => {}
        }
    }
    any
}

fn parse_alacritty(text: &str, dst: &mut PartialTheme) -> bool {
    let mut any = false;
    let mut table = String::new();
    for raw in text.lines() {
        let line = content_line(raw);
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            table = name.trim().to_owned();
            continue;
        }
        let Some((key, value)) = split_kv(line) else {
            continue;
        };
        match (table.as_str(), key) {
            ("colors.primary", "background") => set_color(&mut dst.background, value, &mut any),
            ("colors.primary", "foreground") => set_color(&mut dst.foreground, value, &mut any),
            ("colors.cursor", "cursor") => set_color(&mut dst.cursor, value, &mut any),
            ("colors.selection", "background") => set_color(&mut dst.selection, value, &mut any),
            ("colors.normal", k) => set_named(&mut dst.named, normal_index(k), value, &mut any),
            ("colors.bright", k) => set_named(&mut dst.named, bright_index(k), value, &mut any),
            _ => {}
        }
    }
    any
}

fn set_color(slot: &mut Option<String>, value: &str, any: &mut bool) {
    if let Some(color) = parse_hex(value) {
        *slot = Some(color);
        *any = true;
    }
}

fn set_named(named: &mut [Option<String>; 16], index: Option<usize>, value: &str, any: &mut bool) {
    let Some(i) = index else {
        return;
    };
    if let Some(color) = parse_hex(value) {
        named[i] = Some(color);
        *any = true;
    }
}

fn normal_index(key: &str) -> Option<usize> {
    Some(match key {
        "black" => 0,
        "red" => 1,
        "green" => 2,
        "yellow" => 3,
        "blue" => 4,
        "magenta" => 5,
        "cyan" => 6,
        "white" => 7,
        _ => return None,
    })
}

fn bright_index(key: &str) -> Option<usize> {
    normal_index(key).map(|i| i + 8)
}

fn split_kv(line: &str) -> Option<(&str, &str)> {
    let (k, v) = line.split_once('=')?;
    Some((k.trim(), v.trim()))
}

fn unquote(value: &str) -> String {
    let v = value.trim();
    let v = v
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(v);
    let v = v
        .strip_prefix('\'')
        .and_then(|s| s.strip_suffix('\''))
        .unwrap_or(v);
    v.trim().to_owned()
}

fn parse_hex(value: &str) -> Option<String> {
    let v = unquote(value);
    let v = v.strip_prefix('#')?;
    if v.len() == 6 && v.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(format!("#{}", v.to_ascii_lowercase()))
    } else {
        None
    }
}

fn adjust_surface_3(lighter: &str, mode: &str) -> String {
    if mode == "light" {
        mix_black(lighter, 0.06).unwrap_or_else(|| PEN_SURFACE_3.to_owned())
    } else {
        mix_white(lighter, 0.06).unwrap_or_else(|| PEN_SURFACE_3.to_owned())
    }
}

fn rgb(hex: &str) -> Option<[u8; 3]> {
    let v = hex.strip_prefix('#')?;
    if v.len() != 6 {
        return None;
    }
    Some([
        u8::from_str_radix(&v[0..2], 16).ok()?,
        u8::from_str_radix(&v[2..4], 16).ok()?,
        u8::from_str_radix(&v[4..6], 16).ok()?,
    ])
}

fn mix_white(hex: &str, t: f32) -> Option<String> {
    let [r, g, b] = rgb(hex)?;
    let adj = |c: u8| {
        (c as f32 + (255.0 - c as f32) * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Some(format!("#{:02x}{:02x}{:02x}", adj(r), adj(g), adj(b)))
}

fn mix_black(hex: &str, t: f32) -> Option<String> {
    let [r, g, b] = rgb(hex)?;
    let adj = |c: u8| (c as f32 * (1.0 - t)).round().clamp(0.0, 255.0) as u8;
    Some(format!("#{:02x}{:02x}{:02x}", adj(r), adj(g), adj(b)))
}

fn fingerprint(dir: &Path) -> String {
    let mut out = String::new();
    if let Ok(link) = fs::read_link(dir) {
        out.push_str("link:");
        out.push_str(&link.to_string_lossy());
        out.push('|');
    }
    match fs::canonicalize(dir) {
        Ok(canon) => out.push_str(&canon.to_string_lossy()),
        Err(_) => out.push_str(&dir.to_string_lossy()),
    }
    for name in ["colors.toml", "ghostty.conf", "alacritty.toml"] {
        out.push('|');
        match fs::read(dir.join(name)) {
            Ok(bytes) => {
                let mut hasher = DefaultHasher::new();
                bytes.hash(&mut hasher);
                out.push_str(&format!("{name}:{}", hasher.finish()));
            }
            Err(_) => out.push_str(&format!("{name}:-")),
        }
    }
    out
}
