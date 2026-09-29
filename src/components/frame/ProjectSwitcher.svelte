<script lang="ts">
  // Project selector of the clean title bar (spec 018 AC-018-01/03; spec 025 AC-025-03).
  // The label is `<host> › <workspace> ⎇ <branch>` for the focused workspace of the selected
  // host; the menu lists the engine workspaces per host (click focuses) and the desktop groups
  // with their closed projects. `Abrir projeto…` is the 016 native folder picker.
  import type { HostWorkspaceDto } from "../../connections/types";
  import { pickProjectFolder } from "../../projects/bridge";
  import { requestNewWorkspace } from "../../projects/new-workspace";
  import { t } from "../../i18n/index.svelte";
  import type { FrameContext } from "../../shell/frame-context";
  import {
    openProjectItem,
    projectSwitcherLabel,
    projectSwitcherMenu,
    workspaceSwitcherMenu,
  } from "./project-switcher";

  let {
    ctx = null,
    session = "default",
    label = null,
    pickFolder = null,
    compact = false,
  }: {
    ctx?: FrameContext | null;
    session?: string;
    label?: string | null;
    pickFolder?: (() => Promise<string | null>) | null;
    compact?: boolean;
  } = $props();

  let open = $state(false);

  const hosts = $derived(ctx?.connections?.view?.hub.hosts ?? []);
  const selectedHost = $derived(hosts.find((host) => host.endpoint === ctx?.selectedEndpoint) ?? null);
  const focused = $derived<HostWorkspaceDto | null>(
    selectedHost?.workspaces?.find((workspace) => workspace.focused) ?? null,
  );
  const shown = $derived(
    label ??
      projectSwitcherLabel({
        host: selectedHost ? (selectedHost.kind === "local" ? t("frame.switcher.thisComputer") : selectedHost.label || selectedHost.endpoint) : null,
        workspace: focused?.label ?? null,
        branch: focused?.branch ?? ctx?.branch ?? null,
        session,
      }),
  );
  const hostMenu = $derived(workspaceSwitcherMenu(hosts));
  // AC-025-05: a saved project whose cwd is an open workspace is offered as a live row, never
  // as a closed duplicate in the group.
  const liveRoots = $derived(
    hosts.flatMap((host) => (host.workspaces ?? []).flatMap((workspace) => [workspace.cwd, ...(workspace.cwds ?? [])].filter((root): root is string => Boolean(root)))),
  );
  const groups = $derived(
    projectSwitcherMenu(ctx?.navigator?.snapshot?.collections ?? [], ctx?.navigator?.snapshot?.projects ?? [], liveRoots),
  );

  function stopDrag(event: MouseEvent) {
    event.stopPropagation();
  }

  function toggle() {
    open = !open;
  }

  function chooseWorkspace(endpoint: string, workspaceId: string) {
    open = false;
    void ctx?.controllers.projects.focusWorkspace(endpoint, workspaceId);
  }

  function chooseClosed(projectId: string) {
    open = false;
    void ctx?.controllers.projects.openClosedProject(projectId);
  }

  async function openFolder() {
    open = false;
    if (!ctx) return;
    const endpoint = selectedHost?.endpoint ?? "local";
    // AC-046-04: the 016 native dialog only browses this computer, so on an SSH host it is never
    // opened — the remote path is typed in "Novo workspace", already marking this host. No
    // folder of this machine is ever sent to another one and nothing falls back to Local.
    if (endpoint !== "local") {
      requestNewWorkspace(endpoint);
      return;
    }
    const pick = pickFolder ?? pickProjectFolder;
    await ctx.controllers.projects.openFolder(pick, selectedHost?.session ?? session, endpoint);
  }
</script>

<div class="switcher" class:compact data-project-compact={compact ? "true" : null}>
  <button
    type="button"
    class="trigger"
    data-topbar-item="project"
    aria-haspopup="menu"
    aria-expanded={open}
    aria-label={t("frame.switcher.project", { label: shown })}
    title={shown}
    onclick={toggle}
    onmousedown={stopDrag}
  >
    <span class="label">{shown}</span>
    <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <polyline points="6 9 12 15 18 9" />
    </svg>
  </button>
  {#if open}
    <div class="menu" role="menu" aria-label={t("frame.switcher.workspaces")}>
      {#each hostMenu as host (host.endpoint)}
        <div class="group" data-host={host.endpoint}>
          <span class="group-name">{host.name}</span>
          {#each host.workspaces as item (item.id)}
            <button type="button" role="menuitem" data-workspace={item.id} onclick={() => chooseWorkspace(host.endpoint, item.id)}>
              {item.label}{item.branch ? ` ⎇ ${item.branch}` : ""}
            </button>
          {/each}
        </div>
      {/each}
      {#each groups as group (group.id)}
        <div class="group" data-group={group.id}>
          <span class="group-name">{group.name}</span>
          {#each group.projects as item (item.id)}
            <button type="button" role="menuitem" data-project={item.id} onclick={() => chooseClosed(item.id)}>{item.label}</button>
          {/each}
        </div>
      {/each}
      <button type="button" role="menuitem" data-open-project onclick={() => void openFolder()}>{openProjectItem()}</button>
    </div>
  {/if}
</div>

<style>
  .switcher {
    position: relative;
    flex: none;
    min-width: 0;
    max-width: 280px;
  }
  .switcher.compact {
    max-width: 160px;
    flex: 0 1 160px;
  }
  .compact .trigger {
    max-width: 160px;
  }
  .trigger {
    display: flex;
    align-items: center;
    gap: 6px;
    max-width: 280px;
    height: 30px;
    padding: 0 8px;
    background: transparent;
    border: 1px solid transparent;
    border-radius: 6px;
    color: var(--text);
    font-size: 13px;
  }
  .trigger:hover,
  .trigger[aria-expanded="true"] {
    background: var(--surface-3);
    border-color: var(--border);
  }
  .label {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .menu {
    position: absolute;
    top: calc(100% + 6px);
    left: 0;
    min-width: 240px;
    max-height: 360px;
    overflow: auto;
    display: flex;
    flex-direction: column;
    padding: 4px;
    background: var(--surface-2);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 12px 32px rgba(0, 0, 0, 0.5);
    z-index: 40;
  }
  .group-name {
    padding: 6px 8px 2px;
    font-size: 11px;
    color: var(--text-muted);
  }
  .menu button {
    text-align: left;
    background: transparent;
    border: none;
    border-radius: 6px;
    padding: 6px 8px;
    color: var(--text);
    font-size: 13px;
  }
  .menu button:hover {
    background: var(--surface-3);
  }
</style>
