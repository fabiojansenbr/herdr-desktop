// Single selection → single terminal surface (spec 007). The selected host is the only one whose
// frames reach the view and whose identity addresses input. Every attach gets its own token and
// captured SelectionDto; events and results of a superseded token are dropped, never retargeted.
// Selection operations run one at a time so the backend observes them in the user's order.
// Input batches form a separate bounded queue with one surface_input in flight: a host switch never
// waits for a slow write, and a batch not sent yet is refused once its channel, identity or live
// state is gone (contract: .local/orchestration/input-dispatch-contract.md). A native paste
// (Ctrl+Shift+V) enters that same queue and reserves a small payload until its turn, so a slow
// paste is never overtaken by the next key (contract: .local/orchestration/native-paste-contract.md).

import { t } from "../i18n/index.svelte";
import { MAX_PENDING_INPUT_BATCHES, MAX_PENDING_INPUT_BYTES, type SurfaceBridge } from "./bridge";
import type { WorkbenchConnection } from "./Workbench.types";
import type {
  FrameEvent,
  GeometryDto,
  InputDto,
  RuntimeError,
  SelectionDto,
  StatusDto,
  SurfaceIdentityDto,
} from "./types";

export type SurfacePhase = "empty" | "switching" | "connecting" | "live" | "stale" | "disconnected";

/** What the last native paste of this attach did (spec 028 AC-028-04); absent before any paste. */
export interface PasteState {
  pane_id: string;
  sent: boolean;
  bytes: number;
  kind: "text" | "image" | "empty" | "forward_key";
}

export interface SurfaceState {
  selection: SelectionDto | null;
  status: StatusDto | null;
  phase: SurfacePhase;
  reason: string | null;
  error: RuntimeError | null;
  /** Identity confirmed by the selected host's channel; null blocks input. */
  identity: SurfaceIdentityDto | null;
  /** Changes whenever the surface must start empty (new attach): the view remounts on it. */
  surfaceKey: number;
  busy: boolean;
  /** Result of the last native paste (spec 028); never carries clipboard content. */
  paste?: PasteState | null;
}

/**
 * Payload a native paste reserves in the queue while it waits for its turn (mirrors
 * `NATIVE_PASTE_RESERVED_BYTES` in src-tauri/src/bridge/composition.rs): the clipboard text is
 * unknown until the backend reads it at that turn.
 */
export const NATIVE_PASTE_RESERVED_BYTES = 64;

export interface SurfaceController {
  readonly state: SurfaceState;
  subscribe(handler: (event: FrameEvent) => void): () => void;
  /** Reads the selection; attaches only when a host and session are configured. */
  load(): Promise<void>;
  /** Selects a host explicitly; rejects when refused or superseded. Never falls back. */
  select(endpoint: string): Promise<SelectionDto>;
  /** Reattaches the currently selected host ("Tentar novamente"). */
  retry(): Promise<void>;
  /** Explicit "Iniciar sessão": starts the configured Local session and attaches. */
  startSession(): Promise<void>;
  canInput(): boolean;
  /**
   * Resolves once `endpoint` is the selected host and its surface is live; rejects when another
   * host is selected (`selection_changed`), the attach fails (its error) or `timeoutMs` passes.
   */
  whenReady(endpoint: string, options?: { timeoutMs?: number }): Promise<void>;
  /**
   * Resolves with the confirmed identity once `endpoint` is selected, live and the terminal
   * interest is open (the backend acknowledged the show and a full frame of the current attach
   * arrived) — the same gate as `canInput`. Rejects on another selection (`selection_changed`),
   * the attach error, the failed show, the terminal hidden (`surface_hidden`) or `timeoutMs`
   * (`surface_timeout`). Sends nothing itself.
   */
  whenInteractive(endpoint: string, options?: WaitOptions): Promise<SurfaceIdentityDto>;
  /**
   * Presentation readiness (project open): resolves with the connection presented for `endpoint`
   * (live identity frame of the current attach + interest ack + full frame), WITHOUT requiring a
   * focused pane — an empty session can open its first project. Rejects like `whenInteractive`.
   * Input, split and focus keep using `whenInteractive`/`canInput`.
   */
  whenPresented(endpoint: string, options?: WaitOptions): Promise<PresentedIdentityDto>;
  /**
   * Current attach episode: changes on every select, retry, load attach and session start (and
   * dispose). `whenInteractive`/`whenPresented` capture it (or take `episode` captured when the
   * user acted) and reject with `selection_changed` once it changed — even when the new attach is
   * the same endpoint with the same boot, generation and pane ids.
   */
  episode(): number;
  /**
   * Queues one batch with the live identity captured now; resolves true once the backend sent it,
   * false when blocked, over the pending limits, refused, cancelled before its turn or failed.
   * Batches go out in call order, one invoke at a time, each at most once.
   */
  input(events: InputDto[]): Promise<boolean>;
  /**
   * Native Ctrl+Shift+V (spec 007): the backend reads the local clipboard at this paste's turn
   * and sends one `Paste` through the same ordered queue as `input`; the text never reaches the
   * WebView. Resolves true once the backend sent it, false when blocked, over the pending limits,
   * refused, cancelled before its turn or failed; an empty clipboard is false without error.
   * Pastes go out in call order, one invoke at a time, each at most once.
   */
  nativePaste(): Promise<boolean>;
  resize(geometry: GeometryDto): Promise<void>;
  focus(focused: boolean): Promise<void>;
  /**
   * Window interest in the terminal (document visibility and files layer; never window focus).
   * Hiding closes input at once and tells the backend; showing keeps input closed until the
   * backend acknowledged and a full frame of the current attach arrived. Repeated values and
   * superseded changes are not sent. Errors are shown and never retried.
   */
  setInterest(active: boolean): Promise<void>;
  /** Refuses queued and future input (window going away); the batch in flight still answers. */
  dispose(): void;
}

export interface SurfaceOptions {
  onChange?: (state: SurfaceState) => void;
  /** Geometry of the mounted view, if any. */
  geometry?: () => GeometryDto | null;
  /**
   * Awaited before every surface_attach, after the surface was reset: lets the (re)mounted view
   * subscribe first, so frames emitted during the attach call itself are not lost.
   */
  ready?: () => Promise<void>;
  /** Synchronously at the start of an explicit selection, before any command is sent. */
  onSelectionStart?: (endpoint: string) => void;
  /** Once per confirmed connection (attach token + boot + connection generation) that is live. */
  onLive?: (selection: SelectionDto, identity: IdentityFrame) => void;
}

export type IdentityFrame = Extract<FrameEvent, { type: "identity" }>;

const DEFAULT_GEOMETRY: GeometryDto = { cols: 80, rows: 24, cell_width_px: 9, cell_height_px: 18 };
const PHASES: readonly SurfacePhase[] = ["connecting", "live", "stale", "disconnected"];

export function asRuntimeError(error: unknown): RuntimeError {
  if (error && typeof error === "object" && "code" in error && "message" in error) return error as RuntimeError;
  return { code: "ipc_error", message: t("shell.error.ipc"), retryable: true };
}

const encoder = new TextEncoder();

function sameIdentity(a: SurfaceIdentityDto | null, b: SurfaceIdentityDto): boolean {
  return (
    a !== null &&
    a.endpoint === b.endpoint &&
    a.session === b.session &&
    a.connection_generation === b.connection_generation &&
    a.boot_id === b.boot_id &&
    a.pane_id === b.pane_id
  );
}

const inputError = (code: string, message: string, retryable = false): RuntimeError => ({ code, message, retryable });

/** Connection whose surface is presented, before any pane is confirmed focused. */
export type PresentedIdentityDto = Omit<SurfaceIdentityDto, "pane_id">;

export interface WaitOptions {
  timeoutMs?: number;
  /** Episode captured when the user acted (`episode()`); defaults to the episode at the call. */
  episode?: number;
}

const surfaceHidden = (): RuntimeError => ({
  code: "surface_hidden",
  message: t("shell.error.surfaceHidden"),
  retryable: false,
});

const selectionChanged = (): RuntimeError => ({
  code: "selection_changed",
  message: t("shell.error.selectionChanged"),
  retryable: false,
});

export function createSurfaceController(bridge: SurfaceBridge, options: SurfaceOptions = {}): SurfaceController {
  let state: SurfaceState = {
    selection: null,
    status: null,
    phase: "empty",
    reason: null,
    error: null,
    identity: null,
    surfaceKey: 0,
    busy: false,
    paste: null,
  };
  interface Waiter {
    endpoint: string;
    /**
     * `ready`: live identity frame. `presented`: also the interest open (ack + full). `interactive`:
     * also a confirmed identity (focused pane) — the input gate.
     */
    mode: "ready" | "presented" | "interactive";
    /** Episode the wait belongs to; `null` for `whenReady` (not fenced). */
    own: number | null;
    resolve(): void;
    reject(error: RuntimeError): void;
  }
  const waiters = new Set<Waiter>();
  // Private copy of the confirmed identity: input captures and compares against it, never against
  // the object exposed in `state` (which a consumer could mutate).
  let confirmed: SurfaceIdentityDto | null = null;
  const commit = (patch: Partial<SurfaceState>) => {
    if ("identity" in patch) {
      confirmed = patch.identity ? { ...patch.identity } : null;
      patch = { ...patch, identity: patch.identity ? { ...patch.identity } : null };
    }
    state = { ...state, ...patch };
    options.onChange?.(state);
    for (const waiter of [...waiters]) settle(waiter);
    refuseObsoleteInput();
  };

  interface QueuedInput {
    own: number;
    expected: SurfaceIdentityDto;
    /** Events of a typed batch; empty for a native paste (the backend reads its own text). */
    events: InputDto[];
    /** Native paste: `pumpInput` invokes `bridge.pasteClipboard` instead of `bridge.input`. */
    native: boolean;
    bytes: number;
    resolve(sent: boolean): void;
  }
  const inputQueue: QueuedInput[] = [];
  let inFlight: QueuedInput | null = null;
  let pendingBytes = 0;
  let disposed = false;
  /** Legacy mocks may omit the native command: the controller refuses instead of fabricating it. */
  const hasNativePaste = typeof bridge.pasteClipboard === "function";

  /** The batch may still be sent: same attach token, live, same confirmed identity. */
  const inputCurrent = (item: QueuedInput) =>
    !disposed && item.own === token && inputAllowed(state) && interestOpen() && sameIdentity(confirmed, item.expected);

  function refuseObsoleteInput() {
    if (inputQueue.length === 0 || inputQueue.every(inputCurrent)) return;
    const refused = inputQueue.filter((item) => !inputCurrent(item));
    inputQueue.splice(0, inputQueue.length, ...inputQueue.filter(inputCurrent));
    for (const item of refused) {
      pendingBytes -= item.bytes;
      item.resolve(false);
    }
    // Reported only to the attach they belonged to; a new selection starts without it.
    if (!disposed && refused.some((item) => item.own === token)) {
      commit({
        error: inputError(
          "input_cancelled",
          t("shell.error.inputCancelled", { count: refused.length }),
        ),
      });
    }
  }

  function pumpInput() {
    if (inFlight) return;
    const item = inputQueue.shift();
    if (!item) return;
    inFlight = item;
    const finish = (sent: boolean, error: RuntimeError | null) => {
      inFlight = null;
      pendingBytes -= item.bytes;
      // A late answer of an older attach or connection never overwrites the current state.
      if (error && item.own === token && sameIdentity(confirmed, item.expected)) commit({ error });
      item.resolve(sent);
      pumpInput();
    };
    if (item.native) {
      // One command per paste: the backend reads the clipboard at its turn and answers with the
      // pasted bytes (0: empty clipboard, nothing sent). Never retried, text never returned here.
      // The receipt's kind is reported on the state (spec 028): text, image or empty.
      bridge.pasteClipboard!(item.expected).then(
        (receipt) => {
          const kind = receipt.kind ?? (receipt.sent ? "text" : "empty");
          if (item.own === token && sameIdentity(confirmed, item.expected)) {
            commit({ paste: { pane_id: receipt.pane_id, sent: receipt.sent, bytes: receipt.pasted_bytes, kind } });
          }
          finish(receipt.sent, null);
        },
        (error) => finish(false, asRuntimeError(error)),
      );
      return;
    }
    bridge.input(item.expected, item.events).then(
      () => finish(true, null),
      (error) => finish(false, asRuntimeError(error)),
    );
  }
  function settle(waiter: Waiter) {
    const selected = state.selection?.endpoint ?? null;
    // Another attach episode began (even A→B→A to identical identity): the intent never waits for it.
    if (waiter.own !== null && waiter.own !== token) return waiter.reject(selectionChanged());
    if (waiter.mode !== "ready" && selected === waiter.endpoint) {
      if (!desired) return waiter.reject(surfaceHidden());
      if (showFailure) return waiter.reject(showFailure);
    }
    const ready =
      waiter.mode === "ready"
        ? frame !== null
        : waiter.mode === "presented"
          ? interestOpen() && fullSeen && presented() !== null
          : inputAllowed(state) && interestOpen() && confirmed !== null;
    if (state.phase === "live" && ready && selected === waiter.endpoint) waiter.resolve();
    else if (state.phase === "disconnected" && state.error && (selected === waiter.endpoint || selected === null)) {
      waiter.reject(state.error);
    } else if (state.phase !== "switching" && selected !== null && selected !== waiter.endpoint) {
      waiter.reject(selectionChanged());
    }
  }
  /** Connection presented by the current attach (identity frame + its session), pane not needed. */
  function presented(): PresentedIdentityDto | null {
    const selection = state.selection;
    const session = selection?.session ?? state.status?.session ?? null;
    if (!frame || !selection?.endpoint || !session) return null;
    return {
      endpoint: selection.endpoint,
      session,
      connection_generation: frame.connection_generation,
      boot_id: frame.boot_id,
    };
  }
  function wait<T>(endpoint: string, mode: Waiter["mode"], own: number | null, timeoutMs: number, value: () => T): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      const done = () => {
        clearTimeout(timer);
        waiters.delete(waiter);
      };
      const waiter: Waiter = {
        endpoint,
        mode,
        own,
        resolve: () => {
          done();
          resolve(value());
        },
        reject: (error) => {
          done();
          reject(error);
        },
      };
      const timer = setTimeout(
        () => waiter.reject({ code: "surface_timeout", message: t("shell.error.surfaceTimeout"), retryable: true }),
        timeoutMs,
      );
      waiters.add(waiter);
      settle(waiter);
    });
  }
  const handlers = new Set<(event: FrameEvent) => void>();
  // Window interest (.local/orchestration/interest-contract.md). `acked`: the current show was
  // resolved; `awaitingFull`: a show was sent and no full frame arrived since; `hiddenOn`: endpoints
  // whose backend interest may still be false.
  let desired = true;
  let interestCycle = 0;
  let acked = true;
  let awaitingFull = false;
  /** Error of the current show that was never acknowledged (cleared by the next change/attach). */
  let showFailure: RuntimeError | null = null;
  const hiddenOn = new Set<string>();
  const interestOpen = () => desired && acked && !awaitingFull;

  let token = 0;
  let chain: Promise<unknown> = Promise.resolve();
  let geometry: GeometryDto | null = null;
  // Identity frame of the current token, combined with its captured selection.
  let frame: IdentityFrame | null = null;
  /** A full frame of the current token arrived (presentation of this attach, not only its identity). */
  let fullSeen = false;
  let liveKey: string | null = null;

  function reportLive(own: number, selection: SelectionDto) {
    if (own !== token || state.phase !== "live" || !frame) return;
    const key = `${own}|${frame.boot_id}|${frame.connection_generation}`;
    if (key === liveKey) return;
    liveKey = key;
    options.onLive?.(selection, frame);
  }

  const serial = <T>(run: () => Promise<T>): Promise<T> => {
    const next = chain.then(run, run);
    chain = next.catch(() => {});
    return next;
  };

  /** Starts a new episode: old channel events and results stop mattering, input blocks. */
  function begin(phase: SurfacePhase): number {
    token += 1;
    frame = null;
    fullSeen = false;
    commit({ phase, reason: null, error: null, identity: null, surfaceKey: state.surfaceKey + 1 });
    return token;
  }

  function identityFor(selection: SelectionDto, status: StatusDto | null): SurfaceIdentityDto | null {
    const session = selection.session ?? status?.session ?? null;
    if (!frame || !frame.pane_id || !selection.endpoint || !session) return null;
    return {
      endpoint: selection.endpoint,
      session,
      connection_generation: frame.connection_generation,
      boot_id: frame.boot_id,
      pane_id: frame.pane_id,
    };
  }

  /** Sends `active` for `endpoint`; `cycle` is the interest change it belongs to. */
  async function sendInterest(cycle: number, endpoint: string, active: boolean): Promise<boolean> {
    // The backend records the interest (and closes the surface) before waiting on the network.
    if (active) awaitingFull = true;
    else hiddenOn.add(endpoint);
    if (!bridge.interest) {
      if (active) hiddenOn.delete(endpoint);
    } else {
      try {
        await bridge.interest(active);
      } catch (error) {
        if (cycle === interestCycle) {
          const runtime = asRuntimeError(error);
          if (active) showFailure = runtime;
          commit({ error: runtime });
        }
        return false;
      }
      if (active) hiddenOn.delete(endpoint);
    }
    if (active && cycle === interestCycle && desired) acked = true;
    return true;
  }

  function onFrame(own: number, selection: SelectionDto, event: FrameEvent) {
    if (own !== token) return;
    // The gate opening is a state change (the view's input gate and interactive waiters follow).
    const opened = event.type === "full" && ((desired && awaitingFull) || !fullSeen);
    if (event.type === "full") fullSeen = true;
    if (event.type === "full" && desired) awaitingFull = false;
    if (event.type === "identity") {
      frame = event;
      commit({ identity: identityFor(selection, state.status) });
    } else if (event.type === "state") {
      const phase = PHASES.includes(event.state as SurfacePhase) ? (event.state as SurfacePhase) : state.phase;
      // A live frame clears old errors, except an interest error still keeping input closed.
      // Connection loss after a live attach is the literal `desconectado: <código>` (AC-016-02).
      const lost =
        phase === "disconnected" && !event.error && event.reason
          ? {
              code: event.reason,
              message: `desconectado: ${event.reason}`,
              retryable: true,
            }
          : null;
      const error = event.error ?? lost ?? (phase === "live" && interestOpen() ? null : state.error);
      commit({ phase, reason: event.reason, error });
    }
    for (const handler of handlers) handler(event);
    if (opened) commit({});
    reportLive(own, selection);
  }

  async function attach(own: number, selection: SelectionDto) {
    commit({ phase: "connecting" });
    const endpoint = selection.endpoint;
    if (endpoint && bridge.interest && (!desired || hiddenOn.has(endpoint))) {
      // Before the channel exists: a hidden window gets no replayed frames, and a host hidden
      // earlier is shown again before its surface is replayed.
      if (desired) acked = false;
      showFailure = null;
      await sendInterest(interestCycle, endpoint, desired);
      if (own !== token) return;
    }
    if (options.ready) {
      try {
        await options.ready();
      } catch {
        // Readiness only orders the attach; it never cancels the user's choice.
      }
      if (own !== token) return;
    }
    try {
      const status = await bridge.attach(options.geometry?.() ?? geometry ?? DEFAULT_GEOMETRY, (event) =>
        onFrame(own, selection, event),
      );
      if (own !== token) return;
      const phase = state.phase === "connecting" && PHASES.includes(status.state as SurfacePhase)
        ? (status.state as SurfacePhase)
        : state.phase;
      commit({ status, phase, identity: identityFor(selection, status) });
      reportLive(own, selection);
    } catch (error) {
      if (own !== token) return;
      commit({ phase: "disconnected", error: asRuntimeError(error), identity: null });
      try {
        const status = await bridge.status();
        if (own === token) commit({ status });
      } catch {
        // The error already shown is the one that matters to the user.
      }
    }
  }

  const controller: SurfaceController = {
    get state() {
      return state;
    },
    subscribe(handler) {
      handlers.add(handler);
      return () => handlers.delete(handler);
    },
    load: () =>
      serial(async () => {
        const own = token;
        let selection: SelectionDto;
        try {
          selection = await bridge.selectionGet();
        } catch (error) {
          // The window stays usable: the error is shown and "Tentar novamente" reads again.
          if (own === token) commit({ phase: "disconnected", error: asRuntimeError(error) });
          return;
        }
        if (own === token && state.error) commit({ error: null });
        let status: StatusDto | null = null;
        try {
          status = await bridge.status();
        } catch (error) {
          if (own === token) commit({ error: asRuntimeError(error) });
        }
        if (own !== token) return;
        commit({ selection, status });
        if (selection.endpoint && (selection.session ?? status?.session)) {
          await attach(begin("connecting"), selection);
        }
      }),
    select(endpoint) {
      const own = begin("switching");
      options.onSelectionStart?.(endpoint);
      return serial(async () => {
        if (own !== token) throw selectionChanged();
        try {
          await bridge.detach();
        } catch {
          // Detach only releases the channel; a failure must not block the explicit choice.
        }
        if (own !== token) throw selectionChanged();
        let selection: SelectionDto;
        try {
          selection = await bridge.selectionSet(endpoint);
        } catch (error) {
          if (own !== token) throw selectionChanged();
          const runtime = asRuntimeError(error);
          commit({ phase: "disconnected", error: runtime });
          throw runtime;
        }
        if (own !== token) throw selectionChanged();
        commit({ selection, status: null });
        await attach(own, selection);
        if (own !== token) throw selectionChanged();
        return selection;
      });
    },
    retry() {
      const selection = state.selection;
      if (!selection) return controller.load();
      if (!selection.endpoint) return Promise.resolve();
      const own = begin("connecting");
      return serial(async () => {
        if (own !== token) return;
        try {
          await bridge.detach();
        } catch {
          // Same as select: detaching is best effort.
        }
        if (own === token) await attach(own, selection);
      });
    },
    startSession() {
      const selection = state.selection;
      if (!selection?.endpoint) return Promise.resolve();
      const own = begin("connecting");
      commit({ busy: true });
      return serial(async () => {
        try {
          const status = await bridge.startSession();
          if (own !== token) return;
          commit({ status });
          await attach(own, selection);
        } catch (error) {
          if (own === token) commit({ phase: "disconnected", error: asRuntimeError(error) });
        } finally {
          commit({ busy: false });
        }
      });
    },
    canInput: () => inputAllowed(state) && interestOpen(),
    setInterest(active) {
      if (disposed || desired === active) return Promise.resolve();
      desired = active;
      showFailure = null;
      const cycle = ++interestCycle;
      if (active) acked = false;
      commit({});
      return serial(async () => {
        // A newer change replaces this one before it reached the backend: nothing to send.
        if (cycle !== interestCycle || disposed) return;
        const endpoint = state.selection?.endpoint;
        if (!endpoint || active !== hiddenOn.has(endpoint)) {
          // Nothing selected, or the backend already has this interest (a change undone before
          // it was sent): nothing to send; a show still waiting for its full frame keeps waiting.
          if (active) acked = true;
        } else {
          await sendInterest(cycle, endpoint, active);
        }
        commit({});
      });
    },
    whenReady(endpoint, { timeoutMs = 30_000 } = {}) {
      return wait(endpoint, "ready", null, timeoutMs, () => undefined);
    },
    whenInteractive(endpoint, { timeoutMs = 30_000, episode = token } = {}) {
      return wait(endpoint, "interactive", episode, timeoutMs, () => ({ ...confirmed! }));
    },
    whenPresented(endpoint, { timeoutMs = 30_000, episode = token } = {}) {
      return wait(endpoint, "presented", episode, timeoutMs, () => ({ ...presented()! }));
    },
    episode: () => token,
    input(events) {
      if (disposed || !confirmed || !controller.canInput() || events.length === 0) return Promise.resolve(false);
      // Owned snapshot taken now: the budget is computed from, and the backend receives, exactly these
      // bytes; later mutations of the caller's array/events (mouse pixel/geometry included) or of the
      // exposed identity do not reach the queue.
      const payload = JSON.stringify(events);
      const bytes = encoder.encode(payload).length;
      if (bytes > MAX_PENDING_INPUT_BYTES) {
        commit({ error: inputError("input_too_large", t("shell.error.inputTooLarge")) });
        return Promise.resolve(false);
      }
      const pending = inputQueue.length + (inFlight ? 1 : 0);
      if (pending >= MAX_PENDING_INPUT_BATCHES || pendingBytes + bytes > MAX_PENDING_INPUT_BYTES) {
        commit({
          error: inputError("input_overflow", t("shell.error.inputOverflowBatch"), true),
        });
        return Promise.resolve(false);
      }
      const snapshot = JSON.parse(payload) as InputDto[];
      const expected: SurfaceIdentityDto = { ...confirmed };
      return new Promise<boolean>((resolve) => {
        pendingBytes += bytes;
        inputQueue.push({ own: token, expected, events: snapshot, native: false, bytes, resolve });
        pumpInput();
      });
    },
    nativePaste() {
      if (disposed || !confirmed || !controller.canInput()) return Promise.resolve(false);
      if (!hasNativePaste) {
        commit({
          error: inputError(
            "native_paste_unavailable",
            t("shell.error.nativePasteUnavailable"),
          ),
        });
        return Promise.resolve(false);
      }
      const pending = inputQueue.length + (inFlight ? 1 : 0);
      if (pending >= MAX_PENDING_INPUT_BATCHES || pendingBytes + NATIVE_PASTE_RESERVED_BYTES > MAX_PENDING_INPUT_BYTES) {
        commit({
          error: inputError("input_overflow", t("shell.error.inputOverflowPaste"), true),
        });
        return Promise.resolve(false);
      }
      // Same lane and gates as `input`: the attach token and a private copy of the confirmed
      // identity are captured now, and a paste not sent yet is refused once either changes.
      const expected: SurfaceIdentityDto = { ...confirmed };
      return new Promise<boolean>((resolve) => {
        pendingBytes += NATIVE_PASTE_RESERVED_BYTES;
        inputQueue.push({ own: token, expected, events: [], native: true, bytes: NATIVE_PASTE_RESERVED_BYTES, resolve });
        pumpInput();
      });
    },
    dispose() {
      disposed = true;
      token += 1;
      refuseObsoleteInput();
    },
    async resize(next) {
      geometry = next;
      if (!state.selection?.endpoint || state.phase === "switching") return;
      try {
        await bridge.resize(next);
      } catch (error) {
        if (!state.error) commit({ error: asRuntimeError(error) });
      }
    },
    async focus(focused) {
      if (!state.selection?.endpoint) return;
      try {
        await bridge.focus(focused);
      } catch {
        // Focus is advisory; the next frame shows the confirmed focus.
      }
    },
  };
  return controller;
}

/** Input only with a live surface whose confirmed identity belongs to the selected host. */
export function inputAllowed(state: SurfaceState): boolean {
  return state.phase === "live" && state.identity !== null && state.selection?.endpoint === state.identity.endpoint;
}

/**
 * Zero-config bootstrap of the engine's default session (AC-016-01). A function, not a constant:
 * the words follow the current language, and the caller compares against this same call.
 */
export function startingHerdr(): string {
  return t("shell.status.startingHerdr");
}

/** Text for the surface state; color is only a complement. */
export function surfaceStatusText(state: SurfaceState): string {
  if (!state.selection?.endpoint) return t("shell.status.noSession");
  switch (state.phase) {
    case "live":
      return t("shell.status.connected");
    case "stale":
      return t("shell.status.resyncing");
    case "connecting": {
      // "Iniciando o Herdr…" só aparece quando o desktop está de fato iniciando/esperando o servidor Local (016).
      const isLocal = state.selection.kind === "local" || state.selection.endpoint === "local";
      return isLocal && state.selection.session === "default" ? startingHerdr() : t("shell.status.connecting");
    }
    case "switching":
      return t("shell.status.switchingHost");
    case "disconnected":
      return t("shell.status.disconnected");
    default:
      return t("shell.status.noSession");
  }
}

/** Header pill for the Workbench: host label plus state text (the dot color only complements). */
export function workbenchConnection(state: SurfaceState): WorkbenchConnection | undefined {
  const selection = state.selection;
  if (!selection?.endpoint) return undefined;
  const kind = selection.kind ?? (selection.endpoint === "local" ? "local" : "ssh");
  const label = selection.label ?? (kind === "local" ? t("connections.kind.local") : selection.endpoint);
  const status: WorkbenchConnection["status"] =
    state.phase === "live"
      ? "connected"
      : state.phase === "stale"
        ? "stale"
        : state.phase === "connecting" || state.phase === "switching"
          ? "connecting"
          : "disconnected";
  return { name: `${label} — ${surfaceStatusText(state)}`, kind, status };
}
