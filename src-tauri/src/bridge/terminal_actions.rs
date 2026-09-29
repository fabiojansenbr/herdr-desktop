//! Terminal actions of the composed window (007): focus a visible pane, scroll a pane's history,
//! copy a selection and open a link. Four bounded IPC commands, each carrying the full identity
//! the WebView captured with the action ([`SurfaceIdentityDto`]) and one concrete request DTO.
//!
//! - Every action validates, before any effect: the host is still the selected one, the
//!   connection (endpoint/session/generation/boot) is current, the engine's confirmed focused pane
//!   is still `expected.pane_id`, and the committed surface of that connection is live. The
//!   request is then validated against that surface with the `terminal` validators (pane present,
//!   scrollback clamp, content revision, cell link). The target is always the requested pane,
//!   never replaced by the focused one.
//! - Engine actions (`pane.focus`, `pane.scroll`, `pane.selection.read`) go through the hub's
//!   `run_endpoint_scoped_admitted` on the endpoint lane of that same connection (composed
//!   window): the attachment ticket taken at the call, focus, surface/interest and the request are
//!   checked again when the action gets the lane's turn, identity and pane under the host lock;
//!   the reply is awaited without locks and an unknown result is never re-sent. That admission
//!   reflects facts observed at that point; the lane still acquires its state/writer before the
//!   write, and nothing is claimed about the server's focus after it. A method the server does
//!   not announce is refused for that action only.
//! - Copy writes only the text returned by the engine for the requested pane, once, to the local
//!   native clipboard, and only when no newer copy started and the host/connection are still the
//!   same when the reply arrives. Links are opened once on the local desktop, only http/https URIs
//!   that match the current cell and content revision; nothing is sent to the host.
//! - Native effects are behind [`NativeEffects`] (Tauri clipboard-manager/opener plugins in the
//!   product, recorders in contracts). The WebView never receives a clipboard or opener command.

use std::future::Future;
#[cfg(target_os = "linux")]
use std::io::Read;
#[cfg(target_os = "linux")]
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use herdr_client::protocol::wire::PaneSurfaceFrame;
use herdr_client::{LiveIdentity, RuntimeError, SurfaceState};
use serde::Serialize;
use serde_json::{json, Value};

#[cfg(target_os = "linux")]
use super::composition::MAX_CLIPBOARD_IMAGE_PAYLOAD;
use super::composition::{
    ActionTicket, ClipboardPayload, ComposedSurface, PasteKind, SurfaceIdentityDto,
};
use super::selection::SelectionState;
use crate::connections::hub::EndpointScope;
use crate::terminal::{
    validate_focus_request, validate_link_request, validate_scroll_request,
    validate_selection_request, FocusRequestDto, LinkRequestDto, ScrollRequestDto,
    SelectionRequestDto,
};

/// Commands this module exposes (kept in sync with `lib.rs` by a test).
pub const COMMANDS: &[&str] = &[
    "surface_pane_focus",
    "surface_scroll",
    "surface_copy_selection",
    "surface_open_link",
    "surface_paste_clipboard",
    "surface_focus_host",
];

/// Largest selection text written to the clipboard.
pub const MAX_SELECTION_TEXT_BYTES: usize = 4 * 1024 * 1024;

/// One clipboard image read locally: encoded bytes (png/jpg/gif/webp/bmp) and their extension.
/// The bytes are never returned to the WebView; they go to the engine as a `ClipboardImage`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardImagePayload {
    pub bytes: Vec<u8>,
    pub extension: String,
}

/// Desktop-local effects of terminal actions.
pub trait NativeEffects: Send + Sync {
    /// Writes `text` (already returned by the engine) to the local clipboard.
    fn write_clipboard(&self, text: &str) -> Result<(), RuntimeError>;
    /// Opens a confirmed http/https link with the desktop's default handler.
    fn open_link(&self, uri: &str) -> Result<(), RuntimeError>;
    /// Reads the local clipboard text for a native paste (Rust only; never returned to the
    /// WebView). Effects without a clipboard reader refuse instead of fabricating a paste.
    fn read_clipboard(&self) -> Result<String, RuntimeError> {
        Err(native_paste_unavailable())
    }
    /// Reads an image from the local clipboard, when there is one (image has priority over text,
    /// as in the TUI). Effects without an image reader answer `None`: the text path is used.
    fn read_clipboard_image(&self) -> Result<Option<ClipboardImagePayload>, RuntimeError> {
        Ok(None)
    }
}

fn native_paste_unavailable() -> RuntimeError {
    RuntimeError::new(
        "native_paste_unavailable",
        "pasting through the native clipboard is not available here; nothing was sent",
    )
}

#[cfg(not(target_os = "linux"))]
fn image_encode_failed() -> RuntimeError {
    RuntimeError::new(
        "clipboard_image_failed",
        "could not encode the clipboard image; nothing was sent",
    )
}

/// Encodes one RGBA8 image (the shape arboard returns) as PNG, once, for the engine.
#[cfg(not(target_os = "linux"))]
fn encode_png(width: usize, height: usize, rgba: &[u8]) -> Result<Vec<u8>, RuntimeError> {
    let width = u32::try_from(width).map_err(|_| image_encode_failed())?;
    let height = u32::try_from(height).map_err(|_| image_encode_failed())?;
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|_| image_encode_failed())?;
        writer
            .write_image_data(rgba)
            .map_err(|_| image_encode_failed())?;
    }
    Ok(out)
}

/// MIME types of a clipboard image, in the TUI order (`../herdr/src/platform/linux.rs`): the first
/// offer whose magic bytes match its extension wins.
#[cfg(target_os = "linux")]
const CLIPBOARD_IMAGE_MIME_TYPES: [(&str, &str); 6] = [
    ("image/png", "png"),
    ("image/jpeg", "jpg"),
    ("image/jpg", "jpg"),
    ("image/gif", "gif"),
    ("image/webp", "webp"),
    ("image/bmp", "bmp"),
];

/// Clipboard sockets of this session: Wayland is tried before X11, exactly like the TUI.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LinuxClipboardSockets {
    pub wayland: bool,
    pub x11: bool,
}

#[cfg(target_os = "linux")]
impl LinuxClipboardSockets {
    pub fn from_env() -> Self {
        Self {
            wayland: std::env::var_os("WAYLAND_DISPLAY").is_some(),
            x11: std::env::var_os("DISPLAY").is_some(),
        }
    }
}

/// Reads one clipboard image through the same commands as the TUI: `wl-paste --type <mime>`
/// (Wayland) and `xclip -selection clipboard -t <mime> -o` (X11), validated by magic bytes so a
/// text/plain offer served for an image MIME is never bridged. `run` returns the bounded stdout
/// of one command; `None` means the offer (or the program) is unavailable.
#[cfg(target_os = "linux")]
pub fn read_linux_clipboard_image(
    sockets: LinuxClipboardSockets,
    mut run: impl FnMut(&str, &[&str]) -> Option<Vec<u8>>,
) -> Option<ClipboardImagePayload> {
    for (mime, extension) in CLIPBOARD_IMAGE_MIME_TYPES {
        if sockets.wayland {
            if let Some(bytes) = run("wl-paste", &["--type", mime]) {
                if bytes_match_image_signature(extension, &bytes) {
                    return Some(ClipboardImagePayload {
                        bytes,
                        extension: extension.into(),
                    });
                }
            }
        }
        if sockets.x11 {
            if let Some(bytes) = run("xclip", &["-selection", "clipboard", "-t", mime, "-o"]) {
                if bytes_match_image_signature(extension, &bytes) {
                    return Some(ClipboardImagePayload {
                        bytes,
                        extension: extension.into(),
                    });
                }
            }
        }
    }
    None
}

/// Magic bytes of one extension, mirroring the TUI validator.
#[cfg(target_os = "linux")]
fn bytes_match_image_signature(extension: &str, bytes: &[u8]) -> bool {
    match extension {
        "png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "jpg" => bytes.starts_with(&[0xFF, 0xD8, 0xFF]),
        "gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        "webp" => bytes.len() >= 12 && bytes.starts_with(b"RIFF") && bytes[8..12] == *b"WEBP",
        "bmp" => {
            if bytes.len() < 26 || !bytes.starts_with(b"BM") {
                return false;
            }
            let offset = u32::from_le_bytes([bytes[10], bytes[11], bytes[12], bytes[13]]) as usize;
            (26..=bytes.len()).contains(&offset)
        }
        _ => false,
    }
}

/// Reads at most `max` bytes of one clipboard command; `None` when the offer is empty, larger than
/// `max` or unreadable (the caller kills an oversized child).
#[cfg(target_os = "linux")]
pub fn read_bounded(mut reader: impl Read, max: usize) -> Option<Vec<u8>> {
    let mut buffer = Vec::new();
    let mut limited = reader.by_ref().take(max as u64 + 1);
    if limited.read_to_end(&mut buffer).is_err() {
        return None;
    }
    if buffer.is_empty() || buffer.len() > max {
        return None;
    }
    Some(buffer)
}

/// Runs one command of the TUI image reader, bounded to `MAX_CLIPBOARD_IMAGE_PAYLOAD`; a missing
/// program, a failed command or an oversized offer answer `None`.
#[cfg(target_os = "linux")]
fn run_clipboard_command(program: &str, args: &[&str]) -> Option<Vec<u8>> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let bytes = read_bounded(stdout, MAX_CLIPBOARD_IMAGE_PAYLOAD);
    if bytes.is_none() {
        let _ = child.kill();
    }
    let status = child.wait().ok()?;
    if !status.success() {
        return None;
    }
    bytes
}

/// Tauri implementation (clipboard-manager + opener plugins, initialized in `lib.rs`).
pub struct TauriEffects<R: tauri::Runtime> {
    app: tauri::AppHandle<R>,
}

impl<R: tauri::Runtime> TauriEffects<R> {
    pub fn new(app: tauri::AppHandle<R>) -> Self {
        Self { app }
    }
}

impl<R: tauri::Runtime> NativeEffects for TauriEffects<R> {
    fn write_clipboard(&self, text: &str) -> Result<(), RuntimeError> {
        use tauri_plugin_clipboard_manager::ClipboardExt;
        self.app.clipboard().write_text(text).map_err(|error| {
            RuntimeError::new(
                "clipboard_failed",
                format!("could not write to the clipboard: {error}"),
            )
            .retryable()
        })
    }

    fn read_clipboard(&self) -> Result<String, RuntimeError> {
        use tauri_plugin_clipboard_manager::ClipboardExt;
        self.app.clipboard().read_text().map_err(|error| {
            RuntimeError::new(
                "clipboard_failed",
                format!("could not read the clipboard: {error}"),
            )
            .retryable()
        })
    }

    /// Best-effort image probe of the local clipboard. On Linux it follows the proven TUI path
    /// (`wl-paste`/`xclip` per MIME, PNG/JPEG/GIF/WebP/BMP validated by signature): arboard only
    /// reads `image/png` there and its Wayland backend is not the one the TUI validated. On the
    /// other platforms arboard stays the platform reader (RGBA encoded to PNG once here). No image
    /// (or no clipboard owner at all) answers `None` and the text path continues.
    fn read_clipboard_image(&self) -> Result<Option<ClipboardImagePayload>, RuntimeError> {
        #[cfg(target_os = "linux")]
        {
            Ok(read_linux_clipboard_image(
                LinuxClipboardSockets::from_env(),
                run_clipboard_command,
            ))
        }
        #[cfg(not(target_os = "linux"))]
        {
            let Ok(mut clipboard) = arboard::Clipboard::new() else {
                return Ok(None);
            };
            let image = match clipboard.get_image() {
                Ok(image) => image,
                Err(arboard::Error::ContentNotAvailable) => return Ok(None),
                Err(_) => return Ok(None),
            };
            if image.bytes.is_empty() || image.width == 0 || image.height == 0 {
                return Ok(None);
            }
            let bytes = encode_png(image.width, image.height, &image.bytes)?;
            Ok(Some(ClipboardImagePayload {
                bytes,
                extension: "png".into(),
            }))
        }
    }

    fn open_link(&self, uri: &str) -> Result<(), RuntimeError> {
        use tauri_plugin_opener::OpenerExt;
        self.app
            .opener()
            .open_url(uri, None::<&str>)
            .map_err(|error| {
                RuntimeError::new(
                    "open_link_failed",
                    format!("could not open the link on the desktop: {error}"),
                )
                .retryable()
            })
    }
}

/// Outcome of an accepted action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActionReceipt {
    pub pane_id: String,
    /// A command was sent to the engine (false: already focused, or a link opened locally).
    pub sent: bool,
    /// Offset actually requested (scroll, clamped to the pane's history).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset_from_bottom: Option<u64>,
    /// Bytes written to the clipboard (copy).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copied_bytes: Option<usize>,
}

impl ActionReceipt {
    fn new(pane_id: &str, sent: bool) -> Self {
        Self {
            pane_id: pane_id.to_owned(),
            sent,
            offset_from_bottom: None,
            copied_bytes: None,
        }
    }
}

/// Outcome of a native paste; never carries the clipboard text or image bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PasteReceipt {
    pub pane_id: String,
    /// One paste was sent (false: the clipboard had nothing to paste).
    pub sent: bool,
    /// UTF-8 bytes of the pasted text, or bytes of the pasted image.
    pub pasted_bytes: usize,
    /// What was pasted: `text`, `image` or `empty` (nothing was sent).
    pub kind: PasteKind,
}

/// One action with its captured identity (the dispatch unit of the IPC commands).
#[derive(Debug, Clone)]
pub enum TerminalAction {
    Focus {
        expected: SurfaceIdentityDto,
        request: FocusRequestDto,
    },
    Scroll {
        expected: SurfaceIdentityDto,
        request: ScrollRequestDto,
    },
    CopySelection {
        expected: SurfaceIdentityDto,
        request: SelectionRequestDto,
    },
    OpenLink {
        expected: SurfaceIdentityDto,
        request: LinkRequestDto,
    },
}

struct Shared {
    selection: SelectionState,
    /// Composed window whose attachment tickets fence the actions (`None`: standalone seam).
    surface: Option<ComposedSurface>,
    effects: Arc<dyn NativeEffects>,
    /// Sequence of the newest copy; an older reply is never written after it.
    copies: AtomicU64,
}

/// Terminal actions of the selected host.
#[derive(Clone)]
pub struct TerminalActions {
    shared: Arc<Shared>,
}

fn live_of(expected: &SurfaceIdentityDto) -> LiveIdentity {
    LiveIdentity {
        endpoint: expected.endpoint.clone(),
        session: expected.session.clone(),
        connection_generation: expected.connection_generation,
        boot_id: expected.boot_id.clone(),
    }
}

fn selection_changed(endpoint: &str) -> RuntimeError {
    RuntimeError::new(
        "selection_changed",
        "the selected host changed; the action was cancelled",
    )
    .with_endpoint(endpoint)
}

fn action_cancelled(endpoint: &str) -> RuntimeError {
    RuntimeError::new(
        "action_cancelled",
        "the surface changed before this action's turn; nothing was sent",
    )
    .with_endpoint(endpoint)
}

impl TerminalActions {
    /// Standalone seam without a composed window: checks selection/connection/focus/surface at
    /// the call and under the host lock, but has no attachment ticket and no admission at the
    /// lane's turn. The window installs [`TerminalActions::composed`].
    pub fn new(selection: SelectionState, effects: Arc<dyn NativeEffects>) -> Self {
        Self {
            shared: Arc::new(Shared {
                selection,
                surface: None,
                effects,
                copies: AtomicU64::new(0),
            }),
        }
    }

    /// Actions of the composed window: each one captures the attachment ticket of `surface` at
    /// the call and is admitted again at the endpoint lane's turn, before the lane writes
    /// (ticket, selection, connection, confirmed focus, live surface/interest and the request
    /// revalidated against the current surface). A lane that cannot run that admission refuses.
    pub fn composed(surface: ComposedSurface, effects: Arc<dyn NativeEffects>) -> Self {
        Self {
            shared: Arc::new(Shared {
                selection: surface.selection().clone(),
                surface: Some(surface),
                effects,
                copies: AtomicU64::new(0),
            }),
        }
    }

    fn hub(&self) -> &crate::connections::hub::HostHub {
        self.shared.selection.connections().hub()
    }

    fn check_selected(&self, endpoint: &str) -> Result<(), RuntimeError> {
        if self.shared.selection.selected().as_deref() == Some(endpoint) {
            Ok(())
        } else {
            Err(selection_changed(endpoint))
        }
    }

    /// Ticket of an action issued now (composed window only).
    fn ticket(&self, expected: &SurfaceIdentityDto) -> Result<Option<ActionTicket>, RuntimeError> {
        match &self.shared.surface {
            Some(surface) => surface.action_ticket(expected).map(Some),
            None => Ok(None),
        }
    }

    fn check_ticket(&self, ticket: Option<&ActionTicket>) -> Result<(), RuntimeError> {
        match (&self.shared.surface, ticket) {
            (Some(surface), Some(ticket)) => surface.check_action_ticket(ticket),
            (None, None) => Ok(()),
            // A composed action always carries its ticket; anything else is refused.
            (_, _) => Err(RuntimeError::new(
                "action_cancelled",
                "the action does not belong to this surface; nothing was sent",
            )),
        }
    }

    /// Selected host, current connection, unchanged confirmed focus and a live committed surface
    /// of that connection. Returns the surface the request is validated against.
    fn prepare(
        &self,
        expected: &SurfaceIdentityDto,
    ) -> Result<(LiveIdentity, PaneSurfaceFrame), RuntimeError> {
        let endpoint = expected.endpoint.as_str();
        self.check_selected(endpoint)?;
        let live = live_of(expected);
        let hub = self.hub();
        hub.check_identity(&live)?;
        let link = hub.link(endpoint)?;
        if link.identity.as_ref() != Some(&live) {
            // Changed between the two reads: judged again by the identity check.
            hub.check_identity(&live)?;
            return Err(selection_changed(endpoint));
        }
        let focused = link.focused.as_ref().map(|(pane, _)| pane.as_str());
        if focused != Some(expected.pane_id.as_str()) {
            return Err(RuntimeError::new(
                "focus_changed",
                "the focused pane changed since the action; nothing was sent",
            )
            .retryable()
            .with_endpoint(endpoint));
        }
        match link.surface {
            Some(SurfaceState::Live) => {}
            Some(SurfaceState::Stale(_)) => {
                return Err(RuntimeError::new(
                    "surface_stale",
                    "the surface is resynchronizing; try again",
                )
                .retryable()
                .with_endpoint(endpoint))
            }
            _ => {
                return Err(RuntimeError::new(
                    "surface_unavailable",
                    "this host's surface is not available yet",
                )
                .retryable()
                .with_endpoint(endpoint))
            }
        }
        let frame = hub.surface_for(&live).ok_or_else(|| {
            RuntimeError::new(
                "surface_unavailable",
                "this host's surface is not available yet",
            )
            .retryable()
            .with_endpoint(endpoint)
        })?;
        Ok((live, frame))
    }

    /// Ticket, then [`Self::prepare`]; `check` revalidates the request against that surface.
    fn prepare_admitted<T>(
        &self,
        expected: &SurfaceIdentityDto,
        ticket: Option<&ActionTicket>,
        check: impl FnOnce(&PaneSurfaceFrame) -> Result<T, RuntimeError>,
    ) -> Result<(LiveIdentity, T), RuntimeError> {
        self.check_ticket(ticket)?;
        let (live, frame) = self.prepare(expected)?;
        let value = check(&frame).map_err(|e| e.with_endpoint(expected.endpoint.as_str()))?;
        Ok((live, value))
    }

    /// Sends one command for the requested pane (never replaced by the focused one). In the
    /// composed window, `admit` (ticket + prepare + request revalidation) runs again at the
    /// lane's turn, before the lane writes; a refusal sends nothing.
    fn send(
        &self,
        live: &LiveIdentity,
        pane_id: &str,
        method: &str,
        params: Value,
        admit: &dyn Fn() -> Result<(), RuntimeError>,
    ) -> Result<Value, RuntimeError> {
        let scope = EndpointScope::Pane {
            pane_id: pane_id.to_owned(),
            workspace_id: None,
        };
        if self.shared.surface.is_some() {
            self.hub()
                .run_endpoint_scoped_admitted(live, &scope, method, params, admit)
        } else {
            self.hub().run_endpoint_scoped(live, &scope, method, params)
        }
    }

    /// `pane.focus` for a visible pane of the current surface; nothing is sent when the engine
    /// already confirms it focused.
    pub fn focus(
        &self,
        expected: &SurfaceIdentityDto,
        request: &FocusRequestDto,
    ) -> Result<ActionReceipt, RuntimeError> {
        let ticket = self.ticket(expected)?;
        self.focus_ticketed(expected, request, ticket.as_ref())
    }

    fn focus_ticketed(
        &self,
        expected: &SurfaceIdentityDto,
        request: &FocusRequestDto,
        ticket: Option<&ActionTicket>,
    ) -> Result<ActionReceipt, RuntimeError> {
        let pane = request.pane_id.as_str();
        let (live, needed) = self.prepare_admitted(expected, ticket, |frame| {
            validate_focus_request(frame, request)
        })?;
        if !needed || expected.pane_id == pane {
            return Ok(ActionReceipt::new(pane, false));
        }
        self.send(
            &live,
            pane,
            "pane.focus",
            json!({ "pane_id": pane }),
            &|| {
                let (_, needed) = self.prepare_admitted(expected, ticket, |frame| {
                    validate_focus_request(frame, request)
                })?;
                if needed {
                    Ok(())
                } else {
                    Err(action_cancelled(&expected.endpoint))
                }
            },
        )?;
        Ok(ActionReceipt::new(pane, true))
    }

    /// `pane.scroll` with the offset clamped to the pane's history.
    pub fn scroll(
        &self,
        expected: &SurfaceIdentityDto,
        request: &ScrollRequestDto,
    ) -> Result<ActionReceipt, RuntimeError> {
        let ticket = self.ticket(expected)?;
        self.scroll_ticketed(expected, request, ticket.as_ref())
    }

    fn scroll_ticketed(
        &self,
        expected: &SurfaceIdentityDto,
        request: &ScrollRequestDto,
        ticket: Option<&ActionTicket>,
    ) -> Result<ActionReceipt, RuntimeError> {
        let (live, clamped) = self.prepare_admitted(expected, ticket, |frame| {
            validate_scroll_request(frame, request)
        })?;
        let pane = clamped.pane_id.as_str();
        self.send(
            &live,
            pane,
            "pane.scroll",
            json!({ "pane_id": pane, "offset_from_bottom": clamped.offset_from_bottom }),
            &|| {
                let (_, now) = self.prepare_admitted(expected, ticket, |frame| {
                    validate_scroll_request(frame, request)
                })?;
                if now.pane_id == clamped.pane_id
                    && now.offset_from_bottom == clamped.offset_from_bottom
                {
                    Ok(())
                } else {
                    Err(action_cancelled(&expected.endpoint))
                }
            },
        )?;
        let mut receipt = ActionReceipt::new(pane, true);
        receipt.offset_from_bottom = Some(clamped.offset_from_bottom);
        Ok(receipt)
    }

    /// `pane.selection.read`, then the engine's text for that pane to the local clipboard, once.
    pub fn copy_selection(
        &self,
        expected: &SurfaceIdentityDto,
        request: &SelectionRequestDto,
    ) -> Result<ActionReceipt, RuntimeError> {
        let ticket = self.ticket(expected)?;
        self.copy_ticketed(expected, request, ticket.as_ref())
    }

    fn copy_ticketed(
        &self,
        expected: &SurfaceIdentityDto,
        request: &SelectionRequestDto,
        ticket: Option<&ActionTicket>,
    ) -> Result<ActionReceipt, RuntimeError> {
        let endpoint = expected.endpoint.as_str();
        let (live, request) = self.prepare_admitted(expected, ticket, |frame| {
            validate_selection_request(frame, request)
        })?;
        let pane = request.pane_id.as_str();
        let seq = self.shared.copies.fetch_add(1, Ordering::AcqRel) + 1;
        let reply = self.send(
            &live,
            pane,
            "pane.selection.read",
            json!({
                "pane_id": pane,
                "anchor": { "row": request.anchor.row, "col": request.anchor.col },
                "cursor": { "row": request.cursor.row, "col": request.cursor.col },
                "content_revision": request.content_revision,
            }),
            &|| {
                self.prepare_admitted(expected, ticket, |frame| {
                    validate_selection_request(frame, &request)
                })
                .map(|_| ())
            },
        )?;
        let invalid = || {
            RuntimeError::new(
                "selection_reply_invalid",
                "unexpected server response while reading the selection; nothing was copied",
            )
            .with_endpoint(endpoint)
        };
        if reply.get("type").and_then(Value::as_str) != Some("pane_selection") {
            return Err(invalid());
        }
        if reply.get("pane_id").and_then(Value::as_str) != Some(pane) {
            return Err(RuntimeError::new(
                "selection_reply_mismatch",
                "the server answered with another pane's selection; nothing was copied",
            )
            .with_endpoint(endpoint));
        }
        let text = reply
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?;
        if text.len() > MAX_SELECTION_TEXT_BYTES {
            return Err(RuntimeError::new(
                "selection_too_large",
                "the selection exceeds the clipboard limit; nothing was copied",
            )
            .with_endpoint(endpoint));
        }
        // The reply may arrive after the window moved on (selection, connection, attachment or
        // interest): never write it then.
        self.check_selected(endpoint)?;
        self.hub().check_identity(&live)?;
        self.check_ticket(ticket)?;
        if self.shared.copies.load(Ordering::Acquire) != seq {
            return Err(RuntimeError::new(
                "copy_superseded",
                "a newer copy replaced this one; nothing was copied",
            )
            .with_endpoint(endpoint));
        }
        self.shared
            .effects
            .write_clipboard(text)
            .map_err(|e| e.with_endpoint(endpoint))?;
        let mut receipt = ActionReceipt::new(pane, true);
        receipt.copied_bytes = Some(text.len());
        Ok(receipt)
    }

    /// Opens the link of the current cell on the local desktop, once.
    pub fn open_link(
        &self,
        expected: &SurfaceIdentityDto,
        request: &LinkRequestDto,
    ) -> Result<ActionReceipt, RuntimeError> {
        let ticket = self.ticket(expected)?;
        self.open_link_ticketed(expected, request, ticket.as_ref())
    }

    fn open_link_ticketed(
        &self,
        expected: &SurfaceIdentityDto,
        request: &LinkRequestDto,
        ticket: Option<&ActionTicket>,
    ) -> Result<ActionReceipt, RuntimeError> {
        let endpoint = expected.endpoint.as_str();
        let (_, uri) = self.prepare_admitted(expected, ticket, |frame| {
            validate_link_request(frame, request)
        })?;
        // Nothing waits between here and the effect; the ticket is checked right before it.
        self.check_ticket(ticket)?;
        self.shared
            .effects
            .open_link(&uri)
            .map_err(|e| e.with_endpoint(endpoint))?;
        Ok(ActionReceipt::new(&request.pane_id, false))
    }

    /// Entry of the IPC commands: the attachment ticket is captured now, at the call, before any
    /// wait; the returned future runs the action on a blocking worker, off the calling (GUI)
    /// thread, and delivers its result once. Nothing is retried.
    pub fn dispatch(
        self,
        action: TerminalAction,
    ) -> impl Future<Output = Result<ActionReceipt, RuntimeError>> + Send + 'static {
        let expected = match &action {
            TerminalAction::Focus { expected, .. }
            | TerminalAction::Scroll { expected, .. }
            | TerminalAction::CopySelection { expected, .. }
            | TerminalAction::OpenLink { expected, .. } => expected,
        };
        let ticket = self.ticket(expected);
        async move {
            let ticket = ticket?;
            tauri::async_runtime::spawn_blocking(move || {
                let ticket = ticket.as_ref();
                match action {
                    TerminalAction::Focus { expected, request } => {
                        self.focus_ticketed(&expected, &request, ticket)
                    }
                    TerminalAction::Scroll { expected, request } => {
                        self.scroll_ticketed(&expected, &request, ticket)
                    }
                    TerminalAction::CopySelection { expected, request } => {
                        self.copy_ticketed(&expected, &request, ticket)
                    }
                    TerminalAction::OpenLink { expected, request } => {
                        self.open_link_ticketed(&expected, &request, ticket)
                    }
                }
            })
            .await
            .map_err(|_| RuntimeError::new("action_failed", "internal terminal action failure"))?
        }
    }
}

impl TerminalActions {
    /// Native Ctrl+Shift+V of the composed window: the clipboard is read by [`NativeEffects`] at
    /// the paste's turn in the surface input lane and sent there once
    /// ([`ComposedSurface::dispatch_clipboard_with`]); admission happens now, at the call. An image
    /// on the clipboard has priority over text, exactly as the TUI (`clipboard_images.rs`), and
    /// goes out as one `ClipboardImage` for the confirmed focused pane on a remote (SSH) host. On
    /// the Local host the r3 rule applies: the image is not bridged, the Ctrl+V key is sent once
    /// and the locally running app reads the clipboard itself, as in a local TUI session. The
    /// standalone seam has no input lane and refuses (`native_paste_unavailable`).
    pub fn dispatch_paste(
        &self,
        expected: SurfaceIdentityDto,
    ) -> impl Future<Output = Result<PasteReceipt, RuntimeError>> + Send + 'static {
        let pane_id = expected.pane_id.clone();
        let effects = self.shared.effects.clone();
        let dispatched = self.shared.surface.as_ref().map(|surface| {
            surface.dispatch_clipboard_with(expected, move || {
                if let Some(image) = effects.read_clipboard_image()? {
                    return Ok(ClipboardPayload::Image {
                        extension: image.extension,
                        data: image.bytes,
                    });
                }
                Ok(ClipboardPayload::Text(effects.read_clipboard()?))
            })
        });
        async move {
            let sent = dispatched.ok_or_else(native_paste_unavailable)?.await?;
            Ok(PasteReceipt {
                pane_id,
                sent: sent.kind != PasteKind::Empty,
                pasted_bytes: sent.bytes,
                kind: sent.kind,
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Commands (async: validation, host wait and native effects run off the GUI thread)
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn surface_pane_focus(
    state: tauri::State<'_, TerminalActions>,
    expected: SurfaceIdentityDto,
    request: FocusRequestDto,
) -> Result<ActionReceipt, RuntimeError> {
    let actions = state.inner().clone();
    actions
        .dispatch(TerminalAction::Focus { expected, request })
        .await
}

#[tauri::command]
pub async fn surface_scroll(
    state: tauri::State<'_, TerminalActions>,
    expected: SurfaceIdentityDto,
    request: ScrollRequestDto,
) -> Result<ActionReceipt, RuntimeError> {
    let actions = state.inner().clone();
    actions
        .dispatch(TerminalAction::Scroll { expected, request })
        .await
}

#[tauri::command]
pub async fn surface_copy_selection(
    state: tauri::State<'_, TerminalActions>,
    expected: SurfaceIdentityDto,
    request: SelectionRequestDto,
) -> Result<ActionReceipt, RuntimeError> {
    let actions = state.inner().clone();
    actions
        .dispatch(TerminalAction::CopySelection { expected, request })
        .await
}

#[tauri::command]
pub async fn surface_open_link(
    state: tauri::State<'_, TerminalActions>,
    expected: SurfaceIdentityDto,
    request: LinkRequestDto,
) -> Result<ActionReceipt, RuntimeError> {
    let actions = state.inner().clone();
    actions
        .dispatch(TerminalAction::OpenLink { expected, request })
        .await
}

/// Label of the composed product window (the single window of `tauri.conf.json`).
pub const PRODUCT_WINDOW_LABEL: &str = "main";

/// Window that invoked [`surface_focus_host`] (Tauri's `WebviewWindow` in the product).
pub trait HostFocusWindow {
    fn label(&self) -> &str;
    /// Asks the platform to focus this window (one request, never retried).
    fn set_focus(&self) -> Result<(), RuntimeError>;
}

/// Returns keyboard focus to the WebView of the invoking window when it is the product window; any
/// other window is refused untouched. A failed request is reported as an error.
pub fn focus_host(window: &impl HostFocusWindow) -> Result<(), RuntimeError> {
    if window.label() != PRODUCT_WINDOW_LABEL {
        return Err(RuntimeError::new(
            "focus_host_refused",
            "the host's focus is only returned to the main window",
        ));
    }
    window.set_focus()
}

impl<R: tauri::Runtime> HostFocusWindow for tauri::WebviewWindow<R> {
    fn label(&self) -> &str {
        tauri::WebviewWindow::label(self)
    }
    /// Widget focus of the window's WebView (wry `grab_focus` on GTK), not the toplevel.
    fn set_focus(&self) -> Result<(), RuntimeError> {
        tauri::Webview::set_focus(self.as_ref()).map_err(|_| {
            RuntimeError::new(
                "focus_host_failed",
                "could not return keyboard focus to the window",
            )
        })
    }
}

/// After a modal closes (ConnectionDialog), the page asks the host to return keyboard focus to the
/// WebView (GUI r9: the WebView lost GTK widget focus when the modal left). No WebView arguments:
/// only the window Tauri injects for the invoking webview is focused.
#[tauri::command]
pub async fn surface_focus_host<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
) -> Result<(), RuntimeError> {
    focus_host(&window)
}

/// Ctrl+Shift+V on the active terminal: identity only; the clipboard text stays in the backend.
#[tauri::command]
pub async fn surface_paste_clipboard(
    state: tauri::State<'_, TerminalActions>,
    expected: SurfaceIdentityDto,
) -> Result<PasteReceipt, RuntimeError> {
    let dispatched = state.dispatch_paste(expected);
    dispatched.await
}
