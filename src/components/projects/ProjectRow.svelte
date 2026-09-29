<script lang="ts">
  // Workspace row of the 025 tree: the label with `⎇ branch` of the engine workspace, agent dots
  // (011), the focus accent of the active workspace, and the `…` menu (Mover para grupo,
  // Renomear, Fechar workspace). A saved project without a live workspace is a dimmed `fechado`
  // row whose click creates the workspace with its cwd (AC-025-02/03).
  import type { WorkspaceRow } from "./tree-model";
  import { t } from "../../i18n/index.svelte";

  interface Props {
    item: WorkspaceRow;
    groups?: readonly { id: string; name: string }[];
    onOpen: (item: WorkspaceRow) => void;
    onMoveToGroup?: (groupId: string, item: WorkspaceRow) => void;
    onRename?: (item: WorkspaceRow, label: string) => void;
    onCloseWorkspace?: (item: WorkspaceRow) => void;
  }

  let { item, groups = [], onOpen, onMoveToGroup, onRename, onCloseWorkspace }: Props = $props();

  let menuOpen = $state(false);
  let moveOpen = $state(false);
  let renaming = $state(false);
  let confirming = $state(false);
  let draft = $state("");
  let renameInput: HTMLInputElement | undefined = $state();

  const closed = $derived(item.kind === "closed");
  const branchText = $derived(item.disabled ? t("projects.offline") : item.branch);

  function toggleMenu(e: MouseEvent | KeyboardEvent) {
    e.stopPropagation();
    menuOpen = !menuOpen;
    moveOpen = false;
    confirming = false;
  }

  function handleOpen(e?: MouseEvent) {
    e?.stopPropagation();
    menuOpen = false;
    if (item.disabled) return;
    onOpen(item);
  }

  function startRename(e: MouseEvent) {
    e.stopPropagation();
    menuOpen = false;
    renaming = true;
    draft = item.name;
    queueMicrotask(() => renameInput?.focus());
  }

  function commitRename() {
    if (!renaming) return;
    renaming = false;
    const label = draft.trim();
    if (label && label !== item.name) onRename?.(item, label);
  }

  function onRenameKey(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      commitRename();
    } else if (e.key === "Escape") {
      e.preventDefault();
      renaming = false;
    }
  }

  function startClose(e: MouseEvent) {
    e.stopPropagation();
    menuOpen = false;
    confirming = true;
  }

  function confirmClose() {
    confirming = false;
    onCloseWorkspace?.(item);
  }

  function moveTo(groupId: string, e: MouseEvent) {
    e.stopPropagation();
    menuOpen = false;
    moveOpen = false;
    onMoveToGroup?.(groupId, item);
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      handleOpen();
    } else if (e.key === "Escape") {
      if (moveOpen) moveOpen = false;
      else if (menuOpen) menuOpen = false;
      else if (renaming) renaming = false;
      else if (confirming) confirming = false;
    }
  }

  function onWindowClick() {
    if (menuOpen) menuOpen = false;
  }
</script>

<svelte:window onclick={onWindowClick} />

<div
  class="project-row"
  class:active={item.active}
  class:closed
  class:disabled={item.disabled}
  role="button"
  tabindex={item.disabled ? -1 : 0}
  data-project={item.id}
  data-project-id={item.projectId ?? item.id}
  data-workspace={item.workspaceId ?? ""}
  data-closed={closed ? "true" : undefined}
  data-active={item.active ? "true" : undefined}
  draggable={!item.disabled && !closed}
  aria-current={item.active ? "true" : undefined}
  aria-disabled={item.disabled ? "true" : undefined}
  aria-label={[item.name, branchText, item.agentSummary].filter(Boolean).join(", ")}
  onclick={() => handleOpen()}
  onkeydown={onKeydown}
  ondragstart={(e) => {
    if (item.disabled || closed) return;
    e.dataTransfer?.setData("text/herdr-workspace", item.id);
  }}
>
  {#if item.active}
    <div class="accent-bar" aria-hidden="true"></div>
  {/if}

  <div class="main-info">
    <div class="title-line">
      {#if renaming}
        <input
          class="rename-input"
          bind:this={renameInput}
          bind:value={draft}
          aria-label={t("projects.row.rename", { name: item.name })}
          onclick={(e) => e.stopPropagation()}
          onkeydown={onRenameKey}
          onblur={commitRename}
        />
      {:else}
        <span class="name project-name">{item.name}</span>
      {/if}
    </div>
    {#if closed}
      <div class="branch-line">
        <span class="closed-label" data-branch="closed">{t("projects.row.closed")}</span>
      </div>
    {:else if branchText}
      <div class="branch-line">
        <span class="branch branch-name" title={t("projects.row.branch", { branch: branchText })}>⎇ {branchText}</span>
      </div>
    {/if}
  </div>

  <div class="status-and-actions">
    {#if item.allDots.length > 0}
      <div class="agent-dots" aria-label={item.agentSummary} title={item.agentSummary}>
        {#each item.visibleDots as dot, i (i)}
          <span
            class="agent-dot"
            class:working={dot.status === "working"}
            class:blocked={dot.status === "blocked"}
            class:idle={dot.status === "idle"}
            data-status={dot.status}
            style="background-color: {dot.color};"
            aria-label={dot.label}
          ></span>
        {/each}
        {#if item.overflowCount > 0}
          <span class="overflow-badge" title={item.agentSummary}>+{item.overflowCount}</span>
        {/if}
      </div>
    {/if}

    {#if !item.disabled}
      <div class="menu-container">
        <button
          type="button"
          class="menu-btn"
          data-row-menu
          aria-label={t("projects.row.actions", { name: item.name })}
          aria-haspopup="true"
          aria-expanded={menuOpen}
          onclick={toggleMenu}
        >
          …
        </button>

        {#if menuOpen}
          <div class="dropdown-menu" role="menu">
            {#if closed}
              <button type="button" class="menu-item" role="menuitem" data-action="open" onclick={(e) => { e.stopPropagation(); handleOpen(); }}>
                {t("projects.row.openAction")}
              </button>
            {:else if groups.length > 0 && onMoveToGroup}
              <button
                type="button"
                class="menu-item"
                role="menuitem"
                data-action="move"
                aria-haspopup="true"
                aria-expanded={moveOpen}
                onclick={(e) => { e.stopPropagation(); moveOpen = !moveOpen; }}
              >
                {t("projects.row.moveToGroup")}
              </button>
              {#if moveOpen}
                <div class="submenu">
                  {#each groups as group (group.id)}
                    <button
                      type="button"
                      class="menu-item"
                      role="menuitem"
                      data-action="move-to"
                      data-group-target={group.id}
                      onclick={(e) => moveTo(group.id, e)}
                    >
                      {group.name}
                    </button>
                  {/each}
                </div>
              {/if}
            {/if}
            {#if !closed && onRename}
              <button type="button" class="menu-item" role="menuitem" data-action="rename" onclick={startRename}>
                {t("projects.row.renameAction")}
              </button>
            {/if}
            {#if !closed && onCloseWorkspace}
              <button type="button" class="menu-item danger" role="menuitem" data-action="close" onclick={startClose}>
                {t("projects.row.close")}
              </button>
            {/if}
          </div>
        {/if}
      </div>
    {/if}
  </div>

  {#if confirming}
    <div
      class="confirm"
      role="alertdialog"
      tabindex="-1"
      aria-label={t("projects.row.closeLabel", { name: item.name })}
      onclick={(e) => e.stopPropagation()}
      onkeydown={(e) => e.stopPropagation()}
    >
      <span>{t("projects.row.closeConfirm", { name: item.name })}</span>
      <button type="button" class="confirm-close" data-confirm-close onclick={confirmClose}>{t("projects.close")}</button>
      <button type="button" onclick={(e) => { e.stopPropagation(); confirming = false; }}>{t("projects.cancel")}</button>
    </div>
  {/if}

  <!-- Compatibility helpers for the 007/011 regression harnesses (hidden visually). -->
  <button type="button" class="sr-only-btn" onclick={handleOpen} disabled={item.opening || item.disabled}>{t("projects.row.openAction")}</button>
  <div class="meta sr-only">{t("projects.row.session", { session: item.session, cwd: item.cwd ?? "" })}</div>
  <div class="status sr-only" data-status={item.opening ? "opening" : item.workspaceId ? "active" : "idle"} data-workspace={item.workspaceId ?? ""}>
    {item.opening ? t("projects.row.opening") : item.workspaceId ? t("projects.row.open") : t("projects.row.closed")}
  </div>
</div>

<style>
  .project-row {
    position: relative;
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 6px 12px 6px 28px;
    cursor: pointer;
    user-select: none;
    font-size: 13px;
    color: var(--text, #e7e9ee);
    background-color: transparent;
    transition: background-color 0.15s ease;
  }
  .accent-bar {
    position: absolute;
    left: 0;
    top: 0;
    bottom: 0;
    width: 2px;
    background-color: var(--accent, #8fa8ff);
  }
  .sr-only,
  .sr-only-btn {
    position: absolute;
    width: 1px;
    height: 1px;
    padding: 0;
    margin: -1px;
    overflow: hidden;
    clip: rect(0, 0, 0, 0);
    white-space: nowrap;
    border: 0;
    opacity: 0;
    pointer-events: none;
  }
  .project-row:hover {
    background-color: var(--surface-2, #171a21);
  }
  .project-row.active {
    background-color: var(--surface-3, #1e222b);
  }
  .project-row:focus-visible {
    outline: 2px solid var(--accent, #8fa8ff);
    outline-offset: -2px;
  }
  .project-row.closed,
  .project-row.disabled {
    opacity: 0.55;
  }
  .main-info {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    flex: 1;
  }
  .title-line {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }
  .name {
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .rename-input {
    width: 100%;
    min-width: 0;
    background: var(--surface, #111318);
    border: 1px solid var(--accent, #8fa8ff);
    border-radius: 4px;
    color: var(--text, #e7e9ee);
    font: inherit;
    font-size: 12px;
    padding: 2px 6px;
  }
  .branch-line {
    display: flex;
    align-items: center;
  }
  .branch {
    font-size: 11px;
    color: var(--text-muted, #8c93a3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .closed-label {
    font-size: 11px;
    font-style: italic;
    color: var(--text-muted, #8c93a3);
  }
  .status-and-actions {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
    margin-left: 8px;
  }
  .agent-dots {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .agent-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    flex-shrink: 0;
  }
  .overflow-badge {
    font-size: 10px;
    color: var(--text-muted, #8c93a3);
  }
  .menu-container {
    position: relative;
  }
  .menu-btn {
    background: transparent;
    border: none;
    color: var(--text-muted, #8c93a3);
    font-size: 14px;
    line-height: 1;
    padding: 2px 4px;
    border-radius: 4px;
    cursor: pointer;
    transition: background-color 0.15s ease;
  }
  .menu-btn:hover {
    background-color: var(--surface-2, #171a21);
  }
  .menu-btn:focus-visible {
    outline: 2px solid var(--accent, #8fa8ff);
  }
  .dropdown-menu {
    position: absolute;
    top: 100%;
    right: 0;
    margin-top: 4px;
    background-color: var(--surface-2, #171a21);
    border: 1px solid var(--border, #242833);
    border-radius: 6px;
    padding: 4px;
    display: flex;
    flex-direction: column;
    gap: 2px;
    z-index: 100;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.5);
    min-width: 160px;
  }
  .submenu {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding-left: 8px;
    border-left: 1px solid var(--border, #242833);
  }
  .menu-item {
    background: transparent;
    border: none;
    border-radius: 4px;
    padding: 6px 10px;
    font-size: 12px;
    text-align: left;
    color: var(--text, #e7e9ee);
    cursor: pointer;
    transition: background-color 0.15s ease;
    white-space: nowrap;
  }
  .menu-item:hover {
    background-color: var(--surface-3, #1e222b);
  }
  .menu-item:focus-visible {
    outline: 2px solid var(--accent, #8fa8ff);
  }
  .menu-item.danger:hover {
    color: var(--error, #f2777a);
  }
  .confirm {
    position: absolute;
    left: 24px;
    right: 8px;
    bottom: -24px;
    z-index: 110;
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 8px;
    background: var(--surface-2, #171a21);
    border: 1px solid var(--border, #242833);
    border-radius: 6px;
    font-size: 11px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.5);
  }
  .confirm button {
    background: var(--surface-3, #1e222b);
    color: var(--text, #e7e9ee);
    border: 1px solid var(--border, #242833);
    border-radius: 4px;
    padding: 2px 6px;
    font-size: 11px;
    cursor: pointer;
  }
  .confirm .confirm-close {
    color: var(--error, #f2777a);
    border-color: var(--error, #f2777a);
  }
</style>
