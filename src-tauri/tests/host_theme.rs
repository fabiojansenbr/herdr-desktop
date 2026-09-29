//! Spec 066: exact engine theme, with and without Omarchy.
use herdr_client::protocol::wire::{
    ClientHostAppearance, ClientHostColor as Color, ClientHostDefaultColorKind as Kind,
    ClientHostThemeUpdate as Update,
};
use herdr_desktop::theme::{host_theme_updates, load_theme_from, opaque_rgba};

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

fn expected(named: &[String]) -> Vec<Update> {
    vec![
        Update::DefaultColor {
            kind: Kind::Background,
            color: Color {
                r: 15,
                g: 15,
                b: 15,
            },
        },
        Update::DefaultColor {
            kind: Kind::Foreground,
            color: Color {
                r: 237,
                g: 237,
                b: 237,
            },
        },
        Update::PaletteColors(
            named
                .iter()
                .enumerate()
                .map(|(i, hex)| {
                    let [r, g, b, _] = opaque_rgba(hex).unwrap();
                    (i as u8, Color { r, g, b })
                })
                .collect(),
        ),
        Update::Appearance(ClientHostAppearance::Dark),
    ]
}

#[test]
fn host_theme_uses_neutral_defaults_and_exact_omarchy_palette() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("colors.toml"), COLORS).unwrap();
    std::fs::write(dir.path().join("ghostty.conf"), GHOSTTY).unwrap();
    let named = load_theme_from(Some(dir.path())).named;
    let updates = host_theme_updates(Some(dir.path()));
    assert_eq!(updates, expected(&named));
    let Update::PaletteColors(palette) = &updates[2] else {
        panic!("palette missing")
    };
    assert_eq!(palette.len(), 16);
    assert_eq!(
        palette[4],
        (
            4,
            Color {
                r: 122,
                g: 162,
                b: 247
            }
        )
    );
}

#[test]
fn host_theme_without_omarchy_uses_all_sixteen_fallback_colors_in_order() {
    let named = [
        "#1b1f24", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#abb2bf",
        "#5c6370", "#f27983", "#a9d48a", "#f0cc8a", "#7cbdf5", "#d38fe6", "#6fc7d3", "#ffffff",
    ]
    .map(str::to_owned);
    assert_eq!(host_theme_updates(None), expected(&named));
}
