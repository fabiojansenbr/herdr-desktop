# Building

Herdr Desktop is a [Tauri 2](https://v2.tauri.app) application: a Svelte 5 frontend bundled
by Vite, hosted by a Rust binary. Building it needs the JavaScript toolchain, a Rust
toolchain, and the native WebView libraries of the operating system.

## Toolchain

| Tool | Where it comes from |
|---|---|
| Rust (stable) | [`mise.toml`](../mise.toml) — `mise install` |
| just | [`mise.toml`](../mise.toml) — `mise install` |
| Bun | [bun.sh](https://bun.sh), installed separately |
| Node | used by the repository scripts; any current LTS |

```bash
mise install
bun install
```

## System dependencies

### Linux

The WebView is WebKitGTK. On Arch Linux:

```bash
sudo pacman -S --needed webkit2gtk-4.1 gtk3 base-devel curl wget file librsvg
```

On Debian and Ubuntu:

```bash
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
    libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
```

On Fedora:

```bash
sudo dnf install webkit2gtk4.1-devel openssl-devel curl wget file \
    libappindicator-gtk3-devel librsvg2-devel
sudo dnf group install "c-development"
```

Check that the library the build links against is visible:

```bash
pkg-config --modversion webkit2gtk-4.1
```

The canonical, always-current list is Tauri's own page:
<https://v2.tauri.app/start/prerequisites/>.

### macOS

Xcode Command Line Tools (`xcode-select --install`). The WebView (WKWebView) ships with the
system. Building for both architectures needs the matching Rust targets
(`aarch64-apple-darwin`, `x86_64-apple-darwin`).

### Windows

Microsoft C++ Build Tools (the "Desktop development with C++" workload) and the WebView2
runtime, which is already present on Windows 11 and on up-to-date Windows 10.

> The maintainers build and test on Linux only. The macOS and Windows instructions follow
> Tauri's prerequisites and are not verified on those systems.

## Development build

The frontend must be built at least once, because the Rust host embeds `dist/` at compile
time:

```bash
bun run build            # vite build -> dist/
cargo build -p herdr-desktop
```

Or, with the recipe that does both:

```bash
mise exec -- just build
```

To run the app against a Herdr session while developing — use a disposable session, not the
one you are working in:

```bash
mise exec -- just dev <session>
```

That is `HERDR_DESKTOP_SESSION=<session> bunx tauri dev`: Vite serves the frontend with hot
reload and the Rust host rebuilds on change.

## Release build

```bash
bun run build
bunx tauri build
```

`bunx tauri build` compiles the host in release mode and, for every bundle target enabled in
[`src-tauri/tauri.conf.json`](../src-tauri/tauri.conf.json), packages it. The results land
under `src-tauri/target/release/` (the binary) and `src-tauri/target/release/bundle/` (the
packages).

For a release build without packaging — the one used for resource measurements:

```bash
mise exec -- just build-release
```

See [releasing.md](releasing.md) for how a tagged release is produced.

## Troubleshooting

- **`webkit2gtk-4.1` not found.** The `-dev`/`-devel` package is missing, or `PKG_CONFIG_PATH`
  does not reach it. `pkg-config --modversion webkit2gtk-4.1` has to answer.
- **The window opens empty.** `dist/` was not built. Run `bun run build` before `cargo build`.
- **Rust rebuilds everything each time.** The debug profile is set to
  `debug = "line-tables-only"`; full debug info made the test binaries enormous. Keep it.
