// Canvas renderer split in two: `planRow` turns one grid row into deterministic draw
// operations (testable without a canvas), `paint` executes them. Rows are painted only
// when dirty and only while the surface is visible. `planEdgeRow`/`paintExtensions` are
// the same contract for the ring outside the grid (window-padding extend, spec 028 r4c).

import { packedToCss, type Theme, DEFAULT_THEME } from "./colors";
import type { TerminalGrid } from "./grid";
import type { CellDto } from "./types";
import { M_BOLD, M_DIM, M_HIDDEN, M_ITALIC, M_REVERSED, M_UNDERLINED, M_CROSSED_OUT } from "./types";
import { graphemeWidth } from "./wcwidth";

export interface CellMetrics {
  cellWidth: number;
  cellHeight: number;
  baseline: number;
  font: string;
}

export type DrawOp =
  | { op: "rect"; x: number; y: number; w: number; h: number; color: string }
  | { op: "text"; x: number; y: number; text: string; color: string; font: string; cells: number }
  | { op: "underline"; x: number; y: number; w: number; color: string }
  | { op: "strike"; x: number; y: number; w: number; color: string }
  | { op: "cursor"; x: number; y: number; w: number; h: number; shape: number; color: string };

export interface CellStyle {
  fg: string;
  bg: string;
  font: string;
  underline: boolean;
  strike: boolean;
  hidden: boolean;
}

/**
 * Drawable area of one surface in CSS px, relative to the grid origin. `left`/`top` are negative
 * when the ring around the grid is painted (window-padding extend): the CSS padding of the
 * container and the sub-cell remainder, filled with the background of the nearest edge cell.
 */
export interface DrawBounds {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

export function resolveStyle(cell: CellDto, metrics: CellMetrics, theme: Theme = DEFAULT_THEME): CellStyle {
  let fg = packedToCss(cell.fg, "fg", theme);
  let bg = packedToCss(cell.bg, "bg", theme);
  if (cell.m & M_REVERSED) [fg, bg] = [bg, fg];
  if (cell.m & M_DIM) fg = dim(fg);
  const bold = (cell.m & M_BOLD) !== 0;
  const italic = (cell.m & M_ITALIC) !== 0;
  const font = `${italic ? "italic " : ""}${bold ? "bold " : ""}${metrics.font}`;
  return {
    fg,
    bg,
    font,
    underline: (cell.m & M_UNDERLINED) !== 0,
    strike: (cell.m & M_CROSSED_OUT) !== 0,
    hidden: (cell.m & M_HIDDEN) !== 0,
  };
}

function dim(color: string): string {
  if (!/^#[0-9a-f]{6}$/i.test(color)) return color;
  const n = parseInt(color.slice(1), 16);
  const c = (v: number) => Math.round(v * 0.6).toString(16).padStart(2, "0");
  return `#${c((n >> 16) & 0xff)}${c((n >> 8) & 0xff)}${c(n & 0xff)}`;
}

/** Per-cell interaction marks of one row (surface columns). */
export interface RowDecor {
  selected?: (x: number) => boolean;
  link?: (x: number) => boolean;
}

export const SELECTION_COLOR = "#3e5c8a";

/** Draw operations for one row: background rects, selection, glyph runs, decorations, cursor. */
export function planRow(grid: TerminalGrid, y: number, metrics: CellMetrics, theme: Theme = DEFAULT_THEME, decor?: RowDecor): DrawOp[] {
  const ops: DrawOp[] = [];
  const top = y * metrics.cellHeight;
  // Row background first (theme background), then per-cell backgrounds.
  ops.push({ op: "rect", x: 0, y: top, w: grid.width * metrics.cellWidth, h: metrics.cellHeight, color: theme.background });
  let x = 0;
  while (x < grid.width) {
    const cell = grid.cellAt(x, y);
    const style = resolveStyle(cell, metrics, theme);
    const width = cell.s === "" ? 1 : Math.max(1, graphemeWidth(cell.s));
    const span = Math.min(width, grid.width - x);
    if (style.bg !== theme.background) {
      ops.push({ op: "rect", x: x * metrics.cellWidth, y: top, w: span * metrics.cellWidth, h: metrics.cellHeight, color: style.bg });
    }
    if (decor?.selected?.(x)) {
      ops.push({ op: "rect", x: x * metrics.cellWidth, y: top, w: span * metrics.cellWidth, h: metrics.cellHeight, color: SELECTION_COLOR });
    }
    // Continuation cells (empty symbol after a wide grapheme) draw no text.
    if (cell.s !== "" && cell.s !== " " && !style.hidden) {
      ops.push({
        op: "text",
        x: x * metrics.cellWidth,
        y: top + metrics.baseline,
        text: cell.s,
        color: style.fg,
        font: style.font,
        cells: span,
      });
    }
    if (style.underline || decor?.link?.(x)) {
      ops.push({ op: "underline", x: x * metrics.cellWidth, y: top + metrics.cellHeight - 2, w: span * metrics.cellWidth, color: style.fg });
    }
    if (style.strike) {
      ops.push({ op: "strike", x: x * metrics.cellWidth, y: top + Math.floor(metrics.cellHeight / 2), w: span * metrics.cellWidth, color: style.fg });
    }
    x += span;
  }
  const cursor = grid.cursor;
  if (cursor && cursor.visible && cursor.y === y && cursor.x < grid.width) {
    ops.push({
      op: "cursor",
      x: cursor.x * metrics.cellWidth,
      y: top,
      w: metrics.cellWidth,
      h: metrics.cellHeight,
      shape: cursor.shape,
      color: theme.cursor,
    });
  }
  return ops;
}

export interface PaintTarget {
  fillRect(x: number, y: number, w: number, h: number): void;
  fillText(text: string, x: number, y: number, maxWidth?: number): void;
  fillStyle: string | CanvasGradient | CanvasPattern;
  font: string;
  textBaseline: CanvasTextBaseline;
  globalAlpha: number;
}

export function executeOps(ctx: PaintTarget, ops: DrawOp[]): void {
  ctx.textBaseline = "alphabetic";
  for (const op of ops) {
    switch (op.op) {
      case "rect":
        ctx.fillStyle = op.color;
        ctx.fillRect(op.x, op.y, op.w, op.h);
        break;
      case "text":
        ctx.fillStyle = op.color;
        ctx.font = op.font;
        ctx.fillText(op.text, op.x, op.y);
        break;
      case "underline":
        ctx.fillStyle = op.color;
        ctx.fillRect(op.x, op.y, op.w, 1);
        break;
      case "strike":
        ctx.fillStyle = op.color;
        ctx.fillRect(op.x, op.y, op.w, 1);
        break;
      case "cursor":
        ctx.fillStyle = op.color;
        if (op.shape === 3 || op.shape === 4) {
          ctx.fillRect(op.x, op.y + op.h - 2, op.w, 2);
        } else if (op.shape === 5 || op.shape === 6) {
          ctx.fillRect(op.x, op.y, 2, op.h);
        } else {
          ctx.globalAlpha = 0.6;
          ctx.fillRect(op.x, op.y, op.w, op.h);
          ctx.globalAlpha = 1;
        }
        break;
    }
  }
}

/**
 * Paints `dirtyRows` (row background + cells). When `full` is true — canvas
 * creation, resize, or a live theme change — fills the whole bitmap with the
 * theme background and paints every row. Hidden surfaces paint nothing.
 */
export function paint(
  ctx: PaintTarget,
  grid: TerminalGrid,
  dirtyRows: Iterable<number>,
  metrics: CellMetrics,
  visible: boolean,
  theme: Theme = DEFAULT_THEME,
  decorFor?: (y: number) => RowDecor | undefined,
  full = false,
): number {
  if (!visible || !grid.hasSurface) return 0;
  if (full) {
    // Opaque fill of the whole bitmap: a 2d context with alpha leaves unpainted
    // pixels transparent, and WebKitGTK then composites the wallpaper through them.
    ctx.fillStyle = theme.background;
    ctx.fillRect(0, 0, grid.width * metrics.cellWidth, grid.height * metrics.cellHeight);
  }
  const rows = full ? Array.from({ length: grid.height }, (_, i) => i) : dirtyRows;
  let painted = 0;
  for (const y of rows) {
    if (y < 0 || y >= grid.height) continue;
    executeOps(ctx, planRow(grid, y, metrics, theme, decorFor?.(y)));
    painted++;
  }
  return painted;
}

// --- Window-padding extend (spec 028 r4c) -----------------------------------------------------
// The CSS padding of the container and the sub-cell remainder are outside the cell grid: an app
// with its own background (grok) left them in the theme colour, a frame of another colour. The
// rule (ghostty's `window-padding-color = extend`): the area outside the grid takes the background
// of the nearest edge cell — right of a row, its last cell; below the last row, the cell of each
// column; left/top padding, the first column/row; corners, the corner cell.

/** Extension background of one edge cell: null when it is the theme background (nothing to extend). */
function extensionBg(cell: CellDto, metrics: CellMetrics, theme: Theme): string | null {
  const bg = resolveStyle(cell, metrics, theme).bg;
  return bg === theme.background ? null : bg;
}

/** Extension background of every column of a row (a wide cell covers its whole span). */
function rowExtensionBgs(grid: TerminalGrid, y: number, metrics: CellMetrics, theme: Theme): (string | null)[] {
  const bgs: (string | null)[] = new Array(grid.width).fill(null);
  for (let x = 0; x < grid.width; ) {
    const cell = grid.cellAt(x, y);
    const span = Math.min(cell.s === "" ? 1 : Math.max(1, graphemeWidth(cell.s)), grid.width - x);
    const bg = extensionBg(cell, metrics, theme);
    for (let i = 0; i < span; i++) bgs[x + i] = bg;
    x += span;
  }
  return bgs;
}

/** Background of the cell covering the last column of a row (continuation cell follows its glyph). */
function lastColumnBg(grid: TerminalGrid, y: number, metrics: CellMetrics, theme: Theme): string | null {
  let x = 0;
  let bg: string | null = null;
  while (x < grid.width) {
    const cell = grid.cellAt(x, y);
    bg = extensionBg(cell, metrics, theme);
    x += Math.min(cell.s === "" ? 1 : Math.max(1, graphemeWidth(cell.s)), grid.width - x);
  }
  return bg;
}

/**
 * Extra rects for one dirty row, in grid coordinates: the left/right side bands of the row plus,
 * when a border row is dirty, the top (row 0) or bottom (last row) band. Each band repaints its
 * theme base first — a patch may have turned a coloured edge cell back to default — then one rect
 * per non-default edge cell. Nothing here touches the cell grid itself.
 */
export function planEdgeRow(grid: TerminalGrid, y: number, metrics: CellMetrics, theme: Theme, bounds: DrawBounds): DrawOp[] {
  if (!grid.hasSurface || y < 0 || y >= grid.height) return [];
  const ops: DrawOp[] = [];
  const cw = metrics.cellWidth;
  const ch = metrics.cellHeight;
  const top = y * ch;
  const gridW = grid.width * cw;
  const gridH = grid.height * ch;
  if (bounds.left < 0) {
    ops.push({ op: "rect", x: bounds.left, y: top, w: -bounds.left, h: ch, color: theme.background });
    const bg = extensionBg(grid.cellAt(0, y), metrics, theme);
    if (bg) ops.push({ op: "rect", x: bounds.left, y: top, w: -bounds.left, h: ch, color: bg });
  }
  if (bounds.right > gridW) {
    ops.push({ op: "rect", x: gridW, y: top, w: bounds.right - gridW, h: ch, color: theme.background });
    const bg = lastColumnBg(grid, y, metrics, theme);
    if (bg) ops.push({ op: "rect", x: gridW, y: top, w: bounds.right - gridW, h: ch, color: bg });
  }
  // Spec 050: the top/bottom band extends, column by column, only a *continuous* background — the
  // edge cell and the one next to it in the neighbouring row must agree. A highlighted border row
  // (Claude Code's pinned header, a status bar) then stops at the grid and does not read as an
  // extra line in the padding, while an app that paints the whole screen (grok) still has no frame.
  const band = (bandTop: number, bandHeight: number, from: number, neighbour: number): void => {
    if (bandHeight <= 0) return;
    ops.push({ op: "rect", x: bounds.left, y: bandTop, w: bounds.right - bounds.left, h: bandHeight, color: theme.background });
    const edge = rowExtensionBgs(grid, from, metrics, theme);
    // A grid of a single row has no neighbour: it extends its only row, as before spec 050.
    const near = neighbour === from ? edge : rowExtensionBgs(grid, neighbour, metrics, theme);
    for (let x = 0; x < grid.width; ) {
      const cell = grid.cellAt(x, from);
      const span = Math.min(cell.s === "" ? 1 : Math.max(1, graphemeWidth(cell.s)), grid.width - x);
      const bg = edge[x];
      let continuous = bg !== null;
      for (let i = 0; continuous && i < span; i++) continuous = near[x + i] === bg;
      if (continuous && bg) ops.push({ op: "rect", x: x * cw, y: bandTop, w: span * cw, h: bandHeight, color: bg });
      x += span;
    }
  };
  if (y === 0) band(bounds.top, -bounds.top, 0, Math.min(1, grid.height - 1));
  if (y === grid.height - 1) band(gridH, bounds.bottom - gridH, grid.height - 1, Math.max(0, grid.height - 2));
  return ops;
}

/** True when a row has at least one cell to extend (anything but the theme background). */
function rowExtends(grid: TerminalGrid, y: number, metrics: CellMetrics, theme: Theme): boolean {
  for (let x = 0; x < grid.width; x++) {
    if (extensionBg(grid.cellAt(x, y), metrics, theme)) return true;
  }
  return false;
}

/**
 * Paints the extension ring on its own canvas (behind the grid canvas): dirty rows repaint their
 * side bands, and a dirty border row repaints its top/bottom band. A band follows the neighbouring
 * row too (spec 050), so a patch that only touches row 1 (or the penultimate row) repaints the
 * border row in the same frame — there is no periodic repaint to fix it later. A border row with
 * nothing to extend is skipped: its band is the theme background whatever the neighbour does.
 * Hidden surfaces paint nothing.
 */
export function paintExtensions(
  ctx: PaintTarget,
  grid: TerminalGrid,
  dirtyRows: Iterable<number>,
  metrics: CellMetrics,
  visible: boolean,
  theme: Theme,
  bounds: DrawBounds,
  full = false,
): number {
  if (!visible || !grid.hasSurface) return 0;
  if (full) {
    ctx.fillStyle = theme.background;
    ctx.fillRect(bounds.left, bounds.top, bounds.right - bounds.left, bounds.bottom - bounds.top);
  }
  const rows = full ? Array.from({ length: grid.height }, (_, i) => i) : [...dirtyRows];
  const dirty = new Set(rows.filter((y) => y >= 0 && y < grid.height));
  const borders = new Set<number>();
  if (grid.height > 1) {
    for (const [edge, neighbour] of [[0, 1], [grid.height - 1, grid.height - 2]] as const) {
      if (dirty.has(neighbour) && !dirty.has(edge) && rowExtends(grid, edge, metrics, theme)) borders.add(edge);
    }
  }
  let painted = 0;
  for (const y of [...rows, ...borders]) {
    if (y < 0 || y >= grid.height) continue;
    executeOps(ctx, planEdgeRow(grid, y, metrics, theme, bounds));
    painted++;
  }
  return painted;
}

/** Creation / resize / theme: one total fill then every row. */
export function paintAll(
  ctx: PaintTarget,
  grid: TerminalGrid,
  metrics: CellMetrics,
  visible: boolean,
  theme: Theme = DEFAULT_THEME,
  decorFor?: (y: number) => RowDecor | undefined,
): number {
  return paint(ctx, grid, [], metrics, visible, theme, decorFor, true);
}
