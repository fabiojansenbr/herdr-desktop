// mouse-scroll-links phase of the spec 007 native flow (AC-007-02). Prepared module: e2e.ts does
// not call it until the root links it (.local/orchestration/mouse-contract.md). The page only
// computes output points of literal pane-local cells from the painted terminal geometry, records
// the pointer/wheel DOM events on the real terminal target and reads the link toolbar; the parent
// (tests/fidelity-native/mouse_flow.rs) moves the pointer on the private compositor, reads PTY
// bytes, engine scroll state and the private URI recorder. Nothing here fabricates a result.

import { waitFor } from "../../harness/dom";
import { terminalActionLog, type ActionLog, type ActionLogEntry } from "../../shell/actions";
import { terminalProbe, type TerminalProbe } from "../../terminal/probe";
import { cellCenter, cellMetrics, paneCells, type AwaitParent } from "./paste-flow";
import { parseIdentity } from "./ssh-flow";

// Must equal mouse_flow::STEPS and the *_CELL constants (asserted by mouse-flow.test.ts).
export const MOUSE_STEPS = ["alt-screen-mouse", "scrollback-wheel", "link-ctrl-click"] as const;
export const MOUSE_CELLS = { click: [2, 5], wheel: [4, 9], scroll: [3, 4], link: [0, 3] } as const;
/** Private headless output (sway.conf HEADLESS-1). */
export const OUTPUT = { width: 1280, height: 720 } as const;

type P = { x: number; y: number };

/** Output point of a client point; `swaymsg seat cursor set` needs 1 ≤ x,y inside the output. */
export function outputPoint(client: P, origin: P): P {
  const p = { x: client.x + origin.x, y: client.y + origin.y };
  if (!(p.x >= 1 && p.y >= 1 && p.x < OUTPUT.width && p.y < OUTPUT.height)) throw new Error(`point ${JSON.stringify(p)} outside the private output`);
  return p;
}

/** A client point (the declared coordinate_space); refused outside the client viewport. */
export function clientPoint(p: P, viewport: { innerWidth: number; innerHeight: number }): P {
  if (!(p.x >= 0 && p.y >= 0 && p.x < viewport.innerWidth && p.y < viewport.innerHeight)) throw new Error(`point ${JSON.stringify(p)} outside the client viewport`);
  return { x: p.x, y: p.y };
}

export type ElementAtPoint = { tag: string; classes: string[] } | null;

/** Returns tag and classes of document.elementFromPoint(p.x, p.y), or null. */
export function elementFromPoint(
  p: P,
  doc: { elementFromPoint?: (x: number, y: number) => Element | null } | null = typeof document !== "undefined" ? document : null,
): ElementAtPoint {
  if (!doc || typeof doc.elementFromPoint !== "function") return null;
  const el = doc.elementFromPoint(p.x, p.y);
  if (!el) return null;
  return {
    tag: el.tagName.toLowerCase(),
    classes: Array.from(el.classList ?? []),
  };
}

export type PointerSeen = { type: string; trusted: boolean; button: number; ctrl: boolean; deltaY: number; x: number; y: number };

/** Pointer and wheel events reaching the terminal target, trusted or not (never filtered). */
export function recordPointer(target: EventTarget) {
  const events: PointerSeen[] = [];
  const push = (e: Event) => {
    const m = e as Partial<PointerEvent & WheelEvent>;
    const wheel = e.type === "wheel";
    events.push({ type: e.type, trusted: e.isTrusted, button: wheel ? -1 : (m.button ?? -1), ctrl: !!m.ctrlKey, deltaY: wheel ? (m.deltaY ?? 0) : 0, x: m.clientX ?? 0, y: m.clientY ?? 0 });
  };
  for (const type of ["pointerdown", "pointerup", "wheel"]) target.addEventListener(type, push, { capture: true });
  return { take: () => events.splice(0) };
}

/** What the app's own action layer (src/shell/actions.ts observer → harness action log) saw for the
 *  link step: whether TerminalView requested the open-link action (and the lane it found), whether
 *  the bridge was called, whether it settled, the receipt's `sent` and the refusal (backend code, or
 *  a refusal before any call such as `no_identity`). `log: "unavailable"` = no harness log in this
 *  window (nothing is inferred). The last link request of the step is summarised. */
export function linkActionSummary(entries: ActionLogEntry[] | null) {
  if (entries === null) {
    return { log: "unavailable", link_action_requested: null, link_action_queue: null, link_action_called: null, link_action_settled: null, link_action_sent: null, link_action_error: null, other_kinds: [] as string[] };
  }
  const links = entries.filter((e) => e.kind === "link");
  const lastRequest = links.map((e, i) => (e.stage === "requested" ? i : -1)).filter((i) => i >= 0).at(-1);
  const scope = lastRequest === undefined ? links : links.slice(lastRequest);
  const request = scope.find((e) => e.stage === "requested");
  const settled = scope.find((e) => e.stage === "settled");
  const error = settled && settled.stage === "settled" && !settled.result.ok ? { code: settled.result.code, message: settled.result.message } : null;
  return {
    log: entries.length === 0 ? "empty" : "recorded",
    link_action_requested: request !== undefined,
    link_action_queue: request && request.stage === "requested" ? { waiting: request.waiting, running: request.running } : null,
    link_action_called: scope.some((e) => e.stage === "called"),
    link_action_settled: settled !== undefined,
    link_action_sent: settled && settled.stage === "settled" && settled.receipt ? settled.receipt.sent : null,
    link_action_error: error,
    other_kinds: [...new Set(entries.filter((e) => e.kind !== "link").map((e) => e.kind))],
  };
}

/** SurfaceState.error as App renders it (banner `[role=alert][data-error]`), or null. */
export function surfaceError(root: HTMLElement): { code: string; message: string } | null {
  const banner = root.querySelector<HTMLElement>("[role=alert][data-error]");
  return banner ? { code: banner.dataset.error ?? "", message: banner.innerText.trim() } : null;
}

function confirmedIdentity(root: HTMLElement) {
  const id = parseIdentity(root.querySelector<HTMLElement>(".status-bar [data-phase=live]")?.title ?? "");
  const endpoint = Array.from(root.querySelectorAll<HTMLElement>(".status-bar .status-item")).map((i) => i.innerText)[1] ?? "";
  return id && endpoint ? { ...id, endpoint } : null;
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/** Runs the phase in the composed window; every missing control throws (reported as phase error). */
export async function runMouseScrollLinks(
  root: HTMLElement,
  awaitParent: AwaitParent,
  log: Pick<ActionLog, "take"> | null = terminalActionLog(),
  settleMs = 5000,
  // Opt-in terminal probe (params.terminal_probe, as resize-dpi): exit of each pointerdown; null = not observable.
  probe: Pick<TerminalProbe, "takePointerRoutes"> | null = terminalProbe(),
): Promise<Record<string, unknown>> {
  if (!root) throw new Error("mouse-scroll-links without root");
  const target = await waitFor("terminal input target", () => root.querySelector<HTMLTextAreaElement>("textarea.ime-target"));
  target.focus();
  const before = await waitFor("confirmed identity", () => confirmedIdentity(root), 60000);
  const terminal = await waitFor("terminal container", () => root.querySelector<HTMLElement>(".terminal"));
  const canvas = await waitFor("terminal canvas", () => terminal.querySelector<HTMLCanvasElement>("canvas"));
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("terminal canvas without 2d context");
  const metrics = cellMetrics(ctx.font, (font) => ((ctx.font = font), ctx.measureText("M").width));
  // Pane origin in surface cells from the agents panel (single pane: assumed equal to inner_rect).
  const origin = paneCells(root, before.pane_id);
  if (!origin) throw new Error(`pane ${before.pane_id} cells not shown`);
  // Client (CSS px) points: the parent maps them through the observed sway tree + GTK header once.
  const at = (cell: readonly number[]) => clientPoint(cellCenter(canvas.getBoundingClientRect(), metrics, [origin[0], origin[1]], cell[0]!, cell[1]!), window);
  const requested_points = { click: at(MOUSE_CELLS.click), wheel: at(MOUSE_CELLS.wheel), scroll: at(MOUSE_CELLS.scroll), link: at(MOUSE_CELLS.link) };
  const doc = root.ownerDocument ?? (typeof document !== "undefined" ? document : null);
  const elements_from_point = {
    click: elementFromPoint(requested_points.click, doc),
    wheel: elementFromPoint(requested_points.wheel, doc),
    scroll: elementFromPoint(requested_points.scroll, doc),
    link: elementFromPoint(requested_points.link, doc),
  };
  const events = recordPointer(terminal);
  const routes = () => (probe ? probe.takePointerRoutes() : null);
  routes();
  const mouse = await awaitParent("alt-screen-mouse", { ...before, points: { click: requested_points.click, wheel: requested_points.wheel } });
  const mouse_events = events.take();
  const mouse_pointer_routes = routes();
  await sleep(500);
  const scroll = await awaitParent("scrollback-wheel", { ...before, points: { scroll: requested_points.scroll } });
  const scroll_events = events.take();
  const scroll_pointer_routes = routes();
  // Actions settled before the link step (e.g. the scroll) are not the link's.
  log?.take();
  const surface_error_before_link = surfaceError(root);
  const link = await awaitParent("link-ctrl-click", { ...before, points: { link: requested_points.link } });
  const link_events = events.take();
  const link_pointer_routes = routes();
  const parent_link_done_ms = performance.now();
  // Observation only (the parent already judged the recorder): waits, bounded, for a link action
  // to settle so "never requested", "queued", "called but pending" and "settled late" are told apart.
  const link_actions: ActionLogEntry[] | null = log ? log.take() : null;
  if (log && link_actions) {
    const settleBy = parent_link_done_ms + settleMs;
    while (!link_actions.some((e) => e.kind === "link" && e.stage === "settled") && performance.now() < settleBy) {
      await sleep(100);
      link_actions.push(...log.take());
    }
  }
  const link_uri = await waitFor("link toolbar", () => root.querySelector<HTMLInputElement>('[role=toolbar][aria-label="Ações do terminal"] input.uri')?.value || null, 10000).catch(() => null);
  const link_status = root.querySelector<HTMLElement>('[role=toolbar][aria-label="Ações do terminal"] [role=status]')?.innerText.trim() ?? "";
  return {
    identity_before: before,
    identity_after: confirmedIdentity(root),
    cells: MOUSE_CELLS,
    metrics,
    pane_origin: origin,
    requested_points,
    elements_from_point,
    element_from_point: elements_from_point,
    mouse_events,
    scroll_events,
    link_events,
    mouse_pointer_routes,
    scroll_pointer_routes,
    link_pointer_routes,
    link_uri,
    link_status,
    link_actions,
    link_action: linkActionSummary(link_actions),
    parent_link_done_ms,
    surface_error_before_link,
    surface_error_after_link: surfaceError(root),
    parent: { mouse, scroll, link },
  };
}
