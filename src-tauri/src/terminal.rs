//! Terminal module: IPC commands, the gateway↔WebView bridge and the surface trace.
//!
//! Flow: `terminal_connect` negotiates generation 1 through `LocalGateway`, then a bridge
//! thread applies every gateway event to the `FrameStore` and forwards compact frame DTOs
//! to the WebView through one `Channel`. Input is accepted only while the store is live;
//! a rejected patch marks the surface stale, blocks input and requests a full surface
//! (same geometry) — buffered input is never replayed.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use herdr_client::bootstrap::{
    ensure_session_running, start_session_detached, wait_for_session, BootstrapConfig,
    SESSION_START_TIMEOUT,
};
use herdr_client::local::GatewayEvents;
use herdr_client::protocol::wire::{
    key_modifiers, CellData, ClientKeyCode, ClientMouseButton, ClientMouseGeometry,
    ClientMouseKind, ClientMousePosition, ClientPaneInputEvent, CursorState, PaneSurfaceFrame,
    PaneSurfacePane, PaneSurfacePatch, PaneSurfaceScrollMetrics, SurfaceRect,
};
use herdr_client::{
    ApplyOutcome, ConnectOptions, FrameStore, GatewayEvent, LocalGateway, Negotiated,
    QualifiedTarget, RuntimeError, RuntimeGateway, SessionPaths, StaleReason, SurfaceGeometry,
    SurfaceState,
};
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::State;

/// Commands this module exposes (kept in sync with `lib.rs` by a test).
pub const COMMANDS: &[&str] = &[
    "terminal_status",
    "terminal_connect",
    "terminal_input",
    "terminal_resize",
    "terminal_focus",
    "terminal_detach",
    "session_start",
];

// ---------------------------------------------------------------------------
// DTOs (everything the WebView sees)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct CellDto {
    pub s: String,
    pub fg: u32,
    pub bg: u32,
    pub m: u16,
    /// Index into the hyperlink table of the last full frame (`FrameEvent::Metadata`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub h: Option<u32>,
}

impl From<&CellData> for CellDto {
    fn from(c: &CellData) -> Self {
        Self {
            s: c.symbol.clone(),
            fg: c.fg,
            bg: c.bg,
            m: c.modifier,
            h: c.hyperlink,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CursorDto {
    pub x: u16,
    pub y: u16,
    pub visible: bool,
    pub shape: u8,
}

impl From<&CursorState> for CursorDto {
    fn from(c: &CursorState) -> Self {
        Self {
            x: c.x,
            y: c.y,
            visible: c.visible,
            shape: c.shape,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RowDto {
    pub x: u16,
    pub y: u16,
    pub cells: Vec<CellDto>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PaneDto {
    pub pane_id: String,
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    pub focused: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RectDto {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl From<SurfaceRect> for RectDto {
    fn from(r: SurfaceRect) -> Self {
        Self {
            x: r.x,
            y: r.y,
            width: r.width,
            height: r.height,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScrollDto {
    pub offset_from_bottom: u64,
    pub max_offset_from_bottom: u64,
    pub viewport_rows: u64,
}

impl From<PaneSurfaceScrollMetrics> for ScrollDto {
    fn from(m: PaneSurfaceScrollMetrics) -> Self {
        Self {
            offset_from_bottom: m.offset_from_bottom,
            max_offset_from_bottom: m.max_offset_from_bottom,
            viewport_rows: m.viewport_rows,
        }
    }
}

/// Per-pane metadata the WebView needs for hit testing, mouse, selection and scrollback.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PaneMetaDto {
    pub pane_id: String,
    pub content_revision: u64,
    pub rect: RectDto,
    pub inner_rect: RectDto,
    pub scroll: Option<ScrollDto>,
    pub focused: bool,
    pub mouse_reporting: bool,
    pub sgr_pixel_mouse: bool,
    pub alternate_screen_active: bool,
    pub pixel_width: u32,
    pub pixel_height: u32,
}

impl From<&PaneSurfacePane> for PaneMetaDto {
    fn from(p: &PaneSurfacePane) -> Self {
        Self {
            pane_id: p.pane_id.clone(),
            content_revision: p.content_revision,
            rect: p.rect.into(),
            inner_rect: p.inner_rect.into(),
            scroll: p.scroll.map(ScrollDto::from),
            focused: p.focused,
            mouse_reporting: p.mouse_reporting,
            sgr_pixel_mouse: p.sgr_pixel_mouse,
            alternate_screen_active: p.alternate_screen_active,
            pixel_width: p.pixel_width,
            pixel_height: p.pixel_height,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FrameEvent {
    Identity {
        boot_id: String,
        generation: u32,
        connection_generation: u64,
        server_version: String,
        pane_id: Option<String>,
    },
    Full {
        revision: u64,
        width: u16,
        height: u16,
        cells: Vec<CellDto>,
        cursor: Option<CursorDto>,
        panes: Vec<PaneDto>,
    },
    Patch {
        revision: u64,
        rows: Vec<RowDto>,
        cursor: Option<CursorDto>,
    },
    State {
        state: String,
        reason: Option<String>,
        error: Option<RuntimeError>,
    },
    /// Additive: follows the `Full`/`Patch` of the same `revision`. `panes` lists every pane
    /// after a full frame and only the updated ones after a patch; `hyperlinks` is the complete
    /// (sanitized) table after a full frame and `None` (keep) after a patch.
    Metadata {
        revision: u64,
        panes: Vec<PaneMetaDto>,
        hyperlinks: Option<Vec<String>>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusDto {
    pub session: Option<String>,
    pub session_available: bool,
    pub connected: bool,
    pub state: String,
    pub reason: Option<String>,
    pub generation: Option<u32>,
    pub connection_generation: u64,
    pub boot_id: Option<String>,
    pub server_version: Option<String>,
    pub pane_id: Option<String>,
    pub last_error: Option<RuntimeError>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GeometryDto {
    pub cols: u16,
    pub rows: u16,
    pub cell_width_px: u32,
    pub cell_height_px: u32,
}

impl From<GeometryDto> for SurfaceGeometry {
    fn from(g: GeometryDto) -> Self {
        SurfaceGeometry {
            cols: g.cols.max(2),
            rows: g.rows.max(1),
            cell_width_px: g.cell_width_px,
            cell_height_px: g.cell_height_px,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InputDto {
    Text {
        text: String,
    },
    Paste {
        text: String,
    },
    Key {
        code: String,
        modifiers: u8,
        ch: Option<char>,
    },
    /// Pane-local mouse event (cells relative to the pane `inner_rect`).
    Mouse {
        action: String,
        #[serde(default)]
        button: Option<String>,
        column: u16,
        row: u16,
        #[serde(default)]
        pixel: Option<MousePixelDto>,
        #[serde(default)]
        geometry: Option<MouseGeometryDto>,
        #[serde(default)]
        modifiers: u8,
        #[serde(default = "default_scroll_lines")]
        lines: u16,
    },
}

fn default_scroll_lines() -> u16 {
    3
}

/// Largest wheel step accepted from the WebView.
pub const MAX_SCROLL_LINES: u16 = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct MousePixelDto {
    pub x: u32,
    pub y: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct MouseGeometryDto {
    pub cols: u16,
    pub rows: u16,
    pub width_px: u32,
    pub height_px: u32,
}

fn invalid(message: &str) -> RuntimeError {
    RuntimeError::new("invalid_input", message.to_owned())
}

fn modifier_mask(modifiers: u8) -> u8 {
    modifiers
        & (key_modifiers::SHIFT
            | key_modifiers::CONTROL
            | key_modifiers::ALT
            | key_modifiers::SUPER)
}

fn mouse_button(button: Option<&str>) -> Result<ClientMouseButton, RuntimeError> {
    match button {
        Some("left") => Ok(ClientMouseButton::Left),
        Some("right") => Ok(ClientMouseButton::Right),
        Some("middle") => Ok(ClientMouseButton::Middle),
        _ => Err(invalid("invalid mouse button")),
    }
}

// The arguments are exactly the destructured fields of `InputDto::Mouse`; grouping them in a new
// struct would only duplicate that variant.
#[allow(clippy::too_many_arguments)]
fn mouse_event(
    action: &str,
    button: Option<&str>,
    column: u16,
    row: u16,
    pixel: Option<MousePixelDto>,
    geometry: Option<MouseGeometryDto>,
    modifiers: u8,
    lines: u16,
) -> Result<ClientPaneInputEvent, RuntimeError> {
    let kind = match action {
        "down" => ClientMouseKind::Down(mouse_button(button)?),
        "up" => ClientMouseKind::Up(mouse_button(button)?),
        "drag" => ClientMouseKind::Drag(mouse_button(button)?),
        "moved" | "scroll_up" | "scroll_down" | "scroll_left" | "scroll_right"
            if button.is_some() =>
        {
            return Err(invalid("a buttonless mouse event carried a button"))
        }
        "moved" => ClientMouseKind::Moved,
        "scroll_up" => ClientMouseKind::ScrollUp,
        "scroll_down" => ClientMouseKind::ScrollDown,
        "scroll_left" => ClientMouseKind::ScrollLeft,
        "scroll_right" => ClientMouseKind::ScrollRight,
        _ => return Err(invalid("unknown mouse action")),
    };
    if !(1..=MAX_SCROLL_LINES).contains(&lines) {
        return Err(invalid("scroll lines out of range"));
    }
    let (position, geometry) = match (pixel, geometry) {
        (None, None) => (ClientMousePosition::Cell { column, row }, None),
        (Some(p), Some(g)) => {
            let exact = g.cols > 0
                && g.rows > 0
                && column < g.cols
                && row < g.rows
                && (1..=g.width_px).contains(&p.x)
                && (1..=g.height_px).contains(&p.y);
            if !exact {
                return Err(invalid("invalid mouse position in pixels"));
            }
            (
                ClientMousePosition::Pixels {
                    x: p.x,
                    y: p.y,
                    column,
                    row,
                },
                Some(ClientMouseGeometry {
                    cols: g.cols,
                    rows: g.rows,
                    width_px: g.width_px,
                    height_px: g.height_px,
                }),
            )
        }
        _ => return Err(invalid("mouse pixels require a geometry")),
    };
    Ok(ClientPaneInputEvent::Mouse {
        kind,
        position,
        geometry,
        modifiers: modifier_mask(modifiers),
        lines,
    })
}

/// Converts WebView input for one confirmed pane. Mouse events must fall inside the pane's
/// `inner_rect`; clicks/drags/moves require mouse reporting (the wheel is always allowed: the
/// engine decides between application and scrollback); pixel positions degrade to cells when
/// the pane has no SGR pixel mode, and a pixel geometry must match the pane size.
pub fn input_events_for_pane(
    pane: Option<&PaneSurfacePane>,
    events: &[InputDto],
) -> Result<Vec<ClientPaneInputEvent>, RuntimeError> {
    events
        .iter()
        .map(|event| {
            let converted = event.to_event()?;
            let ClientPaneInputEvent::Mouse {
                kind,
                position,
                geometry,
                modifiers,
                lines,
            } = converted
            else {
                return Ok(converted);
            };
            let pane = pane.ok_or_else(|| {
                RuntimeError::new("no_target", "no pane confirmed for the mouse").retryable()
            })?;
            let (column, row) = match position {
                ClientMousePosition::Cell { column, row }
                | ClientMousePosition::Pixels { column, row, .. } => (column, row),
            };
            if column >= pane.inner_rect.width || row >= pane.inner_rect.height {
                return Err(invalid("the mouse is outside the pane"));
            }
            if let Some(g) = geometry {
                if g.cols != pane.inner_rect.width || g.rows != pane.inner_rect.height {
                    return Err(invalid("the mouse geometry diverges from the pane"));
                }
                // Pixels are only used with SGR pixel mode; there they must match the pane's current
                // pixel size (a geometry from an old size/DPI is stale). Otherwise they degrade below.
                if pane.sgr_pixel_mouse
                    && (g.width_px != pane.pixel_width || g.height_px != pane.pixel_height)
                {
                    return Err(invalid("stale mouse geometry in pixels"));
                }
            }
            let is_wheel = matches!(
                kind,
                ClientMouseKind::ScrollUp
                    | ClientMouseKind::ScrollDown
                    | ClientMouseKind::ScrollLeft
                    | ClientMouseKind::ScrollRight
            );
            if !is_wheel && !pane.mouse_reporting {
                return Err(RuntimeError::new(
                    "mouse_not_reporting",
                    "the pane's application did not announce mouse reporting",
                ));
            }
            let (position, geometry) = if pane.sgr_pixel_mouse && geometry.is_some() {
                (position, geometry)
            } else {
                (ClientMousePosition::Cell { column, row }, None)
            };
            Ok(ClientPaneInputEvent::Mouse {
                kind,
                position,
                geometry,
                modifiers,
                lines,
            })
        })
        .collect()
}

impl InputDto {
    /// Maps the WebView key name to the semantic code. Unknown names are rejected so the
    /// WebView cannot craft arbitrary input kinds.
    pub fn to_event(&self) -> Result<ClientPaneInputEvent, RuntimeError> {
        Ok(match self {
            InputDto::Text { text } => ClientPaneInputEvent::TextCommit(text.clone()),
            InputDto::Paste { text } => ClientPaneInputEvent::Paste(text.clone()),
            InputDto::Key {
                code,
                modifiers,
                ch,
            } => {
                let code = match code.as_str() {
                    "Backspace" => ClientKeyCode::Backspace,
                    "Enter" => ClientKeyCode::Enter,
                    "Left" => ClientKeyCode::Left,
                    "Right" => ClientKeyCode::Right,
                    "Up" => ClientKeyCode::Up,
                    "Down" => ClientKeyCode::Down,
                    "Home" => ClientKeyCode::Home,
                    "End" => ClientKeyCode::End,
                    "PageUp" => ClientKeyCode::PageUp,
                    "PageDown" => ClientKeyCode::PageDown,
                    "Tab" => ClientKeyCode::Tab,
                    "BackTab" => ClientKeyCode::BackTab,
                    "Delete" => ClientKeyCode::Delete,
                    "Insert" => ClientKeyCode::Insert,
                    "Esc" => ClientKeyCode::Esc,
                    "Char" => ClientKeyCode::Char(ch.ok_or_else(|| {
                        RuntimeError::new("invalid_input", "Char key without a character")
                    })?),
                    f if f.starts_with('F') => {
                        let n: u8 = f[1..].parse().map_err(|_| {
                            RuntimeError::new("invalid_input", "invalid function key")
                        })?;
                        if !(1..=24).contains(&n) {
                            return Err(RuntimeError::new(
                                "invalid_input",
                                "function key out of range",
                            ));
                        }
                        ClientKeyCode::F(n)
                    }
                    _ => return Err(RuntimeError::new("invalid_input", "unknown key")),
                };
                ClientPaneInputEvent::key_press(code, modifier_mask(*modifiers))
            }
            InputDto::Mouse {
                action,
                button,
                column,
                row,
                pixel,
                geometry,
                modifiers,
                lines,
            } => mouse_event(
                action,
                button.as_deref(),
                *column,
                *row,
                *pixel,
                *geometry,
                *modifiers,
                *lines,
            )?,
        })
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

struct Inner {
    config: BootstrapConfig,
    gateway: Option<LocalGateway>,
    negotiated: Option<Negotiated>,
    store: FrameStore,
    target: Option<QualifiedTarget>,
    geometry: Option<SurfaceGeometry>,
    channel: Option<Channel<FrameEvent>>,
    bridge: Option<JoinHandle<()>>,
    last_error: Option<RuntimeError>,
    shell_pid: Option<u32>,
    disconnected_reason: Option<String>,
}

pub struct TerminalState {
    inner: Arc<Mutex<Inner>>,
}

impl TerminalState {
    pub fn new(config: BootstrapConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                config,
                gateway: None,
                negotiated: None,
                store: FrameStore::new(),
                target: None,
                geometry: None,
                channel: None,
                bridge: None,
                last_error: None,
                shell_pid: None,
                disconnected_reason: None,
            })),
        }
    }

    pub fn detach(&self) {
        let bridge = {
            let mut inner = self.inner.lock().expect("terminal state");
            if let Some(mut gateway) = inner.gateway.take() {
                gateway.detach();
            }
            inner.negotiated = None;
            inner.target = None;
            inner.store.mark_stale(StaleReason::Disconnected);
            inner.disconnected_reason = Some("detached".into());
            inner.bridge.take()
        };
        if let Some(handle) = bridge {
            let _ = handle.join();
        }
    }
}

fn state_label(inner: &Inner) -> (String, Option<String>) {
    if inner.gateway.is_none() {
        return (
            if inner.config.session.is_none() {
                "empty"
            } else {
                "disconnected"
            }
            .into(),
            inner.disconnected_reason.clone(),
        );
    }
    match inner.store.state() {
        SurfaceState::Empty => ("connecting".into(), None),
        SurfaceState::Live => ("live".into(), None),
        SurfaceState::Stale(reason) => ("stale".into(), Some(format!("{reason:?}").to_lowercase())),
    }
}

fn status_of(inner: &Inner) -> StatusDto {
    let (state, reason) = state_label(inner);
    let session_available = inner
        .config
        .session
        .as_ref()
        .map(|s| {
            herdr_client::bootstrap::session_available(&SessionPaths::for_session(
                &inner.config.config_dir,
                s,
            ))
        })
        .unwrap_or(false);
    StatusDto {
        session: inner.config.session.as_ref().map(|s| s.as_str().to_owned()),
        session_available,
        connected: inner.gateway.as_ref().is_some_and(|g| g.is_connected()),
        state,
        reason,
        generation: inner.negotiated.as_ref().map(|n| n.generation),
        connection_generation: inner
            .gateway
            .as_ref()
            .map(|g| g.connection_generation())
            .unwrap_or(0),
        boot_id: inner.target.as_ref().map(|t| t.boot_id.clone()),
        server_version: inner.negotiated.as_ref().map(|n| n.server_version.clone()),
        pane_id: inner.target.as_ref().map(|t| t.pane_id.clone()),
        last_error: inner.last_error.clone(),
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn terminal_status(state: State<'_, TerminalState>) -> StatusDto {
    let inner = state.inner.lock().expect("terminal state");
    status_of(&inner)
}

#[tauri::command]
pub fn terminal_connect(
    state: State<'_, TerminalState>,
    geometry: GeometryDto,
    on_event: Channel<FrameEvent>,
) -> Result<StatusDto, RuntimeError> {
    state.detach();
    let mut inner = state.inner.lock().expect("terminal state");
    let Some(session) = inner.config.session.clone() else {
        let error = RuntimeError::new("no_session", "no session is configured");
        inner.last_error = Some(error.clone());
        return Err(error);
    };
    let geometry: SurfaceGeometry = geometry.into();
    // Zero-config (AC-016-01): the TUI's own bootstrap — an absent default session is started
    // detached and waited for (10 s). A named session without `auto_start` keeps the previous
    // behavior: only the explicit `session_start` starts it.
    let bootstrap = if inner.config.auto_start {
        let cwd = std::env::current_dir().ok();
        ensure_session_running(
            &inner.config.herdr_bin,
            &inner.config.config_dir,
            &session,
            cwd.as_deref(),
            SESSION_START_TIMEOUT,
        )
    } else {
        Ok(())
    };
    let mut gateway = LocalGateway::new(&inner.config.config_dir, session);
    let negotiated = match bootstrap.and_then(|()| {
        gateway.connect(ConnectOptions {
            geometry,
            surface_active: true,
        })
    }) {
        Ok(negotiated) => negotiated,
        Err(error) => {
            // Not fatal: the window stays open in `disconnected` with a retryable error; the
            // bootstrap above is the only start this connect performs, and nothing else is
            // retried here (an explicit retry repeats it).
            inner.last_error = Some(error.clone());
            inner.disconnected_reason = Some(error.code.clone());
            let _ = on_event.send(FrameEvent::State {
                state: "disconnected".into(),
                reason: Some(error.code.clone()),
                error: Some(error.clone()),
            });
            if let Some(path) = inner.config.surface_trace.clone() {
                write_trace(&inner, &path);
            }
            return Err(error);
        }
    };
    let events = gateway
        .take_event_stream()
        .expect("fresh connection has an event stream");
    let _ = gateway.set_focus(true);
    inner.gateway = Some(gateway);
    inner.negotiated = Some(negotiated.clone());
    inner.geometry = Some(geometry);
    inner.store = FrameStore::new();
    inner.target = None;
    inner.shell_pid = None;
    inner.last_error = None;
    inner.disconnected_reason = None;
    inner.channel = Some(on_event.clone());
    let _ = on_event.send(FrameEvent::State {
        state: "connecting".into(),
        reason: None,
        error: None,
    });
    let shared = state.inner.clone();
    inner.bridge = Some(
        std::thread::Builder::new()
            .name("herdr-desktop-bridge".into())
            .spawn(move || bridge_loop(shared, events, on_event))
            .map_err(|e| RuntimeError::from_io_kind(e.kind(), "bridge thread"))?,
    );
    Ok(status_of(&inner))
}

#[tauri::command]
pub fn terminal_input(
    state: State<'_, TerminalState>,
    events: Vec<InputDto>,
) -> Result<(), RuntimeError> {
    let inner = state.inner.lock().expect("terminal state");
    let Some(gateway) = inner.gateway.as_ref() else {
        return Err(
            RuntimeError::new("not_connected", "disconnected from the Herdr server").retryable(),
        );
    };
    if !inner.store.input_allowed() {
        let (state_name, reason) = state_label(&inner);
        return Err(RuntimeError::new(
            "surface_stale",
            format!(
                "surface {state_name}{}: input is blocked until it resynchronizes",
                reason.map(|r| format!(" ({r})")).unwrap_or_default()
            ),
        )
        .retryable());
    }
    let Some(target) = inner.target.as_ref() else {
        return Err(RuntimeError::new("no_target", "no qualified target pane").retryable());
    };
    let pane = inner
        .store
        .surface()
        .and_then(|s| s.panes.iter().find(|p| p.pane_id == target.pane_id));
    let events = input_events_for_pane(pane, &events)?;
    gateway.send_input(target, events)
}

#[tauri::command]
pub fn terminal_resize(
    state: State<'_, TerminalState>,
    geometry: GeometryDto,
) -> Result<(), RuntimeError> {
    let mut inner = state.inner.lock().expect("terminal state");
    let geometry: SurfaceGeometry = geometry.into();
    if inner.geometry == Some(geometry) {
        return Ok(());
    }
    let Some(gateway) = inner.gateway.as_ref() else {
        inner.geometry = Some(geometry);
        return Ok(());
    };
    gateway.resize(geometry)?;
    inner.geometry = Some(geometry);
    Ok(())
}

#[tauri::command]
pub fn terminal_focus(state: State<'_, TerminalState>, focused: bool) -> Result<(), RuntimeError> {
    let inner = state.inner.lock().expect("terminal state");
    match inner.gateway.as_ref() {
        Some(gateway) => gateway.set_focus(focused),
        None => Ok(()),
    }
}

/// Surface lease (`client_shell.surface.set`) is independent of keyboard focus
/// (`ClientShellFocus` / `terminal_focus`). The server sizes a shared pane from the remaining
/// active client when this lease drops; blur must not drop it.
pub fn surface_lease_active(minimized: bool, visible: bool) -> bool {
    visible && !minimized
}

#[cfg(test)]
mod surface_lease_tests {
    use super::surface_lease_active;

    #[test]
    fn blur_does_not_drop_the_lease() {
        assert!(surface_lease_active(false, true));
        assert!(!surface_lease_active(true, true));
        assert!(!surface_lease_active(false, false));
        assert!(!surface_lease_active(true, false));
    }
}

#[tauri::command]
pub fn terminal_detach(state: State<'_, TerminalState>) -> StatusDto {
    state.detach();
    let inner = state.inner.lock().expect("terminal state");
    status_of(&inner)
}

/// Explicit user action: start the configured session detached from this window (the default
/// session included). Never runs on render; a running session is left untouched.
#[tauri::command]
pub fn session_start(state: State<'_, TerminalState>) -> Result<StatusDto, RuntimeError> {
    let mut inner = state.inner.lock().expect("terminal state");
    let Some(session) = inner.config.session.clone() else {
        return Err(RuntimeError::new("no_session", "no session is configured"));
    };
    let paths = SessionPaths::for_session(&inner.config.config_dir, &session);
    if herdr_client::bootstrap::session_available(&paths) {
        return Ok(status_of(&inner));
    }
    let herdr_bin = inner.config.herdr_bin.clone();
    let cwd = std::env::current_dir().ok();
    match start_session_detached(&herdr_bin, &session, cwd.as_deref())
        .and_then(|_| wait_for_session(&paths, SESSION_START_TIMEOUT))
    {
        Ok(()) => {
            inner.last_error = None;
            Ok(status_of(&inner))
        }
        Err(error) => {
            inner.last_error = Some(error.clone());
            Err(error)
        }
    }
}

// ---------------------------------------------------------------------------
// Bridge
// ---------------------------------------------------------------------------

/// `Full` event of a committed surface (cells carry their hyperlink index).
pub fn full_event(surface: &PaneSurfaceFrame) -> FrameEvent {
    FrameEvent::Full {
        revision: surface.surface_revision,
        width: surface.frame.width,
        height: surface.frame.height,
        cells: surface.frame.cells.iter().map(CellDto::from).collect(),
        cursor: surface.frame.cursor.as_ref().map(CursorDto::from),
        panes: surface
            .panes
            .iter()
            .map(|p| PaneDto {
                pane_id: p.pane_id.clone(),
                x: p.inner_rect.x,
                y: p.inner_rect.y,
                width: p.inner_rect.width,
                height: p.inner_rect.height,
                focused: p.focused,
            })
            .collect(),
    }
}

/// Longest hyperlink URI forwarded to the WebView.
pub const MAX_LINK_URI_BYTES: usize = 2048;

/// URIs with control characters or above the size limit become `""` (index kept).
pub fn sanitize_link_uri(uri: &str) -> String {
    if uri.len() > MAX_LINK_URI_BYTES || uri.chars().any(char::is_control) {
        String::new()
    } else {
        uri.to_owned()
    }
}

/// `Metadata` event that follows `full_event` for the same surface.
pub fn metadata_event(surface: &PaneSurfaceFrame) -> FrameEvent {
    FrameEvent::Metadata {
        revision: surface.surface_revision,
        panes: surface.panes.iter().map(PaneMetaDto::from).collect(),
        hyperlinks: Some(
            surface
                .frame
                .hyperlinks
                .iter()
                .map(|u| sanitize_link_uri(u))
                .collect(),
        ),
    }
}

/// `Patch` event plus the `Metadata` of its updated panes (none when no pane changed). Send
/// both only after the store applied the patch.
pub fn patch_events(patch: &PaneSurfacePatch) -> (FrameEvent, Option<FrameEvent>) {
    let rows = patch
        .rows
        .iter()
        .map(|r| RowDto {
            x: r.x,
            y: r.y,
            cells: r.cells.iter().map(CellDto::from).collect(),
        })
        .collect();
    let event = FrameEvent::Patch {
        revision: patch.surface_revision,
        rows,
        cursor: patch.cursor.as_ref().map(CursorDto::from),
    };
    let meta = (!patch.panes.is_empty()).then(|| FrameEvent::Metadata {
        revision: patch.surface_revision,
        panes: patch.panes.iter().map(PaneMetaDto::from).collect(),
        hyperlinks: None,
    });
    (event, meta)
}

// ---------------------------------------------------------------------------
// Validators for pane actions (the integrator registers the IPC and calls the endpoint)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusRequestDto {
    pub pane_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScrollRequestDto {
    pub pane_id: String,
    pub offset_from_bottom: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextPointDto {
    pub row: u32,
    pub col: u16,
}

/// `pane.selection.read` params: rows are absolute buffer rows
/// (`max_offset_from_bottom - offset_from_bottom + viewport_row`), columns pane-local.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectionRequestDto {
    pub pane_id: String,
    pub anchor: TextPointDto,
    pub cursor: TextPointDto,
    pub content_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkRequestDto {
    pub pane_id: String,
    pub uri: String,
    pub viewport_row: u16,
    pub col: u16,
    pub content_revision: u64,
}

fn find_pane<'a>(
    surface: &'a PaneSurfaceFrame,
    pane_id: &str,
) -> Result<&'a PaneSurfacePane, RuntimeError> {
    surface
        .panes
        .iter()
        .find(|p| p.pane_id == pane_id)
        .ok_or_else(|| RuntimeError::new("pane_not_found", format!("missing pane: {pane_id}")))
}

fn stale_content() -> RuntimeError {
    RuntimeError::new("stale_content", "the pane's content changed").retryable()
}

/// `Ok(true)` when the pane exists and is not focused (a `pane.focus` is required).
pub fn validate_focus_request(
    surface: &PaneSurfaceFrame,
    request: &FocusRequestDto,
) -> Result<bool, RuntimeError> {
    Ok(!find_pane(surface, &request.pane_id)?.focused)
}

/// Clamps the offset to the pane scrollback; panes without scroll metrics are refused.
pub fn validate_scroll_request(
    surface: &PaneSurfaceFrame,
    request: &ScrollRequestDto,
) -> Result<ScrollRequestDto, RuntimeError> {
    let pane = find_pane(surface, &request.pane_id)?;
    let Some(scroll) = pane.scroll else {
        return Err(RuntimeError::new(
            "scroll_unavailable",
            "the pane has no scrollable history",
        ));
    };
    Ok(ScrollRequestDto {
        pane_id: request.pane_id.clone(),
        offset_from_bottom: request
            .offset_from_bottom
            .min(scroll.max_offset_from_bottom),
    })
}

pub fn validate_selection_request(
    surface: &PaneSurfaceFrame,
    request: &SelectionRequestDto,
) -> Result<SelectionRequestDto, RuntimeError> {
    let pane = find_pane(surface, &request.pane_id)?;
    if pane.content_revision != request.content_revision {
        return Err(stale_content());
    }
    let rows = pane
        .scroll
        .map(|m| {
            m.max_offset_from_bottom
                .saturating_add(u64::from(pane.inner_rect.height))
        })
        .unwrap_or(u64::from(pane.inner_rect.height));
    let fits = |p: TextPointDto| p.col < pane.inner_rect.width && u64::from(p.row) < rows;
    if !fits(request.anchor) || !fits(request.cursor) {
        return Err(RuntimeError::new(
            "invalid_selection",
            "the selection is outside the pane's content",
        ));
    }
    Ok(request.clone())
}

/// Allowed link schemes: web URLs only (same rule as the reference client).
pub fn safe_web_uri(uri: &str) -> bool {
    (uri.starts_with("https://") || uri.starts_with("http://"))
        && uri == sanitize_link_uri(uri)
        && !uri.chars().any(char::is_whitespace)
}

/// Resolves the link at a pane-local cell and returns its URI when it still matches the request.
pub fn validate_link_request(
    surface: &PaneSurfaceFrame,
    request: &LinkRequestDto,
) -> Result<String, RuntimeError> {
    let pane = find_pane(surface, &request.pane_id)?;
    let inner = pane.inner_rect;
    if request.col >= inner.width || request.viewport_row >= inner.height {
        return Err(RuntimeError::new(
            "invalid_link",
            "the link is outside the pane",
        ));
    }
    if pane.content_revision != request.content_revision {
        return Err(stale_content());
    }
    let frame = &surface.frame;
    let x = usize::from(inner.x + request.col);
    let y = usize::from(inner.y + request.viewport_row);
    let uri = frame
        .cells
        .get(y * usize::from(frame.width) + x)
        .and_then(|c| c.hyperlink)
        .and_then(|i| frame.hyperlinks.get(i as usize));
    match uri {
        Some(uri) if *uri == request.uri => {
            if safe_web_uri(uri) {
                Ok(uri.clone())
            } else {
                Err(RuntimeError::new("unsafe_link", "link scheme not allowed"))
            }
        }
        _ => Err(RuntimeError::new("stale_link", "the link is no longer in this cell").retryable()),
    }
}

fn qualify_target(inner: &mut Inner, preferred_pane: Option<String>) -> Option<QualifiedTarget> {
    let gateway = inner.gateway.as_ref()?;
    let identity = gateway.identity()?;
    let pane_id = preferred_pane
        .or_else(|| inner.target.as_ref().map(|t| t.pane_id.clone()))
        .or_else(|| {
            inner.store.surface().and_then(|s| {
                s.panes
                    .iter()
                    .find(|p| p.focused)
                    .or(s.panes.first())
                    .map(|p| p.pane_id.clone())
            })
        })?;
    let target = QualifiedTarget::new(&identity, None, pane_id);
    inner.target = Some(target.clone());
    Some(target)
}

fn write_trace(inner: &Inner, path: &PathBuf) {
    let (state, reason) = state_label(inner);
    let surface = inner.store.surface();
    let trace = serde_json::json!({
        "state": state,
        "reason": reason,
        "connected": inner.gateway.as_ref().is_some_and(|g| g.is_connected()),
        // Same gate `terminal_input` applies: a connection and a live surface.
        "input_enabled": inner.gateway.is_some() && inner.store.input_allowed(),
        "last_error": inner.last_error,
        "revision": inner.store.revision().unwrap_or(0),
        "boot_id": inner.target.as_ref().map(|t| t.boot_id.clone()),
        "generation": inner.negotiated.as_ref().map(|n| n.generation),
        "connection_generation": inner.gateway.as_ref().map(|g| g.connection_generation()),
        "pane_id": inner.target.as_ref().map(|t| t.pane_id.clone()),
        "shell_pid": inner.shell_pid,
        "width": surface.map(|s| s.frame.width),
        "height": surface.map(|s| s.frame.height),
        "rows": inner.store.text_rows(),
        "stats": inner.store.stats(),
    });
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, serde_json::to_vec(&trace).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

fn bridge_loop(shared: Arc<Mutex<Inner>>, events: GatewayEvents, channel: Channel<FrameEvent>) {
    let trace_path = shared
        .lock()
        .expect("terminal state")
        .config
        .surface_trace
        .clone();
    while let Ok(event) = events.recv() {
        let mut inner = shared.lock().expect("terminal state");
        if inner.gateway.is_none() {
            break;
        }
        match event {
            GatewayEvent::Snapshot(snapshot) => {
                let preferred = snapshot.focused_pane_id.clone();
                if let Some(target) = qualify_target(&mut inner, preferred) {
                    if inner.shell_pid.is_none() {
                        inner.shell_pid = inner
                            .gateway
                            .as_ref()
                            .and_then(|g| g.api().pane_shell_pid(&target.pane_id).ok().flatten());
                    }
                    let negotiated = inner.negotiated.clone();
                    let _ = channel.send(FrameEvent::Identity {
                        boot_id: target.boot_id.clone(),
                        generation: negotiated.as_ref().map(|n| n.generation).unwrap_or(0),
                        connection_generation: target.connection_generation,
                        server_version: negotiated.map(|n| n.server_version).unwrap_or_default(),
                        pane_id: Some(target.pane_id),
                    });
                }
            }
            GatewayEvent::Surface(surface) => match inner.store.apply_full(*surface) {
                Ok(boot_changed) => {
                    if boot_changed || inner.target.is_none() {
                        inner.target = None;
                        qualify_target(&mut inner, None);
                        if inner.shell_pid.is_none() {
                            if let Some(target) = inner.target.clone() {
                                inner.shell_pid = inner.gateway.as_ref().and_then(|g| {
                                    g.api().pane_shell_pid(&target.pane_id).ok().flatten()
                                });
                            }
                        }
                    }
                    if let Some(surface) = inner.store.surface() {
                        let _ = channel.send(full_event(surface));
                        let _ = channel.send(metadata_event(surface));
                    }
                    let _ = channel.send(FrameEvent::State {
                        state: "live".into(),
                        reason: None,
                        error: None,
                    });
                }
                Err(reason) => {
                    let _ = channel.send(FrameEvent::State {
                        state: "stale".into(),
                        reason: Some(format!("{reason:?}").to_lowercase()),
                        error: None,
                    });
                }
            },
            GatewayEvent::Patch(patch) => {
                let (patch_event, patch_meta) = patch_events(&patch);
                match inner.store.apply_patch(*patch) {
                    ApplyOutcome::Applied => {
                        let _ = channel.send(patch_event);
                        if let Some(meta) = patch_meta {
                            let _ = channel.send(meta);
                        }
                    }
                    ApplyOutcome::Rejected(reason) => {
                        let _ = channel.send(FrameEvent::State {
                            state: "stale".into(),
                            reason: Some(format!("{reason:?}").to_lowercase()),
                            error: None,
                        });
                        request_full_surface(&inner);
                    }
                }
            }
            GatewayEvent::QueueOverflow { dropped_frames } => {
                inner.store.mark_stale(StaleReason::QueueOverflow);
                let _ = channel.send(FrameEvent::State {
                    state: "stale".into(),
                    reason: Some(format!("queue_overflow:{dropped_frames}")),
                    error: None,
                });
                request_full_surface(&inner);
            }
            GatewayEvent::ShellError(message) => {
                let _ = channel.send(FrameEvent::State {
                    state: "live".into(),
                    reason: Some("endpoint_error".into()),
                    error: Some(
                        RuntimeError::new("endpoint_error", message).with_endpoint("local"),
                    ),
                });
            }
            GatewayEvent::Shutdown(reason) => {
                inner.store.mark_stale(StaleReason::Disconnected);
                inner.disconnected_reason = Some("server_shutdown".into());
                let error = RuntimeError::new(
                    "server_shutdown",
                    reason.unwrap_or_else(|| "the server shut down".into()),
                )
                .retryable();
                inner.last_error = Some(error.clone());
                let _ = channel.send(FrameEvent::State {
                    state: "disconnected".into(),
                    reason: Some("server_shutdown".into()),
                    error: Some(error),
                });
            }
            GatewayEvent::Disconnected(error) => {
                // Loss after attach is recoverable: drop the gateway, keep the window and the
                // retryable error; nothing is restarted here.
                inner.store.mark_stale(StaleReason::Disconnected);
                inner.disconnected_reason = Some(error.code.clone());
                inner.last_error = Some(error.clone());
                inner.gateway = None;
                inner.negotiated = None;
                let _ = channel.send(FrameEvent::State {
                    state: "disconnected".into(),
                    reason: Some(error.code.clone()),
                    error: Some(error),
                });
                if let Some(path) = trace_path.as_ref() {
                    write_trace(&inner, path);
                }
                break;
            }
            GatewayEvent::EndpointResponse { .. }
            | GatewayEvent::KeyboardReportAll(_)
            | GatewayEvent::MouseCapture { .. }
            | GatewayEvent::Unsupported { .. } => {}
        }
        if let Some(path) = trace_path.as_ref() {
            write_trace(&inner, path);
        }
    }
}

/// Recovery = full surface with the current geometry; no input replay.
fn request_full_surface(inner: &Inner) {
    if let (Some(gateway), Some(geometry)) = (inner.gateway.as_ref(), inner.geometry) {
        let _ = gateway.resize(geometry);
    }
}
