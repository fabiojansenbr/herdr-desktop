// Input mapping for AC-001-01: typed text becomes TextCommit, Enter/controls become Key
// events with crossterm bits, paste is one Paste event, IME composition is not forwarded.
import { describe, expect, it } from "vitest";
import { keyEventToInput, pasteToInput } from "./input";
import { MOD_ALT, MOD_CONTROL, MOD_SHIFT } from "./types";

const key = (k: string, mods: Partial<{ ctrlKey: boolean; altKey: boolean; shiftKey: boolean; metaKey: boolean; isComposing: boolean }> = {}) => ({
  key: k,
  ctrlKey: false,
  altKey: false,
  shiftKey: false,
  metaKey: false,
  ...mods,
});

describe("keyEventToInput", () => {
  // Would catch: printable keys sent as Key events (breaking dead keys/IME), or Enter sent as text.
  it("maps printable characters to text and Enter to a key press", () => {
    expect(keyEventToInput(key("a"))).toEqual({ kind: "text", text: "a" });
    expect(keyEventToInput(key("á"))).toEqual({ kind: "text", text: "á" });
    expect(keyEventToInput(key("界"))).toEqual({ kind: "text", text: "界" });
    expect(keyEventToInput(key("A", { shiftKey: true }))).toEqual({ kind: "text", text: "A" });
    expect(keyEventToInput(key("Enter"))).toEqual({ kind: "key", code: "Enter", modifiers: 0 });
    expect(keyEventToInput(key("Backspace"))).toEqual({ kind: "key", code: "Backspace", modifiers: 0 });
    expect(keyEventToInput(key("Escape"))).toEqual({ kind: "key", code: "Esc", modifiers: 0 });
  });

  // Would catch: Ctrl+C arriving as the letter "c" (no interrupt), wrong modifier bits, Shift+Tab not BackTab.
  it("keeps control/alt chords as key events with crossterm bits", () => {
    expect(keyEventToInput(key("c", { ctrlKey: true }))).toEqual({ kind: "key", code: "Char", modifiers: MOD_CONTROL, ch: "c" });
    expect(keyEventToInput(key("C", { ctrlKey: true, shiftKey: true }))).toEqual({ kind: "key", code: "Char", modifiers: MOD_CONTROL | MOD_SHIFT, ch: "c" });
    expect(keyEventToInput(key("x", { altKey: true }))).toEqual({ kind: "key", code: "Char", modifiers: MOD_ALT, ch: "x" });
    expect(keyEventToInput(key("Tab", { shiftKey: true }))).toEqual({ kind: "key", code: "BackTab", modifiers: 0 });
    expect(keyEventToInput(key("ArrowUp", { ctrlKey: true }))).toEqual({ kind: "key", code: "Up", modifiers: MOD_CONTROL });
    expect(keyEventToInput(key("F5"))).toEqual({ kind: "key", code: "F5", modifiers: 0 });
  });

  // Would catch: modifier-only keys or composition being forwarded as text.
  it("ignores modifier-only keys and in-progress composition", () => {
    expect(keyEventToInput(key("Shift"))).toBeNull();
    expect(keyEventToInput(key("Dead"))).toBeNull();
    expect(keyEventToInput(key("Unidentified"))).toBeNull();
    expect(keyEventToInput(key("a", { isComposing: true }))).toBeNull();
  });
});

describe("pasteToInput", () => {
  // Would catch: multi-line paste split into per-line text/Enter events (would run twice or
  // interleave), or empty paste producing an event.
  it("forwards multi-line text as a single paste event", () => {
    expect(pasteToInput("echo A\necho B\n")).toEqual({ kind: "paste", text: "echo A\necho B\n" });
    expect(pasteToInput("")).toBeNull();
  });
});
