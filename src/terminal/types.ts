// DTOs shared with src-tauri/src/terminal.rs (FrameEvent / InputDto / StatusDto).

export interface CellDto {
  s: string;
  fg: number;
  bg: number;
  m: number;
  /** Hyperlink index into the last full frame table (absent when the cell has no link). */
  h?: number;
}

export interface CursorDto {
  x: number;
  y: number;
  visible: boolean;
  shape: number;
}

export interface RowDto {
  x: number;
  y: number;
  cells: CellDto[];
}

export interface PaneDto {
  pane_id: string;
  x: number;
  y: number;
  width: number;
  height: number;
  focused: boolean;
}

export type FrameEvent =
  | {
      type: "identity";
      boot_id: string;
      generation: number;
      connection_generation: number;
      server_version: string;
      pane_id: string | null;
    }
  | {
      type: "full";
      revision: number;
      width: number;
      height: number;
      cells: CellDto[];
      cursor: CursorDto | null;
      panes: PaneDto[];
    }
  | { type: "patch"; revision: number; rows: RowDto[]; cursor: CursorDto | null }
  | { type: "state"; state: string; reason: string | null; error: RuntimeError | null }
  | { type: "metadata"; revision: number; panes: PaneMeta[]; hyperlinks: string[] | null };

export interface RectDto {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface ScrollDto {
  offset_from_bottom: number;
  max_offset_from_bottom: number;
  viewport_rows: number;
}

/** Per-pane metadata (FrameEvent `metadata`), mirrors terminal.rs PaneMetaDto. */
export interface PaneMeta {
  pane_id: string;
  content_revision: number;
  rect: RectDto;
  inner_rect: RectDto;
  scroll: ScrollDto | null;
  focused: boolean;
  mouse_reporting: boolean;
  sgr_pixel_mouse: boolean;
  alternate_screen_active: boolean;
  pixel_width: number;
  pixel_height: number;
}

export type MouseAction = "down" | "up" | "drag" | "moved" | "scroll_up" | "scroll_down" | "scroll_left" | "scroll_right";
export type MouseButton = "left" | "right" | "middle";

export interface PaneFocusRequest {
  pane_id: string;
  surface_revision: number;
}
export interface ScrollRequest {
  pane_id: string;
  offset_from_bottom: number;
}
export interface TextPoint {
  row: number;
  col: number;
}
/** `pane.selection.read` params: absolute buffer rows, pane-local columns. */
export interface SelectionRequest {
  pane_id: string;
  anchor: TextPoint;
  cursor: TextPoint;
  content_revision: number;
}
export interface LinkRequest {
  pane_id: string;
  uri: string;
  viewport_row: number;
  col: number;
  content_revision: number;
}
export type ActionResult = { ok: true } | { ok: false; code: string; message: string };

export interface RuntimeError {
  code: string;
  message: string;
  retryable: boolean;
  endpoint?: string;
}

export interface StatusDto {
  session: string | null;
  session_available: boolean;
  connected: boolean;
  state: string;
  reason: string | null;
  generation: number | null;
  connection_generation: number;
  boot_id: string | null;
  server_version: string | null;
  pane_id: string | null;
  last_error: RuntimeError | null;
}

export interface GeometryDto {
  cols: number;
  rows: number;
  cell_width_px: number;
  cell_height_px: number;
}

export type InputDto =
  | { kind: "text"; text: string }
  | { kind: "paste"; text: string }
  | { kind: "key"; code: string; modifiers: number; ch?: string }
  | {
      kind: "mouse";
      action: MouseAction;
      button?: MouseButton;
      column: number;
      row: number;
      pixel?: { x: number; y: number };
      geometry?: { cols: number; rows: number; width_px: number; height_px: number };
      modifiers: number;
      lines: number;
    };

// crossterm KeyModifiers bits (mirror of herdr-protocol::wire::key_modifiers)
export const MOD_SHIFT = 0b0000_0001;
export const MOD_CONTROL = 0b0000_0010;
export const MOD_ALT = 0b0000_0100;
export const MOD_SUPER = 0b0000_1000;

// ratatui Modifier bits carried in CellDto.m
export const M_BOLD = 0b0000_0000_0001;
export const M_DIM = 0b0000_0000_0010;
export const M_ITALIC = 0b0000_0000_0100;
export const M_UNDERLINED = 0b0000_0000_1000;
export const M_REVERSED = 0b0000_0100_0000;
export const M_HIDDEN = 0b0000_1000_0000;
export const M_CROSSED_OUT = 0b0001_0000_0000;
