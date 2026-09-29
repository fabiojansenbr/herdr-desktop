// Going to a tab of any host from the sidebar, through the paths that already exist (spec 043):
// the host switch of 035/037 (no reconnect), the workspace focus of 025 and the `tab.focus` of
// 042 — the last one only after the new host's metadata confirms, so the focus never lands on the
// host the user just left. Nothing here answers an agent or sends it a key.
//
// Spec 052 extracted it from the "Precisa de você" box so an expanded workspace that is not the
// focused one uses the very same chain: one host switch when needed, one `workspace_focus`, one
// `tab.focus`. The refusal of the switch is shown on the surface; nothing else is sent to a host
// we did not reach.
import type { FrameContext } from "../../shell/frame-context";

export interface TabTarget {
  readonly endpoint: string;
  readonly workspaceId: string;
  /** Tab to focus after the workspace; null focuses the workspace alone. */
  readonly tabId: string | null;
}

export interface TabNavigator {
  /** Runs the chain for `target`; a second call while one is running is dropped. */
  open(target: TabTarget): Promise<void>;
  /**
   * Sends the pending `tab.focus` as soon as the window holds the metadata of the host the target
   * lives on: the engine only accepts a tab of the connection it confirmed (spec 027/042). Call it
   * from an `$effect` — it reads `ctx.agents` first, so the arrival of the metadata re-runs it.
   */
  settle(): void;
}

export function createTabNavigator(ctx: FrameContext, selectedEndpoint: () => string | null): TabNavigator {
  /** Tab the click is waiting to focus once the new host's metadata confirms (AC-043-02). */
  let pending: { endpoint: string; tabId: string } | null = null;
  let busy = false;

  function settle() {
    const state = ctx.agents;
    if (!pending) return;
    if (state?.identity?.endpoint !== pending.endpoint) return;
    if (!state.tabs.some((tab) => tab.tab_id === pending!.tabId)) return;
    const { tabId } = pending;
    pending = null;
    void ctx.controllers.agents.focusTab(tabId);
  }

  return {
    settle,
    async open(target) {
      if (busy) return;
      busy = true;
      try {
        if (target.endpoint !== selectedEndpoint()) {
          try {
            await ctx.controllers.surface.select(target.endpoint);
          } catch {
            // The refusal is shown on the surface; nothing else is sent to a host we did not reach.
            return;
          }
        }
        await ctx.controllers.projects.focusWorkspace(target.endpoint, target.workspaceId);
        if (!target.tabId) return;
        pending = { endpoint: target.endpoint, tabId: target.tabId };
        settle();
      } finally {
        busy = false;
      }
    },
  };
}
