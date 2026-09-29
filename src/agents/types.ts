// DTOs shared with src-tauri/src/bridge/agent_commands.rs (spec 004).

import type { InputDto, RuntimeError } from "../terminal/types";

export type { InputDto, RuntimeError };

export type AgentStatus = "working" | "blocked" | "idle" | "done" | "unknown";

export interface AgentDto {
  pane_id: string;
  workspace_id: string;
  tab_id: string;
  name: string | null;
  kind: string | null;
  status: AgentStatus;
  launch_pending: boolean;
  ready: boolean;
  focused: boolean;
  /** Engine's `terminal_title_stripped` of the agent's pane (spec 014); never composed here. */
  terminal_title?: string | null;
  /** Engine's `state_change_seq`: it changes when the engine publishes another state. */
  state_change_seq?: number;
  /**
   * Last line of the pane's detection snapshot, read by the backend once when the engine
   * reported this agent waiting for the user; absent until such a transition is observed.
   */
  detection_last_line?: string | null;
}

/** Autonomy flags of one kind (`agent_autonomy_flags`, spec 076): the arguments an autonomous
 *  start would carry, straight from the backend table. Empty means the kind has no auto mode. */
export interface AgentAutonomyFlagsDto {
  kind: string;
  flags: string[];
}

export interface Capabilities {
  list_agents: boolean;
  start_agent: boolean;
  send_prompt: boolean;
  open_attention: boolean;
  split: boolean;
  focus: boolean;
  split_ratio: boolean;
  input: boolean;
  create_tab: boolean;
  focus_tab: boolean;
  /** `tab.close` announced by the endpoint. */
  close_tab?: boolean;
  /** `tab.rename` announced by the endpoint. */
  rename_tab?: boolean;
  /** `pane.zoom` announced by the endpoint (expand/restore one pane of the tab). */
  zoom?: boolean;
  /** `pane.rename` announced by the endpoint (set/clear the manual name of one pane). */
  rename_pane?: boolean;
  /** `pane.swap` announced by the endpoint (exchange two panes of the same tab). */
  swap?: boolean;
  /** `pane.input.set` announced by the endpoint (right clicks to the pane or the Herdr menu). */
  input_set?: boolean;
  /** `pane.close` announced by the endpoint (close one pane). */
  close_pane?: boolean;
}

/** `pane.zoom` result carried back to the window (`PaneZoomReceipt` in Rust, spec 028). */
export interface PaneZoomReceipt {
  pane_id: string;
  /** Engine's own `zoom.zoomed`; absent when its reply did not carry it. */
  zoomed?: boolean;
}

/** `pane.split` result carried back to the window (`PaneSplitReceipt` in Rust, spec 075). */
export interface PaneSplitReceipt {
  /** Pane the engine created; null when its reply did not carry one (nothing is guessed). */
  pane_id: string | null;
}

export interface PaneBox {
  pane_id: string;
  x: number;
  y: number;
  width: number;
  height: number;
  focused: boolean;
  /** Working directory the engine reported for this pane (`pane.list`), when known. */
  cwd?: string | null;
  /** Manual name the engine reported for this pane (`pane.list`), when it has one. */
  label?: string | null;
}

export interface SplitBox {
  path: boolean[];
  direction: "right" | "down";
  pos: number;
  x: number;
  y: number;
  width: number;
  height: number;
  ratio: number;
}

export interface Topology {
  revision: number;
  width: number;
  height: number;
  focused_pane_id: string | null;
  panes: PaneBox[];
  splits: SplitBox[];
}

export interface TabDto {
  tab_id: string;
  workspace_id: string;
  label: string;
  /** Engine `tab.list` number; used when `label` is empty. */
  number?: number;
  focused: boolean;
  /** Panes of this tab as the engine counted them (`tab.list`); the client sees only the active tab.
   * The desktop DTO always sends it (src-tauri/tests/center_runtime.rs); optional for the fixtures
   * of the older specs, which predate the field. */
  pane_count?: number;
  /** Aggregated agent state of the tab, published by the engine (`tab.list`). */
  agent_status?: AgentStatus;
}

export interface LiveIdentity {
  endpoint: string;
  session: string;
  connection_generation: number;
  boot_id: string;
}

/** Tab focused by one connection: its snapshot revision matches the committed full surface. */
export interface TabFocus extends LiveIdentity {
  revision: number;
  tab_id: string | null;
}

/** `pane.zoom` modes the engine accepts. */
export type ZoomMode = "toggle" | "on" | "off";

export interface QualifiedTarget {
  endpoint: string;
  session: string;
  connection_generation: number;
  boot_id: string;
  workspace_id?: string;
  pane_id: string;
}

export type PromptOutcome = { outcome: "sent"; agent: AgentDto } | { outcome: "unknown"; error: RuntimeError };

export interface Overview {
  state: string;
  session: string | null;
  identity: LiveIdentity | null;
  server_version: string | null;
  capabilities: Capabilities | null;
  kinds: string[];
  agents: AgentDto[];
  tabs: TabDto[];
  topology: Topology | null;
  /** Composed window only; absent/null in the standalone panel. */
  tab_focus?: TabFocus | null;
  error: RuntimeError | null;
}

export type AgentsEvent =
  | { type: "identity"; identity: LiveIdentity }
  | { type: "topology"; topology: Topology }
  | { type: "agents"; agents: AgentDto[] }
  | { type: "tabs"; tabs: TabDto[] }
  | { type: "tab_focus"; focus: TabFocus }
  /**
   * Engine tab/workspace/pane lifecycle event forwarded by the backend (spec 027). It carries no
   * list: the controller coalesces it and reconciles with one `overview` (one `tab.list`).
   */
  | { type: "structure"; events: string[] }
  | { type: "state"; state: string; error: RuntimeError | null };
