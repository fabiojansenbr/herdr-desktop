// Spec 010 — explicit shortcut list (AC-010-03, premise 2): Ctrl+K opens the palette only with the
// focus outside the terminal; every other chord, and Ctrl+K inside the terminal, is not claimed.
import { describe, expect, it } from "vitest";
import { isTerminalTarget, platformOf, resolveShortcut, shortcutLabel, SHORTCUTS, type KeyLike } from "./shortcuts";

const outside = { closest: () => null };
const insideTerminal = { closest: (selector: string) => (selector.includes(".ime-target") ? {} : null) };

function key(k: string, mods: Partial<KeyLike> = {}, target: KeyLike["target"] = outside): KeyLike {
  return { key: k, ctrlKey: false, metaKey: false, shiftKey: false, altKey: false, isComposing: false, repeat: false, target, ...mods };
}

describe("shortcuts", () => {
  // Would catch: the palette not opening from a focused frame control on Linux/Windows, or macOS
  // bound to Ctrl instead of Command.
  it("Ctrl+K (Cmd+K on macOS) outside the terminal resolves to the palette", () => {
    expect(resolveShortcut(key("k", { ctrlKey: true }), "linux")).toBe("palette");
    expect(resolveShortcut(key("K", { ctrlKey: true }), "windows")).toBe("palette");
    expect(resolveShortcut(key("k", { metaKey: true }), "macos")).toBe("palette");
    expect(resolveShortcut(key("k", { ctrlKey: true }), "macos")).toBeNull();
    expect(resolveShortcut(key("k", { ctrlKey: true }, null), "linux")).toBe("palette");
  });

  // Would catch: intercepting Ctrl+K typed in the terminal (the shell's kill-line) or while the
  // IME composes, which would drop \x0b before the PTY.
  it("never claims Ctrl+K inside the terminal or during composition", () => {
    expect(resolveShortcut(key("k", { ctrlKey: true }, insideTerminal), "linux")).toBeNull();
    expect(resolveShortcut(key("k", { ctrlKey: true, isComposing: true }), "linux")).toBeNull();
  });

  // Would catch: reserving whole Ctrl/Ctrl+Shift families (Ctrl+C, Ctrl+Shift+K, Alt+K, auto-repeat).
  it("claims no other chord", () => {
    for (const event of [
      key("c", { ctrlKey: true }),
      key("k", { ctrlKey: true, shiftKey: true }),
      key("k", { ctrlKey: true, altKey: true }),
      key("k", { altKey: true }),
      key("k"),
      key("v", { ctrlKey: true, shiftKey: true }),
      key("k", { ctrlKey: true, metaKey: true }),
      key("k", { ctrlKey: true, repeat: true }),
    ]) {
      expect(resolveShortcut(event, "linux")).toBeNull();
    }
  });

  // Would catch: a hidden shortcut list (the guide asks for an explicit per-platform list) or a
  // terminal-owned chord claimed by the window.
  it("lists the shortcuts explicitly per platform and context", () => {
    expect(SHORTCUTS.linux.filter((s) => s.context === "window").map((s) => s.id)).toEqual(["palette", "sidebar", "new-agent", "new-tab"]);
    expect(resolveShortcut(key("t", { ctrlKey: true }), "linux")).toBe("new-tab");
    expect(SHORTCUTS.macos.find((s) => s.id === "new-tab")).toMatchObject({ key: "t", meta: true, ctrl: false });
    expect(SHORTCUTS.macos.find((s) => s.id === "palette")).toMatchObject({ key: "k", meta: true, ctrl: false });
    expect(SHORTCUTS.linux.filter((s) => s.context === "terminal").map((s) => s.label)).toEqual(["Ctrl+Shift+C", "Ctrl+Shift+V", "Ctrl+Shift+F6"]);
    expect(shortcutLabel("palette", "linux")).toBe("Ctrl K");
    expect(shortcutLabel("palette", "macos")).toBe("⌘ K");
    expect(platformOf("Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0)")).toBe("macos");
    expect(platformOf("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")).toBe("windows");
    expect(platformOf("Mozilla/5.0 (X11; Linux x86_64)")).toBe("linux");
  });

  it("recognises the terminal input target by its element", () => {
    expect(isTerminalTarget(insideTerminal)).toBe(true);
    expect(isTerminalTarget(outside)).toBe(false);
    expect(isTerminalTarget(null)).toBe(false);
  });
});
