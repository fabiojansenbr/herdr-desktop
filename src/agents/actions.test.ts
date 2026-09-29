// Spec 004 — AC-004-03: GUI shortcuts vs terminal keys (src/terminal/actions.ts).
import { describe, expect, it } from "vitest";
import { GUI_SHORTCUTS, routeKey, type KeyLike } from "../terminal/actions";
import { MOD_CONTROL } from "../terminal/types";

function key(k: string, mods: Partial<KeyLike> = {}): KeyLike {
  return { key: k, ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, ...mods };
}

describe("key routing", () => {
  // Would catch: a GUI shortcut also forwarded to the PTY (leak) or not recognised at all.
  it("consumes GUI shortcuts without producing terminal input", () => {
    expect(routeKey(key("D", { ctrlKey: true, shiftKey: true }))).toEqual({ kind: "gui", action: { type: "split", direction: "right" } });
    expect(routeKey(key("E", { ctrlKey: true, shiftKey: true }))).toEqual({ kind: "gui", action: { type: "split", direction: "down" } });
    expect(routeKey(key("ArrowRight", { ctrlKey: true, shiftKey: true }))).toEqual({ kind: "gui", action: { type: "focus", step: 1 } });
    expect(routeKey(key("ArrowLeft", { ctrlKey: true, shiftKey: true }))).toEqual({ kind: "gui", action: { type: "focus", step: -1 } });
    expect(routeKey(key("ArrowLeft", { altKey: true, shiftKey: true }))).toEqual({ kind: "gui", action: { type: "ratio", delta: -0.1 } });
    expect(routeKey(key("ArrowRight", { altKey: true, shiftKey: true }))).toEqual({ kind: "gui", action: { type: "ratio", delta: 0.1 } });
    expect(routeKey(key("T", { ctrlKey: true, shiftKey: true }))).toEqual({ kind: "gui", action: { type: "new_tab" } });
    for (const shortcut of GUI_SHORTCUTS) {
      expect(routeKey(shortcut.example).kind).toBe("gui");
      expect(shortcut.label.length).toBeGreaterThan(0);
    }
  });

  // Would catch: terminal shortcuts swallowed by the GUI table (Ctrl+E, Ctrl+D, Alt+B) or
  // mapped to a different key.
  it("routes terminal shortcuts and text to the terminal exactly as typed", () => {
    expect(routeKey(key("e", { ctrlKey: true }))).toEqual({ kind: "terminal", input: { kind: "key", code: "Char", modifiers: MOD_CONTROL, ch: "e" } });
    expect(routeKey(key("d", { ctrlKey: true }))).toEqual({ kind: "terminal", input: { kind: "key", code: "Char", modifiers: MOD_CONTROL, ch: "d" } });
    expect(routeKey(key("y")).kind).toBe("terminal");
    expect(routeKey(key("y"))).toEqual({ kind: "terminal", input: { kind: "text", text: "y" } });
    expect(routeKey(key("Enter"))).toEqual({ kind: "terminal", input: { kind: "key", code: "Enter", modifiers: 0 } });
    expect(routeKey(key("ArrowRight", { shiftKey: true })).kind).toBe("terminal");
    expect(routeKey(key("ArrowLeft", { altKey: true })).kind).toBe("terminal");
  });

  // Would catch: IME composition or bare modifiers sent to the PTY.
  it("leaves composition and bare modifiers to the browser", () => {
    expect(routeKey(key("a", { isComposing: true }))).toEqual({ kind: "browser" });
    expect(routeKey(key("Shift", { shiftKey: true }))).toEqual({ kind: "browser" });
    expect(routeKey(key("D", { ctrlKey: true, shiftKey: true, isComposing: true }))).toEqual({ kind: "browser" });
  });
});
