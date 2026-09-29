// AC-001-02 (paint plan): wide glyphs span two cells, continuation cells draw no text,
// combining clusters stay one glyph run, colours/modifiers resolve per cell, cursor is
// drawn from the last state; hidden surfaces produce zero repaints.
import { describe, expect, it } from "vitest";
import { fixtureEvents } from "../harness/fixture";
import { DEFAULT_THEME, indexedToCss, packedToCss } from "./colors";
import { TerminalGrid } from "./grid";
import {
  paint,
  paintAll,
  paintExtensions,
  planEdgeRow,
  planRow,
  resolveStyle,
  type CellMetrics,
  type DrawBounds,
  type PaintTarget,
} from "./renderer";
import { graphemeWidth } from "./wcwidth";

const metrics: CellMetrics = { cellWidth: 10, cellHeight: 20, baseline: 15, font: "14px monospace" };

function loadedGrid(): TerminalGrid {
  const grid = new TerminalGrid();
  const events = fixtureEvents();
  grid.apply(events.full);
  for (const patch of events.patches) grid.apply(patch);
  return grid;
}

class FakeCtx implements PaintTarget {
  fillStyle: string | CanvasGradient | CanvasPattern = "";
  font = "";
  textBaseline: CanvasTextBaseline = "alphabetic";
  globalAlpha = 1;
  rects: { x: number; y: number; w: number; h: number; color: string }[] = [];
  texts: { text: string; x: number; y: number; color: string; font: string }[] = [];
  fillRect(x: number, y: number, w: number, h: number) {
    this.rects.push({ x, y, w, h, color: String(this.fillStyle) });
  }
  fillText(text: string, x: number, y: number) {
    this.texts.push({ text, x, y, color: String(this.fillStyle), font: this.font });
  }
}

describe("wcwidth", () => {
  // Would catch: emoji/CJK measured as 1 cell, combining marks measured as 1.
  it("measures wide, combining and narrow graphemes", () => {
    expect(graphemeWidth("a")).toBe(1);
    expect(graphemeWidth("á")).toBe(1);
    expect(graphemeWidth("界")).toBe(2);
    expect(graphemeWidth("🦀")).toBe(2);
    expect(graphemeWidth("é")).toBe(1);
    expect(graphemeWidth("́")).toBe(0);
    expect(graphemeWidth("")).toBe(0);
    expect(graphemeWidth("☕️")).toBe(2);
  });
});

describe("colors", () => {
  // Would catch: swapped rgb byte order, indexed cube math, reset resolving to a named colour.
  it("decodes packed colours like wire.rs", () => {
    expect(packedToCss(0x02_ff_80_00, "fg", DEFAULT_THEME)).toBe("#ff8000");
    expect(packedToCss(0x01_00_00_dc, "fg", DEFAULT_THEME)).toBe(indexedToCss(220, DEFAULT_THEME));
    expect(indexedToCss(196, DEFAULT_THEME)).toBe("#ff0000");
    expect(indexedToCss(16, DEFAULT_THEME)).toBe("#000000");
    expect(indexedToCss(231, DEFAULT_THEME)).toBe("#ffffff");
    expect(indexedToCss(232, DEFAULT_THEME)).toBe("#080808");
    expect(packedToCss(0x02, "fg", DEFAULT_THEME)).toBe(DEFAULT_THEME.named[1]); // Red
    expect(packedToCss(0, "fg", DEFAULT_THEME)).toBe(DEFAULT_THEME.foreground);
    expect(packedToCss(0, "bg", DEFAULT_THEME)).toBe(DEFAULT_THEME.background);
    expect(packedToCss(0x07_00_00_00, "bg", DEFAULT_THEME)).toBe(DEFAULT_THEME.background);
  });
});

describe("planRow", () => {
  // Would catch: text drawn for the continuation cell, wide glyph limited to one cell,
  // combining cluster split into two text ops, bold/italic not reaching the font.
  it("draws wide and combining graphemes as single runs over the right span", () => {
    const grid = loadedGrid();
    const ops = planRow(grid, 0, metrics);
    const texts = ops.filter((op) => op.op === "text");
    expect(texts.map((t) => (t.op === "text" ? t.text : ""))).toEqual(["á", "A", "B", "🦀", "é", "x"]);
    const crab = texts.find((t) => t.op === "text" && t.text === "🦀");
    expect(crab).toMatchObject({ x: 30, cells: 2 });
    const accented = texts.find((t) => t.op === "text" && t.text === "á");
    expect(accented).toMatchObject({ x: 0, color: "#ff8000", font: "bold 14px monospace" });
    const combining = texts.find((t) => t.op === "text" && t.text === "é");
    expect(combining).toMatchObject({ x: 50, cells: 1, font: "italic 14px monospace" });
    // The 'x' cell is underlined (m=8): one underline op at its position.
    expect(ops.filter((op) => op.op === "underline")).toEqual([
      { op: "underline", x: 60, y: 18, w: 10, color: DEFAULT_THEME.foreground },
    ]);
    // 🦀 has bg Blue (named 5) → a background rect spanning two cells.
    expect(ops).toContainEqual({ op: "rect", x: 30, y: 0, w: 20, h: 20, color: DEFAULT_THEME.named[4] });
  });

  // Would catch: coloured rows losing per-cell colours, or the cursor drawn from the
  // full frame instead of the latest patch (hidden after patch 2).
  it("resolves coloured rows and the latest cursor state", () => {
    const grid = loadedGrid();
    const row1 = planRow(grid, 1, metrics);
    const red = row1.find((op) => op.op === "text" && op.text === "r");
    expect(red).toMatchObject({ color: indexedToCss(196, DEFAULT_THEME) });
    // fixture: fg 33554431 = 0x01_FF_FF_FF → indexed 255 (#eeeeee); bg 33619967 = 0x02_00_FF_FF → rgb #00ffff
    const blue = row1.find((op) => op.op === "text" && op.text === "b");
    expect(blue).toMatchObject({ color: indexedToCss(255, DEFAULT_THEME) });
    expect(indexedToCss(255, DEFAULT_THEME)).toBe("#eeeeee");
    expect(row1).toContainEqual({ op: "rect", x: 40, y: 20, w: 10, h: 20, color: "#00ffff" });
    expect(row1.filter((op) => op.op === "rect" && op.color === "#00ffff")).toHaveLength(4);
    // cursor hidden by the second patch → no cursor op on row 2
    expect(planRow(grid, 2, metrics).some((op) => op.op === "cursor")).toBe(false);

    const fresh = new TerminalGrid();
    fresh.apply(fixtureEvents().full);
    const cursorOps = planRow(fresh, 2, metrics).filter((op) => op.op === "cursor");
    expect(cursorOps).toEqual([{ op: "cursor", x: 0, y: 40, w: 10, h: 20, shape: 2, color: DEFAULT_THEME.cursor }]);
  });

  it("swaps colours for reversed cells", () => {
    const style = resolveStyle({ s: "x", fg: 0x02_11_22_33, bg: 0x02_44_55_66, m: 0b0100_0000 }, metrics);
    expect(style.fg).toBe("#445566");
    expect(style.bg).toBe("#112233");
  });
});

function blankGrid(width: number, height: number): TerminalGrid {
  const grid = new TerminalGrid();
  const cells = Array.from({ length: width * height }, () => ({ s: "x", fg: 0, bg: 0, m: 0 }));
  grid.apply({
    type: "full",
    revision: 1,
    width,
    height,
    cells,
    cursor: { x: 0, y: 0, visible: false, shape: 2 },
    panes: [],
  });
  return grid;
}

function isFullFill(
  r: { x: number; y: number; w: number; h: number },
  width: number,
  height: number,
): boolean {
  return r.x === 0 && r.y === 0 && r.w === width * metrics.cellWidth && r.h === height * metrics.cellHeight;
}

describe("paint", () => {
  // Would catch: a hidden surface still painting (AGENTS: panes ocultos não provocam
  // repaints), or paint touching rows outside the dirty set.
  it("paints only dirty rows and nothing while hidden", () => {
    const grid = loadedGrid();
    const ctx = new FakeCtx();
    expect(paint(ctx, grid, [0, 1, 2], metrics, false)).toBe(0);
    expect(ctx.rects).toHaveLength(0);
    expect(ctx.texts).toHaveLength(0);

    expect(paint(ctx, grid, [2], metrics, true)).toBe(1);
    expect(ctx.texts.map((t) => t.text)).toEqual(["$", "o", "k"]);
    // Partial paint: row background only (AC-021-01). A whole-canvas fill here is the
    // 020-r2 regression that wiped every non-dirty row on hover/scroll.
    expect(ctx.rects[0]).toEqual({ x: 0, y: 40, w: 80, h: 20, color: DEFAULT_THEME.background });
    expect(ctx.rects.some((r) => isFullFill(r, grid.width, grid.height))).toBe(false);

    const all = new FakeCtx();
    expect(paint(all, grid, [0, 1, 2, 99], metrics, true)).toBe(3);
    expect(all.texts.some((t) => t.text === "🦀")).toBe(true);
  });

  // Would catch: AC-020-04 fill on every paint (the 020-r2 wipe), or create/resize/theme
  // skipping the opaque fill so the bitmap outside the dirty rows stays transparent/black.
  it("fills the whole canvas with the theme background only on create, resize and theme", () => {
    const grid = loadedGrid();
    const partial = new FakeCtx();
    expect(paint(partial, grid, [1], metrics, true)).toBe(1);
    expect(partial.rects.some((r) => isFullFill(r, grid.width, grid.height))).toBe(false);
    expect(partial.rects.some((r) => r.y === 20 && r.h === 20)).toBe(true);

    const created = new FakeCtx();
    expect(paintAll(created, grid, metrics, true)).toBe(3);
    expect(created.rects[0]).toEqual({ x: 0, y: 0, w: 80, h: 60, color: DEFAULT_THEME.background });
    expect(created.rects.filter((r) => r.x === 0 && r.w === 80 && r.h === 20).map((r) => r.y)).toEqual([0, 20, 40]);

    const resized = blankGrid(8, 5);
    const resizeCtx = new FakeCtx();
    expect(paint(resizeCtx, resized, [0], metrics, true, DEFAULT_THEME, undefined, true)).toBe(5);
    expect(resizeCtx.rects[0]).toEqual({ x: 0, y: 0, w: 80, h: 100, color: DEFAULT_THEME.background });
    expect(resizeCtx.rects.filter((r) => r.x === 0 && r.w === 80 && r.h === 20).map((r) => r.y)).toEqual([0, 20, 40, 60, 80]);

    const themed = { ...DEFAULT_THEME, background: "#ff00ff" };
    const themeCtx = new FakeCtx();
    expect(paint(themeCtx, grid, [0, 1, 2], metrics, true, themed, undefined, true)).toBe(3);
    expect(themeCtx.rects[0]).toEqual({ x: 0, y: 0, w: 80, h: 60, color: "#ff00ff" });
  });

  // Would catch: a subset dirty set redrawing other rows (or filling the bitmap) after a
  // complete first paint — the hover/scroll wipe of AC-021-01.
  it("after painting every row, a subset {3} redraws only that row", () => {
    const grid = blankGrid(8, 5);
    const first = new FakeCtx();
    expect(paintAll(first, grid, metrics, true)).toBe(5);

    const ctx = new FakeCtx();
    expect(paint(ctx, grid, [3], metrics, true)).toBe(1);
    expect(ctx.rects.some((r) => isFullFill(r, 8, 5))).toBe(false);
    const top = 3 * metrics.cellHeight;
    const bottom = 4 * metrics.cellHeight;
    expect(ctx.rects.length).toBeGreaterThan(0);
    for (const r of ctx.rects) {
      expect(r.y).toBeGreaterThanOrEqual(top);
      expect(r.y + r.h).toBeLessThanOrEqual(bottom);
    }
    expect(ctx.texts.every((t) => t.y >= top && t.y < bottom)).toBe(true);
  });
});

// Spec 028 r4c (window-padding-color=extend): the area around the cell grid — the CSS padding of
// the container and the sub-cell remainder — takes the background of the nearest edge cell, so an
// app with its own background (grok) shows no frame of the theme colour.
describe("edge extension", () => {
  const bounds: DrawBounds = { left: -8, top: -8, right: 4 * metrics.cellWidth + 11, bottom: 3 * metrics.cellHeight + 9 };
  const THEME_BG = DEFAULT_THEME.background;

  function colored(grid: TerminalGrid, x: number, y: number, bg: number): void {
    grid.cells[y * grid.width + x] = { s: "x", fg: 0, bg, m: 0 };
  }

  // Would catch: the right frame staying in the theme colour after the last cell of a row gets a
  // background — the moldura of design/bug-028-grok-background.png.
  it("extends the last cell colour of a row to the canvas border", () => {
    const grid = blankGrid(4, 3);
    colored(grid, 3, 1, 0x02_11_22_33);
    const ops = planEdgeRow(grid, 1, metrics, DEFAULT_THEME, bounds);
    // Last cell of row 1 → extension rect from the grid edge to the canvas border.
    expect(ops).toContainEqual({ op: "rect", x: 40, y: 20, w: 11, h: 20, color: "#112233" });
    // First cell of that row is default: the left band stays theme (it is still repainted).
    expect(ops).toContainEqual({ op: "rect", x: -8, y: 20, w: 8, h: 20, color: THEME_BG });
    expect(ops.filter((op) => op.op === "rect" && op.color === "#112233")).toHaveLength(1);
  });

  // Would catch: a colour of the last row leaking into the bottom padding although the row above
  // is in the theme colour (spec 050: only a continuous background extends downwards).
  it("draws the bottom band per column when the last row is dirty", () => {
    const grid = blankGrid(4, 3);
    colored(grid, 0, 2, 0x02_11_22_33);
    colored(grid, 2, 2, 0x02_44_55_66);
    const ops = planEdgeRow(grid, 2, metrics, DEFAULT_THEME, bounds);
    // Row 1 is in the theme colour: the band is the theme base and nothing else (spec 050).
    expect(ops).toContainEqual({ op: "rect", x: -8, y: 60, w: 59, h: 9, color: THEME_BG });
    expect(ops.filter((op) => op.op === "rect" && op.y === 60 && op.color !== THEME_BG)).toHaveLength(0);
    expect(ops.filter((op) => op.op === "rect" && op.x === 10 && op.y === 60)).toHaveLength(0);
    // The left band of the same row follows its first cell too.
    expect(ops).toContainEqual({ op: "rect", x: -8, y: 40, w: 8, h: 20, color: "#112233" });
  });

  // Would catch: a colour of the first row leaking into the top padding although row 1 is in the
  // theme colour (spec 050: only a continuous background extends upwards).
  it("draws the top band per column when the first row is dirty", () => {
    const grid = blankGrid(4, 3);
    colored(grid, 1, 0, 0x02_aa_bb_cc);
    const ops = planEdgeRow(grid, 0, metrics, DEFAULT_THEME, bounds);
    expect(ops).toContainEqual({ op: "rect", x: -8, y: -8, w: 59, h: 8, color: THEME_BG });
    expect(ops.filter((op) => op.op === "rect" && op.y === -8 && op.color !== THEME_BG)).toHaveLength(0);
    // A middle row never paints the top/bottom bands.
    const middle = planEdgeRow(blankGrid(4, 3), 1, metrics, DEFAULT_THEME, bounds);
    expect(middle.some((op) => op.op === "rect" && (op.y < 0 || op.y >= 60))).toBe(false);
  });

  // Would catch: a corner taking the colour of a neighbouring cell instead of the corner cell —
  // the side band and the band of the border row must agree on both corners.
  it("fills each corner with the corner cell", () => {
    const grid = blankGrid(4, 3);
    colored(grid, 0, 0, 0x02_11_22_33);
    colored(grid, 3, 0, 0x02_44_55_66);
    colored(grid, 0, 2, 0x02_aa_bb_cc);
    colored(grid, 3, 2, 0x02_dd_ee_ff);
    const first = planEdgeRow(grid, 0, metrics, DEFAULT_THEME, bounds);
    expect(first).toContainEqual({ op: "rect", x: -8, y: 0, w: 8, h: 20, color: "#112233" });
    expect(first).toContainEqual({ op: "rect", x: 40, y: 0, w: 11, h: 20, color: "#445566" });
    // Row 1 is in the theme colour: the vertical corners stay theme (spec 050).
    expect(first).toContainEqual({ op: "rect", x: -8, y: -8, w: 59, h: 8, color: THEME_BG });
    expect(first.filter((op) => op.op === "rect" && op.y === -8 && op.color !== THEME_BG)).toHaveLength(0);
    const last = planEdgeRow(grid, 2, metrics, DEFAULT_THEME, bounds);
    expect(last).toContainEqual({ op: "rect", x: -8, y: 40, w: 8, h: 20, color: "#aabbcc" });
    expect(last).toContainEqual({ op: "rect", x: 40, y: 40, w: 11, h: 20, color: "#ddeeff" });
    expect(last).toContainEqual({ op: "rect", x: -8, y: 60, w: 59, h: 9, color: THEME_BG });
    expect(last.filter((op) => op.op === "rect" && op.y === 60 && op.color !== THEME_BG)).toHaveLength(0);
  });

  // AC-050-01 — Would catch: a highlighted border row (Claude Code's pinned header, a status bar)
  // painted over the top/bottom padding, which reads as one extra line above the header.
  it("leaves the top/bottom band in the theme background when the colour does not continue", () => {
    const top = blankGrid(4, 3);
    for (let x = 0; x < 4; x++) colored(top, x, 0, 0x02_aa_bb_cc);
    const first = planEdgeRow(top, 0, metrics, DEFAULT_THEME, bounds);
    expect(first.some((op) => op.op === "rect" && op.y < 0 && op.color === "#aabbcc")).toBe(false);
    expect(first).toContainEqual({ op: "rect", x: -8, y: -8, w: 59, h: 8, color: THEME_BG });
    // The side bands of the same row are out of scope: they keep following the row.
    expect(first).toContainEqual({ op: "rect", x: -8, y: 0, w: 8, h: 20, color: "#aabbcc" });
    expect(first).toContainEqual({ op: "rect", x: 40, y: 0, w: 11, h: 20, color: "#aabbcc" });

    const bottom = blankGrid(4, 3);
    for (let x = 0; x < 4; x++) colored(bottom, x, 2, 0x02_aa_bb_cc);
    const last = planEdgeRow(bottom, 2, metrics, DEFAULT_THEME, bounds);
    expect(last.some((op) => op.op === "rect" && op.y >= 60 && op.color === "#aabbcc")).toBe(false);
    expect(last).toContainEqual({ op: "rect", x: -8, y: 60, w: 59, h: 9, color: THEME_BG });
    expect(last).toContainEqual({ op: "rect", x: -8, y: 40, w: 8, h: 20, color: "#aabbcc" });
    expect(last).toContainEqual({ op: "rect", x: 40, y: 40, w: 11, h: 20, color: "#aabbcc" });
  });

  // AC-050-02 — Would catch: the continuity rule killing spec 028 r4c, the frame of the theme
  // colour around an app that paints the whole screen (design/bug-028-grok-background.png).
  it("extends a background that continues into the neighbouring row", () => {
    const grok = blankGrid(4, 3);
    for (let y = 0; y < 3; y++) for (let x = 0; x < 4; x++) colored(grok, x, y, 0x02_11_22_33);
    const first = planEdgeRow(grok, 0, metrics, DEFAULT_THEME, bounds);
    for (let x = 0; x < 4; x++) {
      expect(first).toContainEqual({ op: "rect", x: x * 10, y: -8, w: 10, h: 8, color: "#112233" });
    }
    const last = planEdgeRow(grok, 2, metrics, DEFAULT_THEME, bounds);
    for (let x = 0; x < 4; x++) {
      expect(last).toContainEqual({ op: "rect", x: x * 10, y: 60, w: 10, h: 9, color: "#112233" });
    }

    // Only column 1 is continuous: only column 1 extends.
    const column = blankGrid(4, 3);
    colored(column, 1, 0, 0x02_44_55_66);
    colored(column, 1, 1, 0x02_44_55_66);
    const ops = planEdgeRow(column, 0, metrics, DEFAULT_THEME, bounds);
    expect(ops).toContainEqual({ op: "rect", x: 10, y: -8, w: 10, h: 8, color: "#445566" });
    expect(ops.filter((op) => op.op === "rect" && op.y === -8 && op.color !== THEME_BG)).toHaveLength(1);
  });

  // Would catch: a one-row grid losing its extension because there is no neighbouring row.
  it("extends as before on a grid of a single row", () => {
    const grid = blankGrid(4, 1);
    colored(grid, 1, 0, 0x02_aa_bb_cc);
    const single: DrawBounds = { left: -8, top: -8, right: 4 * metrics.cellWidth + 11, bottom: metrics.cellHeight + 9 };
    const ops = planEdgeRow(grid, 0, metrics, DEFAULT_THEME, single);
    expect(ops).toContainEqual({ op: "rect", x: 10, y: -8, w: 10, h: 8, color: "#aabbcc" });
    expect(ops).toContainEqual({ op: "rect", x: 10, y: 20, w: 10, h: 9, color: "#aabbcc" });
  });

  // AC-050-03 — Would catch: the band keeping the old colour until some later frame because only
  // the neighbouring row was dirty (no periodic repaint exists to fix it).
  it("recomputes the band in the same frame when only the neighbouring row changed", () => {
    const grid = blankGrid(4, 3);
    for (let x = 0; x < 4; x++) {
      colored(grid, x, 0, 0x02_aa_bb_cc);
      colored(grid, x, 1, 0x02_aa_bb_cc);
    }
    const before = new FakeCtx();
    expect(paintExtensions(before, grid, [0], metrics, true, DEFAULT_THEME, bounds)).toBe(1);
    expect(before.rects.some((r) => r.y === -8 && r.color === "#aabbcc")).toBe(true);

    // Row 1 loses the colour; only row 1 is dirty.
    for (let x = 0; x < 4; x++) grid.cells[grid.width + x] = { s: "x", fg: 0, bg: 0, m: 0 };
    const ctx = new FakeCtx();
    expect(paintExtensions(ctx, grid, [1], metrics, true, DEFAULT_THEME, bounds)).toBe(2);
    expect(ctx.rects).toContainEqual({ x: -8, y: -8, w: 59, h: 8, color: THEME_BG });
    expect(ctx.rects.some((r) => r.y === -8 && r.color === "#aabbcc")).toBe(false);
    // The last row is default: no bottom band, and nothing at all while hidden.
    expect(ctx.rects.some((r) => r.y === 60)).toBe(false);
    const hidden = new FakeCtx();
    expect(paintExtensions(hidden, grid, [1], metrics, false, DEFAULT_THEME, bounds)).toBe(0);
    expect(hidden.rects).toHaveLength(0);
  });

  // Would catch: painting an invented colour (or nothing) when the edge cell is default; the theme
  // background must continue across the extension.
  it("keeps the theme background when the edge cell is default", () => {
    const grid = blankGrid(4, 3);
    const ops = planEdgeRow(grid, 1, metrics, DEFAULT_THEME, bounds);
    expect(ops.length).toBeGreaterThan(0);
    expect(ops.every((op) => op.op !== "rect" || op.color === THEME_BG)).toBe(true);
    expect(ops).toContainEqual({ op: "rect", x: 40, y: 20, w: 11, h: 20, color: THEME_BG });
  });

  // Would catch: the extension canvas painting the cell grid (double text/bg) or painting while
  // the surface is hidden; it owns only the ring around the grid.
  it("paints only the extension ring and nothing while hidden", () => {
    const grid = blankGrid(4, 3);
    colored(grid, 3, 1, 0x02_11_22_33);
    const hidden = new FakeCtx();
    expect(paintExtensions(hidden, grid, [1, 2], metrics, false, DEFAULT_THEME, bounds)).toBe(0);
    expect(hidden.rects).toHaveLength(0);

    const ctx = new FakeCtx();
    expect(paintExtensions(ctx, grid, [1], metrics, true, DEFAULT_THEME, bounds)).toBe(1);
    expect(ctx.rects.some((r) => r.x === 40 && r.color === "#112233")).toBe(true);
    // The last row was not dirty: no bottom band, and never the grid area itself.
    expect(ctx.rects.some((r) => r.y === 60)).toBe(false);
    expect(ctx.rects.some((r) => r.x === 0 && r.y === 0 && r.w === 40 && r.h === 60)).toBe(false);
  });

  // Would catch: create/resize/theme leaving the ring with stale colours of the previous theme.
  it("fills the whole ring with the theme background on a full paint", () => {
    const grid = blankGrid(4, 3);
    const ctx = new FakeCtx();
    expect(paintExtensions(ctx, grid, [], metrics, true, DEFAULT_THEME, bounds, true)).toBe(3);
    expect(ctx.rects[0]).toEqual({ x: -8, y: -8, w: 59, h: 77, color: THEME_BG });
    expect(ctx.rects.filter((r) => r.y === -8 && r.h === 8)).toHaveLength(1);
    expect(ctx.rects.filter((r) => r.y === 60 && r.h === 9)).toHaveLength(1);
  });
});
