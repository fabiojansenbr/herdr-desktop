import { describe, expect, it, vi } from "vitest";
import {
  createResponsivePanelState,
  handleWorkbenchMediaChange,
  userExplicitCloseAgents,
  userExplicitCloseSidebar,
  userSelectActivity,
  userToggleAgents,
  userToggleSidebar,
} from "./workbench-media";

describe("workbench responsive panel behavior (spec 007)", () => {
  it("case 1: auto-collapses on narrow viewport and auto-restores when window returns to wide", () => {
    const state = createResponsivePanelState({ sidebarOpen: true, agentsOpen: true });
    const onToggleSidebar = vi.fn();
    const onToggleAgents = vi.fn();
    const callbacks = { onToggleSidebar, onToggleAgents };

    // Window becomes narrow (max-width: 960px matches)
    handleWorkbenchMediaChange(state, true, callbacks);

    expect(state.sidebarOpen).toBe(false);
    expect(state.agentsOpen).toBe(false);
    expect(state.sidebarAutoCollapsed).toBe(true);
    expect(state.agentsAutoCollapsed).toBe(true);
    expect(onToggleSidebar).toHaveBeenCalledWith(false);
    expect(onToggleAgents).toHaveBeenCalledWith(false);

    onToggleSidebar.mockClear();
    onToggleAgents.mockClear();

    // Window returns to wide (max-width: 960px no longer matches)
    handleWorkbenchMediaChange(state, false, callbacks);

    expect(state.sidebarOpen).toBe(true);
    expect(state.agentsOpen).toBe(true);
    expect(state.sidebarAutoCollapsed).toBe(false);
    expect(state.agentsAutoCollapsed).toBe(false);
    expect(onToggleSidebar).toHaveBeenCalledWith(true);
    expect(onToggleAgents).toHaveBeenCalledWith(true);
  });

  it("case 2: panel explicitly closed by the user does NOT reopen when window returns to wide", () => {
    const state = createResponsivePanelState({ sidebarOpen: true, agentsOpen: true });
    const onToggleSidebar = vi.fn();
    const onToggleAgents = vi.fn();
    const callbacks = { onToggleSidebar, onToggleAgents };

    // User explicitly closes the sidebar while in wide view
    userToggleSidebar(state, false, callbacks);
    expect(state.sidebarOpen).toBe(false);
    expect(state.sidebarAutoCollapsed).toBe(false);

    onToggleSidebar.mockClear();
    onToggleAgents.mockClear();

    // Window becomes narrow
    handleWorkbenchMediaChange(state, true, callbacks);
    expect(state.sidebarOpen).toBe(false);
    expect(state.sidebarAutoCollapsed).toBe(false); // Was not auto-collapsed, already closed
    expect(state.agentsOpen).toBe(false);
    expect(state.agentsAutoCollapsed).toBe(true);
    expect(onToggleAgents).toHaveBeenCalledWith(false);
    expect(onToggleSidebar).not.toHaveBeenCalled();

    onToggleSidebar.mockClear();
    onToggleAgents.mockClear();

    // Window returns to wide
    handleWorkbenchMediaChange(state, false, callbacks);

    // Agents panel auto-restores, but explicitly closed sidebar stays closed
    expect(state.agentsOpen).toBe(true);
    expect(state.agentsAutoCollapsed).toBe(false);
    expect(state.sidebarOpen).toBe(false);
    expect(state.sidebarAutoCollapsed).toBe(false);
    expect(onToggleAgents).toHaveBeenCalledWith(true);
    expect(onToggleSidebar).not.toHaveBeenCalled();
  });

  it("panel explicitly closed via Escape does NOT reopen on wide", () => {
    const state = createResponsivePanelState({ sidebarOpen: true, agentsOpen: true });
    const onToggleSidebar = vi.fn();
    const onToggleAgents = vi.fn();
    const callbacks = { onToggleSidebar, onToggleAgents };

    // User presses Escape on agents panel
    userExplicitCloseAgents(state, callbacks);
    expect(state.agentsOpen).toBe(false);
    expect(state.agentsAutoCollapsed).toBe(false);

    // Window becomes narrow
    handleWorkbenchMediaChange(state, true, callbacks);
    expect(state.sidebarAutoCollapsed).toBe(true);
    expect(state.agentsAutoCollapsed).toBe(false);

    // Window returns to wide
    handleWorkbenchMediaChange(state, false, callbacks);
    expect(state.sidebarOpen).toBe(true);
    expect(state.agentsOpen).toBe(false);
  });

  it("panel closed while narrow does not restore when returning to wide", () => {
    const state = createResponsivePanelState({ sidebarOpen: true, agentsOpen: true });
    const onToggleSidebar = vi.fn();
    const onToggleAgents = vi.fn();
    const callbacks = { onToggleSidebar, onToggleAgents };

    // Auto-collapsed on narrow
    handleWorkbenchMediaChange(state, true, callbacks);
    expect(state.sidebarAutoCollapsed).toBe(true);

    // User explicitly opens and then closes sidebar while narrow
    userToggleSidebar(state, true, callbacks);
    expect(state.sidebarOpen).toBe(true);
    expect(state.sidebarAutoCollapsed).toBe(false);

    userExplicitCloseSidebar(state, callbacks);
    expect(state.sidebarOpen).toBe(false);
    expect(state.sidebarAutoCollapsed).toBe(false);

    // Window returns to wide
    handleWorkbenchMediaChange(state, false, callbacks);
    expect(state.sidebarOpen).toBe(false);
    expect(state.agentsOpen).toBe(true);
  });

  it("userSelectActivity respects explicit user intent", () => {
    const state = createResponsivePanelState({ sidebarOpen: true, agentsOpen: false });
    const onToggleSidebar = vi.fn();
    const onToggleAgents = vi.fn();
    const callbacks = { onToggleSidebar, onToggleAgents };

    // Toggling projects activity when already active closes sidebar
    userSelectActivity(state, "projects", "projects", false, callbacks);
    expect(state.sidebarOpen).toBe(false);
    expect(state.sidebarAutoCollapsed).toBe(false);

    // Opening agents activity explicitly opens agents
    userSelectActivity(state, "agents", "projects", false, callbacks);
    expect(state.agentsOpen).toBe(true);
    expect(state.agentsAutoCollapsed).toBe(false);
  });
});
