// Frame `…` menu of the clean casca (spec 018, AC-018-03). Order is the acceptance list;
// each item maps to one existing engine action (split right/down, pane.close).
import { t } from "../../i18n/index.svelte";

// The labels are getters so the list follows the current language: the array is built once at
// import time, long before the window knows which language it opens in (spec 069).
export const PANE_MENU_ITEMS = [
  { id: "split-right", get label() { return t("center.pane.splitRight"); } },
  { id: "split-down", get label() { return t("center.pane.splitDown"); } },
  { id: "close", get label() { return t("center.pane.close"); } },
] as const;

export type PaneMenuItemId = (typeof PANE_MENU_ITEMS)[number]["id"];

// ---------------------------------------------------------------------------------------
// Spec 028 AC-028-03 — right-click menu of one pane, in the order and with the translated
// labels of the TUI (../herdr/src/client/shell/context_menu.rs:55-75): `Renomear pane`,
// `Limpar nome do pane` (only with a manual name), `Trocar com o pane focado` (only with a
// source), `Dividir à direita`, `Dividir abaixo`, `Zoom` / `Desfazer zoom`,
// `Enviar cliques direitos ao pane` / `Usar menu do Herdr`, `Fechar pane`.
// ---------------------------------------------------------------------------------------

export type PaneContextMenuItemId =
  | "rename"
  | "clear-name"
  | "swap"
  | "split-right"
  | "split-down"
  | "zoom"
  | "toggle-right-click"
  | "close";

export interface PaneContextMenuEntry {
  id: PaneContextMenuItemId;
  label: string;
}

export interface PaneMenuContext {
  paneId: string;
  /** The pane carries a manual name read from the engine (`pane.list`). */
  hasManualLabel: boolean;
  /** Confirmed focused pane when it is not `paneId`; null when the clicked pane is the focused one. */
  swapSourcePaneId: string | null;
  /** Right clicks currently go to the pane (true) or open this menu (false). */
  rightClickPassthrough: boolean;
  /** The engine's own `zoomed` for this pane's tab, when known (false otherwise). */
  zoomed: boolean;
}

export function paneZoomLabel(zoomed: boolean): string {
  return zoomed ? t("center.pane.unzoom") : t("center.pane.zoom");
}

export function rightClickToggleLabel(passthrough: boolean): string {
  return passthrough ? t("center.pane.useHerdrMenu") : t("center.pane.sendRightClicks");
}

/** Items of the pane context menu, in the TUI order, with the conditional ones. */
export function paneContextMenuItems(context: PaneMenuContext): PaneContextMenuEntry[] {
  const items: PaneContextMenuEntry[] = [{ id: "rename", label: t("center.pane.rename") }];
  if (context.hasManualLabel) items.push({ id: "clear-name", label: t("center.pane.clearName") });
  if (context.swapSourcePaneId) items.push({ id: "swap", label: t("center.pane.swap") });
  items.push(
    { id: "split-right", label: t("center.pane.splitRight") },
    { id: "split-down", label: t("center.pane.splitDown") },
    { id: "zoom", label: paneZoomLabel(context.zoomed) },
    { id: "toggle-right-click", label: rightClickToggleLabel(context.rightClickPassthrough) },
    { id: "close", label: t("center.pane.close") },
  );
  return items;
}

// ---------------------------------------------------------------------------------------
// Right-click passthrough per pane, persisted in the window session (spec 028 AC-028-03):
// the engine stores it too (`pane.input.set`), this store keeps the label of the item alive
// across menu openings and window reloads of the same session.
// ---------------------------------------------------------------------------------------

export const RIGHT_CLICK_STORAGE_KEY = "herdr.pane.right_click";

/** Subset of `Storage` the store uses; a Map-backed fake is accepted in tests. */
export interface RightClickStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

const memory = new Map<string, boolean>();

function defaultStorage(): RightClickStorage | null {
  try {
    return typeof sessionStorage === "undefined" ? null : sessionStorage;
  } catch {
    return null;
  }
}

function readAll(storage: RightClickStorage | null): Record<string, boolean> {
  if (!storage) return {};
  try {
    const raw = storage.getItem(RIGHT_CLICK_STORAGE_KEY);
    if (!raw) return {};
    const parsed: unknown = JSON.parse(raw);
    return parsed && typeof parsed === "object" ? (parsed as Record<string, boolean>) : {};
  } catch {
    return {};
  }
}

export function readRightClickPassthrough(paneId: string, storage: RightClickStorage | null = defaultStorage()): boolean {
  if (!paneId) return false;
  const persisted = readAll(storage)[paneId];
  return persisted ?? memory.get(paneId) ?? false;
}

export function writeRightClickPassthrough(
  paneId: string,
  value: boolean,
  storage: RightClickStorage | null = defaultStorage(),
): void {
  if (!paneId) return;
  memory.set(paneId, value);
  if (!storage) return;
  try {
    const all = readAll(storage);
    all[paneId] = value;
    storage.setItem(RIGHT_CLICK_STORAGE_KEY, JSON.stringify(all));
  } catch {
    // The in-memory copy of the window session is enough when the storage refuses.
  }
}
