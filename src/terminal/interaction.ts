// Pointer, wheel, selection, link and pane-focus decisions of the terminal canvas (spec 007,
// AC-007-02). Pure logic: TerminalView feeds DOM events in and forwards the results through its
// callbacks; nothing here touches the DOM, the clipboard or IPC. Coordinates:
// - surface cell: column/row of the whole canvas (one canvas per surface);
// - pane-local cell: relative to the pane `inner_rect` (what the engine expects for mouse/links);
// - absolute row: `max_offset_from_bottom - offset_from_bottom + viewport_row` (pane.selection.read).

import type { KeyLike } from "./input";
import type { TerminalGrid } from "./grid";
import type { RowDecor } from "./renderer";
import type {
  FrameEvent,
  GeometryDto,
  InputDto,
  LinkRequest,
  MouseAction,
  MouseButton,
  PaneMeta,
  ScrollRequest,
  SelectionRequest,
} from "./types";

export interface CellSize {
  cellWidth: number;
  cellHeight: number;
}

/** Pane metadata and hyperlink table that belong to the grid's current revision. */
export class SurfaceMeta {
  revision = 0;
  panes: PaneMeta[] = [];
  links: string[] = [];
  /** True between a full frame and its metadata: layout unknown, pointer targets invalid. */
  stale = false;
  private safe: boolean[] = [];

  /** `validate` runs once per URI of each new link table (never per cell or frame). */
  constructor(private readonly validate: (uri: string) => boolean = isSafeWebUri) {}

  invalidate(): void {
    this.stale = true;
  }

  safeLink(index: number): boolean {
    return this.safe[index] ?? false;
  }

  /** Applies a `metadata` event only when it describes the revision the grid holds. */
  apply(event: FrameEvent, gridRevision: number): boolean {
    if (event.type !== "metadata" || event.revision !== gridRevision) return false;
    if (event.hyperlinks) {
      this.panes = event.panes.slice();
      this.links = event.hyperlinks.slice();
      this.safe = this.links.map((uri) => uri !== "" && this.validate(uri));
      this.stale = false;
    } else {
      this.panes = this.panes.map((p) => event.panes.find((u) => u.pane_id === p.pane_id) ?? p);
    }
    this.revision = event.revision;
    return true;
  }

  pane(id: string): PaneMeta | undefined {
    return this.panes.find((p) => p.pane_id === id);
  }

  focused(): PaneMeta | undefined {
    return this.panes.find((p) => p.focused);
  }
}

export function cellFromPoint(x: number, y: number, size: CellSize, width: number, height: number): { x: number; y: number } | null {
  if (x < 0 || y < 0) return null;
  const cx = Math.floor(x / size.cellWidth);
  const cy = Math.floor(y / size.cellHeight);
  return cx < width && cy < height ? { x: cx, y: cy } : null;
}

export interface PaneHit {
  pane: PaneMeta;
  /** Pane-local column/row. */
  column: number;
  row: number;
}

export function hitPane(panes: readonly PaneMeta[], x: number, y: number): PaneHit | null {
  for (const pane of panes) {
    const r = pane.inner_rect;
    if (x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height) {
      return { pane, column: x - r.x, row: y - r.y };
    }
  }
  return null;
}

/**
 * Mouse input for a pane. `local` is the CSS point relative to the pane inner origin; pixel
 * positions (1-based, pane device pixels) are included only when the pane announced SGR pixels.
 */
export function mouseInput(
  hit: PaneHit,
  action: MouseAction,
  button: MouseButton | undefined,
  modifiers: number,
  lines: number,
  local: { x: number; y: number } | null,
  size: CellSize,
): Extract<InputDto, { kind: "mouse" }> {
  const input: Extract<InputDto, { kind: "mouse" }> = { kind: "mouse", action, column: hit.column, row: hit.row, modifiers, lines };
  if (button) input.button = button;
  const p = hit.pane;
  if (local && p.sgr_pixel_mouse && p.pixel_width > 0 && p.pixel_height > 0) {
    const cssW = p.inner_rect.width * size.cellWidth;
    const cssH = p.inner_rect.height * size.cellHeight;
    const clamp = (v: number, max: number) => Math.min(max, Math.max(1, v));
    input.pixel = {
      x: clamp(Math.floor((local.x * p.pixel_width) / cssW) + 1, p.pixel_width),
      y: clamp(Math.floor((local.y * p.pixel_height) / cssH) + 1, p.pixel_height),
    };
    input.geometry = { cols: p.inner_rect.width, rows: p.inner_rect.height, width_px: p.pixel_width, height_px: p.pixel_height };
  }
  return input;
}

export function geometryChanged(prev: GeometryDto | null, next: GeometryDto): boolean {
  return (
    !prev ||
    prev.cols !== next.cols ||
    prev.rows !== next.rows ||
    prev.cell_width_px !== next.cell_width_px ||
    prev.cell_height_px !== next.cell_height_px
  );
}

/**
 * True while the pane captured by a pointer gesture is still the same confirmed target: same
 * connection identity, metadata current, the pane is the confirmed focus, still reports the mouse
 * and keeps its inner rect and pixel size. Anything else invalidates old coordinates.
 */
export function mouseTargetValid(captured: PaneMeta, capturedIdentity: string, identity: string, meta: SurfaceMeta, focusedPaneId: string | null): boolean {
  if (identity !== capturedIdentity || meta.stale || focusedPaneId !== captured.pane_id) return false;
  const current = meta.pane(captured.pane_id);
  if (!current || !current.focused || !current.mouse_reporting) return false;
  const a = captured.inner_rect;
  const b = current.inner_rect;
  return (
    a.x === b.x &&
    a.y === b.y &&
    a.width === b.width &&
    a.height === b.height &&
    captured.pixel_width === current.pixel_width &&
    captured.pixel_height === current.pixel_height &&
    captured.sgr_pixel_mouse === current.sgr_pixel_mouse
  );
}

/**
 * Press → drag → release sequence delivered to the application of one captured pane. Once the
 * target stops being valid the gesture is cancelled: no drag/release is sent to any other pane
 * and nothing is replayed later.
 */
export class AppMouseGesture {
  private pane: PaneMeta | null = null;
  private identity = "";
  private button: MouseButton = "left";
  private start_: { column: number; row: number } = { column: 0, row: 0 };
  private cell = "";

  get active(): boolean {
    return this.pane !== null;
  }

  start(hit: PaneHit, button: MouseButton, modifiers: number, local: { x: number; y: number } | null, identity: string, size: CellSize): InputDto {
    this.pane = hit.pane;
    this.identity = identity;
    this.button = button;
    this.start_ = { column: hit.column, row: hit.row };
    this.cell = `${hit.column},${hit.row}`;
    return mouseInput(hit, "down", button, modifiers, 3, local, size);
  }

  cancel(): void {
    this.pane = null;
  }

  /** Keeps the gesture only while its target is valid; cancels otherwise. */
  check(identity: string, meta: SurfaceMeta, focusedPaneId: string | null): boolean {
    if (!this.pane) return false;
    if (mouseTargetValid(this.pane, this.identity, identity, meta, focusedPaneId)) return true;
    this.cancel();
    return false;
  }

  drag(
    hit: PaneHit | null,
    modifiers: number,
    local: { x: number; y: number } | null,
    identity: string,
    meta: SurfaceMeta,
    focusedPaneId: string | null,
    size: CellSize,
  ): InputDto | null {
    if (!this.check(identity, meta, focusedPaneId) || !hit || hit.pane.pane_id !== this.pane!.pane_id) return null;
    const key = `${hit.column},${hit.row}`;
    if (key === this.cell) return null;
    this.cell = key;
    return mouseInput(hit, "drag", this.button, modifiers, 3, local, size);
  }

  release(
    hit: PaneHit | null,
    modifiers: number,
    local: { x: number; y: number } | null,
    identity: string,
    meta: SurfaceMeta,
    focusedPaneId: string | null,
    size: CellSize,
  ): InputDto | null {
    if (!this.check(identity, meta, focusedPaneId)) return null;
    const pane = meta.pane(this.pane!.pane_id)!;
    this.cancel();
    const inside = hit && hit.pane.pane_id === pane.pane_id;
    const target: PaneHit = inside ? hit : { pane, ...this.start_ };
    return mouseInput(target, "up", this.button, modifiers, 3, inside ? local : null, size);
  }
}

/**
 * Decorations of one surface row: the selection pane is looked up once per row and link safety
 * comes from the per-table cache, so predicates cost O(1) per cell.
 */
export function rowDecor(meta: SurfaceMeta, selection: TerminalSelection, grid: TerminalGrid, y: number): RowDecor | undefined {
  const pane = selection.paneId ? meta.pane(selection.paneId) : undefined;
  const r = pane?.inner_rect;
  const selected =
    pane && r && selection.nonEmpty && y >= r.y && y < r.y + r.height
      ? (x: number) => x >= r.x && x < r.x + r.width && selection.contains(pane.pane_id, y - r.y, x - r.x, pane)
      : undefined;
  const link =
    meta.links.length > 0
      ? (x: number) => {
          const h = grid.cellAt(x, y).h;
          return h !== undefined && meta.safeLink(h);
        }
      : undefined;
  return selected || link ? { selected, link } : undefined;
}

export type PointerRoute = "focus" | "app" | "select";

/**
 * A press on a pane that is not the confirmed focus asks for focus first; on the focused pane
 * Shift always selects locally, otherwise the application gets the mouse if it announced it.
 */
export function routePointer(hit: PaneHit, mods: { shift: boolean }, focusedPaneId: string | null): PointerRoute {
  if (hit.pane.pane_id !== focusedPaneId) return "focus";
  if (mods.shift) return "select";
  return hit.pane.mouse_reporting ? "app" : "select";
}

export type RightClickRoute = "app" | "menu";

/**
 * Spec 028 AC-028-03 — where a right click on `hit` goes: the pane app receives it only when the
 * pane is the confirmed focus, reports the mouse (`mouse_reporting`), owns right clicks
 * (`passthrough`, the per-pane toggle) and the event is not shifted, exactly as the TUI
 * (`mouse.rs`); anything else opens the Herdr pane menu.
 */
export function rightClickRoute(
  hit: PaneHit,
  mods: { shift: boolean },
  options: { focusedPaneId: string | null; passthrough: boolean },
): RightClickRoute {
  if (options.focusedPaneId !== hit.pane.pane_id) return "menu";
  if (!hit.pane.mouse_reporting) return "menu";
  if (mods.shift) return "menu";
  return options.passthrough ? "app" : "menu";
}

/** Holds keyboard input while a pane focus request is waiting for the engine confirmation. */
export class FocusGate {
  pending: string | null = null;

  /** Returns true when a new request must be sent. */
  request(paneId: string): boolean {
    if (this.pending === paneId) return false;
    this.pending = paneId;
    return true;
  }

  /** Releases the gate when metadata shows the requested pane focused. */
  confirm(panes: readonly PaneMeta[]): boolean {
    if (!this.pending) return false;
    if (!panes.some((p) => p.pane_id === this.pending && p.focused)) return false;
    this.pending = null;
    return true;
  }

  /** A failed request releases only its own wait. */
  fail(paneId: string): void {
    if (this.pending === paneId) this.pending = null;
  }

  allowsInput(): boolean {
    return this.pending === null;
  }
}

type MouseInput = Extract<InputDto, { kind: "mouse" }>;

export type WheelRoute =
  | { kind: "input"; input: MouseInput; repeat: number }
  | { kind: "focus"; input: MouseInput; repeat: number }
  | { kind: "scroll"; request: ScrollRequest }
  | null;

const MAX_WHEEL_LINES = 64;
/** TUI default `ui.mouse_scroll_lines`: one mouse-wheel notch in `deltaMode` line. */
export const MOUSE_SCROLL_LINES = 3;

function wheelLines(deltaY: number, deltaMode: number, size: CellSize, rows: number): number {
  // Line mode: GTK/WebKit often reports one notch as ±1 line; Firefox already sends 3. Never
  // fewer than the engine default; larger |deltaY| is multiple notches already in lines.
  if (deltaMode === 1) {
    return Math.min(MAX_WHEEL_LINES, Math.max(MOUSE_SCROLL_LINES, Math.round(Math.abs(deltaY))));
  }
  const lines = deltaMode === 2 ? Math.abs(deltaY) * rows : Math.abs(deltaY) / size.cellHeight;
  return Math.min(MAX_WHEEL_LINES, Math.max(1, Math.round(lines)));
}

/**
 * Wheel reports per notch when the pane's app takes the wheel (mouse reporting or alternate
 * screen). The engine turns each input into one wheel report or one arrow and ignores `lines`, so
 * the notch is repeated. Measured 2026-09-23 on the reference host: the TUI in foot delivers ~2
 * reports per WebKit notch; the desktop sends one more by user decision.
 */
export const WHEEL_REPORTS_PER_NOTCH = 3;
/** WebKitGTK `deltaY` of one wheel notch in pixel mode on the reference host (Hyprland). */
const WHEEL_NOTCH_PX = 144;

function wheelReports(deltaY: number, deltaMode: number, rows: number): number {
  // Line mode: GTK reports a notch as 1 line, Firefox as 3; either is one notch.
  const notches =
    deltaMode === 1
      ? Math.max(1, Math.abs(deltaY) / MOUSE_SCROLL_LINES)
      : deltaMode === 2
        ? (Math.abs(deltaY) * rows) / MOUSE_SCROLL_LINES
        : Math.abs(deltaY) / WHEEL_NOTCH_PX;
  return Math.min(MAX_WHEEL_LINES, Math.max(1, Math.round(notches * WHEEL_REPORTS_PER_NOTCH)));
}

/**
 * Spec 033 — where a wheel lands, measured from the TUI (`mouse.rs:2264-2283`): the pane under the
 * pointer. On the confirmed focused pane it goes to the engine once as a mouse scroll (the engine
 * chooses application vs scrollback); over another pane the client first asks `pane.focus` for
 * that pane and the host delivers the accumulated notches when the metadata confirms the focus.
 * Shift never changes the focus: it moves that pane's local scrollback through `pane.scroll`, and
 * panes without scrollback ignore it.
 */
export function wheelRoute(
  hit: PaneHit,
  focusedPaneId: string | null,
  wheel: { deltaY: number; deltaMode: number; shift: boolean },
  size: CellSize,
): WheelRoute {
  if (wheel.deltaY === 0) return null;
  const up = wheel.deltaY < 0;
  const lines = wheelLines(wheel.deltaY, wheel.deltaMode, size, hit.pane.inner_rect.height);
  if (wheel.shift) {
    const scroll = hit.pane.scroll;
    if (!scroll) return null;
    const next = up ? scroll.offset_from_bottom + lines : scroll.offset_from_bottom - lines;
    return {
      kind: "scroll",
      request: { pane_id: hit.pane.pane_id, offset_from_bottom: Math.min(scroll.max_offset_from_bottom, Math.max(0, next)) },
    };
  }
  const input = mouseInput(hit, up ? "scroll_up" : "scroll_down", undefined, 0, lines, null, size);
  // Host-scroll panes already move by `lines`; only an app that takes the wheel gets repeats.
  const takesWheel = hit.pane.mouse_reporting || hit.pane.alternate_screen_active;
  const repeat = takesWheel ? wheelReports(wheel.deltaY, wheel.deltaMode, hit.pane.inner_rect.height) : 1;
  return hit.pane.pane_id === focusedPaneId ? { kind: "input", input, repeat } : { kind: "focus", input, repeat };
}

/**
 * Spec 033 — notches received while a pane focus request is pending. They are summed by direction
 * (negative scrolls up, positive scrolls down) and delivered to that pane as one mouse input when
 * the metadata confirms the focus; the position of the latest notch wins. Never sent to the
 * previously focused pane, never before the confirmation. Events repeated for an app that takes
 * the wheel (`repeat > 1`) are summed as reports and delivered as that many inputs.
 */
export class PendingWheel {
  private paneId: string | null = null;
  private column = 0;
  private row = 0;
  private lines = 0;
  private reports = 0;
  private lastLines = 0;

  /** Adds one wheel event; a different pane replaces the pending target with its own notches. */
  add(paneId: string, column: number, row: number, lines: number, repeat = 1): void {
    if (this.paneId !== paneId) {
      this.paneId = paneId;
      this.lines = 0;
      this.reports = 0;
    }
    this.column = column;
    this.row = row;
    this.lines += lines;
    this.reports += repeat > 1 ? Math.sign(lines) * repeat : 0;
    this.lastLines = Math.abs(lines);
  }

  /** Consumes the notches of `paneId` as inputs; null when there is nothing to send. */
  take(paneId: string): MouseInput[] | null {
    if (this.paneId !== paneId) return null;
    const { column, row, lines, reports, lastLines } = this;
    this.clear();
    if (reports !== 0) {
      const input: MouseInput = { kind: "mouse", action: reports < 0 ? "scroll_up" : "scroll_down", column, row, modifiers: 0, lines: lastLines };
      return Array.from({ length: Math.min(MAX_WHEEL_LINES, Math.abs(reports)) }, () => ({ ...input }));
    }
    if (lines === 0) return null;
    return [{ kind: "mouse", action: lines < 0 ? "scroll_up" : "scroll_down", column, row, modifiers: 0, lines: Math.abs(lines) }];
  }

  clear(): void {
    this.paneId = null;
    this.column = 0;
    this.row = 0;
    this.lines = 0;
    this.reports = 0;
    this.lastLines = 0;
  }
}

function viewportTop(pane: PaneMeta): number {
  const s = pane.scroll;
  return s ? Math.max(0, s.max_offset_from_bottom - s.offset_from_bottom) : 0;
}

type Point = { row: number; col: number };

/** Local selection in absolute rows of one pane; text is read by the engine on copy. */
export class TerminalSelection {
  paneId: string | null = null;
  private anchor: Point | null = null;
  private cursor: Point | null = null;

  get active(): boolean {
    return this.paneId !== null;
  }

  start(hit: PaneHit, pane: PaneMeta): void {
    this.paneId = pane.pane_id;
    this.anchor = { row: viewportTop(pane) + hit.row, col: hit.column };
    this.cursor = { ...this.anchor };
  }

  /** Extends to a cell; cells of another pane are clamped to this pane's edge by the caller. */
  extend(hit: PaneHit, pane: PaneMeta): void {
    if (pane.pane_id !== this.paneId) return;
    this.cursor = { row: viewportTop(pane) + hit.row, col: hit.column };
  }

  clear(): void {
    this.paneId = null;
    this.anchor = null;
    this.cursor = null;
  }

  private ordered(): [Point, Point] | null {
    if (!this.anchor || !this.cursor) return null;
    const a = this.anchor;
    const b = this.cursor;
    return a.row < b.row || (a.row === b.row && a.col <= b.col) ? [a, b] : [b, a];
  }

  /** True when there is something to copy (a drag that left its starting cell). */
  get nonEmpty(): boolean {
    const o = this.ordered();
    return !!o && (o[0].row !== o[1].row || o[0].col !== o[1].col);
  }

  request(pane: PaneMeta): SelectionRequest | null {
    const o = this.ordered();
    if (!o || pane.pane_id !== this.paneId || !this.nonEmpty) return null;
    return { pane_id: pane.pane_id, anchor: { ...o[0] }, cursor: { ...o[1] }, content_revision: pane.content_revision };
  }

  contains(paneId: string, viewportRow: number, col: number, pane: PaneMeta): boolean {
    const o = this.ordered();
    if (!o || paneId !== this.paneId || !this.nonEmpty) return false;
    const row = viewportTop(pane) + viewportRow;
    const [s, e] = o;
    if (row < s.row || row > e.row) return false;
    if (row === s.row && col < s.col) return false;
    if (row === e.row && col > e.col) return false;
    return true;
  }

  /** Surface rows currently showing part of the selection (dirty rows for repaint). */
  surfaceRows(pane: PaneMeta): number[] {
    const o = this.ordered();
    if (!o || pane.pane_id !== this.paneId) return [];
    const top = viewportTop(pane);
    const rows: number[] = [];
    for (let r = 0; r < pane.inner_rect.height; r++) {
      if (top + r >= o[0].row && top + r <= o[1].row) rows.push(pane.inner_rect.y + r);
    }
    return rows;
  }
}

const exact = (e: KeyLike, ctrl: boolean, shift: boolean) => e.ctrlKey === ctrl && e.shiftKey === shift && !e.altKey && !e.metaKey;

/** Ctrl+Shift+C copies the selection; Ctrl+C stays SIGINT for the shell. */
export function isCopyChord(e: KeyLike): boolean {
  return exact(e, true, true) && e.key.toLowerCase() === "c";
}

/** Ctrl+Shift+F6 leaves the terminal for the next control; Tab/Shift+Tab stay with the shell. */
export function isExitChord(e: KeyLike): boolean {
  return exact(e, true, true) && e.key === "F6";
}

export const MAX_LINK_URI_LENGTH = 2048;

export function isSafeWebUri(uri: string): boolean {
  if (uri.length === 0 || uri.length > MAX_LINK_URI_LENGTH) return false;
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f-\u009f\s]/.test(uri)) return false;
  if (!/^https?:\/\/[^/?#]+/i.test(uri)) return false;
  return true;
}

export interface LinkHit {
  request: LinkRequest;
  safe: boolean;
}

/** Link under a surface cell (continuation cells of a wide glyph inherit its link). */
export function linkAt(grid: TerminalGrid, meta: SurfaceMeta, x: number, y: number): LinkHit | null {
  const hit = hitPane(meta.panes, x, y);
  if (!hit) return null;
  let cell = grid.cellAt(x, y);
  if (cell.h === undefined && cell.s === "" && x > hit.pane.inner_rect.x) cell = grid.cellAt(x - 1, y);
  if (cell.h === undefined) return null;
  const uri = meta.links[cell.h];
  if (uri === undefined || uri === "") return null;
  return {
    request: { pane_id: hit.pane.pane_id, uri, viewport_row: hit.row, col: hit.column, content_revision: hit.pane.content_revision },
    safe: isSafeWebUri(uri),
  };
}
