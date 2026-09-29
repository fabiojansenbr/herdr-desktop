// Drives the agents panel: each user intent becomes at most one bridge command; server events
// replace local copies. Nothing is sent on render, on a state change or on a timer: a blocked
// agent is only surfaced, a prompt with an unknown outcome is only resent by the user, and
// keyboard input goes to the pane the server confirmed focused, in typing order.

import { t } from "../i18n/index.svelte";
import { routeKey, type GuiAction, type KeyLike } from "../terminal/actions";
import type { GeometryDto } from "../terminal/types";
import type { AgentsBridge } from "./bridge";
import {
  confirmedPane,
  initialState,
  promptDisabledReasonFor,
  reduce,
  splitOfFocusedPane,
  targetFor,
  type AgentsAction,
  type AgentsState,
} from "./reducer";
import type { InputDto, QualifiedTarget, RuntimeError, ZoomMode } from "./types";

export interface AgentsController {
  readonly state: AgentsState;
  connect(geometry: GeometryDto): Promise<void>;
  /**
   * Ends the current attachment (composed window: host switch, reconnect or reboot): state
   * resets, and late connect results, channel events, action results and queued input of the
   * previous attachment are dropped. Nothing is retried. Standalone harnesses never call it.
   */
  invalidate(): void;
  editStart(field: "paneId" | "kind" | "name", value: string): void;
  /**
   * Autonomy of the next start (spec 076): the backend turns it into that kind's own flags, and
   * the WebView never sends arguments. It applies to one start and is cleared by it, so a caller
   * that does not offer the switch (the older form) always starts in the manual mode.
   */
  setStartAutonomy(on: boolean): void;
  startAgent(): Promise<void>;
  editPrompt(paneId: string, text: string): void;
  sendPrompt(paneId: string): Promise<void>;
  openAttention(paneId: string): Promise<void>;
  /** Splits the confirmed pane and answers the pane the engine created (null when it answered
   *  none, when the split is unavailable or when there is no confirmed pane) — spec 075. */
  split(direction: "right" | "down"): Promise<string | null>;
  focusPane(paneId: string): Promise<void>;
  setRatio(ratio: number): Promise<void>;
  createTab(): Promise<void>;
  /** Expand (or restore) a pane in the engine; refused when the server does not offer it. */
  zoomPane(paneId: string, mode?: ZoomMode): Promise<void>;
  /** Set (or clear, with an empty/null label) the manual name of one pane (`pane.rename`). */
  renamePane(paneId: string, label: string | null): Promise<void>;
  /** Exchange the pane under the menu with the confirmed focused pane (`pane.swap`). */
  swapPane(paneId: string): Promise<void>;
  /** Send right clicks of `paneId` to the pane (true) or open the Herdr menu (false). */
  setPaneRightClick(paneId: string, passthrough: boolean): Promise<void>;
  focusTab(tabId: string): Promise<void>;
  /** Close one tab the engine listed (`tab.close`). */
  closeTab(tabId: string): Promise<void>;
  /** Rename one tab the engine listed (`tab.rename`). */
  renameTab(tabId: string, label: string): Promise<void>;
  /** Close one pane the topology lists (`pane.close`). */
  closePane(paneId: string): Promise<void>;
  /**
   * Qualified target of a pane in the selected connection (null without one); the home screen
   * addresses its static `pane.read` snapshots with it, never with another host's identity.
   */
  target(paneId: string): QualifiedTarget | null;
  /** Routes a keydown. Returns true when the desktop handled it (caller prevents default). */
  key(event: KeyLike): boolean;
}

function asRuntimeError(error: unknown): RuntimeError {
  if (error && typeof error === "object" && "code" in error && "message" in error) return error as RuntimeError;
  return { code: "ipc_error", message: t("agents.error.ipc"), retryable: true };
}

const round = (value: number) => Math.round(value * 100) / 100;

/** Engine refusal to focus a tab that no longer exists (endpoint or snapshot guard). */
const TAB_GONE_CODES = ["tab_not_found", "tab_not_in_snapshot"];
/** Shown for three seconds when a click lands on a tab closed in another client (spec 027). */
const TAB_CLOSED_NOTICE_MS = 3000;

export function createAgentsController(bridge: AgentsBridge, onChange: (state: AgentsState) => void = () => {}): AgentsController {
  let state = initialState();
  const dispatch = (action: AgentsAction) => {
    state = reduce(state, action);
    onChange(state);
  };
  let inputChain: Promise<void> = Promise.resolve();
  // `epoch` fences everything issued during one attachment; `channel` additionally fences each
  // connect call (its overview/error and its events), so a reconnect keeps only the newest.
  let epoch = 0;
  let channel = 0;
  /** Dispatch that only lands while the attachment `own` is still current. */
  const dispatchIn = (own: number, action: AgentsAction) => {
    if (own === epoch) dispatch(action);
  };
  // Autonomy asked for the next start (spec 076): an intent, not server state, so it lives here
  // and not in the reducer; `startAgent` reads it once and clears it.
  let startAutonomy = false;
  // Forwarded lifecycle events are coalesced into one reconciliation per tick, never a timer.
  let reconcileQueued = false;
  let reconcileWaiting = false;
  let noticeTimer: ReturnType<typeof setTimeout> | null = null;

  async function layout(run: () => Promise<void>, focusPaneId?: string) {
    const own = epoch;
    dispatch({ type: "layout_started", focusPaneId });
    try {
      await run();
      dispatchIn(own, { type: "layout_done" });
    } catch (error) {
      dispatchIn(own, { type: "layout_failed", error: asRuntimeError(error) });
    }
  }

  function withConfirmed(action: (paneId: string) => Promise<void>): Promise<void> {
    const pane = confirmedPane(state);
    if (!pane) {
      dispatch({ type: "notice", text: t("agents.notice.noPane") });
      return Promise.resolve();
    }
    return action(pane);
  }

  const controller: AgentsController = {
    get state() {
      return state;
    },
    async connect(geometry) {
      const own = ++channel;
      // A channel that reported its connection ended never confirms again: its late overview,
      // late error and later events are dropped; only a new explicit connect attaches again.
      let ended = false;
      const live = () => own === channel && !ended;
      dispatch({ type: "connect_started" });
      try {
        const overview = await bridge.connect(geometry, (event) => {
          if (!live()) return;
          if (event.type === "state" && event.state === "disconnected") ended = true;
          dispatch({ type: "event", event });
          // Engine lifecycle events (tab/workspace/pane closed, created, renamed, focused) are
          // coalesced into one reconciliation per tick, never on a timer (spec 027).
          if (event.type === "structure" && live()) scheduleReconcile();
          // The connection confirmed a tab the list does not have (its engine event may never
          // arrive): the reconciled list is asked once, never on a timer.
          if (event.type === "tab_focus" && live() && event.focus.tab_id && !state.tabs.some((tab) => tab.tab_id === event.focus.tab_id)) {
            scheduleReconcile();
          }
        });
        if (live()) {
          dispatch({ type: "connected", overview });
          // An event that arrived before the initial list waited for it and applies now.
          if (reconcileWaiting) {
            reconcileWaiting = false;
            scheduleReconcile();
          }
        }
      } catch (error) {
        if (live()) dispatch({ type: "connect_failed", error: asRuntimeError(error) });
      }
    },
    invalidate() {
      epoch += 1;
      channel += 1;
      if (noticeTimer !== null) {
        clearTimeout(noticeTimer);
        noticeTimer = null;
      }
      reconcileQueued = false;
      reconcileWaiting = false;
      startAutonomy = false;
      state = initialState();
      onChange(state);
    },
    editStart: (field, value) => dispatch({ type: "start_edit", field, value }),
    setStartAutonomy(on) {
      startAutonomy = on;
    },
    async startAgent() {
      const { paneId, kind, name, busy } = state.start;
      const autonomous = startAutonomy;
      startAutonomy = false;
      if (busy || !state.capabilities?.start_agent || !kind || !name.trim()) return;
      if (state.agents.some((a) => a.pane_id === paneId)) return;
      const target = targetFor(state, paneId);
      if (!target) return;
      const own = epoch;
      dispatch({ type: "start_started" });
      try {
        dispatchIn(own, { type: "start_succeeded", agent: await bridge.startAgent(target, kind, name, autonomous) });
      } catch (error) {
        dispatchIn(own, { type: "start_failed", error: asRuntimeError(error) });
      }
    },
    editPrompt: (paneId, text) => dispatch({ type: "prompt_edit", paneId, text }),
    async sendPrompt(paneId) {
      const current = state.prompts[paneId];
      // Same gate the panel shows (capability, blocked, sending, engine readiness on this
      // connection, text): a direct call never sends what the disabled button would not.
      if (!current || promptDisabledReasonFor(state, paneId) !== null) return;
      const target = targetFor(state, paneId);
      if (!target) return;
      const resend = current.outcome === "unknown";
      const own = epoch;
      dispatch({ type: "prompt_started", paneId });
      try {
        const outcome = await bridge.prompt(target, current.text, resend);
        if (outcome.outcome === "sent") dispatchIn(own, { type: "prompt_sent", paneId, agent: outcome.agent });
        else dispatchIn(own, { type: "prompt_unknown", paneId, error: outcome.error });
      } catch (error) {
        dispatchIn(own, { type: "prompt_failed", paneId, error: asRuntimeError(error) });
      }
    },
    async openAttention(paneId) {
      if (!state.capabilities?.open_attention) return;
      const target = targetFor(state, paneId);
      if (!target) return;
      const own = epoch;
      dispatch({ type: "attention_requested", paneId });
      try {
        await bridge.openAttention(target);
      } catch (error) {
        dispatchIn(own, { type: "attention_failed", paneId, error: asRuntimeError(error) });
      }
    },
    async split(direction) {
      if (!state.capabilities?.split) return null;
      const pane = confirmedPane(state);
      if (!pane) {
        dispatch({ type: "notice", text: t("agents.notice.noPane") });
        return null;
      }
      // The engine's answer names the created pane; the topology only shows it on the next frame,
      // so a caller that must act on the pane (spec 075) uses this id and never the topology.
      let created: string | null = null;
      await layout(async () => {
        created = (await bridge.split(targetFor(state, pane)!, direction)) ?? null;
      });
      return created;
    },
    async focusPane(paneId) {
      if (!state.capabilities?.focus) return;
      const target = targetFor(state, paneId);
      if (!target) return;
      await layout(() => bridge.focusPane(target), paneId);
    },
    async setRatio(ratio) {
      const split = splitOfFocusedPane(state);
      if (!state.capabilities?.split_ratio || !split) return;
      const clamped = round(Math.min(0.9, Math.max(0.1, ratio)));
      await withConfirmed((pane) => layout(() => bridge.setSplitRatio(targetFor(state, pane)!, split.path, clamped)));
    },
    createTab() {
      if (!state.capabilities?.create_tab) return Promise.resolve();
      return withConfirmed((pane) => layout(() => bridge.createTab(targetFor(state, pane)!)));
    },
    async zoomPane(paneId, mode = "toggle") {
      // The pane must be one the server confirmed in this topology; the backend refuses the rest.
      if (!state.capabilities?.zoom || !state.topology?.panes.some((p) => p.pane_id === paneId)) return;
      const target = targetFor(state, paneId);
      if (!target) return;
      const own = epoch;
      await layout(async () => {
        const receipt = await bridge.zoomPane(target, mode);
        // Engine truth feeds the `Zoom` / `Desfazer zoom` label; a bridge of an older spec
        // answers nothing and the label keeps its previous state.
        if (receipt && typeof receipt.zoomed === "boolean") {
          dispatchIn(own, { type: "pane_zoomed", paneId, zoomed: receipt.zoomed });
        }
      });
    },
    async renamePane(paneId, label) {
      const pane = confirmedPane(state);
      const trimmed = label?.trim() || null;
      if (!pane || !paneId) {
        dispatch({ type: "notice", text: t("agents.notice.noPane") });
        return;
      }
      if (!state.capabilities?.rename_pane || typeof bridge.renamePane !== "function") return;
      const target = targetFor(state, paneId);
      if (!target) return;
      await layout(() => bridge.renamePane!(target, paneId, trimmed));
    },
    async swapPane(paneId) {
      const pane = confirmedPane(state);
      if (!pane || !paneId || pane === paneId) return;
      if (!state.capabilities?.swap || typeof bridge.swapPane !== "function") return;
      const target = targetFor(state, pane);
      if (!target) return;
      await layout(() => bridge.swapPane!(target, paneId));
    },
    async setPaneRightClick(paneId, passthrough) {
      const pane = confirmedPane(state);
      if (!pane || !paneId) {
        dispatch({ type: "notice", text: t("agents.notice.noPane") });
        return;
      }
      if (!state.capabilities?.input_set || typeof bridge.setPaneRightClick !== "function") return;
      const target = targetFor(state, paneId);
      if (!target) return;
      await layout(() => bridge.setPaneRightClick!(target, paneId, passthrough));
    },
    async focusTab(tabId) {
      const pane = confirmedPane(state);
      if (!state.capabilities?.focus_tab) return;
      if (!pane) {
        // No tab.focus can be sent without a confirmed pane: the click is answered, never mute.
        dispatch({ type: "notice", text: t("agents.notice.noPane") });
        return;
      }
      const target = targetFor(state, pane);
      if (!target) return;
      const own = epoch;
      dispatch({ type: "layout_started" });
      try {
        await bridge.focusTab(target, tabId);
        dispatchIn(own, { type: "layout_done" });
      } catch (error) {
        const failure = asRuntimeError(error);
        dispatchIn(own, { type: "layout_failed", error: failure });
        // The engine no longer has this tab (closed in another client): the bar reconciles now
        // and the status line explains why the click did nothing (spec 027).
        if (TAB_GONE_CODES.includes(failure.code)) {
          noticeFor(own, t("agents.notice.tabClosed"));
          scheduleReconcile();
        }
      }
    },
    async closeTab(tabId) {
      const pane = confirmedPane(state);
      if (!state.tabs.some((tab) => tab.tab_id === tabId) || !pane) return;
      if (state.capabilities?.close_tab === false) return;
      await layout(() => bridge.closeTab(targetFor(state, pane)!, tabId));
    },
    async renameTab(tabId, label) {
      const pane = confirmedPane(state);
      const trimmed = label.trim();
      if (!trimmed || !state.tabs.some((tab) => tab.tab_id === tabId) || !pane) return;
      if (state.capabilities?.rename_tab === false) return;
      await layout(() => bridge.renameTab(targetFor(state, pane)!, tabId, trimmed));
    },
    async closePane(paneId) {
      if (!state.topology?.panes.some((p) => p.pane_id === paneId)) return;
      const target = targetFor(state, paneId);
      if (!target) return;
      await layout(() => bridge.closePane(target));
    },
    target: (paneId) => targetFor(state, paneId),
    key(event) {
      const route = routeKey(event);
      if (route.kind === "browser") return false;
      if (route.kind === "gui") {
        void runGui(route.action);
        return true;
      }
      sendInput(route.input);
      return true;
    },
  };

  /**
   * Coalesces every forwarded lifecycle event of one tick into a single reconciliation: the
   * backend reads `tab.list` once per `overview` (spec 027), never a timer.
   */
  function scheduleReconcile(): void {
    if (reconcileQueued) return;
    reconcileQueued = true;
    queueMicrotask(() => {
      reconcileQueued = false;
      if (state.phase !== "connected") {
        // No list to reconcile against yet: waits for the connect result (once) and applies after.
        reconcileWaiting = true;
        return;
      }
      reconcileTabs();
    });
  }

  /** Reads the reconciled tab list once; a failed read leaves the shown list as it is. */
  function reconcileTabs(): void {
    const own = epoch;
    void bridge
      .overview()
      .then((overview) => dispatchIn(own, { type: "event", event: { type: "tabs", tabs: overview.tabs } }))
      .catch(() => {
        // The list stays as it is; the next forwarded event or confirmation tries again.
      });
  }

  /** Temporary status-line warning: cleared after its duration unless another notice replaced it. */
  function noticeFor(own: number, text: string): void {
    dispatchIn(own, { type: "notice", text });
    if (noticeTimer !== null) clearTimeout(noticeTimer);
    noticeTimer = setTimeout(() => {
      noticeTimer = null;
      if (own === epoch && state.notice === text) dispatch({ type: "notice", text: null });
    }, TAB_CLOSED_NOTICE_MS);
  }

  function runGui(action: GuiAction): Promise<void> {
    switch (action.type) {
      case "split":
        return controller.split(action.direction).then(() => {});
      case "new_tab":
        return controller.createTab();
      case "ratio": {
        const split = splitOfFocusedPane(state);
        return split ? controller.setRatio(split.ratio + action.delta) : Promise.resolve();
      }
      case "focus": {
        const panes = state.topology?.panes ?? [];
        const index = panes.findIndex((p) => p.pane_id === confirmedPane(state));
        if (panes.length < 2 || index < 0) return Promise.resolve();
        const next = panes[(index + action.step + panes.length) % panes.length]!;
        return controller.focusPane(next.pane_id);
      }
    }
  }

  function sendInput(input: InputDto) {
    const pane = confirmedPane(state);
    if (state.pendingFocus || !pane || !state.capabilities?.input) {
      dispatch({ type: "notice", text: t("agents.notice.inputDropped") });
      return;
    }
    const target = targetFor(state, pane);
    if (!target) return;
    const own = epoch;
    inputChain = inputChain.then(async () => {
      // Input queued for a previous attachment is dropped, never delivered to the new host.
      if (own !== epoch) return;
      try {
        await bridge.input(target, [input]);
      } catch (error) {
        dispatchIn(own, { type: "notice", text: t("agents.notice.inputRefused", { message: asRuntimeError(error).message }) });
      }
    });
  }

  return controller;
}
