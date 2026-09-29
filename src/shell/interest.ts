// Spec 019 AC-019-03 — window surface lease vs keyboard focus.
//
// The Herdr server sizes a shared pane from the client that holds the geometry controller
// (`tab_geometry_controllers`). `client_shell.surface.set` / `shell_surface_active` is that
// lease: dropping it (`surface_active=false`) lets the remaining active client (the TUI)
// resize the pane. `ClientShellFocus` only moves the cursor/input and must not drop the lease
// while this window is still visible. Keyboard blur is therefore not hiding.

export interface WindowSurfaceState {
  /** Outer keyboard focus of this window (maps to `ClientShellFocus` / `terminal_focus`). */
  keyboardFocus: boolean;
  /** `document.visibilityState === "hidden"` — may fire on mere blur in some WebViews. */
  documentHidden: boolean;
  /** Tauri `isMinimized()`. */
  minimized: boolean;
  /**
   * Native window visibility (`isVisible()`). `null` when the native API is not available
   * (unit tests, first paint): fall back to `documentHidden`.
   */
  nativeVisible: boolean | null;
}

export const INITIAL_WINDOW_SURFACE: WindowSurfaceState = {
  keyboardFocus: true,
  documentHidden: false,
  minimized: false,
  nativeVisible: null,
};

export type WindowSurfaceAction =
  | { kind: "keyboard-focus"; focused: boolean }
  | { kind: "visibility"; hidden: boolean }
  | { kind: "minimized"; minimized: boolean }
  | { kind: "native-visible"; visible: boolean };

/** Keyboard focus never participates in the surface lease. */
export function surfaceActiveFromWindow(state: WindowSurfaceState): boolean {
  if (state.minimized) return false;
  if (state.nativeVisible === false) return false;
  if (state.nativeVisible === true) return true;
  return !state.documentHidden;
}

export function applyWindowSurfaceAction(state: WindowSurfaceState, action: WindowSurfaceAction): WindowSurfaceState {
  switch (action.kind) {
    case "keyboard-focus":
      return { ...state, keyboardFocus: action.focused };
    case "visibility":
      return { ...state, documentHidden: action.hidden };
    case "minimized":
      return { ...state, minimized: action.minimized };
    case "native-visible":
      return { ...state, nativeVisible: action.visible };
  }
}

type NativeWindow = {
  isMinimized(): Promise<boolean>;
  isVisible(): Promise<boolean>;
  onFocusChanged(handler: (event: { payload: boolean }) => void): Promise<() => void>;
};

/** Native minimize/visibility from Tauri; focus changes are keyboard only (never the lease). */
export async function bindNativeWindowSurface(
  apply: (action: WindowSurfaceAction) => void,
  load: () => Promise<NativeWindow | null> = async () => {
    try {
      const mod = await import("@tauri-apps/api/window");
      return mod.getCurrentWindow();
    } catch {
      return null;
    }
  },
): Promise<() => void> {
  const current = await load();
  if (!current) return () => {};
  const publish = async () => {
    apply({ kind: "minimized", minimized: await current.isMinimized() });
    apply({ kind: "native-visible", visible: await current.isVisible() });
  };
  await publish();
  const unlisten = await current.onFocusChanged(async ({ payload: focused }) => {
    apply({ kind: "keyboard-focus", focused });
    await publish();
  });
  return unlisten;
}
