// Keyboard routing for the pane surface (spec 004). A key is either a GUI shortcut (consumed
// by the desktop, never forwarded to the PTY), terminal input (forwarded exactly once to the
// server-confirmed pane) or left to the browser (IME composition, bare modifiers).

import { t } from "../i18n/index.svelte";
import { keyEventToInput, type KeyLike } from "./input";
import type { InputDto } from "./types";

export type { KeyLike };

export type GuiAction =
  | { type: "split"; direction: "right" | "down" }
  | { type: "focus"; step: 1 | -1 }
  | { type: "ratio"; delta: number }
  | { type: "new_tab" };

export type KeyRoute = { kind: "gui"; action: GuiAction } | { kind: "terminal"; input: InputDto } | { kind: "browser" };

interface Shortcut {
  /** Read at render time, so the table follows a language change (spec 070). */
  readonly label: string;
  example: KeyLike;
  action: GuiAction;
  matches(e: KeyLike): boolean;
}

const base: KeyLike = { key: "", ctrlKey: false, altKey: false, shiftKey: false, metaKey: false };
const ctrlShift = (e: KeyLike) => e.ctrlKey && e.shiftKey && !e.altKey && !e.metaKey;
const altShift = (e: KeyLike) => e.altKey && e.shiftKey && !e.ctrlKey && !e.metaKey;
const is = (e: KeyLike, ...keys: string[]) => keys.includes(e.key.length === 1 ? e.key.toLowerCase() : e.key);

export const GUI_SHORTCUTS: readonly Shortcut[] = [
  {
    get label() {
      return t("terminal.shortcut.splitRight");
    },
    example: { ...base, key: "D", ctrlKey: true, shiftKey: true },
    action: { type: "split", direction: "right" },
    matches: (e) => ctrlShift(e) && is(e, "d"),
  },
  {
    get label() {
      return t("terminal.shortcut.splitDown");
    },
    example: { ...base, key: "E", ctrlKey: true, shiftKey: true },
    action: { type: "split", direction: "down" },
    matches: (e) => ctrlShift(e) && is(e, "e"),
  },
  {
    get label() {
      return t("terminal.shortcut.focusNext");
    },
    example: { ...base, key: "ArrowRight", ctrlKey: true, shiftKey: true },
    action: { type: "focus", step: 1 },
    matches: (e) => ctrlShift(e) && is(e, "ArrowRight", "ArrowDown"),
  },
  {
    get label() {
      return t("terminal.shortcut.focusPrevious");
    },
    example: { ...base, key: "ArrowLeft", ctrlKey: true, shiftKey: true },
    action: { type: "focus", step: -1 },
    matches: (e) => ctrlShift(e) && is(e, "ArrowLeft", "ArrowUp"),
  },
  {
    get label() {
      return t("terminal.shortcut.ratioUp");
    },
    example: { ...base, key: "ArrowRight", altKey: true, shiftKey: true },
    action: { type: "ratio", delta: 0.1 },
    matches: (e) => altShift(e) && is(e, "ArrowRight", "ArrowDown"),
  },
  {
    get label() {
      return t("terminal.shortcut.ratioDown");
    },
    example: { ...base, key: "ArrowLeft", altKey: true, shiftKey: true },
    action: { type: "ratio", delta: -0.1 },
    matches: (e) => altShift(e) && is(e, "ArrowLeft", "ArrowUp"),
  },
  {
    get label() {
      return t("terminal.shortcut.newTab");
    },
    example: { ...base, key: "T", ctrlKey: true, shiftKey: true },
    action: { type: "new_tab" },
    matches: (e) => ctrlShift(e) && is(e, "t"),
  },
];

export function routeKey(e: KeyLike): KeyRoute {
  if (e.isComposing) return { kind: "browser" };
  const shortcut = GUI_SHORTCUTS.find((s) => s.matches(e));
  if (shortcut) return { kind: "gui", action: shortcut.action };
  const input = keyEventToInput(e);
  return input ? { kind: "terminal", input } : { kind: "browser" };
}
