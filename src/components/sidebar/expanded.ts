// Spec 052 — expanded/collapsed of one workspace row of the sidebar.
//
// It is a client state per workspace and it does not follow the focus (the premise this spec
// replaced, P1, tied the tabs to the open workspace): losing the focus changes nothing, clicking
// the focused row toggles it, and a workspace that gains the focus by any other path (a tab, the
// palette, the box, the engine) is expanded. Nothing here reaches the engine or the store.
//
// The identity is the 044 one — endpoint plus the normalized root — so the state follows the
// workspace across restarts, where ids do not survive. A row whose engine reported no cwd has no
// identity to persist under and keeps the state in memory for this window only. Storage can be
// absent or throw (private window, blocked site data), and the sidebar must render anyway, so
// every access is wrapped and falls back to the caller's default (the same rule as 045).
import { normalizeRoot } from "./sidebar-model";

export function expandedKey(endpoint: string, root: string): string {
  return `herdr.sidebar.ws-expanded.${endpoint}:${normalizeRoot(root)}`;
}

/** Stored state of the row, or `fallback` when nothing was stored (or storage is unusable). */
export function readExpanded(endpoint: string, root: string | null, fallback: boolean): boolean {
  if (root === null) return fallback;
  try {
    const stored = globalThis.localStorage?.getItem(expandedKey(endpoint, root));
    if (stored === "1") return true;
    if (stored === "0") return false;
  } catch {
    /* no storage: the row just starts at its default */
  }
  return fallback;
}

export function writeExpanded(endpoint: string, root: string | null, expanded: boolean): void {
  if (root === null) return;
  try {
    globalThis.localStorage?.setItem(expandedKey(endpoint, root), expanded ? "1" : "0");
  } catch {
    /* nothing to persist to; the state stays in memory for this window */
  }
}
