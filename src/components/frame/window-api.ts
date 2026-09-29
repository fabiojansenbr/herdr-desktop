// Window controls of the design title bar (spec 017, AC-017-01). Injected in tests; in the
// product they call the Tauri window API. System decorations stay off (`decorations: false`).

export interface WindowApi {
  minimize(): void | Promise<void>;
  toggleMaximize(): void | Promise<void>;
  close(): void | Promise<void>;
}

export type WindowControl = "minimize" | "maximize" | "close";

/** Dispatches one title-bar control to the injected (or live) window API. */
export function runWindowControl(api: WindowApi, action: WindowControl): void | Promise<void> {
  if (action === "minimize") return api.minimize();
  if (action === "maximize") return api.toggleMaximize();
  return api.close();
}

export async function tauriWindowApi(): Promise<WindowApi> {
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  const current = getCurrentWindow();
  return {
    minimize: () => current.minimize(),
    toggleMaximize: () => current.toggleMaximize(),
    close: () => current.close(),
  };
}
