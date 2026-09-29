// AC-007-02 (contracts, not native proof): pointer/wheel/selection/link/focus/DPI decisions of
// the terminal canvas. Fixture: surface 20x4, cells 10x20 CSS px. Pane `w1:p1` (left, inner
// x=0 w=9, focused, scrollback offset 2 of 10, content 40, no mouse reporting) and `w1:p2`
// (right, inner x=10 w=10, mouse reporting + SGR pixels 90x72, alternate screen, content 7).
import { describe, expect, it, vi } from "vitest";
import { TerminalGrid } from "./grid";
import {
  AppMouseGesture,
  FocusGate,
  PendingWheel,
  SurfaceMeta,
  rowDecor,
  TerminalSelection,
  cellFromPoint,
  geometryChanged,
  hitPane,
  isCopyChord,
  isExitChord,
  isSafeWebUri,
  linkAt,
  mouseInput,
  rightClickRoute,
  routePointer,
  wheelRoute,
} from "./interaction";
import type { CellDto, FrameEvent, PaneMeta } from "./types";

const metrics = { cellWidth: 10, cellHeight: 20 };
const SAFE = "https://herdr.dev/docs?x=1";

function pane(over: Partial<PaneMeta>): PaneMeta {
  return {
    pane_id: "w1:p1",
    content_revision: 40,
    rect: { x: 0, y: 0, width: 9, height: 4 },
    inner_rect: { x: 0, y: 0, width: 9, height: 4 },
    scroll: { offset_from_bottom: 2, max_offset_from_bottom: 10, viewport_rows: 4 },
    focused: true,
    mouse_reporting: false,
    sgr_pixel_mouse: false,
    alternate_screen_active: false,
    pixel_width: 81,
    pixel_height: 72,
    ...over,
  };
}
const left = () => pane({});
const right = () =>
  pane({
    pane_id: "w1:p2",
    content_revision: 7,
    rect: { x: 10, y: 0, width: 10, height: 4 },
    inner_rect: { x: 10, y: 0, width: 10, height: 4 },
    scroll: null,
    focused: false,
    mouse_reporting: true,
    sgr_pixel_mouse: true,
    alternate_screen_active: true,
    pixel_width: 90,
    pixel_height: 72,
  });

function loaded(): { grid: TerminalGrid; meta: SurfaceMeta } {
  const cells: CellDto[] = Array.from({ length: 80 }, () => ({ s: " ", fg: 0, bg: 0, m: 0 }));
  for (let x = 0; x < 4; x++) cells[20 + x] = { s: "h", fg: 0, bg: 0, m: 0, h: 0 };
  cells[10] = { s: "j", fg: 0, bg: 0, m: 0, h: 1 };
  cells[2 * 20 + 5] = { s: "界", fg: 0, bg: 0, m: 0, h: 0 };
  cells[2 * 20 + 6] = { s: "", fg: 0, bg: 0, m: 0 };
  cells[3 * 20 + 1] = { s: "z", fg: 0, bg: 0, m: 0, h: 9 };
  const grid = new TerminalGrid();
  grid.apply({ type: "full", revision: 12, width: 20, height: 4, cells, cursor: null, panes: [] });
  const meta = new SurfaceMeta();
  meta.apply({ type: "metadata", revision: 12, panes: [left(), right()], hyperlinks: [SAFE, "javascript:alert(1)"] }, grid.revision);
  return { grid, meta };
}

describe("metadata follows real full/patch frames", () => {
  // Would catch: metadata of another revision applied (stale scroll offset after a rejected
  // patch), patch metadata replacing the whole pane list, or the link table dropped by a patch.
  it("merges patch panes by id and ignores revisions the grid does not hold", () => {
    const { meta } = loaded();
    const updated = pane({ content_revision: 41, scroll: { offset_from_bottom: 0, max_offset_from_bottom: 11, viewport_rows: 4 } });
    expect(meta.apply({ type: "metadata", revision: 14, panes: [updated], hyperlinks: null }, 13)).toBe(false);
    expect(meta.pane("w1:p1")?.content_revision).toBe(40);
    expect(meta.apply({ type: "metadata", revision: 13, panes: [updated], hyperlinks: null }, 13)).toBe(true);
    expect(meta.pane("w1:p1")?.content_revision).toBe(41);
    expect(meta.pane("w1:p2")?.mouse_reporting).toBe(true);
    expect(meta.links).toEqual([SAFE, "javascript:alert(1)"]);
    const other: FrameEvent = { type: "state", state: "live", reason: null, error: null };
    expect(meta.apply(other, 13)).toBe(false);
  });
});

describe("coordinates", () => {
  // Would catch: rounding instead of flooring, points outside the canvas mapped to edge cells,
  // or pane-local columns computed from the surface origin (right pane shifted by 10).
  it("maps CSS points to surface cells and pane-local cells", () => {
    expect(cellFromPoint(19.9, 39.9, metrics, 20, 4)).toEqual({ x: 1, y: 1 });
    expect(cellFromPoint(-0.1, 5, metrics, 20, 4)).toBeNull();
    expect(cellFromPoint(200, 5, metrics, 20, 4)).toBeNull();
    const { meta } = loaded();
    expect(hitPane(meta.panes, 13, 2)).toMatchObject({ pane: { pane_id: "w1:p2" }, column: 3, row: 2 });
    expect(hitPane(meta.panes, 9, 2)).toBeNull(); // border column between panes
  });

  // Would catch: pixels sent to a pane without SGR pixel mode, 0-based pixels (engine requires
  // x>0), CSS pixels instead of device pixels of the pane, or geometry of the whole surface.
  it("builds mouse input with pane pixels only when announced", () => {
    const { meta } = loaded();
    const hitRight = hitPane(meta.panes, 19, 3)!;
    // Local CSS point inside the pane: x = 99.9 of 100 → pixel 90 of 90; y = 0 → pixel 1.
    expect(mouseInput(hitRight, "drag", "middle", 0b101, 3, { x: 99.9, y: 0 }, metrics)).toEqual({
      kind: "mouse",
      action: "drag",
      button: "middle",
      column: 9,
      row: 3,
      pixel: { x: 90, y: 1 },
      geometry: { cols: 10, rows: 4, width_px: 90, height_px: 72 },
      modifiers: 0b101,
      lines: 3,
    });
    const hitLeft = hitPane(meta.panes, 2, 1)!;
    expect(mouseInput(hitLeft, "scroll_up", undefined, 0, 5, { x: 25, y: 30 }, metrics)).toEqual({
      kind: "mouse",
      action: "scroll_up",
      column: 2,
      row: 1,
      modifiers: 0,
      lines: 5,
    });
  });

  // Would catch: DPI change with the same cols/rows deduplicated away (canvas stays blurry and
  // the engine keeps stale cell pixels), or identical geometry resent on every observer tick.
  it("treats cell pixel changes as a new geometry", () => {
    const a = { cols: 80, rows: 24, cell_width_px: 9, cell_height_px: 18 };
    expect(geometryChanged(null, a)).toBe(true);
    expect(geometryChanged(a, { ...a })).toBe(false);
    expect(geometryChanged(a, { ...a, cell_width_px: 18, cell_height_px: 36 })).toBe(true);
    expect(geometryChanged(a, { ...a, rows: 25 })).toBe(true);
  });
});

describe("pointer routing and focus", () => {
  // Would catch: clicks forwarded to an unfocused pane's application, Shift not reserving the
  // local selection, or a non-reporting pane receiving mouse bytes.
  it("routes to focus, application or local selection", () => {
    const { meta } = loaded();
    const r = hitPane(meta.panes, 12, 0)!;
    const l = hitPane(meta.panes, 1, 0)!;
    expect(routePointer(r, { shift: false }, "w1:p1")).toBe("focus");
    expect(routePointer(r, { shift: false }, "w1:p2")).toBe("app");
    expect(routePointer(r, { shift: true }, "w1:p2")).toBe("select");
    expect(routePointer(l, { shift: false }, "w1:p1")).toBe("select");
    expect(routePointer(l, { shift: false }, null)).toBe("focus");
  });

  // Spec 028 AC-028-03. Would catch: a right click reaching the app without the passthrough (or
  // from an unfocused pane), or the menu failing to open with Shift/without mouse reporting.
  it("routes right clicks to the app only with mouse reporting and passthrough", () => {
    const reporting = hitPane([pane({ pane_id: "w1:p2", mouse_reporting: true })], 1, 0)!;
    const quiet = hitPane([pane({ pane_id: "w1:p2", mouse_reporting: false })], 1, 0)!;
    const focused = "w1:p2";
    expect(rightClickRoute(reporting, { shift: false }, { focusedPaneId: focused, passthrough: true })).toBe("app");
    expect(rightClickRoute(reporting, { shift: true }, { focusedPaneId: focused, passthrough: true })).toBe("menu");
    expect(rightClickRoute(reporting, { shift: false }, { focusedPaneId: focused, passthrough: false })).toBe("menu");
    expect(rightClickRoute(quiet, { shift: false }, { focusedPaneId: focused, passthrough: true })).toBe("menu");
    expect(rightClickRoute(reporting, { shift: false }, { focusedPaneId: "w1:p1", passthrough: true })).toBe("menu");
    expect(rightClickRoute(reporting, { shift: false }, { focusedPaneId: null, passthrough: true })).toBe("menu");
  });

  // Would catch: typing accepted before the engine confirms the new pane, a confirmation of the
  // old pane releasing the gate, a failure leaving input blocked forever, or duplicate requests.
  it("blocks keyboard input until the requested pane is confirmed focused", () => {
    const gate = new FocusGate();
    expect(gate.allowsInput()).toBe(true);
    expect(gate.request("w1:p2")).toBe(true);
    expect(gate.request("w1:p2")).toBe(false);
    expect(gate.allowsInput()).toBe(false);
    expect(gate.confirm([left(), right()])).toBe(false);
    expect(gate.allowsInput()).toBe(false);
    expect(gate.confirm([pane({ focused: false }), { ...right(), focused: true }])).toBe(true);
    expect(gate.allowsInput()).toBe(true);
    expect(gate.request("w1:p1")).toBe(true);
    gate.fail("w1:p2"); // late failure of another request does not release
    expect(gate.allowsInput()).toBe(false);
    gate.fail("w1:p1");
    expect(gate.allowsInput()).toBe(true);
    expect(gate.pending).toBeNull();
  });
});

describe("wheel", () => {
  // Would catch: wheel on the focused pane kept local (application in alt-screen never scrolls),
  // Shift+wheel sent to the app, offsets beyond the scrollback, pixel deltas counted as lines, or
  // an unfocused pane getting pane.scroll without focus (spec 033: focus + wheel input instead).
  it("sends focused wheel to the engine once; Shift goes to pane.scroll; unfocused asks focus", () => {
    const { meta } = loaded();
    const l = hitPane(meta.panes, 1, 1)!;
    const r = hitPane(meta.panes, 11, 1)!;
    expect(wheelRoute(l, "w1:p1", { deltaY: -60, deltaMode: 0, shift: false }, metrics)).toEqual({
      kind: "input",
      input: { kind: "mouse", action: "scroll_up", column: 1, row: 1, modifiers: 0, lines: 3 },
      repeat: 1,
    });
    expect(wheelRoute(l, "w1:p1", { deltaY: 2, deltaMode: 1, shift: true }, metrics)).toEqual({
      kind: "scroll",
      request: { pane_id: "w1:p1", offset_from_bottom: 0 },
    });
    expect(wheelRoute(l, "w1:p1", { deltaY: -400, deltaMode: 0, shift: true }, metrics)).toEqual({
      kind: "scroll",
      request: { pane_id: "w1:p1", offset_from_bottom: 10 },
    });
    expect(wheelRoute(l, "w1:p2", { deltaY: -20, deltaMode: 0, shift: false }, metrics)).toEqual({
      kind: "focus",
      input: { kind: "mouse", action: "scroll_up", column: 1, row: 1, modifiers: 0, lines: 1 },
      repeat: 1,
    });
    // The right pane has no scrollback: unshifted it still gets the focus route (the engine
    // decides), shifted it is a no-op.
    expect(wheelRoute(r, "w1:p1", { deltaY: -20, deltaMode: 0, shift: false }, metrics)).toEqual({
      kind: "focus",
      input: { kind: "mouse", action: "scroll_up", column: 1, row: 1, modifiers: 0, lines: 1 },
      repeat: 1,
    });
    expect(wheelRoute(r, "w1:p1", { deltaY: -20, deltaMode: 0, shift: true }, metrics)).toBeNull();
    expect(wheelRoute(l, "w1:p1", { deltaY: 0, deltaMode: 0, shift: false }, metrics)).toBeNull();
  });
});

describe("wheel notches while the focus is pending (AC-033-01)", () => {
  // Would catch: notches dropped on confirmation, summed ignoring direction, delivered to a pane
  // that is not the pending one, or a stale position kept instead of the latest notch.
  it("sums the notches by direction and hands them to the confirmed pane once", () => {
    const queue = new PendingWheel();
    expect(queue.take("w1:p2")).toBeNull();
    queue.add("w1:p2", 1, 1, -3);
    queue.add("w1:p2", 2, 1, -3);
    queue.add("w1:p2", 2, 2, 3);
    expect(queue.take("w1:p1")).toBeNull();
    expect(queue.take("w1:p2")).toEqual([{
      kind: "mouse",
      action: "scroll_up",
      column: 2,
      row: 2,
      modifiers: 0,
      lines: 3,
    }]);
    expect(queue.take("w1:p2")).toBeNull();
  });

  // Would catch: the old pane's notches surviving a new focus request (delivered to a pane the
  // user no longer points at), or a zero net sum emitting an input.
  it("drops the old pane's notches when the pending pane changes and emits nothing on a zero sum", () => {
    const queue = new PendingWheel();
    queue.add("w1:p2", 1, 1, -3);
    queue.add("w1:p1", 4, 2, 3);
    expect(queue.take("w1:p2")).toBeNull();
    expect(queue.take("w1:p1")).toEqual([{
      kind: "mouse",
      action: "scroll_down",
      column: 4,
      row: 2,
      modifiers: 0,
      lines: 3,
    }]);
    queue.add("w1:p1", 1, 1, -3);
    queue.add("w1:p1", 1, 1, 3);
    expect(queue.take("w1:p1")).toBeNull();
  });
});

describe("wheel reports while the focus is pending (TUI parity + 1)", () => {
  // Would catch: the repeats of a mouse-reporting pane collapsed into one report on confirmation,
  // or summed ignoring direction.
  it("sums the signed reports and hands them as that many inputs", () => {
    const queue = new PendingWheel();
    queue.add("w1:p2", 1, 1, -7, 3);
    queue.add("w1:p2", 1, 1, -7, 3);
    queue.add("w1:p2", 1, 1, 7, 3);
    const inputs = queue.take("w1:p2")!;
    expect(inputs).toHaveLength(3);
    for (const input of inputs) expect(input).toMatchObject({ action: "scroll_up", lines: 7 });
  });
});

describe("selection", () => {
  // Would catch: viewport rows sent as absolute rows (offset 2 of 10 → top row 8), reversed
  // drags not ordered, content revision missing, or highlight bleeding outside the range.
  it("keeps absolute rows and requests pane.selection.read with the content revision", () => {
    const { meta } = loaded();
    const sel = new TerminalSelection();
    const p = meta.pane("w1:p1")!;
    sel.start(hitPane(meta.panes, 7, 3)!, p);
    sel.extend(hitPane(meta.panes, 2, 1)!, p);
    expect(sel.request(p)).toEqual({
      pane_id: "w1:p1",
      anchor: { row: 9, col: 2 },
      cursor: { row: 11, col: 7 },
      content_revision: 40,
    });
    expect(sel.contains("w1:p1", 1, 1, p)).toBe(false);
    expect(sel.contains("w1:p1", 1, 2, p)).toBe(true);
    expect(sel.contains("w1:p1", 2, 8, p)).toBe(true);
    expect(sel.contains("w1:p1", 3, 8, p)).toBe(false);
    expect(sel.contains("w1:p2", 2, 3, meta.pane("w1:p2")!)).toBe(false);
    expect(sel.surfaceRows(p)).toEqual([1, 2, 3]);
    // Scrolling to the bottom moves the highlight with its content (rows 9..11 → viewport 1..3 at
    // top 8; at offset 0 top is 10 → viewport rows 0..1 still visible, row 9 above).
    const bottom = { ...p, scroll: { offset_from_bottom: 0, max_offset_from_bottom: 10, viewport_rows: 4 } };
    expect(sel.surfaceRows(bottom)).toEqual([0, 1]);
    // A drag that never left its cell selects nothing.
    const click = new TerminalSelection();
    click.start(hitPane(meta.panes, 4, 0)!, p);
    click.extend(hitPane(meta.panes, 4, 0)!, p);
    expect(click.request(p)).toBeNull();
    // Content revision is taken from current metadata, not from the drag start.
    expect(sel.request({ ...p, content_revision: 44 })?.content_revision).toBe(44);
  });

  // Would catch: Ctrl+C captured as copy (SIGINT lost), Tab used to leave the terminal, or the
  // copy/exit chords missing their Shift requirement.
  it("uses explicit chords that never steal shell keys", () => {
    const k = (key: string, mods: { ctrlKey?: boolean; shiftKey?: boolean; altKey?: boolean } = {}) => ({
      key,
      ctrlKey: false,
      shiftKey: false,
      altKey: false,
      metaKey: false,
      ...mods,
    });
    expect(isCopyChord(k("C", { ctrlKey: true, shiftKey: true }))).toBe(true);
    expect(isCopyChord(k("c", { ctrlKey: true }))).toBe(false);
    expect(isExitChord(k("F6", { ctrlKey: true, shiftKey: true }))).toBe(true);
    expect(isExitChord(k("Tab"))).toBe(false);
    expect(isExitChord(k("Tab", { shiftKey: true }))).toBe(false);
    expect(isExitChord(k("F6"))).toBe(false);
  });
});

describe("links", () => {
  // Would catch: javascript:/file: or control-character URIs accepted, whitespace smuggling, or
  // HTML-looking strings treated as safe.
  it("allows only plain web URIs", () => {
    expect(isSafeWebUri(SAFE)).toBe(true);
    expect(isSafeWebUri("http://127.0.0.1:8080/a")).toBe(true);
    expect(isSafeWebUri("javascript:alert(1)")).toBe(false);
    expect(isSafeWebUri("file:///etc/passwd")).toBe(false);
    expect(isSafeWebUri("https://a.b/\u001b]52")).toBe(false);
    expect(isSafeWebUri("https://a.b/ x")).toBe(false);
    expect(isSafeWebUri("https://")).toBe(false);
    expect(isSafeWebUri("")).toBe(false);
    expect(isSafeWebUri(`https://a.b/${"x".repeat(2100)}`)).toBe(false);
  });

  // Would catch: link resolved from the wrong pane origin, wide-char continuation cells losing
  // their link, indices beyond the table resolving, or unsafe links reported as safe.
  it("resolves the link under a cell with pane-local coordinates", () => {
    const { grid, meta } = loaded();
    expect(linkAt(grid, meta, 3, 1)).toEqual({
      request: { pane_id: "w1:p1", uri: SAFE, viewport_row: 1, col: 3, content_revision: 40 },
      safe: true,
    });
    expect(linkAt(grid, meta, 6, 2)).toMatchObject({ request: { col: 6, uri: SAFE }, safe: true });
    expect(linkAt(grid, meta, 10, 0)).toEqual({
      request: { pane_id: "w1:p2", uri: "javascript:alert(1)", viewport_row: 0, col: 0, content_revision: 7 },
      safe: false,
    });
    expect(linkAt(grid, meta, 1, 3)).toBeNull();
    expect(linkAt(grid, meta, 4, 1)).toBeNull();
  });
});

describe("application mouse gesture is bound to the confirmed pane (r1)", () => {
  const ID = "boot-term-3|1|1";
  // Right pane focused and reporting, same inner size as a second reporting pane `w1:p3` below it.
  function setup() {
    const meta = new SurfaceMeta();
    const r = { ...right(), focused: true };
    const other = pane({
      pane_id: "w1:p3",
      content_revision: 9,
      rect: { x: 10, y: 0, width: 10, height: 4 },
      inner_rect: { x: 10, y: 0, width: 10, height: 4 },
      scroll: null,
      focused: false,
      mouse_reporting: true,
      sgr_pixel_mouse: true,
      pixel_width: 90,
      pixel_height: 72,
    });
    meta.apply({ type: "metadata", revision: 12, panes: [{ ...left(), focused: false }, r], hyperlinks: [] }, 12);
    const g = new AppMouseGesture();
    const hit = hitPane(meta.panes, 12, 1)!;
    const down = g.start(hit, "left", 0, { x: 25, y: 30 }, ID, metrics);
    return { meta, g, r, other, down };
  }
  const focusedOf = (m: SurfaceMeta) => m.focused()?.pane_id ?? null;

  // Would catch: a drag/release of the old pane delivered to a newly confirmed pane with the same
  // size, a gesture resumed after cancellation, or a buffered release replayed later.
  it("cancels on focus confirmed elsewhere and never sends drag/up afterwards", () => {
    const { meta, g, r, other, down } = setup();
    expect(down).toMatchObject({ kind: "mouse", action: "down", button: "left", column: 2, row: 1 });
    expect(g.drag(hitPane(meta.panes, 13, 1)!, 0, null, ID, meta, focusedOf(meta), metrics)).toMatchObject({ action: "drag", column: 3 });
    // Engine confirms focus on w1:p3 occupying the same rect (tab/zoom swap).
    meta.apply({ type: "metadata", revision: 12, panes: [{ ...r, focused: false }, { ...other, focused: true }], hyperlinks: null }, 12);
    meta.apply({ type: "metadata", revision: 12, panes: [{ ...left(), focused: false }, { ...other, focused: true }], hyperlinks: [] }, 12);
    expect(g.check(ID, meta, focusedOf(meta))).toBe(false);
    expect(g.active).toBe(false);
    expect(g.drag(hitPane(meta.panes, 14, 1)!, 0, null, ID, meta, focusedOf(meta), metrics)).toBeNull();
    expect(g.release(hitPane(meta.panes, 14, 1)!, 0, null, ID, meta, focusedOf(meta), metrics)).toBeNull();
  });

  // Would catch: gestures surviving a new boot/connection, a removed pane, a layout or pixel size
  // change, reporting turned off, a pending focus request, or a full frame without metadata yet.
  it("cancels on identity, removal, layout, pixels, reporting, pending focus and full frames", () => {
    const cases: ((m: SurfaceMeta, r: PaneMeta) => { id?: string; focused?: string | null })[] = [
      () => ({ id: "boot-term-4|1|1" }),
      () => ({ id: "boot-term-3|2|1" }),
      (m) => (m.apply({ type: "metadata", revision: 12, panes: [left()], hyperlinks: [] }, 12), {}),
      (m, r) => (m.apply({ type: "metadata", revision: 12, panes: [{ ...r, inner_rect: { x: 10, y: 0, width: 9, height: 4 } }], hyperlinks: null }, 12), {}),
      (m, r) => (m.apply({ type: "metadata", revision: 12, panes: [{ ...r, pixel_width: 180, pixel_height: 144 }], hyperlinks: null }, 12), {}),
      (m, r) => (m.apply({ type: "metadata", revision: 12, panes: [{ ...r, mouse_reporting: false }], hyperlinks: null }, 12), {}),
      () => ({ focused: null }),
      (m) => (m.invalidate(), {}),
    ];
    for (const [i, mutate] of cases.entries()) {
      const { meta, g, r } = setup();
      const change = mutate(meta, r);
      const id = change.id ?? ID;
      const focused = "focused" in change ? change.focused! : focusedOf(meta);
      // Release straight after the change (no prior check): nothing is sent, gesture ends.
      expect(g.release(null, 0, null, id, meta, focused, metrics), `case ${i}`).toBeNull();
      expect(g.active, `case ${i}`).toBe(false);
      const d = setup();
      mutate(d.meta, d.r);
      expect(d.g.drag(hitPane(d.meta.panes, 15, 2), 0, null, id, d.meta, focused, metrics), `drag ${i}`).toBeNull();
      expect(d.g.check(id, d.meta, focused), `case ${i}`).toBe(false);
    }
    // Control: unchanged state keeps the gesture and releases once on the original pane.
    const { meta, g } = setup();
    expect(g.check(ID, meta, focusedOf(meta))).toBe(true);
    expect(g.release(null, 0, null, ID, meta, focusedOf(meta), metrics)).toMatchObject({ action: "up", column: 2, row: 1 });
    expect(g.release(null, 0, null, ID, meta, focusedOf(meta), metrics)).toBeNull();
  });

  // Would catch: drag events outside the captured pane forwarded, or one drag per pointermove
  // inside the same cell (duplicates).
  it("drags once per cell change inside the captured pane only", () => {
    const { meta, g } = setup();
    const f = focusedOf(meta);
    expect(g.drag(hitPane(meta.panes, 12, 1)!, 0, null, ID, meta, f, metrics)).toBeNull();
    expect(g.drag(hitPane(meta.panes, 3, 1)!, 0, null, ID, meta, f, metrics)).toBeNull();
    expect(g.drag(hitPane(meta.panes, 12, 2)!, 0, null, ID, meta, f, metrics)).toMatchObject({ action: "drag", row: 2 });
    expect(g.drag(hitPane(meta.panes, 12, 2)!, 0, null, ID, meta, f, metrics)).toBeNull();
    expect(g.active).toBe(true);
  });
});

describe("row decorations cost (r1)", () => {
  // Would catch: URL validation redone per cell/frame instead of once per link table, or the
  // selection predicate looking up/scanning panes per cell instead of once per row.
  it("validates each link once per table and looks the selection pane up once per row", () => {
    const check = vi.fn((uri: string) => uri.startsWith("https://"));
    const meta = new SurfaceMeta(check);
    const { grid } = loaded();
    meta.apply({ type: "metadata", revision: 12, panes: [left(), right()], hyperlinks: [SAFE, "javascript:alert(1)"] }, 12);
    expect(check).toHaveBeenCalledTimes(2);
    const sel = new TerminalSelection();
    const p = meta.pane("w1:p1")!;
    sel.start(hitPane(meta.panes, 7, 3)!, p);
    sel.extend(hitPane(meta.panes, 2, 1)!, p);
    const lookups = vi.spyOn(meta, "pane");
    const marks: string[] = [];
    for (let y = 0; y < grid.height; y++) {
      const decor = rowDecor(meta, sel, grid, y)!;
      for (let x = 0; x < grid.width; x++) {
        if (decor.selected?.(x)) marks.push(`s${x},${y}`);
        if (decor.link?.(x)) marks.push(`l${x},${y}`);
      }
    }
    expect(lookups.mock.calls.length).toBeLessThanOrEqual(grid.height);
    expect(check).toHaveBeenCalledTimes(2);
    expect(marks.filter((m) => m.startsWith("l"))).toEqual(["l0,1", "l1,1", "l2,1", "l3,1", "l5,2"]);
    expect(marks.filter((m) => m.startsWith("s"))).toEqual([
      "s2,1", "s3,1", "s4,1", "s5,1", "s6,1", "s7,1", "s8,1",
      "s0,2", "s1,2", "s2,2", "s3,2", "s4,2", "s5,2", "s6,2", "s7,2", "s8,2",
      "s0,3", "s1,3", "s2,3", "s3,3", "s4,3", "s5,3", "s6,3", "s7,3",
    ]);
    // Patch metadata keeps the table: no revalidation.
    meta.apply({ type: "metadata", revision: 12, panes: [{ ...p, content_revision: 41 }], hyperlinks: null }, 12);
    expect(check).toHaveBeenCalledTimes(2);
    expect(rowDecor(new SurfaceMeta(check), new TerminalSelection(), grid, 0)).toBeUndefined();
  });
});
