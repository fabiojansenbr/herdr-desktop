// paste-selection phase of the spec 007 native flow (AC-007-02). Prepared module: e2e.ts does not
// call it until the root links it (.local/orchestration/paste-contract.md). Acts only through the
// real terminal target (`textarea.ime-target`) and reports raw observations; the parent
// (tests/fidelity-native/paste_flow.rs) presses the native chords on the private display, measures
// PTY bytes and reads the private clipboard. Nothing here reads a clipboard or fabricates a result.

import { waitFor } from "../../harness/dom";
import { parseCells, parseIdentity } from "./ssh-flow";

// Must equal paste_flow::STEPS and SELECTION_END_COL (asserted by paste-flow.test.ts).
export const PASTE_STEPS = ["paste-native", "selection-fixture", "selection-copy"] as const;
export const SELECTION_CELLS = { anchor: [0, 0], cursor: [1, 18] } as const;

export type AwaitParent = (step: string, detail: Record<string, unknown>) => Promise<Record<string, unknown>>;
type Metrics = { cellWidth: number; cellHeight: number };
type Cell = readonly [number, number];

/** Same rule as TerminalView.measure: ceil(width of "M"), ceil(font px × 1.3). */
/** Cells of a pane shown by AgentPanel (surface `.pane` or the pane list); null when not shown. */
export function paneCells(root: HTMLElement, paneId: string): [number, number, number, number] | null {
  return parseCells(root.querySelector<HTMLElement>(`[data-pane="${paneId}"][data-cells]`)?.dataset.cells);
}

export function cellMetrics(font: string, measure: (font: string) => number): Metrics {
  const px = /(\d+(?:\.\d+)?)px/.exec(font);
  if (!px) throw new Error(`terminal canvas font ${font} has no font size`);
  const cellWidth = Math.ceil(measure(font));
  if (!(cellWidth > 0)) throw new Error(`terminal canvas font ${font} gives no cell width`);
  return { cellWidth, cellHeight: Math.ceil(Number(px[1]) * 1.3) };
}

/** Client point at the center of pane-local cell (row, col); origin = pane cells [x, y]. */
export function cellCenter(canvas: { left: number; top: number }, m: Metrics, origin: Cell, row: number, col: number) {
  return { x: canvas.left + (origin[0] + col + 0.5) * m.cellWidth, y: canvas.top + (origin[1] + row + 0.5) * m.cellHeight };
}

export type Seen = { type: string; trusted: boolean; key: string; ctrl: boolean; shift: boolean; t: number };
/**
 * Records paste events and Ctrl+Shift+C/V keydowns. Attach to `window`: its capture listener runs
 * before any listener on `textarea.ime-target`, so the terminal's preventDefault/stop cannot hide the
 * trusted chord (GUI r4 saw 23 PTY bytes and `paste_keydowns: []`). `take(type)` removes only that type.
 */
export function record(scope: Pick<EventTarget, "addEventListener" | "removeEventListener">) {
  let events: Seen[] = [];
  const push = (e: Event) => {
    const k = e as KeyboardEvent;
    if (e.type === "keydown") {
      const key = (k.key ?? "").toLowerCase();
      if (!(k.ctrlKey && k.shiftKey && (key === "c" || key === "v"))) return;
    }
    events.push({ type: e.type, trusted: e.isTrusted, key: k.key ?? "", ctrl: !!k.ctrlKey, shift: !!k.shiftKey, t: performance.timeOrigin + performance.now() });
  };
  scope.addEventListener("paste", push, { capture: true });
  scope.addEventListener("keydown", push, { capture: true });
  return {
    take: (type: string) => {
      const taken = events.filter((e) => e.type === type);
      events = events.filter((e) => e.type !== type);
      return taken;
    },
    stop: () => {
      scope.removeEventListener("paste", push, { capture: true });
      scope.removeEventListener("keydown", push, { capture: true });
    },
  };
}

function confirmedIdentity(root: HTMLElement) {
  const live = root.querySelector<HTMLElement>(".status-bar [data-phase=live]");
  const id = parseIdentity(live?.title ?? "");
  const endpoint = Array.from(root.querySelectorAll<HTMLElement>(".status-bar .status-item")).map((i) => i.innerText)[1] ?? "";
  return id && endpoint ? { ...id, endpoint } : null;
}

const toolbarStatus = (root: HTMLElement) =>
  root.querySelector<HTMLElement>('[role=toolbar][aria-label="Ações do terminal"] [role=status]')?.innerText.trim() ?? "";
const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

function drag(target: HTMLElement, canvas: HTMLCanvasElement, m: Metrics, origin: Cell): void {
  const rect = canvas.getBoundingClientRect();
  const at = (cell: readonly number[]) => cellCenter(rect, m, origin, cell[0]!, cell[1]!);
  const send = (type: string, p: { x: number; y: number }) =>
    target.dispatchEvent(new PointerEvent(type, { bubbles: true, cancelable: true, pointerId: 7, button: 0, buttons: type === "pointerup" ? 0 : 1, shiftKey: true, clientX: p.x, clientY: p.y }));
  send("pointerdown", at(SELECTION_CELLS.anchor));
  send("pointermove", at(SELECTION_CELLS.cursor));
  send("pointerup", at(SELECTION_CELLS.cursor));
}

/** Runs the phase in the composed window; every missing control throws (reported as phase error). */
export async function runPasteSelection(root: HTMLElement, awaitParent: AwaitParent): Promise<Record<string, unknown>> {
  if (!root) throw new Error("paste-selection without root");
  const target = await waitFor("terminal input target", () => root.querySelector<HTMLTextAreaElement>("textarea.ime-target"));
  target.focus();
  await waitFor("terminal focused", () => document.activeElement === target);
  const before = await waitFor("confirmed identity", () => confirmedIdentity(root), 60000);
  const events = record(window);
  const paste = await awaitParent("paste-native", before);
  const paste_events = events.take("paste");
  const paste_keydowns = events.take("keydown");

  const fixture = await awaitParent("selection-fixture", before);
  await sleep(1000);
  const canvas = await waitFor("terminal canvas", () => root.querySelector<HTMLCanvasElement>(".terminal canvas"));
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("terminal canvas without 2d context");
  const metrics = cellMetrics(ctx.font, (font) => ((ctx.font = font), ctx.measureText("M").width));
  // Pane origin in surface cells from the agents panel (single pane: assumed equal to inner_rect).
  const origin = paneCells(root, before.pane_id);
  if (!origin) throw new Error(`pane ${before.pane_id} cells not shown`);
  target.focus();
  drag(target, canvas, metrics, [origin[0], origin[1]]);
  const status_before_copy = await waitFor("selection status", () => toolbarStatus(root).startsWith("Seleção:") && toolbarStatus(root));
  events.take("keydown");
  const copy = await awaitParent("selection-copy", before);
  const copy_keydowns = events.take("keydown");
  events.stop();
  const status_after_copy = await waitFor("copy confirmation", () => /copiada|Falha|\(/.test(toolbarStatus(root)) && toolbarStatus(root), 10000).catch(() => toolbarStatus(root));
  return {
    identity_before: before,
    identity_after: confirmedIdentity(root),
    paste_events,
    paste_keydowns,
    copy_keydowns,
    drag: SELECTION_CELLS,
    drag_trusted: false,
    metrics,
    pane_origin: origin,
    status_before_copy,
    status_after_copy,
    recorder_scope: "window capture",
    parent: { paste, fixture, copy },
  };
}
