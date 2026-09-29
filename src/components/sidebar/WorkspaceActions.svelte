<script lang="ts">
  // Hover actions of a workspace row (spec 044, AC-044-02): the drag handle, `+` (focus the
  // workspace, then the 017 popover) and `…` (the menu). The cluster is invisible until the
  // pointer is over the row or the row takes keyboard focus, so a still list stays as quiet as
  // the design.
  //
  // Spec 045 (AC-045-01): the handle is the only draggable part of the line — grabbing it starts
  // the drag, so a plain click still focuses the workspace. What it carries is decided by
  // `drag.ts`; the drop targets are the other rows and the collection headers.
  import NewAgentPopover from "../center/NewAgentPopover.svelte";
  import type { FrameContext } from "../../shell/frame-context";
  import { t } from "../../i18n/index.svelte";
  import { beginDrag, DRAG_MIME, endDrag } from "./drag";
  import type { SidebarRow } from "./sidebar-model";
  import WorkspaceMenu from "./WorkspaceMenu.svelte";

  let {
    ctx,
    row,
    collectionId,
    onrename,
  }: { ctx: FrameContext; row: SidebarRow; collectionId: string; onrename: () => void } = $props();

  let menu = $state(false);
  let popover = $state(false);
  let menuButton = $state<HTMLButtonElement | undefined>();
  /** Rectangle the 049 layer places the menu by: the `…` button, or the row on a right click. */
  let anchor = $state<HTMLElement | null>(null);

  /** Entry point of the right click and of the `…` button on the row itself. */
  export function openMenu(from?: HTMLElement | null) {
    anchor = from ?? menuButton ?? null;
    menu = true;
  }

  export function closeMenu() {
    menu = false;
  }

  async function newAgent() {
    // The popover always starts an agent in the focused workspace: focus first, once.
    if (row.workspaceId && !row.disabled) await ctx.controllers.projects.focusWorkspace(row.endpoint, row.workspaceId);
    popover = true;
  }
</script>

<span class="cluster" data-workspace-hover>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <span
    class="handle"
    data-workspace-handle
    draggable="true"
    aria-hidden="true"
    ondragstart={(event) => {
      event.stopPropagation();
      event.dataTransfer?.setData(DRAG_MIME, row.id);
      if (event.dataTransfer) event.dataTransfer.effectAllowed = "move";
      beginDrag({
        rowId: row.id,
        projectId: row.projectId,
        endpoint: row.endpoint,
        session: row.session,
        cwd: row.cwd,
        label: row.name,
        collectionId,
      });
    }}
    ondragend={() => endDrag()}
  >
    <svg viewBox="0 0 24 24" width="12" height="12" fill="currentColor">
      <circle cx="9" cy="6" r="1.4" /><circle cx="15" cy="6" r="1.4" /><circle cx="9" cy="12" r="1.4" />
      <circle cx="15" cy="12" r="1.4" /><circle cx="9" cy="18" r="1.4" /><circle cx="15" cy="18" r="1.4" />
    </svg>
  </span>

  <span class="trailing-actions">
    <button
      type="button"
      class="action"
      data-workspace-add
      aria-label={t("sidebar.row.newAgentIn", { name: row.name })}
      aria-haspopup="dialog"
      aria-expanded={popover}
      onclick={(event) => {
        event.stopPropagation();
        void newAgent();
      }}
    >
      <svg viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true">
        <path d="M12 5v14" /><path d="M5 12h14" />
      </svg>
    </button>

    <button
      type="button"
      class="action"
      data-workspace-menu-button
      bind:this={menuButton}
      aria-label={t("sidebar.row.actions", { name: row.name })}
      aria-haspopup="menu"
      aria-expanded={menu}
      onclick={(event) => {
        event.stopPropagation();
        if (menu) menu = false;
        else openMenu(event.currentTarget);
      }}
    >
      <svg viewBox="0 0 24 24" width="13" height="13" fill="currentColor" aria-hidden="true">
        <circle cx="5" cy="12" r="1.6" /><circle cx="12" cy="12" r="1.6" /><circle cx="19" cy="12" r="1.6" />
      </svg>
    </button>
  </span>
</span>

{#if menu}
  <WorkspaceMenu
    {ctx}
    {row}
    {anchor}
    trigger={menuButton}
    onclose={() => (menu = false)}
    onrename={() => {
      menu = false;
      onrename();
    }}
  />
{/if}

{#if popover}
  <span class="popover-anchor">
    <NewAgentPopover {ctx} onclose={() => (popover = false)} />
  </span>
{/if}

<style>
  .cluster {
    display: inline-flex;
    align-items: center;
    opacity: 0;
    transition: opacity 0.12s ease;
  }
  /* The row owns the hover: the cluster only appears with the pointer on the line or with the
     keyboard on the line or on one of its own buttons. */
  :global(.row:hover) .cluster,
  :global(.row:focus-visible) .cluster,
  :global(.row:focus-within) .cluster {
    opacity: 1;
  }
  .handle {
    position: absolute;
    left: 4px;
    top: 7px;
    display: flex;
    align-items: center;
    color: var(--text-muted, #8c93a3);
    cursor: grab;
  }
  .trailing-actions {
    display: inline-flex;
    align-items: center;
    gap: 2px;
  }
  .action {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 18px;
    height: 18px;
    padding: 0;
    background: transparent;
    border: none;
    border-radius: 4px;
    color: var(--text-muted, #8c93a3);
    cursor: pointer;
  }
  .action:hover {
    background-color: var(--surface-3, #1e222b);
    color: var(--text, #e7e9ee);
  }
  .action:focus-visible {
    outline: 2px solid var(--accent, #8fa8ff);
    outline-offset: -2px;
  }
  .popover-anchor {
    position: absolute;
    top: 100%;
    right: 0;
    z-index: 40;
  }
</style>
