// Explicit shortcut list of the composed window (spec 010, premise 2 and design/GUIA-IMPLEMENTACAO.md
// "Contraste e teclado"). The window claims only the `window` entries, and only when the key event
// did not come from the terminal input; everything else reaches its target once. The `terminal`
// entries are handled inside TerminalView and are listed here so the whole set stays explicit.

export type Platform = "linux" | "windows" | "macos";
export type ShortcutId = "palette" | "sidebar" | "new-agent" | "new-tab" | "terminal-copy" | "terminal-paste" | "terminal-exit";

export interface Shortcut {
  id: ShortcutId;
  context: "window" | "terminal";
  key: string;
  ctrl: boolean;
  meta: boolean;
  shift: boolean;
  alt: boolean;
  label: string;
}

export interface KeyLike {
  key: string;
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
  isComposing: boolean;
  repeat: boolean;
  target: { closest?(selector: string): unknown } | null;
}

const chord = (id: ShortcutId, context: Shortcut["context"], key: string, mods: Partial<Shortcut>, label: string): Shortcut => ({
  id,
  context,
  key,
  ctrl: false,
  meta: false,
  shift: false,
  alt: false,
  ...mods,
  label,
});

const terminal = [
  chord("terminal-copy", "terminal", "c", { ctrl: true, shift: true }, "Ctrl+Shift+C"),
  chord("terminal-paste", "terminal", "v", { ctrl: true, shift: true }, "Ctrl+Shift+V"),
  chord("terminal-exit", "terminal", "F6", { ctrl: true, shift: true }, "Ctrl+Shift+F6"),
];

export const SHORTCUTS: Record<Platform, readonly Shortcut[]> = {
  linux: [
    chord("palette", "window", "k", { ctrl: true }, "Ctrl K"),
    chord("sidebar", "window", "b", { ctrl: true }, "Ctrl+B"),
    // Spec 041 (PRD nova-lateral, P8): Ctrl+N belongs to readline/vim inside the terminal, so
    // "Novo agente" claims Ctrl+Shift+N and only on the window.
    chord("new-agent", "window", "n", { ctrl: true, shift: true }, "Ctrl+Shift+N"),
    chord("new-tab", "window", "t", { ctrl: true }, "Ctrl+T"),
    ...terminal,
  ],
  windows: [
    chord("palette", "window", "k", { ctrl: true }, "Ctrl K"),
    chord("sidebar", "window", "b", { ctrl: true }, "Ctrl+B"),
    // Spec 041 (PRD nova-lateral, P8): Ctrl+N belongs to readline/vim inside the terminal, so
    // "Novo agente" claims Ctrl+Shift+N and only on the window.
    chord("new-agent", "window", "n", { ctrl: true, shift: true }, "Ctrl+Shift+N"),
    chord("new-tab", "window", "t", { ctrl: true }, "Ctrl+T"),
    ...terminal,
  ],
  macos: [
    chord("palette", "window", "k", { meta: true }, "⌘ K"),
    chord("sidebar", "window", "b", { meta: true }, "⌘B"),
    chord("new-agent", "window", "n", { meta: true, shift: true }, "⌘⇧N"),
    chord("new-tab", "window", "t", { meta: true }, "⌘T"),
    ...terminal,
  ],
};

/** Selector of the terminal's keyboard target (TerminalView's textarea). */
export const TERMINAL_INPUT_SELECTOR = "textarea.ime-target";

export function platformOf(userAgent: string): Platform {
  if (/Macintosh|Mac OS X/.test(userAgent)) return "macos";
  if (/Windows/.test(userAgent)) return "windows";
  return "linux";
}

export function isTerminalTarget(target: KeyLike["target"]): boolean {
  return Boolean(target?.closest?.(TERMINAL_INPUT_SELECTOR));
}

/** The window shortcut this event triggers, or null (the event is left to its target). */
export function resolveShortcut(event: KeyLike, platform: Platform): ShortcutId | null {
  if (event.isComposing || event.repeat || isTerminalTarget(event.target)) return null;
  const found = SHORTCUTS[platform].find(
    (s) =>
      s.context === "window" &&
      s.key === event.key.toLowerCase() &&
      s.ctrl === event.ctrlKey &&
      s.meta === event.metaKey &&
      s.shift === event.shiftKey &&
      s.alt === event.altKey,
  );
  return found?.id ?? null;
}

export function shortcutLabel(id: ShortcutId, platform: Platform): string {
  return SHORTCUTS[platform].find((s) => s.id === id)?.label ?? "";
}
