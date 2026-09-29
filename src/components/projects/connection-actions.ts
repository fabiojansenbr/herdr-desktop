// Per-host actions and overlay placement shared by the connections list (spec 011/029/032) and by
// the host popover of the new sidebar (spec 048). Extracted from `ConnectionsFooter.svelte`
// unchanged: the same menu (Desconectar/Reconectar, Editar…, Remover), the same rule that Local
// has no menu at all, the same inline remove confirmation and the same fixed overlay that is
// measured from the anchor, flipped upward when there is no room below and clamped to the window.

import { t } from "../../i18n/index.svelte";

export type HostActionId = "disconnect" | "reconnect" | "connect_on_open" | "edit" | "remove";

export interface HostAction {
  readonly id: HostActionId;
  readonly label: string;
  /** Destructive: the caller asks for the inline confirmation instead of running it (AC-029-03). */
  readonly confirms: boolean;
  /** Preference entry: ✓ when it is on (spec 058, AC-058-03); absent for the plain actions. */
  readonly checked?: boolean;
}

// Built on every call (spec 068): the labels are read from the dictionary at that moment, so a
// language change reaches a menu that is opened afterwards without a reload.
const DISCONNECT = (): HostAction => ({ id: "disconnect", label: t("projects.host.disconnect"), confirms: false });
const RECONNECT = (): HostAction => ({ id: "reconnect", label: t("projects.host.reconnect"), confirms: false });
const EDIT = (): HostAction => ({ id: "edit", label: t("projects.host.edit"), confirms: false });
const REMOVE = (): HostAction => ({ id: "remove", label: t("projects.host.remove"), confirms: true });
const CONNECT_ON_OPEN = (): HostAction => ({ id: "connect_on_open", label: t("projects.host.connectOnOpen"), confirms: false, checked: true });

/**
 * Menu of one host: Desconectar when the connection is live, Reconectar otherwise, then Editar…
 * and Remover. Local is not a saved profile, so it offers nothing and no menu is opened for it.
 * A caller that knows the host's saved preference (`connectOnOpen`) also gets the
 * `Conectar ao abrir` toggle of spec 058, checked when it is on; a caller that does not read the
 * profiles keeps the menu it had.
 */
export function hostActions(host: {
  readonly endpoint: string;
  readonly online: boolean;
  readonly connectOnOpen?: boolean;
}): readonly HostAction[] {
  if (host.endpoint === "local") return [];
  const first = host.online ? DISCONNECT() : RECONNECT();
  if (host.connectOnOpen === undefined) return [first, EDIT(), REMOVE()];
  return [first, { ...CONNECT_ON_OPEN(), checked: host.connectOnOpen }, EDIT(), REMOVE()];
}

/** Text of the inline confirmation shown before removing a saved profile. */
export function confirmRemoveText(name: string): string {
  return t("projects.host.confirmRemove", { name });
}

export const VIEWPORT_MARGIN = 8;
export const MENU_Z_INDEX = 100;
export const CONFIRM_Z_INDEX = 110;

/** Fixed base used before the measurement, so the menu already shrinks to fit its content. */
export function baseStyle(zIndex: number): string {
  return `position:fixed;z-index:${zIndex};left:${VIEWPORT_MARGIN}px;top:${VIEWPORT_MARGIN}px`;
}

export function clampedStyle(anchor: DOMRect | null, width: number, height: number, zIndex: number): string {
  const vw = window.innerWidth;
  const vh = window.innerHeight;
  const below = anchor ? anchor.bottom + 2 : VIEWPORT_MARGIN;
  const above = anchor ? anchor.top - height - 2 : VIEWPORT_MARGIN;
  let top = below + height > vh - VIEWPORT_MARGIN && above >= VIEWPORT_MARGIN ? above : below;
  top = Math.max(VIEWPORT_MARGIN, Math.min(top, vh - VIEWPORT_MARGIN - height));
  const right = anchor ? anchor.right : vw - VIEWPORT_MARGIN;
  const left = Math.max(VIEWPORT_MARGIN, Math.min(right - width, vw - VIEWPORT_MARGIN - width));
  return `position:fixed;z-index:${zIndex};left:${Math.round(left)}px;top:${Math.round(top)}px`;
}
