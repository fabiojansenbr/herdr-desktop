// Opt-in terminal repaint probe (spec 007 resource bench). Exercises the same seam TerminalView
// uses (`probedScheduler` over the real RepaintScheduler) with a controlled frame queue.
import { afterEach, describe, expect, it, vi } from "vitest";
import { fixtureEvents } from "../harness/fixture";
import { TerminalGrid } from "./grid";
import { readFileSync } from "node:fs";
import { probedFullFrame, probedMetadata, probedPointerRoute, probeFromSelection, probedScheduler, POINTER_ROUTE_LIMIT, type PointerRouteTrace } from "./probe";
import type { PaneMeta } from "./types";

function frames() {
  let next = 1;
  const queue = new Map<number, () => void>();
  return {
    request: vi.fn((cb: () => void) => {
      const h = next++;
      queue.set(h, cb);
      return h;
    }),
    cancel: vi.fn((h: number) => void queue.delete(h)),
    flush() {
      const pending = Array.from(queue.values());
      queue.clear();
      for (const cb of pending) cb();
    },
    get size() {
      return queue.size;
    },
  };
}

const enabled = { feature: "fidelity", phase: "resource-bench", params: { terminal_probe: true } };

afterEach(() => vi.useRealTimers());

describe("terminal repaint probe", () => {
  it("is off outside an explicit harness selection (product mode allocates nothing)", () => {
    expect(probeFromSelection(undefined)).toBeNull();
    expect(probeFromSelection({ feature: "fidelity", phase: "flow", params: {} })).toBeNull();
    expect(probeFromSelection({ ...enabled, params: { terminal_probe: "true" } })).toBeNull();
    expect(probeFromSelection({ feature: "Fidelity!", phase: "resource-bench", params: { terminal_probe: true } })).toBeNull();
    expect(probeFromSelection(enabled)).not.toBeNull();
  });

  it("disabled: the scheduler gets the raw frame request, no wrapper and no timers", () => {
    vi.useFakeTimers();
    const f = frames();
    const paint = vi.fn(() => 3);
    const scheduler = probedScheduler(null, f.request, f.cancel, paint);
    scheduler.schedule([0, 1, 2]);
    expect(f.request).toHaveBeenCalledTimes(1);
    // A wrapper would hand the frame queue a different callback than the scheduler's own.
    const handed = f.request.mock.calls[0]![0];
    expect(handed.toString()).toContain("this.handle");
    f.flush();
    expect(paint).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("hidden pane with an active producer: zero terminal frames and rows, visible control counts", () => {
    vi.useFakeTimers();
    const probe = probeFromSelection(enabled)!;
    const f = frames();
    const scheduler = probedScheduler(probe, f.request, f.cancel, (rows) => rows.size);

    // Positive control, visible, same scheduler: producer dirties 4 rows twice.
    scheduler.schedule([0, 1, 2, 3]);
    f.flush();
    scheduler.schedule([5, 6]);
    f.flush();
    const visible = probe.snapshot();
    expect(visible).toEqual({ raf_requests: 2, raf_callbacks: 2, paint_calls: 2, painted_rows: 6, full_frames: 0 });

    // Hidden: the producer keeps emitting; no frame may be requested nor rows painted.
    scheduler.setVisible(false);
    for (let i = 0; i < 50; i++) {
      scheduler.schedule([i % 24]);
      f.flush();
    }
    expect(f.size).toBe(0);
    expect(probe.snapshot()).toEqual(visible);

    // Shown again: exactly one frame repaints the accumulated rows (the probe is not dead).
    scheduler.setVisible(true);
    f.flush();
    expect(probe.snapshot()).toEqual({ raf_requests: 3, raf_callbacks: 3, paint_calls: 3, painted_rows: 30, full_frames: 0 });
    expect(vi.getTimerCount()).toBe(0);
  });

  it("a frame cancelled by hiding is neither a callback nor a paint", () => {
    const probe = probeFromSelection(enabled)!;
    const f = frames();
    const scheduler = probedScheduler(probe, f.request, f.cancel, (rows) => rows.size);
    scheduler.schedule([1]);
    scheduler.setVisible(false);
    f.flush();
    expect(probe.snapshot()).toEqual({ raf_requests: 1, raf_callbacks: 0, paint_calls: 0, painted_rows: 0, full_frames: 0 });
  });

  it("counts only the terminal's own frames, never the page's global requestAnimationFrame", () => {
    const probe = probeFromSelection(enabled)!;
    const f = frames();
    probedScheduler(probe, f.request, f.cancel, (rows) => rows.size);
    const raf = vi.fn((cb: FrameRequestCallback) => {
      cb(0);
      return 1;
    });
    vi.stubGlobal("requestAnimationFrame", raf);
    try {
      globalThis.requestAnimationFrame(() => undefined);
      globalThis.requestAnimationFrame(() => undefined);
    } finally {
      vi.unstubAllGlobals();
    }
    expect(raf).toHaveBeenCalledTimes(2);
    expect(probe.snapshot()).toEqual({ raf_requests: 0, raf_callbacks: 0, paint_calls: 0, painted_rows: 0, full_frames: 0 });
  });

  it("a paint that draws nothing counts the call but no rows", () => {
    const probe = probeFromSelection(enabled)!;
    const f = frames();
    const scheduler = probedScheduler(probe, f.request, f.cancel, () => 0);
    scheduler.schedule([0, 1]);
    f.flush();
    expect(probe.snapshot()).toEqual({ raf_requests: 1, raf_callbacks: 1, paint_calls: 1, painted_rows: 0, full_frames: 0 });
  });

  // Would catch (GUI r4 resize-dpi): stty compared with the composed surface instead of the pane
  // inner_rect the frame carried, recording the outer rect, or keeping a live reference that a
  // later metadata mutates.
  it("geometry records the surface and each pane inner_rect of accepted metadata, as copies", () => {
    const probe = probeFromSelection(enabled)!;
    expect(probe.geometry()).toEqual({ surface: null, panes: [], full_frames: 0 });
    const pane = {
      pane_id: "w1-p1", content_revision: 3, rect: { x: 0, y: 0, width: 75, height: 33 }, inner_rect: { x: 0, y: 0, width: 74, height: 33 },
      scroll: null, focused: true, mouse_reporting: false, sgr_pixel_mouse: false, alternate_screen_active: false, pixel_width: 0, pixel_height: 0,
    } as PaneMeta;
    const panes = [pane];
    probedMetadata(probe, { width: 75, height: 33 }, panes);
    const want = { surface: { cols: 75, rows: 33 }, panes: [{ pane_id: "w1-p1", inner_rect: { x: 0, y: 0, width: 74, height: 33 } }], full_frames: 0 };
    expect(probe.geometry()).toEqual(want);
    pane.inner_rect.width = 1;
    panes.push({ ...pane, pane_id: "w1-p2" });
    expect(probe.geometry()).toEqual(want);
    // Counters are untouched by metadata; disabled probe records nothing and does not throw.
    expect(probe.snapshot()).toEqual({ raf_requests: 0, raf_callbacks: 0, paint_calls: 0, painted_rows: 0, full_frames: 0 });
    expect(() => probedMetadata(null, { width: 75, height: 33 }, panes)).not.toThrow();
    // Metadata is stamped with the accepted Full frames at that moment (stale until the next one).
    const grid = new TerminalGrid();
    probedFullFrame(probe, grid.apply(fixtureEvents().full));
    expect(probe.geometry().full_frames).toBe(0);
    probedMetadata(probe, { width: 75, height: 33 }, [pane]);
    expect(probe.geometry().full_frames).toBe(1);
  });

  // Would catch: counting every frame event (patches, gaps, inconsistent Full) as a resize repaint,
  // or a counter that never moves, which would let resize-dpi pass on a stale canvas.
  it("full_frames counts only a Full frame the grid accepted, never patches, gaps or rejected frames", () => {
    const probe = probeFromSelection(enabled)!;
    const grid = new TerminalGrid();
    const events = fixtureEvents();
    const count = (event: Parameters<TerminalGrid["apply"]>[0]) => probedFullFrame(probe, grid.apply(event));
    count(events.patches[0]!); // no base surface: rejected
    count({ ...events.full, cells: (events.full as { cells: unknown[] }).cells.slice(1) } as typeof events.full); // inconsistent Full: rejected
    expect(probe.snapshot().full_frames).toBe(0);
    count(events.full);
    expect(probe.snapshot().full_frames).toBe(1);
    count(events.patches[0]!); // accepted patch
    count({ ...events.patches[0]!, revision: (events.patches[0] as { revision: number }).revision + 5 } as typeof events.full); // revision gap
    expect(probe.snapshot()).toEqual({ raf_requests: 0, raf_callbacks: 0, paint_calls: 0, painted_rows: 0, full_frames: 1 });
    count(events.full);
    expect(probe.snapshot().full_frames).toBe(2);
  });

  it("disabled: a Full frame touches no counter and allocates nothing", () => {
    vi.useFakeTimers();
    const grid = new TerminalGrid();
    expect(() => probedFullFrame(null, grid.apply(fixtureEvents().full))).not.toThrow();
    expect(vi.getTimerCount()).toBe(0);
  });

  // Spec 010 r4: the Ctrl+click on a link never requested the open action; which exit of
  // TerminalView.onPointerDown ran was not observable.
  const trace = (over: Partial<PointerRouteTrace> = {}): PointerRouteTrace => ({
    exit: "link_open", cell: { x: 3, y: 0 }, hit_pane: "w3:p1", button: "left", route: "select", mouse_reporting: false,
    stale: false, input_allowed: true, link_found: true, link_safe: true, ctrl: true, ...over,
  });

  // Would catch: a probe that records by reference (a later mutation rewrites the observed exit),
  // one that never clears (the scroll step's pointerdown attributed to the link step), an unbounded
  // queue, or a disabled probe that allocates or throws in the product.
  it("pointer routes: copies in order, take clears, bounded to the newest; disabled records nothing", () => {
    const probe = probeFromSelection(enabled)!;
    expect(probe.takePointerRoutes()).toEqual([]);
    const first = trace({ exit: "app_blocked", route: "app", mouse_reporting: true, input_allowed: false, link_found: null, link_safe: null });
    probedPointerRoute(probe, first);
    probedPointerRoute(probe, trace());
    first.cell!.x = 99;
    first.exit = "select";
    expect(probe.takePointerRoutes()).toEqual([
      trace({ exit: "app_blocked", route: "app", mouse_reporting: true, input_allowed: false, link_found: null, link_safe: null }),
      trace(),
    ]);
    expect(probe.takePointerRoutes()).toEqual([]);
    for (let i = 0; i < POINTER_ROUTE_LIMIT + 5; i++) probedPointerRoute(probe, trace({ cell: { x: i, y: 1 } }));
    const kept = probe.takePointerRoutes();
    expect(kept).toHaveLength(POINTER_ROUTE_LIMIT);
    expect(kept[0]!.cell).toEqual({ x: 5, y: 1 });
    expect(kept.at(-1)!.cell).toEqual({ x: POINTER_ROUTE_LIMIT + 4, y: 1 });
    // Routes never touch the repaint counters nor the geometry.
    expect(probe.snapshot()).toEqual({ raf_requests: 0, raf_callbacks: 0, paint_calls: 0, painted_rows: 0, full_frames: 0 });
    expect(probe.geometry()).toEqual({ surface: null, panes: [], full_frames: 0 });
    vi.useFakeTimers();
    expect(() => probedPointerRoute(null, trace())).not.toThrow();
    expect(vi.getTimerCount()).toBe(0);
  });

  // Would catch: an exit of onPointerDown that returns (or falls into selection) without being
  // traced, so a silent return is indistinguishable from a route that was never reached; or a
  // trace that builds its record while the probe is null.
  it("TerminalView traces every exit of onPointerDown, and only through the probe guard", () => {
    const view = readFileSync(new URL("./TerminalView.svelte", import.meta.url), "utf8");
    const body = /function onPointerDown\(e: PointerEvent\) \{\n([\s\S]*?)\n  \}\n/.exec(view)?.[1] ?? "";
    expect(body).not.toBe("");
    const lines = body.split("\n");
    const returns = lines.map((l, i) => (/\breturn\b/.test(l) ? i : -1)).filter((i) => i >= 0);
    expect(returns.length).toBeGreaterThanOrEqual(5);
    for (const i of returns) {
      // The block of this return (from the line opening it) must trace before returning.
      let open = i;
      while (open > 0 && !/\{\s*$/.test(lines[open]!)) open -= 1;
      const block = lines.slice(open, i + 1).join("\n");
      expect(block, `untraced exit: ${lines[i]!.trim()}`).toMatch(/tracePointer\(/);
    }
    // The fall-through (selection start) is traced too.
    expect(lines.filter((l) => l.trim() !== "").at(-1)).toMatch(/tracePointer\(.*"select"/);
    for (const exit of ["no_cell", "no_hit", "no_button", "focus", "app_blocked", "app", "non_left", "link_open", "link_unsafe", "select"]) {
      expect(body).toContain(`"${exit}"`);
    }
    const helper = /function tracePointer\([\s\S]*?\n  \}\n/.exec(view)?.[0] ?? "";
    expect(helper).toMatch(/if \(!repaintProbe\) return;[\s\S]*probedPointerRoute\(repaintProbe,/);
  });
});
