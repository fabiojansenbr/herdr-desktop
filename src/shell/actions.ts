// Terminal actions of the composed window (spec 007): pane focus, history scroll, copy selection
// and open link, through four bounded IPC commands (`terminal_actions` module in Rust). Each
// action carries the full identity captured when the user acted (endpoint, session, connection
// generation, boot, confirmed focused pane) plus one concrete request; the backend refuses it when
// any of those changed, and never re-sends it.
//
// - One ordered lane: actions reach the backend one at a time in request order, each once.
//   Consecutive queued scrolls of the same pane and identity collapse into the newest offset and
//   share that request's real result. At most MAX_PENDING_ACTIONS callers wait (every waiter of a
//   coalesced entry counts); more are refused. Requests are copied deeply when admitted.
// - The identity is read synchronously when the callback is called, before any await: a queued
//   action never follows the next selected host.
// - Results are ActionResult for TerminalView: refusals keep the backend's code/message. A method
//   the connection does not announce (`unsupported_method`) denies only that action for that
//   connection, without further invokes.

import { invoke } from "@tauri-apps/api/core";
import { t } from "../i18n/index.svelte";
import { parseNativeHarness } from "../harness/native";
import type { ActionResult, LinkRequest, PaneFocusRequest, ScrollRequest, SelectionRequest } from "../terminal/types";
import type { SurfaceIdentityDto } from "./types";

/** Accepted action (Rust `ActionReceipt`). */
export interface ActionReceipt {
  pane_id: string;
  sent: boolean;
  offset_from_bottom?: number;
  copied_bytes?: number;
}

export interface TerminalActionsBridge {
  focus(expected: SurfaceIdentityDto, request: { pane_id: string }): Promise<ActionReceipt>;
  scroll(expected: SurfaceIdentityDto, request: ScrollRequest): Promise<ActionReceipt>;
  copySelection(expected: SurfaceIdentityDto, request: SelectionRequest): Promise<ActionReceipt>;
  openLink(expected: SurfaceIdentityDto, request: LinkRequest): Promise<ActionReceipt>;
}

export function tauriTerminalActionsBridge(): TerminalActionsBridge {
  return {
    focus: (expected, request) => invoke<ActionReceipt>("surface_pane_focus", { expected, request }),
    scroll: (expected, request) => invoke<ActionReceipt>("surface_scroll", { expected, request }),
    copySelection: (expected, request) => invoke<ActionReceipt>("surface_copy_selection", { expected, request }),
    openLink: (expected, request) => invoke<ActionReceipt>("surface_open_link", { expected, request }),
  };
}

/** TerminalView callbacks bound to the selected surface. */
export interface TerminalActionCallbacks {
  onSelectPane(request: PaneFocusRequest): Promise<ActionResult>;
  onScroll(request: ScrollRequest): Promise<ActionResult>;
  onCopySelection(request: SelectionRequest): Promise<ActionResult>;
  onOpenLink(request: LinkRequest): Promise<ActionResult>;
}

export const MAX_PENDING_ACTIONS = 16;

type Kind = "focus" | "scroll" | "copy" | "link";
export type ActionKind = Kind;

/** One observation of an action (spec 010 r3, harness only):
 *  - `requested`: TerminalView asked for it; `waiting` callers were queued and `running` whether the
 *    lane was busy at that moment (before identity/limit checks);
 *  - `called`: its turn came and the bridge (IPC) was invoked;
 *  - `settled`: the result TerminalView received, the host receipt (null when refused or not
 *    called) and whether the bridge had been called. */
export type ActionEvent =
  | { stage: "requested"; waiting: number; running: boolean }
  | { stage: "called" }
  | { stage: "settled"; called: boolean; receipt: ActionReceipt | null; result: ActionResult };

/** Optional observer of terminal actions. Never alters an action nor the lane. */
export type ActionObserver = (kind: ActionKind, request: { pane_id: string }, event: ActionEvent) => void;

interface Item {
  kind: Kind;
  expected: SurfaceIdentityDto;
  request: { pane_id: string } | ScrollRequest | SelectionRequest | LinkRequest;
  waiters: Array<(result: ActionResult) => void>;
}

/** Copy taken at admission: later mutation of the caller's objects never reaches the backend. */
function admitted(kind: Kind, request: Item["request"]): Item["request"] {
  if (kind === "copy") {
    const r = request as SelectionRequest;
    return { pane_id: r.pane_id, anchor: { row: r.anchor.row, col: r.anchor.col }, cursor: { row: r.cursor.row, col: r.cursor.col }, content_revision: r.content_revision };
  }
  return { ...request };
}

function connectionKey(identity: SurfaceIdentityDto): string {
  return JSON.stringify([identity.endpoint, identity.session, identity.connection_generation, identity.boot_id]);
}

function sameIdentity(a: SurfaceIdentityDto, b: SurfaceIdentityDto): boolean {
  return connectionKey(a) === connectionKey(b) && a.pane_id === b.pane_id;
}

export function actionError(error: unknown): ActionResult {
  if (error && typeof error === "object" && typeof (error as { code?: unknown }).code === "string") {
    const { code, message } = error as { code: string; message?: unknown };
    return { ok: false, code, message: typeof message === "string" ? message : code };
  }
  const message = error instanceof Error ? error.message : String(error);
  return { ok: false, code: "action_failed", message };
}

/** `identity` returns the confirmed identity of the selected surface, or null when there is none. */
export function createTerminalActions(
  bridge: TerminalActionsBridge,
  identity: () => SurfaceIdentityDto | null,
  onResult?: ActionObserver,
): TerminalActionCallbacks {
  const queue: Item[] = [];
  let running = false;
  /** connectionKey|kind → the refusal of a method this connection does not announce. */
  const unsupported = new Map<string, ActionResult>();

  function call(item: Item): Promise<ActionReceipt> {
    switch (item.kind) {
      case "focus":
        return bridge.focus(item.expected, { pane_id: item.request.pane_id });
      case "scroll":
        return bridge.scroll(item.expected, item.request as ScrollRequest);
      case "copy":
        return bridge.copySelection(item.expected, item.request as SelectionRequest);
      case "link":
        return bridge.openLink(item.expected, item.request as LinkRequest);
    }
  }

  function observe(kind: Kind, request: Item["request"], event: ActionEvent) {
    if (!onResult) return;
    try {
      onResult(kind, request, event);
    } catch {
      // Observation never changes the action nor stalls the lane.
    }
  }

  async function run(item: Item): Promise<ActionResult> {
    const denied = unsupported.get(`${connectionKey(item.expected)}|${item.kind}`);
    if (denied) {
      observe(item.kind, item.request, { stage: "settled", called: false, receipt: null, result: denied });
      return denied;
    }
    try {
      observe(item.kind, item.request, { stage: "called" });
      const receipt = await call(item);
      const result: ActionResult = { ok: true };
      observe(item.kind, item.request, { stage: "settled", called: true, receipt: receipt ?? null, result });
      return result;
    } catch (error) {
      const result = actionError(error);
      if (!result.ok && result.code === "unsupported_method") {
        unsupported.set(`${connectionKey(item.expected)}|${item.kind}`, result);
      }
      observe(item.kind, item.request, { stage: "settled", called: true, receipt: null, result });
      return result;
    }
  }

  function pump() {
    if (running) return;
    const item = queue.shift();
    if (!item) return;
    running = true;
    void run(item).then((result) => {
      running = false;
      for (const resolve of item.waiters) resolve(result);
      pump();
    });
  }

  function enqueue(kind: Kind, request: Item["request"]): Promise<ActionResult> {
    // Captured now, before any await: the action belongs to the host the user acted on.
    if (onResult) {
      observe(kind, request, { stage: "requested", waiting: queue.reduce((sum, item) => sum + item.waiters.length, 0), running });
    }
    const expected = identity();
    const refused = (result: ActionResult) => {
      observe(kind, request, { stage: "settled", called: false, receipt: null, result });
      return result;
    };
    if (!expected) {
      return Promise.resolve(refused({ ok: false, code: "no_identity", message: t("shell.actions.noIdentity") }));
    }
    const captured = { ...expected };
    const denied = unsupported.get(`${connectionKey(captured)}|${kind}`);
    if (denied) return Promise.resolve(refused(denied));
    const copy = admitted(kind, request);
    return new Promise((resolve) => {
      // Every waiting caller counts, including those merged into one coalesced scroll.
      const waiting = queue.reduce((sum, item) => sum + item.waiters.length, 0);
      if (waiting >= MAX_PENDING_ACTIONS) {
        resolve(refused({ ok: false, code: "actions_busy", message: t("shell.actions.busy") }));
        return;
      }
      const last = queue[queue.length - 1];
      if (kind === "scroll" && last?.kind === "scroll" && last.request.pane_id === copy.pane_id && sameIdentity(last.expected, captured)) {
        last.request = copy;
        last.waiters.push(resolve);
        return;
      }
      queue.push({ kind, expected: captured, request: copy, waiters: [resolve] });
      pump();
    });
  }

  return {
    onSelectPane: (request) => enqueue("focus", { pane_id: request.pane_id }),
    onScroll: (request) => enqueue("scroll", request),
    onCopySelection: (request) => enqueue("copy", request),
    onOpenLink: (request) => enqueue("link", request),
  };
}

// --- harness action log (spec 010 r3) ----------------------------------------------------------
// Opt-in like the terminal probe (src/terminal/probe.ts): exists only when a native harness
// selection was injected (`window.__HERDR_HARNESS__`); the product window gets null and App passes
// no observer. The fidelity page reads it to report what each terminal action did.

export const ACTION_LOG_LIMIT = 64;

export type ActionLogEntry = { kind: ActionKind; pane_id: string; uri: string | null; at_ms: number } & ActionEvent;

export interface ActionLog {
  record: ActionObserver;
  /** Entries since the last take (oldest first), consumed. */
  take(): ActionLogEntry[];
}

export function actionLogFromSelection(value: unknown, now: () => number = () => performance.now()): ActionLog | null {
  if (!parseNativeHarness(value)) return null;
  const entries: ActionLogEntry[] = [];
  return {
    record: (kind, request, event) => {
      const uri = (request as { uri?: unknown }).uri;
      const copy: ActionEvent =
        event.stage === "settled" ? { ...event, receipt: event.receipt ? { ...event.receipt } : null, result: { ...event.result } } : { ...event };
      entries.push({ kind, pane_id: request.pane_id, uri: typeof uri === "string" ? uri : null, at_ms: now(), ...copy });
      if (entries.length > ACTION_LOG_LIMIT) entries.splice(0, entries.length - ACTION_LOG_LIMIT);
    },
    take: () => entries.splice(0),
  };
}

let logResolved = false;
let log: ActionLog | null = null;

/** The window's action log (resolved once from the injected selection); null in the product. */
export function terminalActionLog(): ActionLog | null {
  if (!logResolved) {
    logResolved = true;
    log = actionLogFromSelection(typeof window === "undefined" ? undefined : window.__HERDR_HARNESS__);
  }
  return log;
}
