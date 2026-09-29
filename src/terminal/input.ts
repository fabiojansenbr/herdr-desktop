// Keyboard → semantic input mapping. Printable characters become TextCommit so IME
// composition and dead keys are preserved; control/navigation keys become Key events with
// crossterm modifier bits. Composition in progress produces nothing.

import { MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_SUPER, type InputDto } from "./types";

export interface KeyLike {
  key: string;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
  isComposing?: boolean;
  /** True while the key is held down (KeyboardEvent.repeat); never sends input twice. */
  repeat?: boolean;
}

const SPECIAL: Record<string, string> = {
  Enter: "Enter",
  Backspace: "Backspace",
  Tab: "Tab",
  Escape: "Esc",
  ArrowLeft: "Left",
  ArrowRight: "Right",
  ArrowUp: "Up",
  ArrowDown: "Down",
  Home: "Home",
  End: "End",
  PageUp: "PageUp",
  PageDown: "PageDown",
  Delete: "Delete",
  Insert: "Insert",
};

export function modifierBits(e: KeyLike): number {
  let bits = 0;
  if (e.shiftKey) bits |= MOD_SHIFT;
  if (e.ctrlKey) bits |= MOD_CONTROL;
  if (e.altKey) bits |= MOD_ALT;
  if (e.metaKey) bits |= MOD_SUPER;
  return bits;
}

/** Returns the semantic event for a keydown, or null when the browser should keep it. */
export function keyEventToInput(e: KeyLike): InputDto | null {
  if (e.isComposing) return null;
  if (e.key === "Tab" && e.shiftKey) return { kind: "key", code: "BackTab", modifiers: 0 };
  const special = SPECIAL[e.key];
  if (special) return { kind: "key", code: special, modifiers: modifierBits(e) & ~MOD_SHIFT | (e.shiftKey ? MOD_SHIFT : 0) };
  const fn = /^F(\d{1,2})$/.exec(e.key);
  if (fn) return { kind: "key", code: `F${fn[1]}`, modifiers: modifierBits(e) };
  if ([...e.key].length !== 1) return null; // Shift, Control, Dead, Unidentified, ...
  if (e.ctrlKey || e.altKey || e.metaKey) {
    const ch = e.key.length === 1 && /[A-Z]/.test(e.key) ? e.key.toLowerCase() : e.key;
    return { kind: "key", code: "Char", modifiers: modifierBits(e), ch };
  }
  return { kind: "text", text: e.key };
}

/** Clipboard paste → one Paste event (bracketed on the server side, delivered once). */
export function pasteToInput(text: string): InputDto | null {
  if (text.length === 0) return null;
  return { kind: "paste", text };
}
