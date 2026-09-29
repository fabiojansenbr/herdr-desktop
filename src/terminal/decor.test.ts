// AC-007-02/03 (contracts): selection/link decorations are drawn without relying only on colour,
// and hidden terminals request zero animation frames while still accepting state.
import { describe, expect, it } from "vitest";
import { TerminalGrid } from "./grid";
import { planRow, type CellMetrics } from "./renderer";
import { RepaintScheduler } from "./scheduler";
import type { CellDto } from "./types";

const metrics: CellMetrics = { cellWidth: 10, cellHeight: 20, baseline: 15, font: "14px monospace" };

function grid(): TerminalGrid {
  const cells: CellDto[] = Array.from({ length: 8 }, (_, i) => ({ s: String.fromCharCode(97 + i), fg: 0, bg: 0, m: 0 }));
  const g = new TerminalGrid();
  g.apply({ type: "full", revision: 1, width: 4, height: 2, cells, cursor: null, panes: [] });
  return g;
}

describe("row decorations", () => {
  // Would catch: selection not painted, painted over the glyphs (text hidden), links without a
  // non-colour mark, or decorations leaking to rows/cells outside the predicate.
  it("paints selection under glyphs and underlines links", () => {
    const ops = planRow(grid(), 1, metrics, undefined, {
      selected: (x) => x === 1 || x === 2,
      link: (x) => x === 3,
    });
    const selection = ops.filter((o) => o.op === "rect" && o.color === "#3e5c8a");
    expect(selection.map((o) => (o.op === "rect" ? [o.x, o.w] : []))).toEqual([[10, 10], [20, 10]]);
    // Each selected cell: its selection rect comes before its own glyph (text stays visible).
    for (const x of [10, 20]) {
      const rect = ops.findIndex((o) => o.op === "rect" && o.color === "#3e5c8a" && o.x === x);
      const text = ops.findIndex((o) => o.op === "text" && o.x === x);
      expect(rect).toBeGreaterThanOrEqual(0);
      expect(rect).toBeLessThan(text);
    }
    const underline = ops.filter((o) => o.op === "underline");
    expect(underline.map((o) => o.x)).toEqual([30]);
    expect(planRow(grid(), 1, metrics).some((o) => o.op === "underline")).toBe(false);
  });
});

describe("repaint scheduler", () => {
  // Would catch: hidden terminals scheduling RAF for frames/selection/DPI changes, lost dirty
  // rows when becoming visible, or more than one RAF per burst.
  it("requests no frame while hidden and one frame when shown", () => {
    const requested: (() => void)[] = [];
    const painted: number[][] = [];
    const s = new RepaintScheduler(
      (cb) => {
        requested.push(cb);
        return requested.length;
      },
      () => {},
      (rows) => painted.push([...rows].sort((a, b) => a - b)),
    );
    s.setVisible(false);
    s.schedule([3, 1]);
    s.schedule([1, 2]);
    expect(requested.length).toBe(0);
    s.setVisible(true);
    expect(requested.length).toBe(1);
    s.schedule([0]);
    expect(requested.length).toBe(1);
    requested[0]!();
    expect(painted).toEqual([[0, 1, 2, 3]]);
    s.setVisible(false);
    s.schedule([5]);
    requested.push(() => {});
    expect(requested.length).toBe(2);
    expect(s.pendingCount).toBe(1);
  });

  // Would catch: a frame that fires after the surface was hidden still painting.
  it("drops a frame that fires after hiding without losing its rows", () => {
    let cb: (() => void) | null = null;
    const painted: number[][] = [];
    let cancelled = 0;
    const s = new RepaintScheduler((f) => ((cb = f), 7), () => cancelled++, (rows) => painted.push([...rows]));
    s.schedule([4]);
    s.setVisible(false);
    expect(cancelled).toBe(1);
    (cb as unknown as () => void)();
    expect(painted).toEqual([]);
    expect(s.pendingCount).toBe(1);
  });
});
