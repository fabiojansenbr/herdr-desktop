// IPC wrapper for the home screen (spec 012). Command names mirror `src-tauri/src/bridge/home.rs`:
// `system_user` (the name of the greeting, no other environment) and `pane_read` (one static
// `pane.read` snapshot of one pane, addressed by the qualified target the window already has).
// The isolated preview uses the fake bridge; this module never falls back to another host.

import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import type { QualifiedTarget } from "../agents/types";

export interface SystemUserDto {
  user: string;
}

export interface PaneSnapshotDto {
  pane_id: string;
  text: string;
  revision: number;
  truncated: boolean;
}

export interface HomeBridge {
  /** System user of the greeting (read once; never a secret). */
  systemUser(): Promise<SystemUserDto>;
  /** One `pane.read`; the backend validates endpoint/session/generation/boot and pane. */
  paneRead(target: QualifiedTarget, paneId: string, lines: number): Promise<PaneSnapshotDto>;
}

/** IPC used by the Tauri bridge; injectable so unit tests never touch the window. */
export interface HomeIpc {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
}

const tauriIpc: HomeIpc = {
  invoke: (command, args) => tauriInvoke(command, args),
};

export function tauriHomeBridge(ipc: HomeIpc = tauriIpc): HomeBridge {
  return {
    systemUser: () => ipc.invoke<SystemUserDto>("system_user"),
    paneRead: (target, paneId, lines) => ipc.invoke<PaneSnapshotDto>("pane_read", { target, paneId, lines }),
  };
}
