// DTOs of the planned `selection` IPC module (spec 007). Source of truth:
// .local/orchestration/surface-contract.md (planned; Rust side lives in
// src-tauri/src/bridge/selection.rs). FrameEvent/StatusDto/GeometryDto/InputDto are reused from
// the terminal module.

import type { LiveIdentity } from "../agents/types";
import type { FrameEvent, GeometryDto, InputDto, RuntimeError, StatusDto } from "../terminal/types";

export type { FrameEvent, GeometryDto, InputDto, LiveIdentity, RuntimeError, StatusDto };

export interface SelectionDto {
  endpoint: string | null;
  kind: "local" | "ssh" | null;
  label: string | null;
  session: string | null;
  online: boolean;
  identity: LiveIdentity | null;
}

/** Result of `surface_interest` (null when no host is selected). */
export interface InterestOutcome {
  endpoint: string;
  active: boolean;
  /** `client_shell.surface.set` was sent to the connection. */
  sent: boolean;
  /** The connection announced the optimization; otherwise only the window suspends locally. */
  supported: boolean;
  floor: number | null;
}

/** Identity the frontend echoes back on `surface_input`; never assembled for another host. */
export interface SurfaceIdentityDto {
  endpoint: string;
  session: string;
  connection_generation: number;
  boot_id: string;
  pane_id: string;
}

/**
 * Result of `surface_paste_clipboard` (Rust `PasteReceipt`); never carries the clipboard text
 * or image bytes (they stay in the backend).
 */
export interface PasteReceipt {
  pane_id: string;
  /** One `Paste`/`ClipboardImage` was sent (false: the clipboard was empty). */
  sent: boolean;
  /** UTF-8 bytes of the pasted text, or bytes of the pasted image. */
  pasted_bytes: number;
  /**
   * What was pasted (spec 028); absent in older replies, read as text/empty from `sent`.
   *
   * `forward_key` is the r3 rule: in a Local host an image on the clipboard is not bridged
   * (the clipboard image bridge exists for remote clients, exactly as in the TUI); the backend
   * sends the Ctrl+V key once and the app reads the image itself.
   */
  kind?: "text" | "image" | "empty" | "forward_key";
}
