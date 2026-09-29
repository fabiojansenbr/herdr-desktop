// AC-001-02 (renderer side): the same fixture the Rust FrameStore consumes yields the
// expected grid in the WebView, and stale patches never touch it.
import { describe, expect, it } from "vitest";
import { fixture, fixtureEvents } from "../harness/fixture";
import { TerminalGrid } from "./grid";

describe("TerminalGrid", () => {
  // Would catch: wrong row-major indexing, wide-char continuation cells dropped, combining
  // cluster split, cursor not taken from the last patch, patches applied out of order.
  it("applies the full frame and patches to the expected cells", () => {
    const grid = new TerminalGrid();
    const events = fixtureEvents();
    expect(grid.hasSurface).toBe(false);
    const full = grid.apply(events.full);
    expect(full).toMatchObject({ applied: true, full: true });
    expect(full.applied && full.dirtyRows).toEqual([0, 1, 2]);
    expect(grid.revision).toBe(10);

    for (const patch of events.patches) {
      const result = grid.apply(patch);
      expect(result.applied, JSON.stringify(result)).toBe(true);
    }
    const expected = fixture.expected_after_patches;
    expect(grid.revision).toBe(expected.surface_revision);
    expect(grid.cursor).toEqual(expected.cursor);
    expect(grid.textRows()).toEqual(expected.text_rows);
    for (const [key, want] of Object.entries(expected.cells)) {
      const [x, y] = key.split(",").map(Number) as [number, number];
      const got = grid.cellAt(x, y);
      expect(got.s, key).toBe(want.s);
      if (want.fg !== undefined) expect(got.fg, `fg ${key}`).toBe(want.fg);
      if (want.bg !== undefined) expect(got.bg, `bg ${key}`).toBe(want.bg);
      if (want.m !== undefined) expect(got.m, `m ${key}`).toBe(want.m);
    }
    expect(grid.cellAt(3, 0).s).toBe("🦀");
    expect(grid.cellAt(4, 0).s).toBe("");
    expect(grid.cellAt(5, 0).s).toBe("é");
  });

  // Would catch: a patch skipping a revision being applied, or dirty rows omitting the
  // previous cursor row (cursor ghost left on screen).
  it("rejects revision gaps and reports dirty rows including the old cursor row", () => {
    const grid = new TerminalGrid();
    const events = fixtureEvents();
    grid.apply(events.full);
    const first = grid.apply(events.patches[0]!);
    expect(first.applied && first.dirtyRows).toEqual([1, 2]);
    const before = grid.textRows();
    const stale = grid.apply(events.stale);
    expect(stale).toEqual({ applied: false, reason: "revision_gap:11->14" });
    expect(grid.textRows()).toEqual(before);
    expect(grid.revision).toBe(11);

    const outOfBounds = grid.apply({
      type: "patch",
      revision: 12,
      rows: [{ x: 7, y: 0, cells: [{ s: "a", fg: 0, bg: 0, m: 0 }, { s: "b", fg: 0, bg: 0, m: 0 }] }],
      cursor: null,
    });
    expect(outOfBounds).toEqual({ applied: false, reason: "row_out_of_bounds" });
  });

  // Would catch: a full frame whose cell count does not match width×height being accepted.
  it("rejects an inconsistent full frame", () => {
    const grid = new TerminalGrid();
    const result = grid.apply({ type: "full", revision: 1, width: 3, height: 2, cells: [], cursor: null, panes: [] });
    expect(result).toEqual({ applied: false, reason: "inconsistent_full_frame" });
    expect(grid.hasSurface).toBe(false);
  });
});
