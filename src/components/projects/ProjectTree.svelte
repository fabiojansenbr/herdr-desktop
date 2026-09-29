<script lang="ts">
  // PROJETOS (spec 025): one section per host (same order as CONEXÕES), each listing exactly the
  // engine workspaces of that host — label and `⎇ branch` of the focused pane, agent dots, focus
  // accent (AC-025-01). Desktop-only groups are folders keyed by the root cwd: live workspaces
  // and closed projects (cwd saved, no workspace) appear under their group's header; workspaces
  // without a group stay loose under the host, like the TUI (AC-025-02).
  import type { AgentDto } from "../../agents/types";
  import type { HostDto } from "../../connections/types";
  import { t } from "../../i18n/index.svelte";
  import { pickProjectFolder } from "../../projects/bridge";
  import type { ProjectsController } from "../../projects/controller";
  import type { NavigatorState } from "../../projects/reducer";
  import GroupHeader from "./GroupHeader.svelte";
  import ProjectRow from "./ProjectRow.svelte";
  import { buildWorkspaceTree, type HostNode, type WorkspaceRow } from "./tree-model";

  interface Props {
    controller: ProjectsController;
    state: NavigatorState | null;
    hosts?: readonly HostDto[] | null;
    agents?: readonly AgentDto[] | null;
    selectedEndpoint?: string | null;
  }

  let { controller, state: navState, hosts = [], agents = [], selectedEndpoint = null }: Props = $props();

  let collapsedGroups = $state<Record<string, boolean>>({});
  let dragging = $state<WorkspaceRow | null>(null);

  const groups = $derived(navState?.snapshot?.collections ?? []);
  const projects = $derived(navState?.snapshot?.projects ?? []);
  const opening = $derived(navState?.opening ?? {});
  const tree = $derived(
    buildWorkspaceTree({
      hosts: hosts ?? [],
      groups,
      projects,
      agents: agents ?? [],
      opening,
      selectedEndpoint,
    }),
  );
  const hasRows = $derived(tree.some((host) => host.rows.length > 0 || host.groups.length > 0));

  function toggleGroup(id: string) {
    collapsedGroups[id] = !collapsedGroups[id];
  }

  function openRow(row: WorkspaceRow) {
    if (row.kind === "closed") {
      if (row.projectId) void controller.openClosedProject(row.projectId);
      return;
    }
    if (row.workspaceId) void controller.focusWorkspace(row.endpoint, row.workspaceId);
  }

  function moveToGroup(groupId: string, row: WorkspaceRow) {
    // A workspace already saved (matched by cwd) keeps its project identity: the move is the
    // association, never a second project keyed by a live subdirectory (AC-025-05).
    if (row.projectId) {
      void controller.addToCollection(groupId, row.projectId);
      return;
    }
    if (!row.cwd) return;
    void controller.assignToGroup(groupId, {
      endpoint_profile_id: row.endpoint,
      session_name: row.session,
      cwd: row.cwd,
      label: row.name,
    });
  }

  function onDrop(event: DragEvent, groupId: string) {
    event.preventDefault();
    const id = event.dataTransfer?.getData("text/herdr-workspace") ?? "";
    const row = id ? findRow(id) : dragging;
    dragging = null;
    if (row && row.cwd) moveToGroup(groupId, row);
  }

  function findRow(id: string): WorkspaceRow | null {
    for (const host of tree) {
      for (const row of host.rows) if (row.id === id) return row;
      for (const group of host.groups) for (const row of group.rows) if (row.id === id) return row;
    }
    return null;
  }

  let newGroup = $state("");
  let newGroupOpen = $state(false);
  let newGroupInput: HTMLInputElement | undefined = $state();
  let groupMenu = $state<string | null>(null);

  async function submitGroup(event: SubmitEvent) {
    event.preventDefault();
    const name = newGroup.trim();
    if (!name) return;
    await controller.createGroup(name);
    if (!navState?.globalError) {
      newGroup = "";
      newGroupOpen = false;
    }
  }

  function openNewGroup() {
    newGroupOpen = true;
    queueMicrotask(() => newGroupInput?.focus());
  }

  function onNewGroupKey(event: KeyboardEvent) {
    if (event.key === "Escape") {
      event.preventDefault();
      newGroupOpen = false;
      newGroup = "";
    }
  }

  function openProjectInGroup(host: HostNode, groupName: string) {
    void controller.openFolder(pickProjectFolder, hostSession(host), host.endpoint, groupName);
  }

  function hostSession(host: HostNode): string {
    const found = (hosts ?? []).find((candidate) => candidate.endpoint === host.endpoint);
    return found?.session ?? "default";
  }
</script>

<div class="project-tree" role="tree" aria-label={t("projects.tree.label")}>
  <div class="tree-header">
    <span class="tree-title">{t("projects.tree.title")}</span>
    <div class="tree-actions">
      <button type="button" class="tree-action-btn" data-new-group aria-label={t("projects.tree.newGroup")} title={t("projects.tree.newGroup")} onclick={openNewGroup}>
        <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true">
          <rect x="2" y="3" width="12" height="10" rx="1.5" />
          <path d="M8 6.5v5M5.5 9h5" />
        </svg>
      </button>
    </div>
  </div>

  {#if newGroupOpen}
    <form class="new-group-form" onsubmit={submitGroup}>
      <input
        data-new-group-input
        placeholder={t("projects.tree.newGroup")}
        aria-label={t("projects.tree.newGroup")}
        bind:value={newGroup}
        bind:this={newGroupInput}
        class="new-group-input"
        onkeydown={onNewGroupKey}
      />
      <button type="submit" class="create-group-btn" disabled={newGroup.trim() === ""}>{t("projects.tree.createGroup")}</button>
    </form>
  {/if}

  <div class="tree-content">
    {#if !hasRows}
      <div class="empty-tree"><span class="empty-tree-text">{t("projects.tree.empty")}</span></div>
    {:else}
      {#each tree as host (host.endpoint)}
        <section class="host" data-host={host.endpoint} data-offline={host.offline ? "true" : "false"}>
          <div class="host-header" role="heading" aria-level="3" aria-label={host.offline ? t("projects.tree.hostOffline", { header: host.header }) : host.header}>
            <span class="host-dot" class:online={!host.offline} class:offline={host.offline} aria-hidden="true"></span>
            <span class="host-name">{host.header}</span>
            {#if host.offline}<span class="host-offline">{t("projects.offline")}</span>{/if}
          </div>

          {#if host.rows.length > 0}
            <ul class="rows-list" role="group" aria-label={host.header}>
              {#each host.rows as row (row.id)}
                <li class="project-item" data-project={row.id}>
                  <ProjectRow
                    item={row}
                    groups={groups}
                    onOpen={openRow}
                    onMoveToGroup={moveToGroup}
                    onRename={(item, label) => item.workspaceId && void controller.renameWorkspace(item.endpoint, item.workspaceId, label)}
                    onCloseWorkspace={(item) => item.workspaceId && void controller.closeWorkspace(item.endpoint, item.workspaceId)}
                  />
                </li>
              {/each}
            </ul>
          {/if}

          {#each host.groups as group (group.id)}
            <div
              class="group-container"
              role="group"
              aria-label={group.name}
              data-group-id={group.id}
              ondragover={(e) => e.preventDefault()}
              ondrop={(e) => onDrop(e, group.id)}
            >
              <div class="group-row">
                <GroupHeader
                  name={group.name}
                  color={group.color}
                  count={group.count}
                  collapsed={Boolean(collapsedGroups[group.id])}
                  onToggle={() => toggleGroup(group.id)}
                />
                <div class="group-menu">
                  <button
                    type="button"
                    class="tree-action-btn"
                    data-group-menu
                    aria-label={t("projects.tree.groupActions", { name: group.name })}
                    aria-haspopup="true"
                    aria-expanded={groupMenu === group.id}
                    onclick={(e) => {
                      e.stopPropagation();
                      groupMenu = groupMenu === group.id ? null : group.id;
                    }}
                  >
                    …
                  </button>
                  {#if groupMenu === group.id}
                    <div class="group-dropdown" role="menu">
                      <button
                        type="button"
                        class="group-menu-item"
                        role="menuitem"
                        data-open-in-group
                        onclick={() => {
                          groupMenu = null;
                          openProjectInGroup(host, group.name);
                        }}
                      >
                        {t("projects.tree.openHere")}
                      </button>
                    </div>
                  {/if}
                </div>
              </div>
              {#if !collapsedGroups[group.id] && group.rows.length > 0}
                <ul class="group-rows-list" role="group" aria-label={group.name}>
                  {#each group.rows as row (row.id)}
                    <li class="project-item" data-project={row.id}>
                      <ProjectRow
                        item={row}
                        groups={groups}
                        onOpen={openRow}
                        onMoveToGroup={moveToGroup}
                        onRename={(item, label) => item.workspaceId && void controller.renameWorkspace(item.endpoint, item.workspaceId, label)}
                        onCloseWorkspace={(item) => item.workspaceId && void controller.closeWorkspace(item.endpoint, item.workspaceId)}
                      />
                    </li>
                  {/each}
                </ul>
              {/if}
            </div>
          {/each}
        </section>
      {/each}
    {/if}
  </div>
</div>

<style>
  .project-tree {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    overflow: hidden;
  }
  .tree-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    height: 26px;
    padding: 0 12px;
    flex-shrink: 0;
  }
  .tree-title {
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.05em;
    color: var(--text-muted, #8c93a3);
  }
  .tree-actions {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .tree-action-btn {
    background: transparent;
    border: none;
    color: var(--text-muted, #8c93a3);
    font-size: 16px;
    line-height: 1;
    padding: 2px 6px;
    border-radius: 4px;
    cursor: pointer;
    transition: color 0.15s ease, background-color 0.15s ease;
  }
  .tree-action-btn:hover {
    color: var(--text, #e7e9ee);
    background-color: var(--surface-2, #171a21);
  }
  .tree-action-btn:focus-visible {
    outline: 2px solid var(--accent, #8fa8ff);
  }
  .tree-content {
    flex: 1;
    overflow-y: auto;
    min-height: 0;
    padding: 0 4px 8px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .host {
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .host-header {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 12px 2px;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.03em;
    color: var(--text, #e7e9ee);
    text-transform: none;
  }
  .host-dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    flex-shrink: 0;
    background-color: var(--idle, #6b7280);
  }
  .host-dot.online {
    background-color: var(--working, #5bd68a);
  }
  .host-offline {
    font-weight: 400;
    font-size: 10px;
    color: var(--text-muted, #8c93a3);
  }
  .group-container {
    display: flex;
    flex-direction: column;
    border-radius: 4px;
  }
  .group-row {
    display: flex;
    align-items: center;
    gap: 2px;
  }
  .group-row :global(.group-header) {
    flex: 1;
    min-width: 0;
  }
  .group-menu {
    position: relative;
    flex-shrink: 0;
  }
  .group-dropdown {
    position: absolute;
    top: 100%;
    right: 0;
    min-width: 170px;
    z-index: 8;
    background: var(--surface-2, #171a21);
    border: 1px solid var(--border, #242833);
    border-radius: 6px;
    padding: 4px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.45);
  }
  .group-menu-item {
    display: block;
    width: 100%;
    text-align: left;
    background: transparent;
    border: none;
    color: var(--text, #e7e9ee);
    font-size: 12px;
    padding: 6px 8px;
    border-radius: 4px;
    cursor: pointer;
  }
  .group-menu-item:hover {
    background: var(--surface-3, #1e222b);
  }
  .rows-list,
  .group-rows-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
  }
  .project-item {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .empty-tree {
    padding: 24px 16px;
    text-align: center;
  }
  .empty-tree-text {
    font-size: 13px;
    color: var(--text-muted, #8c93a3);
  }
  .new-group-form {
    display: flex;
    gap: 6px;
    padding: 6px 12px 8px;
    align-items: center;
  }
  .new-group-input {
    flex: 1;
    min-width: 0;
    background: var(--surface, #111318);
    border: 1px solid var(--border, #242833);
    border-radius: 4px;
    color: var(--text, #e7e9ee);
    padding: 4px 8px;
    font: inherit;
    font-size: 12px;
  }
  .create-group-btn {
    background: var(--accent, #8fa8ff);
    color: #0b0c10;
    font-weight: 600;
    border: 1px solid var(--accent, #8fa8ff);
    border-radius: 4px;
    padding: 4px 8px;
    font-size: 11px;
    cursor: pointer;
  }
  .create-group-btn:disabled {
    opacity: 0.5;
    cursor: default;
  }
</style>
