// Window layer shared by the two sidebar menus (spec 049, AC-049-01/02): `WorkspaceMenu` (044)
// and `CollectionMenu` (045).
//
// Both were drawn `position: absolute` inside the list, so they were children of
// `WorkspacesSection` (`overflow-y: auto`) and of `Sidebar.svelte` (`overflow: hidden`): the
// "Mover para coleção" submenu opened past the 288 px of the sidebar and was clipped at its
// edge. They now mount in a portal on `document.body` and are placed from the rectangle of the
// button or row that opened them — the same rule the host overlays already use
// (`connection-actions.ts`, 032/048) — and they close on a `pointerdown` anywhere outside or on
// Esc, with the `window` listeners of `HostPopover.svelte:67-97`.

/** Distance kept from every window edge, as in `connection-actions.ts`. */
export const MENU_MARGIN = 8;
/** Gap between the anchor and the layer it opens. */
export const MENU_GAP = 4;
export const MENU_Z_INDEX = 120;
export const SUBMENU_Z_INDEX = 130;

/** Moves the node to the window layer; Svelte removes it again when the menu unmounts. */
export function portal(node: HTMLElement): { destroy(): void } {
  document.body.appendChild(node);
  return {
    destroy() {
      node.remove();
    },
  };
}

/** Fixed base used before the first measurement, so the layer already shrinks to its content. */
export function baseMenuStyle(zIndex: number): string {
  return `position:fixed;z-index:${zIndex};left:${MENU_MARGIN}px;top:${MENU_MARGIN}px`;
}

function clamp(value: number, size: number, extent: number): number {
  return Math.max(MENU_MARGIN, Math.min(value, extent - MENU_MARGIN - size));
}

function at(left: number, top: number, zIndex: number): string {
  return `position:fixed;z-index:${zIndex};left:${Math.round(left)}px;top:${Math.round(top)}px`;
}

/**
 * Menu of a row or of a collection header: under the anchor with the right edges aligned,
 * flipped above it when there is no room below, and always inside the window.
 */
export function dropdownStyle(anchor: DOMRect | null, width: number, height: number, zIndex: number): string {
  const vw = window.innerWidth;
  const vh = window.innerHeight;
  if (!anchor) return baseMenuStyle(zIndex);
  const below = anchor.bottom + MENU_GAP;
  const above = anchor.top - height - MENU_GAP;
  const fits = below + height <= vh - MENU_MARGIN;
  const top = !fits && above >= MENU_MARGIN ? above : below;
  return at(clamp(anchor.right - width, width, vw), clamp(top, height, vh), zIndex);
}

/**
 * Submenu of "Mover para coleção": to the right of its own item (AC-049-01), flipped to the left
 * when it would leave the window, and aligned with the item's top.
 */
export function flyoutStyle(anchor: DOMRect | null, width: number, height: number, zIndex: number): string {
  const vw = window.innerWidth;
  const vh = window.innerHeight;
  if (!anchor) return baseMenuStyle(zIndex);
  const right = anchor.right + MENU_GAP;
  const left = right + width <= vw - MENU_MARGIN ? right : anchor.left - MENU_GAP - width;
  return at(clamp(left, width, vw), clamp(anchor.top - MENU_GAP, height, vh), zIndex);
}

export interface DismissOptions {
  /** Nodes of the menu itself: a `pointerdown` in any of them keeps it open. */
  inside: () => readonly (Element | null | undefined)[];
  /**
   * Nodes that toggle the menu on click (its own `…` button): closing on their `pointerdown`
   * would only reopen the menu, so they are left alone entirely.
   */
  ignore?: () => readonly (Element | null | undefined)[];
  onDismiss: () => void;
}

/**
 * Closes the menu on a `pointerdown` anywhere outside it or on Esc, wherever the focus is
 * (AC-049-02). Returns the cleanup, so a caller uses it as `$effect(() => dismissOn({...}))`.
 */
export function dismissOn(options: DismissOptions): () => void {
  const hits = (nodes: readonly (Element | null | undefined)[], target: Element) =>
    nodes.some((node) => node?.contains(target));

  const onPointerDown = (event: Event) => {
    const target = event.target;
    if (!(target instanceof Element)) return;
    if (hits(options.inside(), target)) return;
    if (hits(options.ignore?.() ?? [], target)) return;
    options.onDismiss();
  };
  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key !== "Escape") return;
    options.onDismiss();
  };
  window.addEventListener("pointerdown", onPointerDown, true);
  window.addEventListener("keydown", onKeyDown);
  return () => {
    window.removeEventListener("pointerdown", onPointerDown, true);
    window.removeEventListener("keydown", onKeyDown);
  };
}
