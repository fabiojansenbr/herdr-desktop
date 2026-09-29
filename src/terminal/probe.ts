// Opt-in repaint probe of the terminal canvas (spec 007 resource bench only).
//
// Enabled exclusively by a native harness selection that asks for it
// (`window.__HERDR_HARNESS__.params.terminal_probe === true`, injected by a test binary before
// page scripts). The product never has that selection, so `terminalProbe()` is null and
// `probedScheduler` hands the scheduler the raw frame request: no wrapper, no counters, no
// timers, no Svelte state and no DOM writes.
//
// Counted: frame requests/callbacks of the terminal's own RepaintScheduler and rows the
// terminal paint reported as drawn, and Full frames the grid accepted. Not counted: any other requestAnimationFrame of the page.
// A callback or a painted row is JS submission to the canvas, not physical presentation; it
// does not measure input→paint latency.

import { parseNativeHarness } from "../harness/native";
import type { ApplyResult } from "./grid";
import type { PaneMeta, RectDto } from "./types";
import { RepaintScheduler } from "./scheduler";

export interface TerminalProbeCounters {
  raf_requests: number;
  raf_callbacks: number;
  paint_calls: number;
  painted_rows: number;
  /** Full frames accepted by the terminal grid (resize-dpi); patches and rejected frames excluded. */
  full_frames: number;
}

/** Geometry of the last accepted frame metadata (resize-dpi): surface cells and pane inner_rect. */
export interface TerminalGeometry {
  surface: { cols: number; rows: number } | null;
  panes: { pane_id: string; inner_rect: RectDto }[];
  /** `full_frames` when this metadata was accepted: equal to the counter = metadata of the current Full. */
  full_frames: number;
}

/** Exit TerminalView.onPointerDown took (spec 010 r4 diagnosis of the Ctrl+click link path). */
export type PointerExit = "no_cell" | "no_hit" | "no_button" | "focus" | "app_blocked" | "app" | "non_left" | "link_open" | "link_unsafe" | "select";

/** One pointerdown on the terminal: the inputs of the routing decision and the exit taken. */
export interface PointerRouteTrace {
  exit: PointerExit;
  cell: { x: number; y: number } | null;
  hit_pane: string | null;
  button: string | null;
  route: string | null;
  mouse_reporting: boolean | null;
  stale: boolean;
  input_allowed: boolean;
  /** null when the exit happened before the link lookup. */
  link_found: boolean | null;
  link_safe: boolean | null;
  ctrl: boolean;
}

/** Newest pointer routes kept between two `takePointerRoutes` calls. */
export const POINTER_ROUTE_LIMIT = 32;

export interface TerminalProbe {
  snapshot(): TerminalProbeCounters;
  geometry(): TerminalGeometry;
  /** Pointer routes recorded since the previous call (oldest first); clears them. */
  takePointerRoutes(): PointerRouteTrace[];
}

class Counters implements TerminalProbe {
  raf_requests = 0;
  raf_callbacks = 0;
  paint_calls = 0;
  painted_rows = 0;
  full_frames = 0;
  last: TerminalGeometry = { surface: null, panes: [], full_frames: 0 };
  routes: PointerRouteTrace[] = [];

  takePointerRoutes(): PointerRouteTrace[] {
    return this.routes.splice(0);
  }

  geometry(): TerminalGeometry {
    return this.last;
  }

  snapshot(): TerminalProbeCounters {
    const { raf_requests, raf_callbacks, paint_calls, painted_rows, full_frames } = this;
    return { raf_requests, raf_callbacks, paint_calls, painted_rows, full_frames };
  }
}

/** Probe for an explicit harness selection asking for it; null otherwise. */
export function probeFromSelection(value: unknown): TerminalProbe | null {
  const selection = parseNativeHarness(value);
  return selection?.params.terminal_probe === true ? new Counters() : null;
}

let resolved = false;
let current: TerminalProbe | null = null;

/** The window's probe (resolved once from the injected selection). */
export function terminalProbe(): TerminalProbe | null {
  if (!resolved) {
    resolved = true;
    current = probeFromSelection(typeof window === "undefined" ? undefined : window.__HERDR_HARNESS__);
  }
  return current;
}

/** The terminal's repaint scheduler; counted only when `probe` is set. */
export function probedScheduler(
  probe: TerminalProbe | null,
  request: (cb: () => void) => number,
  cancel: (handle: number) => void,
  paintRows: (rows: Set<number>) => number,
): RepaintScheduler {
  if (!(probe instanceof Counters)) {
    return new RepaintScheduler(request, cancel, (rows) => void paintRows(rows));
  }
  const counters = probe;
  return new RepaintScheduler(
    (cb) => {
      counters.raf_requests += 1;
      return request(() => {
        counters.raf_callbacks += 1;
        cb();
      });
    },
    cancel,
    (rows) => {
      counters.paint_calls += 1;
      counters.painted_rows += paintRows(rows);
    },
  );
}

/** Counts a Full frame the grid accepted (resize/DPI repaint); patches and rejected frames never. */
export function probedFullFrame(probe: TerminalProbe | null, result: ApplyResult): void {
  if (probe instanceof Counters && result.applied && result.full) probe.full_frames += 1;
}

/** Records the surface and pane inner_rects of metadata the view accepted (copies); no-op when disabled. */
export function probedMetadata(probe: TerminalProbe | null, surface: { width: number; height: number }, panes: readonly PaneMeta[]): void {
  if (!(probe instanceof Counters)) return;
  probe.last = {
    surface: { cols: surface.width, rows: surface.height },
    panes: panes.map((p) => ({ pane_id: p.pane_id, inner_rect: { ...p.inner_rect } })),
    full_frames: probe.full_frames,
  };
}

/** Records the exit of one terminal pointerdown (copy, bounded to the newest); no-op when disabled. */
export function probedPointerRoute(probe: TerminalProbe | null, trace: PointerRouteTrace): void {
  if (!(probe instanceof Counters)) return;
  probe.routes.push({ ...trace, cell: trace.cell ? { ...trace.cell } : null });
  if (probe.routes.length > POINTER_ROUTE_LIMIT) probe.routes.splice(0, probe.routes.length - POINTER_ROUTE_LIMIT);
}
