// IPC wrapper for the connections module. Command names mirror
// `connections::commands::COMMANDS` (checked by src-tauri/tests/connections.rs). The commands
// are registered in the window by spec 007; the preview uses the fake bridge.

import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import type { ConnectionsView, ImportView, QualifiedTarget, SshProfileDraft, WorkspaceDto } from "./types";

export interface ConnectionsBridge {
  list(): Promise<ConnectionsView>;
  /** Resolves when the backend revision differs from `revision` (or after a short timeout). */
  watch(revision: number): Promise<ConnectionsView>;
  saveProfile(draft: SshProfileDraft, connect: boolean): Promise<ConnectionsView>;
  importProfiles(): Promise<ImportView>;
  connect(endpoint: string): Promise<ConnectionsView>;
  cancel(endpoint: string): Promise<ConnectionsView>;
  /** Closes only the desktop connection of a saved SSH host (spec 029, AC-029-03). */
  disconnect(endpoint: string): Promise<ConnectionsView>;
  /** Explicit new attempt for a disconnected/failed SSH host. */
  reconnect(endpoint: string): Promise<ConnectionsView>;
  /** Removes the saved profile and its host from the desktop (never from the engine/remote). */
  removeProfile(endpoint: string): Promise<ConnectionsView>;
  sendText(target: QualifiedTarget, text: string, submit: boolean): Promise<void>;
  workspaces(endpoint: string): Promise<WorkspaceDto[]>;
  /** "Conectar ao abrir" of a saved SSH profile (spec 058, AC-058-03). */
  setConnectOnOpen(endpoint: string, enabled: boolean): Promise<ConnectionsView>;
}

/** IPC used by the Tauri bridge; injectable so the call order is testable without a window. */
export interface ConnectionsIpc {
  invoke: <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
}

const tauriIpc: ConnectionsIpc = {
  invoke: (command, args) => tauriInvoke(command, args),
};

/**
 * Keyed call order. A call waits until every earlier call registered under one of its `waitOn`
 * keys settled (result or error), then is sent; it registers itself under `register`. A call
 * with nothing to wait for is sent right away, keeping its order relative to other invokes.
 * Nothing is retried; each caller gets its own result.
 */
function orderedCalls() {
  const tails = new Map<string, Promise<void>>();
  return <T>(waitOn: string[], register: string[], send: () => Promise<T>): Promise<T> => {
    const earlier = waitOn.flatMap((key) => {
      const tail = tails.get(key);
      return tail ? [tail] : [];
    });
    const result =
      earlier.length === 0 ? new Promise<T>((resolve) => resolve(send())) : Promise.all(earlier).then(send);
    const settled = result.then(
      () => undefined,
      () => undefined,
    );
    for (const key of register) {
      tails.set(key, settled);
      void settled.then(() => {
        if (tails.get(key) === settled) tails.delete(key);
      });
    }
    return result;
  };
}

const host = (endpoint: string) => `host:${endpoint}`;
const input = (endpoint: string) => `input:${endpoint}`;
const PROFILES = "profiles";

/**
 * The backend runs connections commands off the GUI thread with no shared lane, in the order
 * their tasks first run, so conflicting invokes sent together could swap. The bridge sends them
 * one at a time in JS call order:
 * - connect/cancel of one host, and the save of that host's profile (which may connect it);
 * - profile save/import (one store);
 * - text to one host, after that host's earlier lifecycle calls; a later cancel never waits for
 *   pending text, so a text still queued when the cancel runs is refused by the backend.
 * list/watch/workspaces and calls of other hosts are sent immediately. Arguments are copied at
 * call time, so a queued call keeps the identity and draft it was made with.
 */
export function tauriConnectionsBridge(ipc: ConnectionsIpc = tauriIpc): ConnectionsBridge {
  const { invoke } = ipc;
  const ordered = orderedCalls();
  return {
    list: () => invoke<ConnectionsView>("connections_list"),
    watch: (revision) => invoke<ConnectionsView>("connections_watch", { revision }),
    saveProfile: (draft, connect) => {
      const copy: SshProfileDraft = { ...draft };
      const keys = copy.id === null ? [PROFILES] : [PROFILES, host(copy.id)];
      return ordered(keys, keys, () => invoke<ConnectionsView>("connection_profile_save", { draft: copy, connect }));
    },
    importProfiles: () => ordered([PROFILES], [PROFILES], () => invoke<ImportView>("connection_profiles_import")),
    connect: (endpoint) =>
      ordered([host(endpoint)], [host(endpoint)], () => invoke<ConnectionsView>("connection_connect", { endpoint })),
    cancel: (endpoint) =>
      ordered([host(endpoint)], [host(endpoint)], () => invoke<ConnectionsView>("connection_cancel", { endpoint })),
    disconnect: (endpoint) =>
      ordered([host(endpoint)], [host(endpoint)], () => invoke<ConnectionsView>("connection_disconnect", { endpoint })),
    reconnect: (endpoint) =>
      ordered([host(endpoint)], [host(endpoint)], () => invoke<ConnectionsView>("connection_reconnect", { endpoint })),
    removeProfile: (endpoint) =>
      ordered([host(endpoint), PROFILES], [host(endpoint), PROFILES], () =>
        invoke<ConnectionsView>("connection_remove", { endpoint }),
      ),
    sendText: (target, text, submit) => {
      const copy: QualifiedTarget = { ...target };
      return ordered([host(copy.endpoint), input(copy.endpoint)], [input(copy.endpoint)], () =>
        invoke<void>("connection_send_text", { target: copy, text, submit }),
      );
    },
    workspaces: (endpoint) => invoke<WorkspaceDto[]>("connection_workspaces", { endpoint }),
    // Writes the profile store, so it keeps the order of the host's own lifecycle calls and of
    // the other profile writes.
    setConnectOnOpen: (endpoint, enabled) =>
      ordered([host(endpoint), PROFILES], [host(endpoint), PROFILES], () =>
        invoke<ConnectionsView>("connections_set_connect_on_open", { endpoint, enabled }),
      ),
  };
}
