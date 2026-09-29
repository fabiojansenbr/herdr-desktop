<script lang="ts">
  // The new sidebar (spec 041, AC-041-01), in the slot `projects` of the Workbench. It composes
  // the head (mark + collapse), the action block, the 043 mount point, the WORKSPACES section and
  // the foot. Spec 060 took the 059 HOSTS REMOTOS section out again: the remote workspaces are back
  // in their collections and in `Sem coleção`, and the reconnection shows on the row itself. Each
  // block is its own component, so the parallel
  // specs fill their mount point without editing this composition. Spec 056 took Buscar out of the
  // action block, leaving Novo agente alone there; the palette keeps its own way in, the Ctrl K
  // chord of the window.
  import NewAgentPopover from "../center/NewAgentPopover.svelte";
  import type { FrameContext } from "../../shell/frame-context";
  import { t } from "../../i18n/index.svelte";
  import { platformOf, shortcutLabel } from "../../shell/shortcuts";
  import HostSwitcher from "./HostSwitcher.svelte";
  import InboxSection from "./InboxSection.svelte";
  import SidebarHeader from "./SidebarHeader.svelte";
  import WorkspacesSection from "./WorkspacesSection.svelte";

  let { ctx }: { ctx: FrameContext } = $props();

  const platform = platformOf(typeof navigator === "undefined" ? "" : navigator.userAgent);
  const newAgentHint = shortcutLabel("new-agent", platform);

  let popover = $state(false);

  /** Entry point of the `new-agent` chord (P8): the window opens this very popover. */
  export function openNewAgent() {
    popover = true;
  }
</script>

<div class="sidebar-v2" data-slot="projects" data-activity={ctx.activity}>
  <SidebarHeader onCollapse={() => ctx.actions.toggleProjects()} />

  <div class="actions" data-sidebar-actions>
    <div class="new-agent">
      <button
        type="button"
        class="action primary"
        data-sidebar-action="newAgent"
        aria-haspopup="dialog"
        aria-expanded={popover}
        onclick={() => (popover = !popover)}
      >
        <span class="action-icon" aria-hidden="true">
          <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
            <path d="M12 20h9" /><path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4Z" />
          </svg>
        </span>
        <span class="action-label">{t("sidebar.newAgent")}</span>
        <kbd class="hint">{newAgentHint}</kbd>
      </button>
      {#if popover}
        <NewAgentPopover {ctx} onclose={() => (popover = false)} />
      {/if}
    </div>

  </div>

  <div class="inbox" data-sidebar-inbox><InboxSection {ctx} /></div>

  <WorkspacesSection {ctx} />

  <HostSwitcher {ctx} />
</div>

<style>
  .sidebar-v2 {
    height: 100%;
    min-height: 0;
    display: flex;
    flex-direction: column;
    background-color: var(--surface, #111318);
  }
  .actions {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 2px 8px 6px;
    flex-shrink: 0;
  }
  .new-agent {
    position: relative;
  }
  .action {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 7px 10px;
    background: transparent;
    border: none;
    border-radius: 8px;
    font: inherit;
    font-size: 13px;
    color: var(--text, #e7e9ee);
    cursor: pointer;
    text-align: left;
    transition: background-color 0.15s ease;
  }
  .action:hover {
    background-color: var(--surface-2, #171a21);
  }
  .action:focus-visible {
    outline: 2px solid var(--accent, #8fa8ff);
    outline-offset: -2px;
  }
  .action.primary {
    background-color: var(--surface-2, #171717);
    border-radius: 8px;
  }
  .action.primary:hover {
    background-color: var(--surface-3, #242424);
  }
  .action-icon {
    display: flex;
    align-items: center;
    color: var(--text-muted, #a3a3a3);
    flex-shrink: 0;
  }
  .action.primary .action-icon {
    color: var(--accent, #d4d4d4);
  }
  .action-label {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 14px;
    font-weight: 500;
  }
  .hint {
    flex-shrink: 0;
    font-family: inherit;
    font-size: 10px;
    line-height: 1.6;
    padding: 0 5px;
    border-radius: 4px;
    color: var(--text-muted, #8c93a3);
    background-color: var(--surface-3, #1e222b);
  }
  .inbox {
    flex-shrink: 0;
  }
</style>
