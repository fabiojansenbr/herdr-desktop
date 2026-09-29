//! Frozen wire types (mirror of `src/protocol/wire.rs` plus the leaf types it reaches:
//! `input::WindowsKeyRecord`, `api::schema::AgentStatus`, `config::ToastHerdrPosition`).
//!
//! Only data is mirrored; engine-side conversions (crossterm, ratatui) are out of scope.
//! Every enum here is append-closed for generation 1: do not reorder or insert variants.

use serde::{Deserialize, Serialize};

/// Same-install private protocol version. Only used by `TerminalHello`, which the desktop
/// never sends; kept so the tag tests can build every variant.
pub const PROTOCOL_VERSION: u32 = 22;

/// Ratatui `Modifier` bits as transported in `CellData::modifier`.
pub mod modifier {
    pub const BOLD: u16 = 0b0000_0000_0001;
    pub const DIM: u16 = 0b0000_0000_0010;
    pub const ITALIC: u16 = 0b0000_0000_0100;
    pub const UNDERLINED: u16 = 0b0000_0000_1000;
    pub const SLOW_BLINK: u16 = 0b0000_0001_0000;
    pub const RAPID_BLINK: u16 = 0b0000_0010_0000;
    pub const REVERSED: u16 = 0b0000_0100_0000;
    pub const HIDDEN: u16 = 0b0000_1000_0000;
    pub const CROSSED_OUT: u16 = 0b0001_0000_0000;
    pub const UNDERLINE_STYLE_SHIFT: u16 = 12;
    pub const UNDERLINE_STYLE_MASK: u16 = 0xF000;
}

/// Crossterm `KeyModifiers` bits as transported in semantic key events.
pub mod key_modifiers {
    pub const NONE: u8 = 0;
    pub const SHIFT: u8 = 0b0000_0001;
    pub const CONTROL: u8 = 0b0000_0010;
    pub const ALT: u8 = 0b0000_0100;
    pub const SUPER: u8 = 0b0000_1000;
    pub const HYPER: u8 = 0b0001_0000;
    pub const META: u8 = 0b0010_0000;
}

/// Herdr underline style extension bits (0 = none).
pub fn underline_style(modifier_bits: u16) -> u8 {
    ((modifier_bits & modifier::UNDERLINE_STYLE_MASK) >> modifier::UNDERLINE_STYLE_SHIFT) as u8
}

/// Decoded form of the packed `u32` colour used by `CellData::fg`/`bg`.
///
/// Encoding (identical to `color_to_u32`): named colours are `0x00_00_00_XX` with
/// `XX` in `0..=16`; indexed palette is `0x01_00_00_XX`; RGB is `0x02_RR_GG_BB`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireColor {
    Reset,
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    Gray,
    DarkGray,
    LightRed,
    LightGreen,
    LightYellow,
    LightBlue,
    LightMagenta,
    LightCyan,
    White,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

impl WireColor {
    pub fn to_u32(self) -> u32 {
        match self {
            WireColor::Reset => 0x00,
            WireColor::Black => 0x01,
            WireColor::Red => 0x02,
            WireColor::Green => 0x03,
            WireColor::Yellow => 0x04,
            WireColor::Blue => 0x05,
            WireColor::Magenta => 0x06,
            WireColor::Cyan => 0x07,
            WireColor::Gray => 0x08,
            WireColor::DarkGray => 0x09,
            WireColor::LightRed => 0x0A,
            WireColor::LightGreen => 0x0B,
            WireColor::LightYellow => 0x0C,
            WireColor::LightBlue => 0x0D,
            WireColor::LightMagenta => 0x0E,
            WireColor::LightCyan => 0x0F,
            WireColor::White => 0x10,
            WireColor::Indexed(i) => 0x01_00_00_00 | u32::from(i),
            WireColor::Rgb(r, g, b) => {
                0x02_00_00_00 | (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
            }
        }
    }

    pub fn from_u32(val: u32) -> Self {
        match val >> 24 {
            0x00 => match val & 0xFF {
                0x00 => WireColor::Reset,
                0x01 => WireColor::Black,
                0x02 => WireColor::Red,
                0x03 => WireColor::Green,
                0x04 => WireColor::Yellow,
                0x05 => WireColor::Blue,
                0x06 => WireColor::Magenta,
                0x07 => WireColor::Cyan,
                0x08 => WireColor::Gray,
                0x09 => WireColor::DarkGray,
                0x0A => WireColor::LightRed,
                0x0B => WireColor::LightGreen,
                0x0C => WireColor::LightYellow,
                0x0D => WireColor::LightBlue,
                0x0E => WireColor::LightMagenta,
                0x0F => WireColor::LightCyan,
                0x10 => WireColor::White,
                _ => WireColor::Reset,
            },
            0x01 => WireColor::Indexed((val & 0xFF) as u8),
            0x02 => WireColor::Rgb(
                ((val >> 16) & 0xFF) as u8,
                ((val >> 8) & 0xFF) as u8,
                (val & 0xFF) as u8,
            ),
            _ => WireColor::Reset,
        }
    }
}

// ---------------------------------------------------------------------------
// Leaf types reached from the enums
// ---------------------------------------------------------------------------

/// Mirror of `input::model::WindowsKeyRecord`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowsKeyRecord {
    pub key_down: bool,
    pub repeat_count: u16,
    pub virtual_key_code: u16,
    pub virtual_scan_code: u16,
    pub unicode: u16,
    pub control_key_state: u32,
}

/// Mirror of `api::schema::AgentStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Idle,
    Working,
    Blocked,
    Done,
    Unknown,
}

/// Mirror of `config::model::ToastHerdrPosition`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ToastHerdrPosition {
    TopLeft,
    TopRight,
    BottomLeft,
    #[default]
    BottomRight,
}

// ---------------------------------------------------------------------------
// Client → Server
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RenderEncoding {
    SemanticFrame,
    TerminalAnsi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientSurfaceSize {
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientKeyKind {
    Press,
    Repeat,
    Release,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ClientKeyCode {
    Backspace,
    Enter,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Tab,
    BackTab,
    Delete,
    Insert,
    Esc,
    Char(char),
    F(u8),
    Null,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ClientMouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMouseKind {
    Down(ClientMouseButton),
    Up(ClientMouseButton),
    Drag(ClientMouseButton),
    Moved,
    ScrollUp,
    ScrollDown,
    ScrollLeft,
    ScrollRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMousePosition {
    Cell {
        column: u16,
        row: u16,
    },
    Pixels {
        x: u32,
        y: u32,
        column: u16,
        row: u16,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientMouseGeometry {
    pub cols: u16,
    pub rows: u16,
    pub width_px: u32,
    pub height_px: u32,
}

/// Pane-domain semantic input (`shell.input.semantic.v1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientPaneInputEvent {
    Key {
        code: ClientKeyCode,
        modifiers: u8,
        kind: ClientKeyKind,
        repeat_count: u16,
        shifted_codepoint: Option<u32>,
        generated_text: Option<String>,
        tracks_release: bool,
        physical_key_id: Option<u32>,
        windows_record: Option<WindowsKeyRecord>,
    },
    TextCommit(String),
    Mouse {
        kind: ClientMouseKind,
        position: ClientMousePosition,
        geometry: Option<ClientMouseGeometry>,
        modifiers: u8,
        lines: u16,
    },
    Paste(String),
}

impl ClientPaneInputEvent {
    /// A synthesized key press that never reports a release (desktop keyboards do not
    /// participate in kitty release tracking in this client).
    pub fn key_press(code: ClientKeyCode, modifiers: u8) -> Self {
        ClientPaneInputEvent::Key {
            code,
            modifiers,
            kind: ClientKeyKind::Press,
            repeat_count: 1,
            shifted_codepoint: None,
            generated_text: None,
            tracks_release: false,
            physical_key_id: None,
            windows_record: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientHostColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientHostDefaultColorKind {
    Foreground,
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientHostAppearance {
    Dark,
    Light,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientHostThemeUpdate {
    DefaultColor {
        kind: ClientHostDefaultColorKind,
        color: ClientHostColor,
    },
    PaletteColors(Vec<(u8, ClientHostColor)>),
    Appearance(ClientHostAppearance),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientClipboardImageTarget {
    DirectTerminal,
    Pane(String),
    Popup(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttachScrollDirection {
    Up,
    Down,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttachScrollSource {
    Wheel,
    PageKey { input: Vec<u8> },
}

/// Messages from client to server. Variant order frozen for generation 1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMessage {
    TerminalHello {
        version: u32,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
        pixel_mouse: bool,
    },
    Input {
        data: Vec<u8>,
    },
    ClipboardImage {
        target: ClientClipboardImageTarget,
        extension: String,
        data: Vec<u8>,
    },
    Resize {
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
        pixel_mouse: bool,
    },
    Detach,
    AttachTerminal {
        terminal_id: String,
        takeover: bool,
    },
    AttachScroll {
        source: AttachScrollSource,
        direction: AttachScrollDirection,
        lines: u16,
        column: Option<u16>,
        row: Option<u16>,
        modifiers: u8,
    },
    ObserveTerminal {
        target: String,
    },
    ControlTerminal {
        target: String,
        takeover: bool,
    },
    GraphicsTransmissionResult {
        transfer_id: u64,
        image_id: u32,
        success: bool,
    },
    GraphicsTransmissionStarted {
        transfer_id: u64,
        image_id: u32,
    },
    ClientShellHello {
        version: u32,
        cell_width_px: u32,
        cell_height_px: u32,
        surface_size: ClientSurfaceSize,
        pixel_mouse: bool,
        direct_graphics: bool,
        endpoint_keybindings: bool,
        mouse_capture: bool,
    },
    ClientShellResize {
        cell_width_px: u32,
        cell_height_px: u32,
        surface_size: ClientSurfaceSize,
        pixel_mouse: bool,
    },
    ClientShellPaneInput {
        pane_id: String,
        events: Vec<ClientPaneInputEvent>,
    },
    ClientShellPopupInput {
        terminal_id: String,
        events: Vec<ClientPaneInputEvent>,
    },
    ClientShellEndpointRequest {
        boot_id: String,
        request: String,
    },
    AttachMouse {
        kind: ClientMouseKind,
        position: ClientMousePosition,
        geometry: Option<ClientMouseGeometry>,
        modifiers: u8,
        lines: u16,
    },
    ClientShellHostTheme {
        update: ClientHostThemeUpdate,
    },
    ClientShellFocus {
        focused: bool,
    },
    ClientShellMouseCapture {
        enabled: bool,
    },
    /// Append-only named control; tag 20 and its two-string payload are frozen.
    EndpointControl {
        kind: String,
        data: String,
    },
}

// ---------------------------------------------------------------------------
// Server → Client
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellData {
    pub symbol: String,
    pub fg: u32,
    pub bg: u32,
    pub modifier: u16,
    pub skip: bool,
    pub hyperlink: Option<u32>,
}

pub type CursorShapeParam = u8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorState {
    pub x: u16,
    pub y: u16,
    pub visible: bool,
    #[serde(default)]
    pub shape: CursorShapeParam,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameData {
    pub cells: Vec<CellData>,
    pub width: u16,
    pub height: u16,
    pub cursor: Option<CursorState>,
    pub hyperlinks: Vec<String>,
    pub graphics: Vec<u8>,
}

impl FrameData {
    /// Whether `cells.len() == width * height`.
    pub fn is_consistent(&self) -> bool {
        self.cells.len() == usize::from(self.width) * usize::from(self.height)
    }
}

fn deserialize_client_shell_agent_status<'de, D>(deserializer: D) -> Result<AgentStatus, D::Error>
where
    D: serde::Deserializer<'de>,
{
    if !deserializer.is_human_readable() {
        return AgentStatus::deserialize(deserializer);
    }
    let value = String::deserialize(deserializer)?;
    Ok(match value.as_str() {
        "idle" => AgentStatus::Idle,
        "working" => AgentStatus::Working,
        "blocked" => AgentStatus::Blocked,
        "done" => AgentStatus::Done,
        _ => AgentStatus::Unknown,
    })
}

/// Initial resource projection (`shell.snapshot.v1`, delivered as JSON named control).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientShellSnapshot {
    pub boot_id: String,
    pub revision: u64,
    pub config_diagnostic: Option<String>,
    pub product_announcement: Option<ClientShellProductAnnouncement>,
    pub update_available: Option<String>,
    pub update_install_command: String,
    pub server_keybindings_toml: Option<String>,
    pub latest_release_notes_available: bool,
    pub integration_updates_available: bool,
    pub worktree_directory: String,
    pub release_notes: Option<ClientShellReleaseNotes>,
    pub focused_workspace_id: Option<String>,
    pub focused_tab_id: Option<String>,
    pub focused_pane_id: Option<String>,
    pub tab_bar_right: Vec<ClientShellTabStatusSegment>,
    pub tab_bar_right_separator: String,
    pub agent_view_label: Option<String>,
    pub agent_order: Vec<String>,
    pub workspaces: Vec<ClientShellWorkspace>,
    pub tabs: Vec<ClientShellTab>,
    pub panes: Vec<ClientShellPane>,
    pub agents: Vec<ClientShellAgent>,
    pub commands: Vec<ClientShellCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientShellProductAnnouncement {
    pub version: String,
    pub id: String,
    pub title: String,
    pub body: String,
    pub preview: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientShellReleaseNotes {
    pub version: String,
    pub body: String,
    pub preview: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientShellCommandAction {
    Shell,
    Pane,
    Popup,
    PluginAction,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientShellCommand {
    pub command_id: String,
    pub binding_label: String,
    pub binding_labels: Vec<String>,
    pub action: ClientShellCommandAction,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientShellTabStatusSegment {
    pub text: String,
    pub accent: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientShellWorkspace {
    pub workspace_id: String,
    pub active_tab_id: String,
    pub new_workspace_cwd: String,
    pub number: usize,
    pub label: String,
    pub custom_label: bool,
    pub branch: Option<String>,
    pub git_ahead_behind: Option<(usize, usize)>,
    pub tokens: Vec<(String, String)>,
    pub worktree: Option<ClientShellWorktree>,
    pub focused: bool,
    #[serde(deserialize_with = "deserialize_client_shell_agent_status")]
    pub agent_status: AgentStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientShellWorktree {
    pub key: String,
    pub label: String,
    pub is_linked_worktree: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientShellTab {
    pub tab_id: String,
    pub workspace_id: String,
    pub number: usize,
    pub label: String,
    pub custom_label: bool,
    pub zoomed: bool,
    pub focused: bool,
    #[serde(deserialize_with = "deserialize_client_shell_agent_status")]
    pub agent_status: AgentStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientShellPane {
    pub pane_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    pub label: Option<String>,
    pub cwd: Option<String>,
    pub foreground_cwd: Option<String>,
    pub focused: bool,
    pub right_click_passthrough: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientShellAgent {
    pub pane_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    pub name: Option<String>,
    pub display_agent: Option<String>,
    pub agent: Option<String>,
    pub title: Option<String>,
    pub terminal_title: Option<String>,
    pub terminal_title_stripped: Option<String>,
    #[serde(deserialize_with = "deserialize_client_shell_agent_status")]
    pub agent_status: AgentStatus,
    pub state_change_seq: u64,
    pub state_labels: Vec<(String, String)>,
    pub tokens: Vec<(String, String)>,
    pub focused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneSurfacePane {
    pub pane_id: String,
    pub content_revision: u64,
    pub rect: SurfaceRect,
    pub inner_rect: SurfaceRect,
    pub scrollbar_rect: Option<SurfaceRect>,
    pub scroll: Option<PaneSurfaceScrollMetrics>,
    pub focused: bool,
    pub mouse_reporting: bool,
    pub sgr_pixel_mouse: bool,
    pub alternate_screen_active: bool,
    pub pixel_width: u32,
    pub pixel_height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneSurfaceScrollMetrics {
    pub offset_from_bottom: u64,
    pub max_offset_from_bottom: u64,
    pub viewport_rows: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneSurfaceSplit {
    pub direction: PaneSurfaceSplitDirection,
    pub pos: u16,
    pub area: SurfaceRect,
    pub hit_rect: SurfaceRect,
    pub path: Vec<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaneSurfaceSplitDirection {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceRect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub enum SurfaceGraphicsTarget {
    Pane { pane_id: String },
    Popup { terminal_id: String },
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub enum SurfaceGraphicsSource {
    Terminal {
        target: SurfaceGraphicsTarget,
        image_id: u32,
    },
    PaneLayer {
        pane_id: String,
        layer_id: String,
    },
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub enum SurfaceGraphicsFormat {
    Rgb,
    Rgba,
    Png,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceGraphicsAssetKey {
    pub source: SurfaceGraphicsSource,
    pub image_width: u32,
    pub image_height: u32,
    pub format: SurfaceGraphicsFormat,
    pub data_len: u64,
    pub data_fingerprint: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceGraphicsAsset {
    pub key: SurfaceGraphicsAssetKey,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceGraphicsPlacement {
    pub asset: SurfaceGraphicsAssetKey,
    pub logical_placement_id: u32,
    pub x: u16,
    pub y: u16,
    pub cols: u32,
    pub rows: u32,
    pub source_x: u32,
    pub source_y: u32,
    pub source_width: u32,
    pub source_height: u32,
    pub x_offset: u32,
    pub y_offset: u32,
    pub z: i32,
    pub scrollback_offset: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceGraphicsScene {
    pub assets: Vec<SurfaceGraphicsAsset>,
    pub placements: Vec<SurfaceGraphicsPlacement>,
    pub retained_assets: Vec<SurfaceGraphicsAssetKey>,
}

/// One server-rendered active-tab surface (`shell.surface.v1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneSurfaceFrame {
    pub boot_id: String,
    pub projection_revision: u64,
    pub surface_revision: u64,
    pub frame: FrameData,
    pub panes: Vec<PaneSurfacePane>,
    pub splits: Vec<PaneSurfaceSplit>,
    pub popup: Option<Box<ClientShellPopupSurface>>,
    pub graphics: SurfaceGraphicsScene,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientShellPopupSize {
    Cells(u16),
    Percent(u8),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneSurfacePatchRow {
    pub x: u16,
    pub y: u16,
    pub cells: Vec<CellData>,
}

/// Incremental update against one committed complete pane surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneSurfacePatch {
    pub boot_id: String,
    pub projection_revision: u64,
    pub base_surface_revision: u64,
    pub surface_revision: u64,
    pub rows: Vec<PaneSurfacePatchRow>,
    pub panes: Vec<PaneSurfacePane>,
    pub cursor: Option<CursorState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientShellPopupSurface {
    pub terminal_id: String,
    pub title: String,
    pub width: Option<ClientShellPopupSize>,
    pub height: Option<ClientShellPopupSize>,
    pub frame: FrameData,
    pub mouse_reporting: bool,
    pub sgr_pixel_mouse: bool,
    pub pixel_width: u32,
    pub pixel_height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalFrame {
    pub seq: u64,
    pub width: u16,
    pub height: u16,
    pub full: bool,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotifyKind {
    Sound,
    Toast,
    SystemToast,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SemanticNotificationKind {
    NeedsAttention,
    Finished,
    UpdateInstalled,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SemanticNotificationSound {
    Done,
    Request,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticNotification {
    pub kind: SemanticNotificationKind,
    pub title: String,
    pub body: Option<String>,
    pub sound: Option<SemanticNotificationSound>,
    pub agent: Option<String>,
    pub workspace_id: Option<String>,
    pub tab_id: Option<String>,
    pub pane_id: Option<String>,
    pub position: Option<ToastHerdrPosition>,
}

/// Messages from server to client. Variant order frozen for generation 1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerMessage {
    Welcome {
        version: u32,
        encoding: RenderEncoding,
        error: Option<String>,
    },
    Terminal(TerminalFrame),
    Graphics {
        bytes: Vec<u8>,
    },
    ServerShutdown {
        reason: Option<String>,
    },
    Notify {
        kind: NotifyKind,
        message: String,
        body: Option<String>,
    },
    Clipboard {
        data: String,
    },
    WindowTitle {
        title: Option<String>,
    },
    ReloadSoundConfig,
    MouseCapture {
        enabled: bool,
        sgr_pixels: bool,
    },
    TerminalBell {
        count: u16,
    },
    GraphicsFile {
        path: String,
        expected_len: u64,
        image_id: u32,
        transfer_id: u64,
        leading: Vec<u8>,
        control: String,
        surface_asset: Option<SurfaceGraphicsAssetKey>,
    },
    GraphicsTransmissionRetired {
        transfer_id: u64,
        image_id: u32,
    },
    ClientShellSnapshot(Box<ClientShellSnapshot>),
    PaneSurface(PaneSurfaceFrame),
    SemanticNotification(SemanticNotification),
    ClientShellError {
        message: String,
    },
    DirectTerminalKeyboardProtocol {
        flags: u16,
        modify_other_keys_level: u8,
    },
    ClientShellKeyboardReportAll {
        enabled: bool,
    },
    ClientShellEndpointResponseChunk {
        boot_id: String,
        request_id: String,
        final_chunk: bool,
        data: Vec<u8>,
    },
    PaneSurfacePatch(PaneSurfacePatch),
    /// Append-only named control; tag 20 and its two-string payload are frozen.
    EndpointControl {
        kind: String,
        data: String,
    },
}

/// Highest ServerMessage tag known to generation 1 (`EndpointControl`).
pub const SERVER_MESSAGE_MAX_TAG: u32 = 20;
