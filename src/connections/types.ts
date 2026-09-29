// DTOs shared with src-tauri/src/connections/{hub,profiles,commands}.rs.

import type { RuntimeError } from "../terminal/types";

export type { RuntimeError };

export type LinkPhase = "offline" | "connecting" | "online" | "reconnecting" | "attention";

export type AttentionReason =
  | "host_key_unknown"
  | "host_key_changed"
  | "authentication_required"
  | "ssh_unavailable"
  | "herdr_missing"
  | "herdr_outdated"
  | "server_not_running"
  | "server_incompatible";

export type InputBlock =
  | "host_offline"
  | "host_connecting"
  | "host_reconnecting"
  | "host_needs_attention"
  | "no_snapshot"
  | "pane_not_in_snapshot"
  | "surface_hidden"
  | "no_surface"
  | "surface_stale"
  | "surface_from_previous_boot"
  | "surface_from_previous_connection";

/** Endpoint + session + connection generation + boot + pane: the only way to address input. */
export interface QualifiedTarget {
  endpoint: string;
  session: string;
  connection_generation: number;
  boot_id: string;
  workspace_id?: string;
  pane_id: string;
}

export interface HostTabDto {
  tab_id: string;
  workspace_id: string;
  number: number;
  label: string;
  custom_label: boolean;
  focused: boolean;
  zoomed: boolean;
  pane_count: number;
  agent_status: string;
}

export interface HostAgentDto {
  pane_id: string;
  workspace_id: string;
  tab_id: string;
  name: string | null;
  display_agent: string | null;
  agent: string | null;
  title: string | null;
  terminal_title: string | null;
  terminal_title_stripped: string | null;
  agent_status: string;
  focused: boolean;
}

export interface PaneDto {
  pane_id: string;
  workspace_id: string;
  tab_id?: string | null;
  cwd?: string | null;
  foreground_cwd?: string | null;
  title?: string | null;
  terminal_title?: string | null;
  agent?: string | null;
  agent_status?: string | null;
  focused: boolean;
  input_enabled: boolean;
  input_block: InputBlock | null;
  target: QualifiedTarget | null;
}

export interface ActionRecord {
  id: number;
  method: string;
  generation: number;
  outcome: "pending" | "succeeded" | "failed" | "unknown";
  error?: RuntimeError;
}

/**
 * One workspace of a host, projected from the engine's own workspace list (spec 025).
 * `branch` and `cwd` come from the workspace's focused pane (foreground cwd), as the TUI does;
 * `cwd` is the root the desktop groups by. Order the rows by `number`.
 */
export interface HostWorkspaceDto {
  workspace_id: string;
  number: number;
  label: string;
  focused: boolean;
  tab_count: number;
  pane_count: number;
  active_tab_id: string;
  agent_status: "working" | "blocked" | "idle" | "done" | "unknown";
  cwd: string | null;
  /** Every cwd the engine reported for the workspace (focused pane first); spec 025 AC-025-05. */
  cwds?: string[];
  branch: string | null;
}

export interface HostDto {
  endpoint: string;
  label: string;
  kind: "local" | "ssh";
  session: string;
  target: string | null;
  visible: boolean;
  phase: LinkPhase;
  phase_label: string;
  attempt: number;
  retry_in_ms: number | null;
  cancelled: boolean;
  attention: AttentionReason | null;
  guidance: string | null;
  connection_error: RuntimeError | null;
  action_error: RuntimeError | null;
  generation: number | null;
  boot_id: string | null;
  /** Branch of the focused tab's workspace from the engine snapshot (spec 010); null when unknown. */
  branch?: string | null;
  /** Server version of the installed connection's negotiated welcome (spec 010); null without one. */
  server_version?: string | null;
  /**
   * Remote herdr binary chosen for this connection (spec 029, AC-029-01): the path discovered by
   * the same candidates as the TUI and the client version of that binary; null for Local or
   * without a connection.
   */
  herdr_binary?: RemoteBinaryDto | null;
  /** Latency in ms measured during the latest handshake (spec 011); null when unknown or offline. */
  latency_ms?: number | null;
  /** The engine's own workspaces of this host (spec 025); cached while offline, empty without one. */
  workspaces?: HostWorkspaceDto[];
  cached: boolean;
  api?: boolean;
  surface: unknown;
  screen: string[];
  tabs?: HostTabDto[];
  panes: PaneDto[];
  agents?: HostAgentDto[];
  actions: ActionRecord[];
}

/** Discovered remote binary (spec 029). `path` is shown in the host tooltip. */
export interface RemoteBinaryDto {
  path: string;
  version: string | null;
}

export interface HubSnapshot {
  revision: number;
  hosts: HostDto[];
}

export interface SshProfile {
  id: string;
  label: string;
  target: string;
  port: number | null;
  session: string;
  /** Explicit authentication choice of the dialog: "ssh-agent", or absent for the key files. */
  auth?: string | null;
  imported_from?: string;
  /** "Conectar ao abrir" of the host menu (spec 058); on by default. */
  connect_on_open: boolean;
  /** Whether this host was connected when the app was last used (spec 058). */
  resume_on_open: boolean;
}

export interface ConnectionsView {
  hub: HubSnapshot;
  profiles: SshProfile[];
  store_error: RuntimeError | null;
}

export interface ImportReport {
  imported: string[];
  skipped: { label: string; code: string }[];
  already_present: number;
}

export interface ImportView {
  report: ImportReport;
  view: ConnectionsView;
}

/** Payload of connection_profile_save. */
export interface SshProfileDraft {
  id: string | null;
  label: string;
  target: string;
  port: number | null;
  session: string;
  /** "ssh-agent" or "key" (OpenSSH default key files). */
  auth?: string | null;
}

/** What the form holds while the user types. */
export interface DraftForm {
  id: string | null;
  label: string;
  target: string;
  port: string;
  session: string;
  /** "ssh-agent" or "key" (OpenSSH default key files). */
  auth: string;
}

export interface WorkspaceDto {
  workspace_id: string;
  label: string;
}
