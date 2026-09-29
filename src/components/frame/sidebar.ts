// Sidebar collapse (spec 018, AC-018-02). Open by default; the last user choice is stored
// under `herdr.sidebar.open` and read on the next mount. Values other than the literal
// "false" (including a missing key) mean open, so a first launch shows the tree.
//
// Spec 047 (AC-047-01/04) replaces the 0 px collapse with the rail of the approved design: the
// sidebar is 288 px open (`design/v2-01-workspace.png`) and 56 px collapsed
// (`design/v2-04-lateral-recolhida.png`), so the status column never disappears — collapsing
// hands the center 232 px back (216 px measured from the 272 px sidebar 047 replaced).

export const SIDEBAR_STORAGE_KEY = "herdr.sidebar.open";
export const SIDEBAR_OPEN_PX = 288;
/** Width of the collapsed sidebar: the rail, never 0 (AC-047-01). */
export const SIDEBAR_RAIL_PX = 56;
export const CENTER_MIN_PX = 400;

/** True when an open sidebar would leave the center narrower than 400 px. */
export function sidebarAutoCollapse(windowWidth: number): boolean {
  if (!(windowWidth > 0)) return false;
  return windowWidth - SIDEBAR_OPEN_PX < CENTER_MIN_PX;
}

/** Visual open state: user preference, unless the center would drop below 400 px. */
export function sidebarVisuallyOpen(preferenceOpen: boolean, windowWidth: number): boolean {
  return preferenceOpen && !sidebarAutoCollapse(windowWidth);
}

/** Width the sidebar paints: 288 px open, 56 px as the rail. */
export function sidebarWidthPx(preferenceOpen: boolean, windowWidth: number): number {
  return sidebarVisuallyOpen(preferenceOpen, windowWidth) ? SIDEBAR_OPEN_PX : SIDEBAR_RAIL_PX;
}

export function readSidebarOpen(storage: Pick<Storage, "getItem"> | null | undefined): boolean {
  return storage?.getItem(SIDEBAR_STORAGE_KEY) !== "false";
}

export function writeSidebarOpen(open: boolean, storage: Pick<Storage, "setItem"> | null | undefined): void {
  storage?.setItem(SIDEBAR_STORAGE_KEY, open ? "true" : "false");
}
