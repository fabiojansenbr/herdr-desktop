// AC-007-02 (contract; the native a11y-navigation sweep is the proof). GUI r8: the exit chord
// pressed on the terminal after a keyboard re-entry (Tab wrap through the GTK host, fcitx5 active
// since native-ime) did not focus the exit marker. H1: a compositionstart without compositionend
// leaves InputRouter.composing true and TerminalView skipped isExitChord while composing.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { exitChordAllowed, InputRouter } from "./composition";
import type { KeyLike } from "./input";

const key = (k: string, mods: Partial<Omit<KeyLike, "key">> = {}): KeyLike => ({ key: k, ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, ...mods });
const exit = key("F6", { ctrlKey: true, shiftKey: true });

describe("composition left open across focus changes", () => {
  // Would catch (H1): composing still true after the target lost and regained focus, so the chord
  // (and every later key) is ignored as if the IME owned the keyboard.
  it("compositionstart → blur → focus → chord: composition cleared and the chord leaves", () => {
    const r = new InputRouter();
    r.compositionStart();
    r.focusChanged(false);
    expect(r.composing).toBe(false);
    r.focusChanged(true);
    expect(r.composing).toBe(false);
    expect(exitChordAllowed(exit, r)).toBe(true);
  });

  // Would catch: only blur handled — a compositionstart delivered with the focus-in itself (no
  // preedit ever shown) keeps the router composing.
  it("focus-in without visible preedit clears a composition that started before it", () => {
    const r = new InputRouter();
    r.compositionStart();
    r.focusChanged(true);
    expect(r.composing).toBe(false);
    expect(r.keydown(key("a"), true)).toEqual({ kind: "text", text: "a" });
  });

  // Would catch: the reset discarding a real preedit the user is still composing on focus-in.
  it("focus-in with a visible preedit keeps the composition", () => {
    const r = new InputRouter();
    r.compositionStart();
    r.compositionUpdate("ni");
    r.focusChanged(true);
    expect(r.composing).toBe(true);
    expect(r.preedit).toBe("ni");
    expect(r.keydown(key("h"), true)).toBeNull();
  });

  // Would catch: a preedit left visible (and composing) after the target lost focus.
  it("blur ends the composition even with preedit", () => {
    const r = new InputRouter();
    r.compositionStart();
    r.compositionUpdate("ni");
    r.focusChanged(false);
    expect([r.composing, r.preedit]).toEqual([false, ""]);
  });

  // Would catch: the chord ignored while composing with nothing visible (r8), or honoured while the
  // IME shows a preedit (the user would lose the candidate window).
  it("exit chord: allowed unless a preedit is visible; composing without preedit does not block it", () => {
    const r = new InputRouter();
    expect(exitChordAllowed(exit, r)).toBe(true);
    r.compositionStart();
    expect(r.composing).toBe(true);
    expect(exitChordAllowed(exit, r)).toBe(true);
    r.compositionUpdate("pin");
    expect(exitChordAllowed(exit, r)).toBe(false);
    expect(exitChordAllowed(key("F6"), new InputRouter())).toBe(false);
    expect(exitChordAllowed(key("F6", { ctrlKey: true }), new InputRouter())).toBe(false);
  });

  // Would catch: an allowed exit leaving composing true behind the marker (next re-entry stuck).
  it("an allowed exit closes an empty composition", () => {
    const r = new InputRouter();
    r.compositionStart();
    expect(exitChordAllowed(exit, r)).toBe(true);
    expect(r.composing).toBe(false);
  });
});

describe("TerminalView wiring", () => {
  const view = readFileSync(new URL("./TerminalView.svelte", import.meta.url), "utf8");
  // Would catch: the view still gating the chord on router.composing, or focus changes not
  // reaching the router (and the preedit span not refreshed).
  it("uses exitChordAllowed and forwards focus/blur to the router", () => {
    expect(view).toMatch(/if \(exitChordAllowed\(e, router\)\) \{\s*e\.preventDefault\(\);\s*leaveTerminal\(\);/);
    expect(view).not.toMatch(/!router\.composing && isExitChord\(e\)/);
    expect(view).toMatch(/onfocus=\{\(\) => \{\s*router\.focusChanged\(true\);\s*preedit = router\.preedit;\s*onFocus\?\.\(true\);\s*\}\}/);
    expect(view).toMatch(/onblur=\{\(\) => \{\s*router\.focusChanged\(false\);\s*preedit = router\.preedit;\s*onFocus\?\.\(false\);\s*\}\}/);
  });
});
