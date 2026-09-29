// IPC wrapper for the agents module. Command names mirror `agent_commands::COMMANDS`
// (checked by src-tauri/tests/agents.rs). The commands are registered in the window by spec
// 007; the isolated preview uses the fake bridge.

import { Channel, invoke as tauriInvoke } from "@tauri-apps/api/core";
import type { GeometryDto } from "../terminal/types";
import type {
  AgentAutonomyFlagsDto,
  AgentDto,
  AgentsEvent,
  InputDto,
  Overview,
  PaneSplitReceipt,
  PaneZoomReceipt,
  PromptOutcome,
  QualifiedTarget,
  ZoomMode,
} from "./types";

export interface AgentsBridge {
  connect(geometry: GeometryDto, onEvent: (event: AgentsEvent) => void): Promise<Overview>;
  overview(): Promise<Overview>;
  detach(): Promise<Overview>;
  /** Starts `kind` in the target pane. `autonomous` (spec 076) only says whether the backend adds
   *  that kind's own autonomy flags: the WebView never sends arguments of its own. */
  startAgent(target: QualifiedTarget, kind: string, name: string, autonomous?: boolean): Promise<AgentDto>;
  prompt(target: QualifiedTarget, text: string, resendAfterUnknown: boolean): Promise<PromptOutcome>;
  openAttention(target: QualifiedTarget): Promise<void>;
  /** Splits the pane in the engine (`pane.split`) and answers the pane it created, when its
   *  reply carries it (spec 075; a fake of an earlier spec answers nothing). The local topology
   *  only learns of the pane on the next frame, so the caller must use this answer. */
  split(target: QualifiedTarget, direction: "right" | "down"): Promise<string | null | void>;
  focusPane(target: QualifiedTarget): Promise<void>;
  setSplitRatio(target: QualifiedTarget, path: boolean[], ratio: number): Promise<void>;
  input(target: QualifiedTarget, events: InputDto[]): Promise<void>;
  createTab(target: QualifiedTarget): Promise<void>;
  /** Expand/restore one pane of the active tab in the engine (`pane.zoom`); answers the engine's
   *  own `zoomed` for the pane when its reply carries it (spec 028; older fakes answer nothing). */
  zoomPane(target: QualifiedTarget, mode: ZoomMode): Promise<PaneZoomReceipt | void>;
  /** Set (or clear, with `null`) the manual name of one pane (`pane.rename`). Optional so the
   *  fixtures of earlier specs stay valid; the product bridge always implements it. */
  renamePane?(target: QualifiedTarget, paneId: string, label: string | null): Promise<void>;
  /** Exchange the right-clicked pane with the confirmed focused pane (`pane.swap`). */
  swapPane?(target: QualifiedTarget, paneId: string): Promise<void>;
  /** Choose whether right clicks go to the pane or open the Herdr menu (`pane.input.set`). */
  setPaneRightClick?(target: QualifiedTarget, paneId: string, passthrough: boolean): Promise<void>;
  focusTab(target: QualifiedTarget, tabId: string): Promise<void>;
  /** Close one tab in the engine (`tab.close`). */
  closeTab(target: QualifiedTarget, tabId: string): Promise<void>;
  /** Rename one tab in the engine (`tab.rename` / TabRenameParams). */
  renameTab(target: QualifiedTarget, tabId: string, label: string): Promise<void>;
  /** Close one pane in the engine (`pane.close`). */
  closePane(target: QualifiedTarget): Promise<void>;
}

/** IPC used by the Tauri bridge; injectable so the lifecycle order is testable without a window. */
export interface AgentsIpc {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
  /** Channel handed to `agents_connect` as `onEvent`, delivering to `handler`. */
  channel(handler: (event: AgentsEvent) => void): unknown;
}

const tauriIpc: AgentsIpc = {
  invoke: (command, args) => tauriInvoke(command, args),
  channel: (handler) => {
    const channel = new Channel<AgentsEvent>();
    channel.onmessage = handler;
    return channel;
  },
};

/**
 * Runs lifecycle calls one at a time in call order: the next one starts only after the previous
 * one settled (result or error). When idle the call is sent right away, keeping its order relative
 * to the other invokes. Nothing is retried; each caller gets its own result.
 */
function lifecycleQueue(): <T>(send: () => Promise<T>) => Promise<T> {
  let tail: Promise<void> = Promise.resolve();
  let pending = 0;
  const settled = () => {
    pending -= 1;
  };
  return <T>(send: () => Promise<T>) => {
    const result = pending === 0 ? new Promise<T>((resolve) => resolve(send())) : tail.then(send);
    pending += 1;
    tail = result.then(settled, settled);
    return result;
  };
}

/**
 * The backend runs agents commands off the GUI thread in the order their tasks first run, so two
 * lifecycle invokes sent together could swap. `connect`/`detach` are therefore sent one at a time,
 * in JS call order; overview and qualified actions are sent immediately and never wait for them.
 */
export function tauriAgentsBridge(ipc: AgentsIpc = tauriIpc): AgentsBridge {
  const { invoke } = ipc;
  const lifecycle = lifecycleQueue();
  return {
    connect: (geometry, onEvent) =>
      lifecycle(() => invoke<Overview>("agents_connect", { geometry, onEvent: ipc.channel(onEvent) })),
    // One `tab.list` read on the backend before it answers (spec 027): the caller uses it only
    // for reconciliation, never on render or a timer.
    overview: () => invoke<Overview>("agents_overview", { reconcile: true }),
    detach: () => lifecycle(() => invoke<Overview>("agents_detach")),
    startAgent: (target, kind, name, autonomous = false) =>
      invoke<AgentDto>("agent_start", { target, kind, name, autonomous }),
    prompt: (target, text, resendAfterUnknown) => invoke<PromptOutcome>("agent_prompt", { target, text, resendAfterUnknown }),
    openAttention: (target) => invoke<void>("agent_open_attention", { target }),
    split: async (target, direction) => (await invoke<PaneSplitReceipt>("pane_split", { target, direction }))?.pane_id ?? null,
    focusPane: (target) => invoke<void>("pane_focus", { target }),
    setSplitRatio: (target, path, ratio) => invoke<void>("pane_set_split_ratio", { target, path, ratio }),
    input: (target, events) => invoke<void>("pane_input", { target, events }),
    createTab: (target) => invoke<void>("tab_create", { target }),
    zoomPane: (target, mode) => invoke<PaneZoomReceipt>("pane_zoom", { target, mode }),
    renamePane: (target, paneId, label) => invoke<void>("pane_rename", { target, paneId, label }),
    swapPane: (target, paneId) => invoke<void>("pane_swap", { target, paneId }),
    setPaneRightClick: (target, paneId, passthrough) => invoke<void>("pane_input_set", { target, paneId, passthrough }),
    focusTab: (target, tabId) => invoke<void>("tab_focus", { target, tabId }),
    closeTab: (target, tabId) => invoke<void>("tab_close", { target, tabId }),
    renameTab: (target, tabId, label) => invoke<void>("tab_rename", { target, tabId, label }),
    closePane: (target) => invoke<void>("pane_close", { target }),
  };
}

/**
 * Autonomy flags per kind, read from the backend table (spec 076) so the "New agent" popup can
 * name the flag each row would use. A read: it starts nothing and takes no arguments back.
 */
export function agentAutonomyFlags(kinds: readonly string[]): Promise<AgentAutonomyFlagsDto[]> {
  return tauriIpc.invoke<AgentAutonomyFlagsDto[]>("agent_autonomy_flags", { kinds: [...kinds] });
}
