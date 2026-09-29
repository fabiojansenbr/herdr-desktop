// IPC wrappers for the engine workspace actions (spec 025). Command names mirror
// `bridge::workspace_commands::COMMANDS` (checked by src-tauri/tests/registry.rs); every action
// validates endpoint/session/generation/boot/workspace in the backend and never falls back to
// another host. Focus never creates a tab, a pane or a workspace.

import { invoke } from "@tauri-apps/api/core";
import type { WorkspaceCreateInput, WorkspaceCreatedDto } from "./types";

/**
 * Spec 077 (AC-077-02): closes one tab of `endpoint` through that host's own live connection
 * (`bridge::workspace_commands::host_tab_close`). The top bar lists a host's tabs from the hub
 * snapshot whenever the agents connection carries none (spec 074), and its × has to reach the
 * same lane `workspaceClose` reaches — the agents connection knows nothing of that tab.
 *
 * Standalone (not part of `WorkspaceCommandsBridge`): the bar calls it directly, with no
 * controller in between, exactly as it calls the agents controller for a tab of the live list.
 */
export function hostTabClose(endpoint: string, tabId: string): Promise<void> {
  return invoke<void>("host_tab_close", { endpoint, tabId });
}

export interface WorkspaceCommandsBridge {
  workspaceFocus(endpoint: string, workspaceId: string): Promise<void>;
  workspaceCreate(input: WorkspaceCreateInput): Promise<WorkspaceCreatedDto>;
  workspaceRename(endpoint: string, workspaceId: string, label: string): Promise<void>;
  workspaceClose(endpoint: string, workspaceId: string): Promise<void>;
}

export function tauriWorkspaceBridge(): WorkspaceCommandsBridge {
  return {
    workspaceFocus: (endpoint, workspaceId) => invoke<void>("workspace_focus", { endpoint, workspaceId }),
    workspaceCreate: (input) =>
      invoke<WorkspaceCreatedDto>("workspace_create", {
        endpointProfileId: input.endpoint_profile_id,
        cwd: input.cwd,
        label: input.label,
        focus: input.focus,
      }),
    workspaceRename: (endpoint, workspaceId, label) =>
      invoke<void>("workspace_rename", { endpoint, workspaceId, label }),
    workspaceClose: (endpoint, workspaceId) => invoke<void>("workspace_close", { endpoint, workspaceId }),
  };
}
