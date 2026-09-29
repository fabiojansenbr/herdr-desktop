// Resource bench page driver of spec 007 (parent: src-tauri/tests/fidelity_bench.rs,
// tests/fidelity-bench/live.rs). One composed window, one PTY: the parent starts the generator
// in the confirmed pane and runs bench/collector.py during each window; the page only selects
// tabs (no file opened) and brackets each window with snapshots of the opt-in terminal probe.
// Probe counts are JS submissions to the canvas, not presentation or input→paint.
//
// Hiding (adapted in spec 009): the clean casca of spec 018 mounts a single terminal layer and no
// Files layer, so "hidden" is a window that is not visible. That is the product's own remaining
// path to `setInterest(false)` — `src/shell/interest.ts` drops the surface lease on
// `document.visibilityState === "hidden"`, minimize or loss of native visibility — and the engine
// then delivers no frame, so a hidden window paints nothing. The page asks the harness to hide the
// real window (the product never exposes `hide` to the WebView) and then reads
// `document.visibilityState` itself; it never asserts the state it asked for.

import { waitFor } from "../../harness/dom";
import type { TerminalProbe, TerminalProbeCounters } from "../../terminal/probe";
import { observePage, createKeyObserver, attemptPayload, type PageMetrics } from "./latency";
import { cellMetrics, paneCells } from "./paste-flow";
import { parseCells } from "./ssh-flow";

export interface BenchIdentity {
  pane_id: string;
  generation: string;
  boot_prefix: string;
}

export interface BenchPage {
  identity(): Promise<BenchIdentity>;
  /** Terminal canvases mounted in the window. */
  canvases(): number;
  /** Canvas backing store (device px), devicePixelRatio and the context font as painted. */
  geometry(): Record<string, unknown>;
  /** Takes the window off the screen (`true`) or brings it back (`false`). */
  setWindowHidden(hidden: boolean): Promise<void>;
  /** This window is not visible, so the product holds no surface interest. */
  terminalHidden(): boolean;
  editorOpen(): boolean;
  latencyMetrics?(): PageMetrics;
  listenKeydown?(handler: (e: KeyboardEvent) => void): () => void;
  focusTerminal?(): Promise<void> | void;
}

export interface BenchDeps {
  page: BenchPage;
  probe: Pick<TerminalProbe, "snapshot"> | null;
  awaitParent(step: string, detail: Record<string, unknown>, timeoutMs?: number): Promise<Record<string, unknown>>;
  sleep(ms: number): Promise<void>;
}

/** harness_await default is 110s; long collector windows need duration + ≥120s margin. */
const DEFAULT_PARENT_WAIT_MS = 110_000;
const PARENT_WAIT_MARGIN_S = 120;

function parentWaitMs(params: Record<string, unknown>): number {
  const warmup = typeof params.warmup_s === "number" ? params.warmup_s : 0;
  const measured = typeof params.measured_s === "number" ? params.measured_s : 0;
  return Math.max(DEFAULT_PARENT_WAIT_MS, Math.round((warmup + measured + PARENT_WAIT_MARGIN_S) * 1000));
}

type Window = { before: TerminalProbeCounters; after: TerminalProbeCounters };

/** Settle attempts for the canvas geometry, one `settle_ms` apart. */
const GEOMETRY_SETTLE_TRIES = 20;

/**
 * Canvas geometry after the composed layout stopped resizing it.
 *
 * The window paints once with the container's own size and resizes the canvas when the engine's
 * confirmed pane arrives, so the first reading is a transient size that does not divide the
 * engine grid. The parent reads `pane.layout` at bench-ready and compares the two, so the page
 * reports only a size it read twice unchanged; a canvas that keeps moving is an error, never a
 * transient number passed off as the measured geometry.
 */
export async function settledGeometry(
  page: Pick<BenchPage, "geometry">,
  sleep: (ms: number) => Promise<void>,
  settleMs: number,
): Promise<Record<string, unknown>> {
  let previous = page.geometry();
  for (let attempt = 0; attempt < GEOMETRY_SETTLE_TRIES; attempt += 1) {
    await sleep(settleMs);
    const current = page.geometry();
    if (JSON.stringify(current) === JSON.stringify(previous)) return current;
    previous = current;
  }
  throw new Error(
    `terminal geometry did not settle in ${GEOMETRY_SETTLE_TRIES} readings ${settleMs} ms apart; last ${JSON.stringify(previous)}`,
  );
}

export async function runResourceBench(deps: BenchDeps, params: Record<string, unknown>): Promise<Record<string, unknown>> {
  if (params.latency) {
    return runLatencyBench(deps, params);
  }
  const { page, probe, awaitParent, sleep } = deps;
  if (!probe) throw new Error("terminal probe is not enabled for this window (params.terminal_probe)");
  const settle = typeof params.settle_ms === "number" ? params.settle_ms : 500;
  const idleOnly = benchWindows(params.windows);
  const waitMs = parentWaitMs(params);
  const parent: Record<string, unknown> = {};
  const step = async (name: string, detail: Record<string, unknown>): Promise<Window> => {
    const before = probe.snapshot();
    parent[name] = await awaitParent(name, { ...detail, probe_before: before }, waitMs);
    return { before, after: probe.snapshot() };
  };

  const identity = await page.identity();
  const canvases = page.canvases();
  const geometry = await settledGeometry(page, sleep, settle);
  parent["bench-ready"] = await awaitParent("bench-ready", { ...identity, canvases, geometry });
  try {
    await sleep(settle);
    if (page.terminalHidden()) throw new Error("terminal is not visible before the visible window");
    if (idleOnly) {
      // Idle: no generator, window visible the whole time, never hidden.
      const idleWindow = await step("bench-idle", { terminal_hidden: false });
      return { ...identity, canvases, geometry, geometry_after: page.geometry(), idle: { ...idleWindow, terminal_hidden: page.terminalHidden() }, parent };
    }
    const visible = await step("bench-visible", { terminal_hidden: false });

    await page.setWindowHidden(true);
    await sleep(settle);
    if (!page.terminalHidden()) throw new Error("the window did not become hidden; no hidden window measured");
    if (page.editorOpen()) throw new Error("an editor is open; the hidden window must not open a file");
    const hiddenWindow = await step("bench-hidden", { terminal_hidden: true });
    const hidden = { ...hiddenWindow, terminal_hidden: page.terminalHidden(), editor_open: page.editorOpen() };

    await page.setWindowHidden(false);
    const reactivatedBefore = probe.snapshot();
    await sleep(settle);
    const reactivated = { before: reactivatedBefore, after: probe.snapshot() };
    return { ...identity, canvases, geometry, geometry_after: page.geometry(), visible, hidden, reactivated, parent };
  } finally {
    parent["bench-stop"] = await awaitParent("bench-stop", {}, waitMs);
  }
}

export async function runLatencyBench(deps: BenchDeps, params: Record<string, unknown>): Promise<Record<string, unknown>> {
  const { page, awaitParent, sleep } = deps;
  const identity = await page.identity();
  const settle = typeof params.settle_ms === "number" ? params.settle_ms : 500;
  await sleep(settle);

  const metrics: PageMetrics = page.latencyMetrics
    ? page.latencyMetrics()
    : (() => {
        const geom = page.geometry();
        const dpr = typeof geom.device_pixel_ratio === "number" ? geom.device_pixel_ratio : (typeof window !== "undefined" ? window.devicePixelRatio : 1);
        const font = typeof geom.font === "string" && geom.font ? geom.font : "14px monospace";
        const m = cellMetrics(font, () => 9);
        const w = typeof geom.canvas_width === "number" && geom.canvas_width > 0 ? geom.canvas_width / dpr : 675;
        const h = typeof geom.canvas_height === "number" && geom.canvas_height > 0 ? geom.canvas_height / dpr : 551;
        return {
          devicePixelRatio: dpr,
          canvasRect: { left: 0, top: 0, width: w, height: h },
          cellWidth: m.cellWidth,
          cellHeight: m.cellHeight,
          cols: Math.floor(w / m.cellWidth),
          rows: Math.floor(h / m.cellHeight),
        };
      })();

  const obs = observePage(metrics);
  if (!obs.ok) throw new Error(obs.error);

  const timeOrigin = typeof performance !== "undefined" ? performance.timeOrigin : 0;
  const keyObs = createKeyObserver(timeOrigin);
  const cleanup = page.listenKeydown?.((e) => keyObs.onKeydown(e));

  const client_w_css = typeof window !== "undefined" ? window.innerWidth : 1280;
  const client_h_css = typeof window !== "undefined" ? window.innerHeight : 720;
  const parent: Record<string, unknown> = {};
  const attempts: unknown[] = [];

  try {
    const readyDetail = {
      ...identity,
      page: obs.page,
      inner_width: client_w_css,
      inner_height: client_h_css,
      client_w_css,
      client_h_css,
    };
    await page.focusTerminal?.();
    parent["latency-ready"] = await awaitParent("latency-ready", readyDetail);

    const readyAck = parent["latency-ready"] as Record<string, unknown> | undefined;
    const transitions =
      typeof params.transitions === "number"
        ? params.transitions
        : typeof readyAck?.transitions === "number"
          ? (readyAck.transitions as number)
          : 3;

    for (let i = 0; i < transitions; i++) {
      await page.focusTerminal?.();
      const stepName = `latency-attempt-${String(i).padStart(3, "0")}`;
      const attemptDetail = {
        ...identity,
        attempt: i,
        page: obs.page,
      };
      parent[stepName] = await awaitParent(stepName, attemptDetail);
      const keys = keyObs.take();
      const payload = attemptPayload(i, obs.page, keys);
      attempts.push(payload);
    }
  } finally {
    cleanup?.();
    parent["latency-stop"] = await awaitParent("latency-stop", {
      ...identity,
      attempts,
    });
  }

  return {
    ...identity,
    page: obs.page,
    attempts,
    parent,
  };
}

/** `params.windows`: absent or ["visible","hidden"] (output load), or ["idle"]; true for idle. */
function benchWindows(windows: unknown): boolean {
  if (windows === undefined) return false;
  const list = Array.isArray(windows) ? windows.join(",") : null;
  if (list === "idle") return true;
  if (list === "visible,hidden") return false;
  throw new Error(`bench windows must be ["visible","hidden"] or ["idle"], got ${JSON.stringify(windows)}`);
}

/** How `domPage` reaches the window itself (injected so the unit tests need no Tauri). */
export interface DomPageDeps {
  /** Harness command `harness_window_visible`; answers the visibility the window reports back. */
  setWindowVisible?(visible: boolean): Promise<unknown>;
  /** Budget for the window to actually change visibility. */
  visibilityTimeoutMs?: number;
}

/** The composed App seen like a user: status diagnostics, the terminal canvas and this window. */
export function domPage(root: HTMLElement, deps: DomPageDeps = {}): BenchPage {
  const canvas = () => root.querySelector<HTMLCanvasElement>(".terminal canvas");
  const setWindowVisible =
    deps.setWindowVisible ??
    (() => Promise.reject(new Error("no window-visibility helper: domPage needs setWindowVisible")));
  const visibilityTimeoutMs = deps.visibilityTimeoutMs ?? 15000;
  const focusTerminal = async (): Promise<void> => {
    const target = await waitFor("terminal input target", () => root.querySelector<HTMLTextAreaElement>("textarea.ime-target"), 60000);
    target.focus();
    await waitFor("terminal focused", () => document.activeElement === target, 5000);
  };
  return {
    async identity() {
      const match = await waitFor("confirmed identity", () => {
        const title = root.querySelector<HTMLElement>(".status-bar [data-phase=live]")?.title ?? "";
        return /^pane (\S+) · geração (\S+) · boot (\S+)/.exec(title);
      }, 60000);
      await waitFor("terminal canvas", () => canvas());
      await focusTerminal();
      // Empty only if the status format changed; the parent rejects an empty pane.
      const [, pane_id = "", generation = "", boot_prefix = ""] = match;
      return { pane_id, generation, boot_prefix };
    },
    focusTerminal,
    // One grid canvas per mounted TerminalView: `canvas.extend` is the window-padding extension
    // layer of the same view (spec 028 r4c), never a second terminal.
    canvases: () => root.querySelectorAll(".terminal canvas:not(.extend)").length,
    geometry() {
      const c = canvas();
      return {
        canvas_width: c?.width ?? null,
        canvas_height: c?.height ?? null,
        css_width: c?.style.width ?? null,
        css_height: c?.style.height ?? null,
        device_pixel_ratio: window.devicePixelRatio,
        font: c?.getContext("2d")?.font ?? null,
        user_agent: navigator.userAgent,
      };
    },
    async setWindowHidden(hidden) {
      if (this.terminalHidden() === hidden) return;
      await setWindowVisible(!hidden);
      // The request is not the state: a compositor that ignores it must fail the run, never let a
      // visible window be measured as hidden.
      await waitFor(
        hidden ? "window hidden" : "window visible",
        () => this.terminalHidden() === hidden,
        visibilityTimeoutMs,
      );
    },
    terminalHidden: () => typeof document !== "undefined" && document.visibilityState === "hidden",
    editorOpen: () => root.querySelector(".cm-editor") !== null,
    latencyMetrics() {
      const c = canvas();
      if (!c) throw new Error("terminal canvas missing");
      const ctx = c.getContext("2d");
      const font = ctx?.font || "14px monospace";
      const m = cellMetrics(font, (f) => {
        if (ctx) ctx.font = f;
        return ctx?.measureText("M").width ?? 9;
      });
      const rect = c.getBoundingClientRect();
      const paneEl = root.querySelector<HTMLElement>("[data-pane][data-cells]");
      const cells = parseCells(paneEl?.dataset.cells);
      const cols = cells ? cells[2] : Math.floor(rect.width / m.cellWidth);
      const rows = cells ? cells[3] : Math.floor(rect.height / m.cellHeight);
      return {
        devicePixelRatio: window.devicePixelRatio,
        canvasRect: { left: rect.left, top: rect.top, width: rect.width, height: rect.height },
        cellWidth: m.cellWidth,
        cellHeight: m.cellHeight,
        cols,
        rows,
      };
    },
    listenKeydown(handler: (e: KeyboardEvent) => void) {
      const ime = root.querySelector<HTMLTextAreaElement>("textarea.ime-target");
      ime?.focus();
      window.addEventListener("keydown", handler, { capture: true });
      return () => window.removeEventListener("keydown", handler, { capture: true });
    },
  };
}
