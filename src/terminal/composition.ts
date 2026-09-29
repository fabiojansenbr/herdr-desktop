// IME composition and editable-target routing for the terminal (spec 007, AC-007-02).
// WebKitGTK only enables input methods for editable elements, so TerminalView focuses a real
// textarea; this router keeps preedit local, commits the confirmed text exactly once and makes
// sure a keydown, an input event or a paste never reaches the PTY twice.

import { keyEventToInput, pasteToInput, type KeyLike } from "./input";
import { isExitChord } from "./interaction";
import type { InputDto } from "./types";

const exact = (e: KeyLike, ctrl: boolean, shift: boolean) => e.ctrlKey === ctrl && e.shiftKey === shift && !e.altKey && !e.metaKey;

/**
 * True for chords the browser resolves through the paste event: Ctrl+V, Cmd+V, Cmd+Shift+V
 * ("Paste and Match Style" on macOS) and Shift+Insert. Ctrl+Shift+V is not here: WebKitGTK emits
 * no ClipboardEvent for it, so it is the native paste chord.
 */
export function isClipboardPasteChord(e: KeyLike): boolean {
  if (e.key === "Insert") return e.shiftKey && !e.ctrlKey && !e.altKey && !e.metaKey;
  if (e.key.length !== 1 || e.key.toLowerCase() !== "v") return false;
  if (e.altKey) return false;
  if (e.metaKey) return true;
  return e.ctrlKey && !e.shiftKey;
}

/**
 * True for Ctrl+Shift+V: WebKitGTK 2.52.6 emits no ClipboardEvent for it, so the view reads the
 * local clipboard through the native paste command instead of waiting for a paste event.
 */
export function isNativePasteChord(e: KeyLike): boolean {
  return exact(e, true, true) && e.key.length === 1 && e.key.toLowerCase() === "v";
}

/** Decision of one keydown for the native paste; the view preventDefaults every non-null answer. */
export type NativePasteRoute = "paste" | "ignore" | "unavailable";

/**
 * Focused pane state observed when routing. Informational since spec 028 r4a: `mouse_reporting`
 * and `alternate_screen_active` no longer gate the route (the host decides from the clipboard
 * content), but the view still reports them for the ui trace.
 */
export interface NativePastePane {
  mouse_reporting: boolean;
  alternate_screen_active: boolean;
}

/**
 * Routes one paste chord of one keydown. Every paste chord (Ctrl+V, Cmd+V, Shift+Insert and
 * Ctrl+Shift+V) is native (spec 028 r4a): the host decides from the clipboard content (Local +
 * image → ^V to the app, SSH + image → ClipboardImage, text → Paste), so the focused pane never
 * gates the route — a mouse-aware app (Claude Code, grok) reaches the same host command a shell
 * does.
 *
 * `null` when the event is not a paste chord or the IME owns the keyboard: every other key keeps
 * its terminal route. `"paste"` asks the host once (active, first press); `"ignore"` is a key
 * repeat or gated input (prevent default, no action); `"unavailable"` is the missing callback
 * (the view shows an explicit notice, never a fabricated ClipboardEvent).
 */
export function nativePasteRoute(
  e: KeyLike,
  state: { composing: boolean; enabled: boolean; hasHandler: boolean; pane?: NativePastePane | null },
): NativePasteRoute | null {
  if ((!isNativePasteChord(e) && !isClipboardPasteChord(e)) || state.composing) return null;
  if (e.repeat || !state.enabled) return "ignore";
  return state.hasHandler ? "paste" : "unavailable";
}

/**
 * Routes keyboard, composition and paste events of one editable terminal target.
 *
 * Observed on WebKitGTK 2.52.6/GTK 3.24.52 (docs/PREFLIGHT-NATIVE-INPUT.md):
 * - during preedit every keydown is `Unidentified` and carries `isComposing=true`;
 * - a GTK dead key emits two `compositionend`s for one commit (`''`, then `'é'`);
 * - fcitx5 commits with one non-empty `compositionend` and may leave a dead key as preedit;
 * - commits also fire `beforeinput`/`input` (`insertFromComposition`), which must not resend;
 * - keysym commits outside the IME (wtype emoji) arrive as keydown `Unidentified` followed by a
 *   `compositionend` with no `compositionstart` (composed window, evidencias/007/native-live/run3).
 */
export class InputRouter {
  private active = false;
  private text = "";
  private committed = false;
  private pasteHandled = false;

  /** True between compositionstart and compositionend. */
  get composing(): boolean {
    return this.active;
  }

  /** Latest preedit text: visible presentation, never input. */
  get preedit(): string {
    return this.text;
  }

  compositionStart(): void {
    this.active = true;
    this.text = "";
    this.committed = false;
  }

  /**
   * Focus change of the editable target. Blur ends any composition (the IME loses the target);
   * a focus-in without visible preedit clears a composition left open (GUI r8: fcitx5 may start
   * one on the GTK focus-in without ever ending it, which kept every key and the exit chord ignored).
   */
  focusChanged(focused: boolean): void {
    this.pasteHandled = false;
    if (focused && this.text !== "") return;
    this.active = false;
    this.text = "";
  }

  /** Updates the visible preedit; composition text is not sent before confirmation. */
  compositionUpdate(data: string): void {
    this.text = data;
  }

  /**
   * Ends composition and returns the confirmed text once. The GTK dead key ends with `''`
   * before the real `'é'`, so an empty end neither emits nor consumes the session; a repeated
   * non-empty end of the same session (or of the same key cycle) is ignored. Text dropped while input is disabled is not
   * replayed later.
   */
  compositionEnd(data: string, enabled: boolean): InputDto | null {
    this.active = false;
    this.text = "";
    if (!data || this.committed) return null;
    this.committed = true;
    return enabled ? { kind: "text", text: data } : null;
  }

  /**
   * Routes a keydown. Nothing is sent while the IME owns the keyboard (preedit active), for
   * paste chords (the view routes every one of them to the host, spec 028 r4a; this guard is the
   * second half of the contract) or for unprintable keys (Dead, Unidentified, bare modifiers,
   * isComposing), which input.ts already filters.
   */
  keydown(e: KeyLike, enabled: boolean): InputDto | null {
    // A new physical key starts a new commit cycle: WebKitGTK delivers keysym commits (emoji)
    // as keydown Unidentified + compositionend without compositionstart, so only a key, like a
    // new session, re-arms the commit. A repeated end without a new key stays ignored.
    this.committed = false;
    if (!enabled) return null;
    if (this.active) return null;
    if (isClipboardPasteChord(e) || isNativePasteChord(e)) return null;
    return keyEventToInput(e);
  }

  /**
   * The view routed this paste chord to the host command (spec 028 r4a). If WebKitGTK still emits
   * the ClipboardEvent of the same gesture (observed with empty text/plain for an image), it must
   * not paste again: `paste` consumes the marker once.
   */
  pasteChordHandled(): void {
    this.pasteHandled = true;
  }

  /** The chord key was released: the gesture is over and a later chordless paste works again. */
  pasteChordEnded(): void {
    this.pasteHandled = false;
  }

  /**
   * Routes a clipboard paste: one Paste event, never combined with the chord keydown. A paste
   * event that belongs to a chord the host already handled is a no-op (spec 028 r4a), so one
   * gesture never pastes twice.
   */
  paste(text: string, enabled: boolean): InputDto | null {
    const handled = this.pasteHandled;
    this.pasteHandled = false;
    if (handled || !enabled) return null;
    return pasteToInput(text);
  }
}

/**
 * Ctrl+Shift+F6 leaves the terminal unless the IME shows a preedit (the candidate window owns the
 * keyboard). A composition with nothing visible does not block it and is closed by the exit.
 */
export function exitChordAllowed(e: KeyLike, router: InputRouter): boolean {
  if (!isExitChord(e) || router.preedit !== "") return false;
  router.focusChanged(false);
  return true;
}
