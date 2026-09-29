// resize-dpi and a11y-navigation phases of the spec 007 native flow (AC-007-02). Prepared module:
// e2e.ts does not call it until the owner links it (.local/orchestration/view-contract.md). The
// parent (tests/fidelity-native/view_flow.rs) commands the PRIVATE sway output, presses native keys,
// reads `stty size` and grim PPM; the page only records what it actually sees (viewport, DPR,
// canvas backing/transform, probe counters, keydown/focus records, names, computed styles). No
// synthetic key/focus event is proof here, and no expected flag is ever written into the report.

import { tick } from "svelte";
import { setModalTraceSink } from "../../components/modal-focus";
import { waitFor } from "../../harness/dom";
import type { TerminalGeometry, TerminalProbe, TerminalProbeCounters } from "../../terminal/probe";
import { cellMetrics, type AwaitParent } from "./paste-flow";
import { parseIdentity } from "./ssh-flow";

// Must equal view_flow::STAGES, RESIZE_STEPS, A11Y_STEPS and the exit chord token (view-flow.test.ts).
export const VIEW_STAGES = [
  { name: "start", width: 1280, height: 720, scale: 1 },
  { name: "grow", width: 1600, height: 900, scale: 1 },
  { name: "scale2", width: 1600, height: 900, scale: 2 },
  { name: "restore", width: 1280, height: 720, scale: 1 },
] as const;
export const RESIZE_STEPS = ["resize-observe-start", "resize-apply-grow", "resize-observe-grow", "resize-apply-scale2", "resize-observe-scale2", "resize-apply-restore", "resize-observe-restore"] as const;
export const A11Y_STEPS = ["a11y-sweep-main", "a11y-open-dialog", "a11y-sweep-dialog", "a11y-close-dialog"] as const;
/** TerminalView isExitChord (Ctrl+Shift+F6), as a parent key token. */
export const EXIT_CHORD = "ctrl+shift+F6";

export type Identity = { pane_id: string; generation: string; boot_prefix: string; endpoint: string };
type Rect = { left: number; top: number; width: number; height: number };
export type StageView = {
  inner_width: number;
  inner_height: number;
  dpr: number;
  terminal_rect: { width: number; height: number };
  metrics: { cellWidth: number; cellHeight: number };
  canvas: { width: number; height: number; css_width: string; css_height: string; transform: number[]; rect: Rect };
};
export interface ViewDeps {
  awaitParent: AwaitParent;
  sleep(ms: number): Promise<void>;
  now(): number;
}

const POLL_MS = 50;

async function until<T>(deps: ViewDeps, what: string, probe: () => T | null | undefined | false, timeoutMs: number): Promise<T> {
  const deadline = deps.now() + timeoutMs;
  for (;;) {
    const v = probe();
    if (v) return v;
    if (deps.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await deps.sleep(POLL_MS);
  }
}

/** Actual status identity; the phases measure on Local only (precondition, never assumed). */
async function localIdentity(deps: ViewDeps, read: () => Identity | null, phase: string): Promise<Identity> {
  const id = await until(deps, "confirmed identity", read, 60000);
  if (!id.endpoint.endsWith(" · Local")) throw new Error(`precondition: ${phase} needs the Local host selected in the UI; status shows ${JSON.stringify(id)}`);
  return id;
}

// ---------------------------------------------------------------- resize-dpi

export interface ResizePage {
  identity(): Identity | null;
  view(): StageView;
}

/**
 * After a parent command: waits for an accepted Full frame AND painted rows beyond `before`
 * (none required for the start stage), then for view+counters unchanged during `quietMs`.
 */
async function settle(page: ResizePage, probe: ResizeProbe, deps: ViewDeps, stage: string, before: TerminalProbeCounters | null, quietMs = 400, timeoutMs = 20000) {
  const deadline = deps.now() + timeoutMs;
  let last = "";
  let since = deps.now();
  for (;;) {
    const counters = probe.snapshot();
    // Surface and pane inner_rect of the accepted frame metadata: the PTY has the pane, not the canvas.
    const geometry = probe.geometry();
    const view = { ...page.view(), surface: geometry.surface, panes: geometry.panes };
    const fresh = !before || (counters.full_frames > before.full_frames && counters.painted_rows > before.painted_rows);
    const ready = fresh && geometry.surface !== null && geometry.full_frames === counters.full_frames;
    const key = JSON.stringify([view, counters]);
    if (key !== last || !ready) [last, since] = [key, deps.now()];
    if (ready && deps.now() - since >= quietMs) return { view, counters };
    if (deps.now() > deadline) {
      if (fresh) throw new Error(`${stage}: no frame metadata (surface/pane inner_rect) received after the Full frame`);
      throw new Error(`${stage}: no settled accepted Full frame and paint after the command (full_frames ${before?.full_frames}→${counters.full_frames}, painted_rows ${before?.painted_rows}→${counters.painted_rows})`);
    }
    await deps.sleep(POLL_MS);
  }
}

type ResizeProbe = Pick<TerminalProbe, "snapshot" | "geometry">;
export type { TerminalGeometry };

export async function runResizeDpi(page: ResizePage, probe: ResizeProbe | null, deps: ViewDeps): Promise<Record<string, unknown>> {
  if (!probe) throw new Error("resize-dpi: terminal probe is not enabled for this window (params.terminal_probe)");
  await localIdentity(deps, () => page.identity(), "resize-dpi");
  const stages: Record<string, unknown>[] = [];
  const parent: Record<string, unknown> = {};
  for (const stage of VIEW_STAGES) {
    let before: TerminalProbeCounters | null = null;
    if (stage.name !== "start") {
      before = probe.snapshot();
      const id = await localIdentity(deps, () => page.identity(), "resize-dpi");
      parent[`resize-apply-${stage.name}`] = await deps.awaitParent(`resize-apply-${stage.name}`, { ...id, stage: stage.name, view: page.view(), probe: before });
    }
    const { view, counters } = await settle(page, probe, deps, stage.name, before);
    const identity = page.identity();
    if (!identity) throw new Error(`${stage.name}: identity not shown after settle`);
    const step = `resize-observe-${stage.name}`;
    parent[step] = await deps.awaitParent(step, { ...identity, stage: stage.name, view });
    stages.push({ stage: stage.name, identity, ...view, probe: { painted_rows: counters.painted_rows, full_frames: counters.full_frames }, probe_counters: counters });
  }
  return { stages, identity_after: page.identity(), parent };
}

function statusIdentity(root: HTMLElement): Identity | null {
  const id = parseIdentity(root.querySelector<HTMLElement>(".status-bar [data-phase=live]")?.title ?? "");
  const endpoint = Array.from(root.querySelectorAll<HTMLElement>(".status-bar .status-item")).map((i) => i.innerText.trim())[1] ?? "";
  return id && endpoint ? { ...id, endpoint } : null;
}

/** Composed window: `.terminal` container, its canvas and the real 2d context. */
export function domResizePage(root: HTMLElement): ResizePage {
  return {
    identity: () => statusIdentity(root),
    view: () => {
      const terminal = root.querySelector<HTMLElement>(".terminal");
      const canvas = terminal?.querySelector<HTMLCanvasElement>("canvas");
      const ctx = canvas?.getContext("2d");
      if (!terminal || !canvas || !ctx) throw new Error("resize-dpi: terminal canvas with 2d context not found");
      const t = ctx.getTransform();
      const r = canvas.getBoundingClientRect();
      const tr = terminal.getBoundingClientRect();
      return {
        inner_width: window.innerWidth,
        inner_height: window.innerHeight,
        dpr: window.devicePixelRatio,
        terminal_rect: { width: tr.width, height: tr.height },
        metrics: cellMetrics(ctx.font, (font) => ((ctx.font = font), ctx.measureText("M").width)),
        canvas: { width: canvas.width, height: canvas.height, css_width: canvas.style.width, css_height: canvas.style.height, transform: [t.a, t.b, t.c, t.d, t.e, t.f], rect: { left: r.left, top: r.top, width: r.width, height: r.height } },
      };
    },
  };
}

// ---------------------------------------------------------------- a11y-navigation

export type Style = { outline_style: string; outline_width: string; outline_color: string; box_shadow: string; border_color: string; background_color: string };
/** `focus`: activeElement matches `:focus`; `document_has_focus`: document.hasFocus() (null when absent). */
export type ActiveElementSample = { tag: string; id: string; classes: string[]; focus_visible: boolean; focus: boolean; document_has_focus: boolean | null };
/** `classes`/`container_style`/`container_baseline`: see {@link carrierMarks}. */
export type Control = { id: string; tag: string; role: string; name: string; name_source: string; disabled: boolean; tabindex: number; focus_visible: boolean; style: Style; baseline: Style | null; active_element?: ActiveElementSample | null; classes?: string[]; container_style?: Style | null; container_baseline?: Style | null };
export type KeySeen = { key: string; shift: boolean; ctrl: boolean; trusted: boolean };
/** `active`: the active element sampled at the keydown (observation only; absent in keys_seen). */
export type SweepEvent = ({ kind: "key"; active?: ActiveElementSample | null } & KeySeen) | { kind: "focus"; control: Control };
/** One entry of the interleaved key/focus order of a sweep (observation only, r11). */
export type SweepOrderEntry =
  | { kind: "key"; index: number; token: string; trusted: boolean; active: ActiveElementSample | null }
  | { kind: "focus"; id: string; tag: string; name: string; keys_before: number; previous_key: string | null; active_element: ActiveElementSample | null };
export type StateSeen = { status: string; own_text: string; carrier_text: string };
/** Element carrying the open connection dialog: implicit/explicit role, name, `:modal` match. */
export type ModalSeen = { tag: string; role: string; name: string; name_source: string; modal: boolean };

export function sampleActiveElement(): ActiveElementSample | null {
  if (typeof document === "undefined" || !document.activeElement) return null;
  const el = document.activeElement;
  if (typeof HTMLElement !== "undefined" && !(el instanceof HTMLElement)) return null;
  if (!el || typeof el !== "object" || !("tagName" in el)) return null;
  const matches = (selector: string) => {
    try {
      return typeof (el as { matches?: (s: string) => boolean }).matches === "function" ? (el as { matches: (s: string) => boolean }).matches(selector) : false;
    } catch {
      return false;
    }
  };
  const focusVisible = matches(":focus-visible");
  const hasFocus = (document as { hasFocus?: () => boolean }).hasFocus;
  const documentHasFocus = typeof hasFocus === "function" ? hasFocus.call(document) : null;
  const idVal = "id" in el && typeof el.id === "string" ? el.id : "";
  const classListVal = "classList" in el && el.classList ? Array.from(el.classList as Iterable<string>) : [];
  return {
    tag: typeof el.tagName === "string" ? el.tagName.toLowerCase() : "",
    id: idVal,
    classes: classListVal,
    focus_visible: focusVisible,
    focus: matches(":focus"),
    document_has_focus: documentHasFocus,
  };
}

/** Event target as recorded: the window itself, an element (tag/id/classes) or none. */
export type TargetRef = "window" | { tag: string; id: string; classes: string[] } | null;
/**
 * One focus sample around a traced key: `at` = keydown | microtask | frame | 100ms | window-blur |
 * window-focus | focusout | focusin (document, with `target`/`related_target`) | a mark (product
 * close steps, e.g. `closer-focus:true`).
 */
export type FocusTraceEntry = { at: string; t: number; active: ActiveElementSample | null; target?: TargetRef; related_target?: TargetRef };
type Listenable = Pick<EventTarget, "addEventListener" | "removeEventListener">;
export interface TraceEnv {
  doc: Listenable;
  win: Listenable;
  sample(): ActiveElementSample | null;
  microtask(cb: () => void): void;
  frame(cb: () => void): void;
  later(ms: number, cb: () => void): void;
  now(): number;
  /** Target description; default: the window or nothing. */
  describe?(target: EventTarget | null): TargetRef;
}

/** Element target description of the live document (tag/id/classes); the window is "window". */
export function describeTarget(target: EventTarget | null): TargetRef {
  if (!target) return null;
  if (typeof window !== "undefined" && target === window) return "window";
  const el = target as { tagName?: unknown; id?: unknown; classList?: Iterable<string> };
  if (typeof el.tagName !== "string") return null;
  return { tag: el.tagName.toLowerCase(), id: typeof el.id === "string" ? el.id : "", classes: el.classList ? Array.from(el.classList) : [] };
}

/**
 * Evidence for focus after a native key (a11y-close-dialog): samples activeElement, :focus and
 * document.hasFocus() in the capture keydown (before the product handles it), after a microtask
 * (Svelte flush), after the next frame and after 100 ms; window blur/focus are recorded when they
 * happen. Only observes: never focuses anything.
 */
export function traceFocusAfterKey(env: TraceEnv, key: string): { take(): FocusTraceEntry[]; stop(): void; mark(at: string): void } {
  let entries: FocusTraceEntry[] = [];
  let live = true;
  const describe = env.describe ?? ((t: EventTarget | null) => (t !== null && t === (env.win as unknown) ? "window" : null));
  const push = (at: string, extra: Partial<FocusTraceEntry> = {}) => {
    if (live) entries.push({ at, t: env.now(), active: env.sample(), ...extra });
  };
  const onKey = (e: Event) => {
    if ((e as KeyboardEvent).key !== key) return;
    push("keydown");
    env.microtask(() => push("microtask"));
    env.frame(() => push("frame"));
    env.later(100, () => push("100ms"));
  };
  const onBlur = (e: Event) => push("window-blur", { target: describe(e.target ?? null) });
  const onFocus = () => push("window-focus");
  const onFocusChange = (e: Event) => push(e.type, { target: describe(e.target ?? null), related_target: describe((e as FocusEvent).relatedTarget ?? null) });
  env.doc.addEventListener("keydown", onKey, { capture: true });
  env.doc.addEventListener("focusout", onFocusChange, { capture: true });
  env.doc.addEventListener("focusin", onFocusChange, { capture: true });
  env.win.addEventListener("blur", onBlur);
  env.win.addEventListener("focus", onFocus);
  return {
    take: () => entries.splice(0),
    mark: (at) => push(at),
    stop: () => {
      live = false;
      env.doc.removeEventListener("keydown", onKey, { capture: true });
      env.doc.removeEventListener("focusout", onFocusChange, { capture: true });
      env.doc.removeEventListener("focusin", onFocusChange, { capture: true });
      env.win.removeEventListener("blur", onBlur);
      env.win.removeEventListener("focus", onFocus);
    },
  };
}

export type CompositionSeen = { type: string; data: string; t: number; target: TargetRef };
/** One exit chord keydown (Ctrl+Shift+F6) as the page saw it. */
export type ChordEntry = {
  t: number;
  /** activeElement at the capture keydown (before any product handler). */
  active: ActiveElementSample | null;
  target: TargetRef;
  is_composing: boolean;
  default_prevented_capture: boolean;
  /** At the window bubble listener (after the product handlers); null when it never ran. */
  default_prevented_bubble: boolean | null;
  /** In the next task after dispatch; null when not sampled (trace stopped). */
  default_prevented_after: boolean | null;
  active_after: ActiveElementSample | null;
  /** Last terminal focus-in time; null when none since the trace started. */
  terminal_focus_in_t: number | null;
  /** composition events since that focus-in (nothing before the first one). */
  compositions_since_focus: CompositionSeen[];
};
export interface ChordEnv {
  doc: Listenable;
  win: Listenable;
  sample(): ActiveElementSample | null;
  describe(target: EventTarget | null): TargetRef;
  isTerminal(target: EventTarget | null): boolean;
  later(ms: number, cb: () => void): void;
  now(): number;
}

const MAX_COMPOSITIONS = 50;

/**
 * Evidence for the exit chord (GUI r8: a 2nd chord after keyboard re-entry did not leave the
 * terminal): per chord keydown, the active element, isComposing, defaultPrevented at capture, at
 * the window bubble and after dispatch, and the composition events since the terminal focus-in.
 * Only observes: never focuses or prevents anything.
 */
export function traceChords(env: ChordEnv): { take(): ChordEntry[]; stop(): void } {
  let entries: ChordEntry[] = [];
  let live = true;
  let focusIn: number | null = null;
  let compositions: CompositionSeen[] = [];
  const pending = new WeakMap<Event, ChordEntry>();
  const isChord = (e: KeyboardEvent) => e.key === "F6" && e.ctrlKey === true && e.shiftKey === true && !e.altKey && !e.metaKey;
  const onKey = (ev: Event) => {
    const e = ev as KeyboardEvent;
    if (!live || !isChord(e)) return;
    const entry: ChordEntry = {
      t: env.now(),
      active: env.sample(),
      target: env.describe(e.target ?? null),
      is_composing: e.isComposing === true,
      default_prevented_capture: e.defaultPrevented === true,
      default_prevented_bubble: null,
      default_prevented_after: null,
      active_after: null,
      terminal_focus_in_t: focusIn,
      compositions_since_focus: [...compositions],
    };
    entries.push(entry);
    pending.set(ev, entry);
    env.later(0, () => {
      if (!live) return;
      entry.default_prevented_after = e.defaultPrevented === true;
      entry.active_after = env.sample();
    });
  };
  const onBubble = (ev: Event) => {
    const entry = pending.get(ev);
    if (live && entry) entry.default_prevented_bubble = ev.defaultPrevented === true;
  };
  const onFocusIn = (e: Event) => {
    if (!live || !env.isTerminal(e.target ?? null)) return;
    focusIn = env.now();
    compositions = [];
  };
  const onComposition = (e: Event) => {
    if (!live || focusIn === null || compositions.length >= MAX_COMPOSITIONS) return;
    compositions.push({ type: e.type, data: (e as CompositionEvent).data ?? "", t: env.now(), target: env.describe(e.target ?? null) });
  };
  const COMPOSITION = ["compositionstart", "compositionupdate", "compositionend"];
  env.doc.addEventListener("keydown", onKey, { capture: true });
  env.doc.addEventListener("focusin", onFocusIn, { capture: true });
  for (const type of COMPOSITION) env.doc.addEventListener(type, onComposition, { capture: true });
  env.win.addEventListener("keydown", onBubble);
  return {
    take: () => entries.splice(0),
    stop: () => {
      live = false;
      env.doc.removeEventListener("keydown", onKey, { capture: true });
      env.doc.removeEventListener("focusin", onFocusIn, { capture: true });
      for (const type of COMPOSITION) env.doc.removeEventListener(type, onComposition, { capture: true });
      env.win.removeEventListener("keydown", onBubble);
    },
  };
}

export interface A11yPage {
  identity(): Identity | null;
  /** Setup, not proof: programmatic focus of the terminal input target. */
  focusTerminal(): boolean;
  /** Tabbables after the terminal exit marker split as [`splitAfterMarker`]; null if absent. */
  mainStops(): MarkerSplit<Control> | null;
  /** Ordered tabbables inside the open dialog; null if absent. */
  expected(scope: "dialog"): Control[] | null;
  /** Records every keydown and focusin (trusted or not) from now on. */
  record(): { take(): SweepEvent[] };
  /** Setup, not proof: makes the real dialog opener focused (after the panel mounted); exact actions done. */
  openerSetup(): Promise<string[]>;
  active(): Control | null;
  dialogOpen(): boolean;
  /** The open dialog's modal element as observed; null when no dialog is shown. */
  modal(): ModalSeen | null;
  /** Cleanup only, after a native Escape that did not close; never part of the proof. */
  cancelDialog(): void;
  states(): StateSeen[];
  /** Focus trace around the native Escape of a11y-close-dialog (observation only). */
  traceClose(): { take(): FocusTraceEntry[]; stop(): void };
  /** Exit chord trace of the main sweep (observation only). */
  traceChords(): { take(): ChordEntry[]; stop(): void };
}

/** Raw keys; focus after the exit chord (before the next Tab) is `exit_focus`, not a sweep stop. */
export function splitSweep(events: SweepEvent[]): { keys_seen: KeySeen[]; focus: Control[]; exit_focus: Control[] } {
  const out = { keys_seen: [] as KeySeen[], focus: [] as Control[], exit_focus: [] as Control[] };
  let exiting = false;
  for (const e of events) {
    if (e.kind === "focus") (exiting ? out.exit_focus : out.focus).push(e.control);
    else {
      const { kind: _kind, active: _active, ...k } = e;
      out.keys_seen.push(k);
      if (k.key === "F6") exiting = true;
      if (k.key === "Tab") exiting = false;
    }
  }
  return out;
}

/** Parent token of a recorded keydown (`Tab`, `shift+Tab`, `ctrl+shift+F6`, …). */
function keyToken(k: KeySeen): string {
  return `${k.ctrl ? "ctrl+" : ""}${k.shift ? "shift+" : ""}${k.key}`;
}

/**
 * Interleaved order of the recorded sweep (GUI r8–r10 lost one Tab, not attributable from the
 * separate keys_seen/focus lists): each non-modifier keydown with its index and the active element
 * at the keydown, and each focusin with the number of non-modifier keys seen before it and the key
 * right before it. Modifier keydowns are left out. Observation only; the evaluator does not read it.
 */
export function interleaveSweep(events: SweepEvent[]): SweepOrderEntry[] {
  const out: SweepOrderEntry[] = [];
  let index = 0;
  let previous: string | null = null;
  for (const e of events) {
    if (e.kind === "key") {
      if (MODIFIERS.includes(e.key)) continue;
      previous = keyToken(e);
      out.push({ kind: "key", index: index++, token: previous, trusted: e.trusted, active: e.active ?? null });
    } else {
      const c = e.control;
      out.push({ kind: "focus", id: c.id, tag: c.tag, name: c.name, keys_before: index, previous_key: previous, active_element: c.active_element ?? null });
    }
  }
  return out;
}

/** The terminal IME target keeps Tab/Shift+Tab for the PTY; only the exit chord leaves it. */
export function retainsTab(c: Pick<Control, "tag" | "classes">): boolean {
  return c.tag === "textarea" && (c.classes ?? []).includes("ime-target");
}

/**
 * Main sweep plan from the terminal (Planner decision r10): every control is proven once and the
 * terminal is left with the exit chord once. The lead chord (setup) focuses the exit marker placed
 * right after the terminal; one Tab per stop reaches each control in the order after the marker,
 * ending on the terminal; the chord leaves it for its marker again and Shift+Tab returns into the
 * terminal (listed again as the last expected stop). No Tab wraps through the end of the document
 * after the terminal (re-entry is host behaviour, GUI r9). Throws unless the terminal is the only
 * and last stop that keeps Tab, among at least two stops.
 *
 * Planner decision r12 (GUI r11): the stops cross the document wrap between the last control
 * following the marker and the first preceding one, and WebKitGTK spends two Tabs there (the first
 * reaches `body` without focusin). The plan declares `wrap_after` (index in `expected` of the last
 * following stop) with the id split it derives from, and carries exactly one extra Tab for it.
 * Throws when nothing follows the marker (no wrap to declare).
 */
export function mainSweepPlan<T extends { id: string }>(
  stops: MarkerSplit<T>,
  keepsTab: (c: T) => boolean,
): { expected: T[]; keys: string[]; wrap_after: number; split: MarkerSplit<string> } {
  const order = [...stops.following, ...stops.preceding];
  if (order.length < 2) throw new Error(`a11y-sweep-main: need at least two stops after the exit marker, got ${order.length}`);
  const last = order[order.length - 1]!;
  if (!keepsTab(last)) throw new Error("a11y-sweep-main: the terminal must be the last stop after the exit marker");
  if (order.slice(0, -1).some(keepsTab)) throw new Error("a11y-sweep-main: the terminal must be the only stop that keeps Tab");
  if (stops.following.length === 0) throw new Error("a11y-sweep-main: no control follows the exit marker (no document wrap to declare)");
  return {
    expected: [...order, last],
    keys: [EXIT_CHORD, ...order.map(() => "Tab"), "Tab", EXIT_CHORD, "shift+Tab"],
    wrap_after: stops.following.length - 1,
    split: { following: stops.following.map((c) => c.id), preceding: stops.preceding.map((c) => c.id) },
  };
}

const MODIFIERS = ["Shift", "Control", "Alt", "Meta"];

/** Waits until the page saw `count` non-modifier keydowns, then a quiet period for late focusin. */
async function collect(rec: { take(): SweepEvent[] }, count: number, deps: ViewDeps, what: string): Promise<SweepEvent[]> {
  const events: SweepEvent[] = [];
  const keys = () => events.filter((e) => e.kind === "key" && !MODIFIERS.includes(e.key)).length;
  await until(deps, `${what}: ${count} keydowns`, () => (events.push(...rec.take()), keys() >= count), 10000).catch(() => undefined);
  await deps.sleep(300);
  events.push(...rec.take());
  return events;
}

export async function runA11yNavigation(page: A11yPage, deps: ViewDeps): Promise<Record<string, unknown>> {
  const id = await localIdentity(deps, () => page.identity(), "a11y-navigation");
  const setup: string[] = [];
  // Installed before the setup focus so the terminal focus-in and its compositions are seen.
  const chords = page.traceChords();
  if (!page.focusTerminal()) throw new Error("a11y-navigation: terminal input target not focusable");
  setup.push("focus textarea.ime-target (programmatic, setup)");
  const rec = page.record();
  const sweep = async (step: string, scope: "main" | "dialog") => {
    const identity = page.identity();
    const stops = scope === "main" ? page.mainStops() : page.expected(scope);
    if (!identity || !stops) throw new Error(`${step}: ${identity ? `no ${scope} controls` : "identity not shown"}`);
    rec.take();
    // Main starts on the terminal (exit chord plan, declared wrap r12); the modal keeps plain Tab order.
    const plan = Array.isArray(stops) ? { expected: stops, keys: [...stops.map(() => "Tab"), "shift+Tab"] } : mainSweepPlan(stops, retainsTab);
    const { expected, keys } = plan;
    const wrap = "wrap_after" in plan ? { wrap_after: plan.wrap_after, split: plan.split } : {};
    const answer = await deps.awaitParent(step, { ...identity, expected, keys, ...wrap });
    const events = await collect(rec, keys.length, deps, step);
    // Main only: interleaved key/focus order (observation r11).
    const interleaved = scope === "main" ? { order: interleaveSweep(events) } : {};
    return { report: { identity, expected, ...wrap, ...splitSweep(events), ...interleaved }, answer };
  };
  const mainSweep = await sweep(A11Y_STEPS[0], "main");
  const main = { ...mainSweep, report: { ...mainSweep.report, chord_trace: chords.take() } };
  chords.stop();
  setup.push(...(await page.openerSetup()));
  const opener = page.active();
  rec.take();
  const open = await deps.awaitParent(A11Y_STEPS[1], { ...(page.identity() ?? id), opener, keys: ["Return"] });
  const openEvents = await collect(rec, 1, deps, A11Y_STEPS[1]);
  await until(deps, "connection dialog after native Return", () => page.dialogOpen(), 10000).catch(() => {
    throw new Error(`a11y-open-dialog: dialog not open after native Return; focus ${JSON.stringify(page.active())}`);
  });
  // Product initial focus (never focused by the harness) and the modal carrying it.
  const dialog_start_focus = page.active();
  const dialog_modal = page.modal();
  const dialog = await sweep(A11Y_STEPS[2], "dialog");
  // One native Escape closes the modal; closure, focus and identity are read after it.
  rec.take();
  const trace = page.traceClose();
  const close = await deps.awaitParent(A11Y_STEPS[3], { ...(page.identity() ?? id), opener, keys: ["Escape"] });
  const closeEvents = await collect(rec, 1, deps, A11Y_STEPS[3]);
  const closed = await until(deps, "dialog closed after native Escape", () => !page.dialogOpen(), 2000).then(() => true, () => false);
  const focus_after = page.active();
  trace.stop();
  const dialog_close = { identity: page.identity(), keys_seen: splitSweep(closeEvents).keys_seen, closed, focus_after, focus_trace: trace.take() };
  if (!closed) {
    page.cancelDialog();
    setup.push("click Cancelar (cleanup after Escape did not close; not measured)");
  }
  return {
    main: main.report,
    dialog_opener: opener,
    dialog_open_keys: splitSweep(openEvents).keys_seen,
    dialog_start_focus,
    dialog_modal,
    dialog: dialog.report,
    dialog_close,
    states: page.states(),
    setup,
    identity_after: page.identity(),
    parent: { main: main.answer, open, dialog: dialog.answer, close },
  };
}

/** Candidates in tree order; `after` restricts to tabindex-0 controls following a tabindex=-1 start. */
export function tabOrder<T extends { tabIndex: number; disabled: boolean; hidden: boolean; inert: boolean }>(els: T[], after?: (e: T) => boolean): T[] {
  const ok = els.filter((e) => e.tabIndex >= 0 && !e.disabled && !e.hidden && !e.inert);
  if (after) return ok.filter((e) => e.tabIndex === 0 && after(e));
  const positive = ok.filter((e) => e.tabIndex > 0).sort((a, b) => a.tabIndex - b.tabIndex);
  return [...positive, ...ok.filter((e) => e.tabIndex === 0)];
}

export type MarkerSplit<T> = { following: T[]; preceding: T[] };

/**
 * Tabbable controls around a marker, split at the document wrap: `following` = tabindex-0
 * controls after the marker in tree order; `preceding` = positive tabindex ascending, then
 * tabindex-0 controls before the marker (reached after wrapping). Skips disabled, negative
 * tabindex, hidden and inert controls.
 */
export function splitAfterMarker<T extends { tabIndex: number; disabled: boolean; hidden: boolean; inert: boolean }>(
  els: T[],
  following: (e: T) => boolean,
): MarkerSplit<T> {
  const ok = els.filter((e) => e.tabIndex >= 0 && !e.disabled && !e.hidden && !e.inert);
  const zero = ok.filter((e) => e.tabIndex === 0);
  const positive = ok.filter((e) => e.tabIndex > 0).sort((a, b) => a.tabIndex - b.tabIndex);
  return { following: zero.filter(following), preceding: [...positive, ...zero.filter((e) => !following(e))] };
}

/**
 * Expected tabbable controls in DOM order after a marker:
 * controls following the marker in tree order first, then preceding controls wrapping around.
 * Skips disabled, negative tabindex, hidden and inert controls.
 */
export function orderAfterMarker<T extends { tabIndex: number; disabled: boolean; hidden: boolean; inert: boolean }>(
  els: T[],
  following: (e: T) => boolean,
): T[] {
  const { following: after, preceding } = splitAfterMarker(els, following);
  return [...after, ...preceding];
}


export interface NameNode {
  nodeType: number;
  textContent: string | null;
  childNodes: ArrayLike<NameNode>;
  tagName?: string;
  getAttribute?(name: string): string | null;
  labels?: ArrayLike<NameNode> | null;
}

const clean = (s: string) => s.replace(/\s+/g, " ").trim();
function textOf(n: NameNode): string {
  if (n.nodeType === 3) return n.textContent ?? "";
  if (n.nodeType !== 1 || n.getAttribute?.("aria-hidden") === "true") return "";
  return Array.from(n.childNodes, textOf).join("");
}

/** aria-labelledby → aria-label → label → own text (aria-hidden excluded) → title. */
export function accessibleName(el: NameNode, byId: (id: string) => NameNode | null): { name: string; source: string } {
  const attr = (k: string) => clean(el.getAttribute?.(k) ?? "");
  const ids = attr("aria-labelledby").split(" ").filter(Boolean);
  const candidates: [string, string][] = [
    ["aria-labelledby", clean(ids.map((i) => { const n = byId(i); return n ? textOf(n) : ""; }).join(" "))],
    ["aria-label", attr("aria-label")],
    ["label", clean(Array.from(el.labels ?? [], textOf).join(" "))],
    ["text", clean(textOf(el))],
    ["title", attr("title")],
  ];
  const hit = candidates.find(([, name]) => name !== "");
  return hit ? { name: hit[1], source: hit[0] } : { name: "", source: "none" };
}

/** `ancestors` nearest first; the carrier is the first whose text contains the status token. */
export function stateRecord(status: string, ownText: string, ancestors: string[]): StateSeen {
  const token = status.toLowerCase();
  const carrier = token ? ancestors.find((t) => t.toLowerCase().includes(token)) : undefined;
  return { status, own_text: clean(ownText), carrier_text: clean(carrier ?? "") };
}

const STYLE_KEYS = { outline_style: "outline-style", outline_width: "outline-width", outline_color: "outline-color", box_shadow: "box-shadow", border_color: "border-color", background_color: "background-color" } as const;
function styleOf(el: Element): Style {
  const cs = getComputedStyle(el);
  return Object.fromEntries(Object.entries(STYLE_KEYS).map(([k, css]) => [k, cs.getPropertyValue(css)])) as Style;
}

/** The IME textarea carries no ring itself; its `.terminal` container does (bindFocusRing). */
function containerStyle(box: Element): Style {
  return { ...styleOf(box), outline_offset: getComputedStyle(box).getPropertyValue("outline-offset"), classes: Array.from(box.classList).join(" ") } as Style;
}

export type CarrierNode = { classList: Iterable<string>; matches(selector: string): boolean; closest(selector: string): object | null };

/**
 * Control classes (the parent identifies `textarea.ime-target` by them) and, for the IME target
 * only, the style of its `.terminal` container: `baseline` records it once (expected pass, same
 * moment as the control baseline) and later samples keep that first baseline; never invented.
 */
export function carrierMarks(el: CarrierNode, baseline: boolean, store: WeakMap<object, Style>, style: (box: object) => Style): { classes: string[]; container_style: Style | null; container_baseline: Style | null } {
  const classes = Array.from(el.classList);
  const box = el.matches(".ime-target") ? el.closest(".terminal") : null;
  if (!box) return { classes, container_style: null, container_baseline: null };
  const current = style(box);
  if (baseline && !store.has(el)) store.set(el, current);
  return { classes, container_style: current, container_baseline: store.get(el) ?? null };
}

const TABBABLE = 'a[href], button, input:not([type="hidden"]), select, textarea, summary, iframe, [tabindex], [contenteditable]:not([contenteditable="false"])';

/** Composed window adapter. Opener path of the real UI: activity "Conexões remotas" (HostsPanel) → "Nova conexão SSH". */
export function domA11yPage(root: HTMLElement): A11yPage {
  const ids = new WeakMap<Element, string>();
  const baselines = new WeakMap<Element, Style>();
  const containerBaselines = new WeakMap<object, Style>();
  let next = 0;
  const idOf = (el: Element) => ids.get(el) ?? (ids.set(el, `c${++next}`), ids.get(el)!);
  const describe = (el: HTMLElement, baseline?: boolean): Control => {
    if (baseline) baselines.set(el, styleOf(el));
    const { name, source } = accessibleName(el as unknown as NameNode, (i) => document.getElementById(i) as unknown as NameNode | null);
    let focusVisible = false;
    try {
      focusVisible = el.matches(":focus-visible");
    } catch {
      focusVisible = false;
    }
    return { id: idOf(el), tag: el.tagName.toLowerCase(), role: el.getAttribute("role") ?? "", name, name_source: source, disabled: (el as HTMLButtonElement).disabled === true, tabindex: el.tabIndex, focus_visible: focusVisible, style: styleOf(el), baseline: baselines.get(el) ?? null, active_element: sampleActiveElement(), ...carrierMarks(el, baseline === true, containerBaselines, (box) => containerStyle(box as Element)) };
  };
  const candidates = (scope: ParentNode) =>
    Array.from(scope.querySelectorAll<HTMLElement>(TABBABLE)).map((el) => ({ el, tabIndex: el.tabIndex, disabled: (el as HTMLButtonElement).disabled === true, hidden: el.getClientRects().length === 0 || getComputedStyle(el).visibility === "hidden", inert: el.closest("[inert]") !== null }));
  const dialog = () => document.querySelector<HTMLElement>("form.dialog");
  return {
    identity: () => statusIdentity(root),
    focusTerminal: () => {
      const t = root.querySelector<HTMLTextAreaElement>("textarea.ime-target");
      t?.focus();
      return !!t && document.activeElement === t;
    },
    expected: () => {
      const d = dialog();
      return d ? tabOrder(candidates(d)).map((c) => describe(c.el, true)) : null;
    },
    mainStops: () => {
      const marker = root.querySelector(".exit-marker");
      if (!marker) return null;
      const after = (c: { el: HTMLElement }) => (marker.compareDocumentPosition(c.el) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0;
      const { following, preceding } = splitAfterMarker(candidates(document), after);
      return { following: following.map((c) => describe(c.el, true)), preceding: preceding.map((c) => describe(c.el, true)) };
    },
    record: () => {
      let events: SweepEvent[] = [];
      const key = (e: KeyboardEvent) => events.push({ kind: "key", key: e.key, shift: e.shiftKey, ctrl: e.ctrlKey, trusted: e.isTrusted, active: sampleActiveElement() });
      const focus = (e: FocusEvent) => {
        if (e.target instanceof HTMLElement) {
          const control = describe(e.target);
          control.active_element = sampleActiveElement();
          events.push({ kind: "focus", control });
        }
      };
      document.addEventListener("keydown", key, { capture: true });
      document.addEventListener("focusin", focus, { capture: true });
      return { take: () => events.splice(0) };
    },
    openerSetup: async () => {
      const done: string[] = [];
      const activity = root.querySelector<HTMLButtonElement>('button[aria-label="Conexões remotas"]');
      if (!activity) throw new Error("a11y-open-dialog: activity Conexões remotas not found");
      if (activity.getAttribute("aria-pressed") !== "true") {
        activity.click();
        done.push("activity Conexões remotas: click (aria-pressed was false)");
        // HostsPanel mounts on the next Svelte tick: wait for it, never query in the same task.
        await tick();
        done.push("await svelte tick");
      } else done.push("activity Conexões remotas: already pressed");
      const find = () => Array.from(root.querySelectorAll<HTMLButtonElement>("section.hosts button")).find((b) => b.textContent?.trim() === "Nova conexão SSH");
      const opener = await waitFor("HostsPanel button Nova conexão SSH", find, 10000).catch(() => {
        throw new Error("a11y-open-dialog: button Nova conexão SSH not found in HostsPanel");
      });
      opener.focus();
      if (document.activeElement !== opener) throw new Error("a11y-open-dialog: Nova conexão SSH did not take focus");
      done.push("focus button Nova conexão SSH (programmatic, setup)");
      return done;
    },
    active: () => (document.activeElement instanceof HTMLElement && document.activeElement !== document.body ? describe(document.activeElement) : null),
    dialogOpen: () => dialog() !== null,
    modal: () => {
      const form = dialog();
      const el = form?.closest<HTMLElement>("dialog") ?? form;
      if (!el) return null;
      const { name, source } = accessibleName(el as unknown as NameNode, (i) => document.getElementById(i) as unknown as NameNode | null);
      let modal = false;
      try {
        modal = el.matches(":modal");
      } catch {
        modal = false;
      }
      return { tag: el.tagName.toLowerCase(), role: el.getAttribute("role") ?? (el.tagName === "DIALOG" ? "dialog" : ""), name, name_source: source, modal };
    },
    traceClose: () => {
      const trace = traceFocusAfterKey(
        {
          doc: document,
          win: window,
          sample: sampleActiveElement,
          microtask: (cb) => queueMicrotask(cb),
          frame: (cb) => void requestAnimationFrame(() => cb()),
          later: (ms, cb) => void setTimeout(cb, ms),
          now: () => performance.timeOrigin + performance.now(),
          describe: describeTarget,
        },
        "Escape",
      );
      // Product close steps (ConnectionDialog modalCloser) placed in the same timeline.
      const removeSink = setModalTraceSink((step, done) => trace.mark(`closer-${step}${done === undefined ? "" : `:${done}`}`));
      return {
        take: () => trace.take(),
        stop: () => {
          removeSink();
          trace.stop();
        },
      };
    },
    traceChords: () =>
      traceChords({
        doc: document,
        win: window,
        sample: sampleActiveElement,
        describe: describeTarget,
        isTerminal: (t) => t instanceof HTMLElement && t.matches("textarea.ime-target") && root.contains(t),
        later: (ms, cb) => void setTimeout(cb, ms),
        now: () => performance.timeOrigin + performance.now(),
      }),
    cancelDialog: () => Array.from(dialog()?.querySelectorAll<HTMLButtonElement>("button") ?? []).find((b) => b.textContent?.trim() === "Cancelar")?.click(),
    states: () =>
      Array.from(root.querySelectorAll<HTMLElement>("[data-status], [role=status]")).map((el) => {
        const ancestors: string[] = [];
        for (let a = el.parentElement; a && root.contains(a); a = a.parentElement) ancestors.push(accessibleName(a as unknown as NameNode, () => null).name);
        return stateRecord(el.dataset.status ?? "", accessibleName(el as unknown as NameNode, () => null).name, ancestors);
      }),
  };
}

/** Real-time dependencies of the composed window (`awaitParent` = harness_await). */
export function windowDeps(awaitParent: AwaitParent): ViewDeps {
  return { awaitParent, sleep: (ms) => new Promise((resolve) => setTimeout(resolve, ms)), now: () => Date.now() };
}
