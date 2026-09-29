// Spec 007 — Responsive collapse/restore state machine for Workbench sidebar & agents.
// Panels collapsed automatically by the (max-width: 960px) media query must restore
// when returning to a wide viewport, but panels explicitly closed by the user must
// not be reopened.

export interface ResponsivePanelState {
  sidebarOpen: boolean;
  agentsOpen: boolean;
  sidebarAutoCollapsed: boolean;
  agentsAutoCollapsed: boolean;
}

export function createResponsivePanelState(initial?: {
  sidebarOpen?: boolean;
  agentsOpen?: boolean;
}): ResponsivePanelState {
  return {
    sidebarOpen: initial?.sidebarOpen ?? true,
    agentsOpen: initial?.agentsOpen ?? true,
    sidebarAutoCollapsed: false,
    agentsAutoCollapsed: false,
  };
}

export interface ResponsivePanelCallbacks {
  onToggleSidebar?: (open: boolean) => void;
  onToggleAgents?: (open: boolean) => void;
}

/**
 * Handles media query changes between narrow (<= 960px) and wide (> 960px).
 * Auto-collapses open panels on narrow; auto-restores only those auto-collapsed on wide.
 */
export function handleWorkbenchMediaChange(
  state: ResponsivePanelState,
  matchesNarrow: boolean,
  callbacks?: ResponsivePanelCallbacks,
): void {
  if (matchesNarrow) {
    if (state.sidebarOpen) {
      state.sidebarAutoCollapsed = true;
      state.sidebarOpen = false;
      callbacks?.onToggleSidebar?.(false);
    }
    if (state.agentsOpen) {
      state.agentsAutoCollapsed = true;
      state.agentsOpen = false;
      callbacks?.onToggleAgents?.(false);
    }
  } else {
    if (state.sidebarAutoCollapsed) {
      state.sidebarAutoCollapsed = false;
      state.sidebarOpen = true;
      callbacks?.onToggleSidebar?.(true);
    }
    if (state.agentsAutoCollapsed) {
      state.agentsAutoCollapsed = false;
      state.agentsOpen = true;
      callbacks?.onToggleAgents?.(true);
    }
  }
}

/**
 * User explicitly toggled sidebar. Clears auto-collapsed flag.
 */
export function userToggleSidebar(
  state: ResponsivePanelState,
  isNarrow: boolean,
  callbacks?: ResponsivePanelCallbacks,
): boolean {
  state.sidebarAutoCollapsed = false;
  const next = !state.sidebarOpen;
  state.sidebarOpen = next;
  if (isNarrow && next && state.agentsOpen) {
    state.agentsAutoCollapsed = false;
    state.agentsOpen = false;
    callbacks?.onToggleAgents?.(false);
  }
  callbacks?.onToggleSidebar?.(next);
  return next;
}

/**
 * User explicitly toggled agents panel. Clears auto-collapsed flag.
 */
export function userToggleAgents(
  state: ResponsivePanelState,
  isNarrow: boolean,
  callbacks?: ResponsivePanelCallbacks,
): boolean {
  state.agentsAutoCollapsed = false;
  const next = !state.agentsOpen;
  state.agentsOpen = next;
  if (isNarrow && next && state.sidebarOpen) {
    state.sidebarAutoCollapsed = false;
    state.sidebarOpen = false;
    callbacks?.onToggleSidebar?.(false);
  }
  callbacks?.onToggleAgents?.(next);
  return next;
}

/**
 * User explicitly closed sidebar (e.g. Escape key). Clears auto-collapsed flag.
 */
export function userExplicitCloseSidebar(
  state: ResponsivePanelState,
  callbacks?: ResponsivePanelCallbacks,
): void {
  state.sidebarAutoCollapsed = false;
  state.sidebarOpen = false;
  callbacks?.onToggleSidebar?.(false);
}

/**
 * User explicitly closed agents panel (e.g. Escape key). Clears auto-collapsed flag.
 */
export function userExplicitCloseAgents(
  state: ResponsivePanelState,
  callbacks?: ResponsivePanelCallbacks,
): void {
  state.agentsAutoCollapsed = false;
  state.agentsOpen = false;
  callbacks?.onToggleAgents?.(false);
}

/**
 * User selected an activity item that changes panel visibility.
 */
export function userSelectActivity(
  state: ResponsivePanelState,
  activity: string,
  currentActivity: string,
  isNarrow: boolean,
  callbacks?: ResponsivePanelCallbacks,
): void {
  if (activity === "projects") {
    if (currentActivity === "projects") {
      userToggleSidebar(state, isNarrow, callbacks);
    } else {
      state.sidebarAutoCollapsed = false;
      state.sidebarOpen = true;
      if (isNarrow && state.agentsOpen) {
        state.agentsAutoCollapsed = false;
        state.agentsOpen = false;
        callbacks?.onToggleAgents?.(false);
      }
      callbacks?.onToggleSidebar?.(true);
    }
  } else if (activity === "agents") {
    userToggleAgents(state, isNarrow, callbacks);
  }
}
