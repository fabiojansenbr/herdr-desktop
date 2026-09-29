//! Spec 020 — Omarchy theme load, fallback, malformed files and live watch.
//! Would catch: reading the machine theme path, a silent fallback that still
//! reports `source = "omarchy"`, keeping a bad field, or emitting more than one
//! event for a single file change.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use herdr_desktop::theme::{
    default_theme, load_theme_from, load_theme_with_warnings, native_background_rgba, opaque_rgba,
    ThemeWatcher, COMMANDS,
};

const COLORS: &str = r##"
mode = "dark"
accent = "#7aa2f7"
selection = "#292e42"
muted = "#414868"
background = "#1a1b26"
dark_background = "#13141c"
darker_background = "#0e0e14"
lighter_background = "#24283b"
foreground = "#a9b1d6"
dark_foreground = "#565f89"
light_foreground = "#b4bee6"
bright_foreground = "#c0caf5"
red = "#f7768e"
yellow = "#e0af68"
orange = "#eb927b"
green = "#9ece6a"
cyan = "#449dab"
blue = "#7aa2f7"
magenta = "#ad8ee6"
brown = "#75493d"
bright_red = "#ff7a93"
bright_yellow = "#ff9e64"
bright_green = "#b9f27c"
bright_cyan = "#0db9d7"
bright_blue = "#7da6ff"
bright_magenta = "#bb9af7"
"##;

const GHOSTTY: &str = r##"
background = #1a1b26
foreground = #a9b1d6
cursor-color = #c0caf5
selection-background = #292e42
selection-foreground = #c0caf5
palette = 0=#1a1b26
palette = 1=#f7768e
palette = 2=#9ece6a
palette = 3=#e0af68
palette = 4=#7aa2f7
palette = 5=#ad8ee6
palette = 6=#449dab
palette = 7=#a9b1d6
palette = 8=#414868
palette = 9=#ff7a93
palette = 10=#b9f27c
palette = 11=#ff9e64
palette = 12=#7da6ff
palette = 13=#bb9af7
palette = 14=#0db9d7
palette = 15=#c0caf5
"##;

const ALACRITTY: &str = r##"
[colors.primary]
background = "#1a1b26"
foreground = "#a9b1d6"

[colors.cursor]
text = "#1a1b26"
cursor = "#c0caf5"

[colors.selection]
text = "#c0caf5"
background = "#292e42"

[colors.normal]
black = "#1a1b26"
red = "#f7768e"
green = "#9ece6a"
yellow = "#e0af68"
blue = "#7aa2f7"
magenta = "#ad8ee6"
cyan = "#449dab"
white = "#a9b1d6"

[colors.bright]
black = "#414868"
red = "#ff7a93"
green = "#b9f27c"
yellow = "#ff9e64"
blue = "#7da6ff"
magenta = "#bb9af7"
cyan = "#0db9d7"
white = "#c0caf5"
"##;

fn write_omarchy(dir: &Path, colors: &str, ghostty: Option<&str>, alacritty: Option<&str>) {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join("colors.toml"), colors).unwrap();
    match ghostty {
        Some(text) => fs::write(dir.join("ghostty.conf"), text).unwrap(),
        None => {
            let _ = fs::remove_file(dir.join("ghostty.conf"));
        }
    }
    match alacritty {
        Some(text) => fs::write(dir.join("alacritty.toml"), text).unwrap(),
        None => {
            let _ = fs::remove_file(dir.join("alacritty.toml"));
        }
    }
}

#[test]
fn command_is_the_additive_theme_current() {
    assert_eq!(COMMANDS, &["theme_current"]);
}

#[test]
fn missing_directory_uses_default_theme() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("no-such-theme");
    let dto = load_theme_from(Some(&missing));
    let fallback = default_theme();
    assert_eq!(dto.source, "default");
    assert_eq!(dto, fallback);
    // Would catch: looking up ~/.local/state/omarchy when the test dir is absent.
    assert_ne!(dto.background.to_ascii_lowercase(), "#1a1b26");
}

#[test]
fn parses_colors_toml_and_ghostty_palette() {
    let dir = tempfile::tempdir().unwrap();
    write_omarchy(dir.path(), COLORS, Some(GHOSTTY), None);
    let dto = load_theme_from(Some(dir.path()));
    assert_eq!(dto.source, "omarchy");
    assert_eq!(dto.mode, "dark");
    assert_eq!(dto.background, "#1a1b26");
    assert_eq!(dto.foreground, "#a9b1d6");
    assert_eq!(dto.cursor, "#c0caf5");
    assert_eq!(dto.selection, "#292e42");
    assert_eq!(dto.named.len(), 16);
    assert_eq!(dto.named[0], "#1a1b26");
    assert_eq!(dto.named[4], "#7aa2f7");
    assert_eq!(dto.named[8], "#414868");
    assert_eq!(dto.named[15], "#c0caf5");
    assert_eq!(dto.surface, dto.background);
    assert_eq!(dto.surface_2, "#24283b");
    // lighter_background #24283b lightened 6 % → #313547
    assert_eq!(dto.surface_3, "#313547");
    assert_eq!(dto.border, "#292e42");
    assert_eq!(dto.text, "#c0caf5");
    assert_eq!(dto.text_muted, "#b4bee6");
    assert_eq!(dto.text_dim, "#565f89");
    assert_eq!(dto.accent, "#7aa2f7");
    assert_eq!(dto.working, "#9ece6a");
    assert_eq!(dto.attention, "#e0af68");
    assert_eq!(dto.error, "#f7768e");
    assert_eq!(dto.idle, "#414868");
}

#[test]
fn alacritty_palette_is_used_when_ghostty_is_absent() {
    let dir = tempfile::tempdir().unwrap();
    write_omarchy(dir.path(), COLORS, None, Some(ALACRITTY));
    let dto = load_theme_from(Some(dir.path()));
    assert_eq!(dto.source, "omarchy");
    assert_eq!(dto.named[15], "#c0caf5");
    assert_eq!(dto.cursor, "#c0caf5");
    assert_eq!(dto.named[2], "#9ece6a");
}

#[test]
fn malformed_file_is_ignored_with_a_warning_and_default_holds() {
    let dir = tempfile::tempdir().unwrap();
    write_omarchy(dir.path(), COLORS, Some(GHOSTTY), None);
    fs::write(dir.path().join("colors.toml"), "this is {{{ not toml\n").unwrap();
    let (dto, warnings) = load_theme_with_warnings(Some(dir.path()));
    assert!(
        warnings.iter().any(|w| w.contains("colors.toml")),
        "malformed colors.toml must produce a warning, got {warnings:?}"
    );
    // Palette still comes from ghostty; CSS named fields from colors.toml fall back.
    assert_eq!(dto.named[15], "#c0caf5");
    assert_eq!(dto.accent, default_theme().accent);
    assert_eq!(dto.text_muted, default_theme().text_muted);
}

#[test]
fn invalid_color_in_one_field_falls_back_only_that_field() {
    let dir = tempfile::tempdir().unwrap();
    let colors = COLORS.replace("accent = \"#7aa2f7\"", "accent = \"not-a-color\"");
    write_omarchy(dir.path(), &colors, Some(GHOSTTY), None);
    let dto = load_theme_from(Some(dir.path()));
    assert_eq!(dto.source, "omarchy");
    assert_eq!(dto.accent, default_theme().accent);
    assert_eq!(dto.background, "#1a1b26");
    assert_eq!(dto.working, "#9ece6a");
}

#[test]
fn light_mode_darkens_surface_3() {
    let dir = tempfile::tempdir().unwrap();
    let colors = COLORS.replace("mode = \"dark\"", "mode = \"light\"");
    write_omarchy(dir.path(), &colors, Some(GHOSTTY), None);
    let dto = load_theme_from(Some(dir.path()));
    assert_eq!(dto.mode, "light");
    // lighter_background #24283b darkened 6 % → #222637
    assert_eq!(dto.surface_3, "#222637");
}

#[test]
fn changing_the_file_emits_exactly_one_event_with_the_new_theme() {
    let dir = tempfile::tempdir().unwrap();
    write_omarchy(dir.path(), COLORS, Some(GHOSTTY), None);
    let mut watcher = ThemeWatcher::new(dir.path().to_path_buf());
    assert!(
        watcher.poll().is_none(),
        "construction must not emit; theme_current delivers the first snapshot"
    );

    let (tx, rx) = mpsc::channel();
    let dir_path = dir.path().to_path_buf();
    let handle = std::thread::spawn(move || {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(2) {
            if let Some(dto) = watcher.poll() {
                let _ = tx.send(dto);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    });

    std::thread::sleep(Duration::from_millis(40));
    let updated = COLORS.replace("accent = \"#7aa2f7\"", "accent = \"#ff0000\"");
    write_omarchy(&dir_path, &updated, Some(GHOSTTY), None);

    let first = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("watch must emit the new theme within 1s of the write");
    assert_eq!(first.source, "omarchy");
    assert_eq!(first.accent, "#ff0000");
    assert_eq!(first.background, "#1a1b26");
    assert!(
        rx.recv_timeout(Duration::from_millis(400)).is_err(),
        "a single write must not emit a second event"
    );
    let _ = handle.join();
}

#[test]
fn retargeting_the_theme_symlink_emits_the_new_directory() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    write_omarchy(&a, COLORS, Some(GHOSTTY), None);
    let colors_b = COLORS.replace("background = \"#1a1b26\"", "background = \"#000000\"");
    let ghostty_b = GHOSTTY.replace("background = #1a1b26", "background = #000000");
    write_omarchy(&b, &colors_b, Some(&ghostty_b), None);
    let link = root.path().join("theme");
    symlink(&a, &link).unwrap();

    let mut watcher = ThemeWatcher::new(link.clone());
    assert!(watcher.poll().is_none());
    fs::remove_file(&link).unwrap();
    symlink(&b, &link).unwrap();
    let dto = (0..50)
        .find_map(|_| {
            let found = watcher.poll();
            if found.is_none() {
                std::thread::sleep(Duration::from_millis(20));
            }
            found
        })
        .expect("retargeted symlink must emit");
    assert_eq!(dto.background, "#000000");
    assert!(watcher.poll().is_none());
}

/// AC-020-04: would catch a window left transparent (WebKitGTK buffer with per-pixel alpha)
/// or a missing native backgroundColor so Hyprland composites the wallpaper through.
#[test]
fn product_window_is_opaque_with_a_background_color() {
    let config: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tauri.conf.json")).unwrap(),
    )
    .unwrap();
    let win = &config["app"]["windows"][0];
    assert_eq!(
        win["transparent"],
        serde_json::json!(false),
        "windows[0].transparent must be explicit false"
    );
    let bg = win["backgroundColor"]
        .as_str()
        .expect("windows[0].backgroundColor must be present");
    let rgba = opaque_rgba(bg).expect("backgroundColor must be an opaque #rrggbb");
    assert_eq!(rgba[3], 255);
}

/// AC-020-04: native window fill uses the theme background at alpha 255, never a translucent RGBA.
#[test]
fn theme_background_is_an_opaque_window_color() {
    assert_eq!(opaque_rgba("#1a1b26"), Some([0x1a, 0x1b, 0x26, 255]));
    assert_eq!(
        opaque_rgba(&default_theme().background),
        Some([15, 15, 15, 255])
    );
    assert_eq!(opaque_rgba("not-a-color"), None);
}

/// AC-061-04: catches Omarchy (including light mode) tinting the native window.
#[test]
fn native_fill_stays_neutral_for_default_and_omarchy() {
    let dir = tempfile::tempdir().unwrap();
    write_omarchy(dir.path(), COLORS, Some(GHOSTTY), None);
    let mut dto = load_theme_from(Some(dir.path()));
    assert_eq!(dto.background, "#1a1b26");
    assert_eq!(native_background_rgba(&dto), [15, 15, 15, 255]);
    dto.mode = "light".into();
    dto.background = "#ffffff".into();
    assert_eq!(native_background_rgba(&dto), [15, 15, 15, 255]);
    assert_eq!(native_background_rgba(&default_theme()), [15, 15, 15, 255]);
}

/// AC-061-04: catches a stale default in any DTO field, including terminal selection.
#[test]
fn default_palette_matches_neutral_tokens_and_original_ansi() {
    let dto = default_theme();
    assert_eq!(dto.source, "default");
    assert_eq!(dto.mode, "dark");
    assert_eq!(dto.background, "#0F0F0F");
    assert_eq!(dto.foreground, "#EDEDED");
    assert_eq!(dto.cursor, "#EDEDED");
    assert_eq!(dto.selection, "#2A2A2A");
    assert_eq!(dto.surface, "#0A0A0A");
    assert_eq!(dto.surface_2, "#171717");
    assert_eq!(dto.surface_3, "#242424");
    assert_eq!(dto.border, "#1C1C1C");
    assert_eq!(dto.text, "#EDEDED");
    assert_eq!(dto.text_muted, "#A3A3A3");
    assert_eq!(dto.text_dim, "#6B6B6B");
    assert_eq!(dto.accent, "#D4D4D4");
    assert_eq!(dto.working, "#4ADE80");
    assert_eq!(dto.attention, "#E9A23B");
    assert_eq!(dto.error, "#F87171");
    assert_eq!(dto.idle, "#6B6B6B");
    assert_eq!(
        dto.named,
        [
            "#1b1f24", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#abb2bf",
            "#5c6370", "#f27983", "#a9d48a", "#f0cc8a", "#7cbdf5", "#d38fe6", "#6fc7d3", "#ffffff",
        ]
    );
}
