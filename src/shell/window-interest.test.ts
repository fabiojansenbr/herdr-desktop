// Spec 019 AC-019-03 — keyboard blur is not a surface hide. Would catch: treating window blur
// as `surface_active=false`, which lets the remaining client (the TUI) resize the shared pane.
import { describe, expect, it } from "vitest";
import {
  applyWindowSurfaceAction,
  bindNativeWindowSurface,
  INITIAL_WINDOW_SURFACE,
  surfaceActiveFromWindow,
  type WindowSurfaceState,
} from "./interest";

describe("window surface lease (AC-019-03)", () => {
  it("keyboard blur does not drop the surface lease", () => {
    const blurred = applyWindowSurfaceAction(INITIAL_WINDOW_SURFACE, { kind: "keyboard-focus", focused: false });
    expect(blurred.keyboardFocus).toBe(false);
    expect(surfaceActiveFromWindow(blurred)).toBe(true);
    expect(surfaceActiveFromWindow(applyWindowSurfaceAction(blurred, { kind: "keyboard-focus", focused: true }))).toBe(true);
  });

  it("document hidden and native minimize drop the lease; restore brings it back", () => {
    const hidden = applyWindowSurfaceAction(INITIAL_WINDOW_SURFACE, { kind: "visibility", hidden: true });
    expect(surfaceActiveFromWindow(hidden)).toBe(false);
    const shown = applyWindowSurfaceAction(hidden, { kind: "visibility", hidden: false });
    expect(surfaceActiveFromWindow(shown)).toBe(true);

    const minimized = applyWindowSurfaceAction(INITIAL_WINDOW_SURFACE, { kind: "minimized", minimized: true });
    expect(surfaceActiveFromWindow(minimized)).toBe(false);
    expect(surfaceActiveFromWindow(applyWindowSurfaceAction(minimized, { kind: "minimized", minimized: false }))).toBe(true);
  });

  it("a native-visible window keeps the lease even when document.hidden fired (WebView blur)", () => {
    let state: WindowSurfaceState = applyWindowSurfaceAction(INITIAL_WINDOW_SURFACE, { kind: "native-visible", visible: true });
    state = applyWindowSurfaceAction(state, { kind: "keyboard-focus", focused: false });
    state = applyWindowSurfaceAction(state, { kind: "visibility", hidden: true });
    expect(surfaceActiveFromWindow(state)).toBe(true);
    state = applyWindowSurfaceAction(state, { kind: "native-visible", visible: false });
    expect(surfaceActiveFromWindow(state)).toBe(false);
  });

  it("native focus changes are keyboard only; minimize drops the lease", async () => {
    const actions: { kind: string }[] = [];
    const unbind = await bindNativeWindowSurface((action) => actions.push(action), async () => ({
      isMinimized: async () => false,
      isVisible: async () => true,
      onFocusChanged: async (handler) => {
        handler({ payload: false });
        return () => {};
      },
    }));
    unbind();
    expect(actions.some((a) => a.kind === "keyboard-focus")).toBe(true);
    expect(actions.some((a) => a.kind === "native-visible")).toBe(true);
    expect(actions.filter((a) => a.kind === "visibility")).toHaveLength(0);
  });
});
