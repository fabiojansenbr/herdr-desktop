<script lang="ts">
  // One workspace line of the new sidebar (spec 041, AC-041-03): name, the `git-branch` line only
  // when the engine reported a branch (038: never a `—`), the host tag on SSH rows, the dimmed
  // `offline` state and the status summary of `sidebarStatus`. The active row (036/P7) is identified
  // and carries the 042 mount point; every row carries the 044 hover actions. Clicking goes
  // through the same controller path as the 025 row: focus for a live workspace, open for a
  // closed project.
  //
  // Spec 044 adds the preference marks (palette colour on the initial tile, pin on a pinned row)
  // and the two entry points of the menu that live on the row itself: the right click and F2,
  // which opens the inline rename editor.
  //
  // Spec 060 (AC-060-02): the host tag carries the connection state of its host — the host name
  // alone while it is online, a spinner and `reconectando…` while it comes back (the line then
  // shows the cached snapshot, dimmed, and takes no focus click), `atenção` in amber when the host
  // needs attention, and the `offline` tag of 041 untouched when it is simply offline.
  //
  // Spec 052 (which replaced the PRD premise P1): expanded/collapsed is a client state of this row
  // and does not follow the focus. Losing the focus keeps the tabs where they are, clicking the
  // focused row toggles them without touching the focus, and a row that gains the focus by any
  // other path is expanded. `expanded.ts` owns the state and its storage.
  import { untrack } from "svelte";
  import type { FrameContext } from "../../shell/frame-context";
  import { t } from "../../i18n/index.svelte";
  import { activeDrag, endDrag, onDragChange, placementOf, planDrop, type DropSpot } from "./drag";
  import { readExpanded, writeExpanded } from "./expanded";
  import type { SidebarRow } from "./sidebar-model";
  import WorkspaceActions from "./WorkspaceActions.svelte";
  import WorkspaceTabList from "./WorkspaceTabList.svelte";

  let {
    ctx,
    row,
    collectionId,
    onDrop,
  }: {
    ctx: FrameContext;
    row: SidebarRow;
    /** The collection this line is shown in (spec 045); `UNGROUPED_ID` for a loose row. */
    collectionId: string;
    onDrop: (spot: DropSpot) => void;
  } = $props();

  const closed = $derived(row.kind === "closed");
  const initial = $derived((row.name.match(/[\p{L}\p{N}]/u)?.[0] ?? [...row.name.trim()][0] ?? "").toUpperCase());

  /**
   * AC-052-03: the stored state wins, and without one the focused row starts expanded and the
   * others collapsed. Read once, when the row is created: reopening the sidebar reads it again.
   */
  let expanded = $state(untrack(() => readExpanded(row.endpoint, row.prefRoot, row.active)));
  /** The focus this row had on the previous pass: only gaining it expands the row. */
  let wasActive = untrack(() => row.active);

  function setExpanded(next: boolean) {
    expanded = next;
    writeExpanded(row.endpoint, row.prefRoot, next);
  }

  // A workspace that gains the focus by any other path (a tab, the palette, the box, the engine)
  // is expanded; the others keep whatever state they had.
  $effect(() => {
    const active = row.active;
    if (active && !wasActive) setExpanded(true);
    wasActive = active;
  });

  let actions = $state<{ openMenu: (from?: HTMLElement | null) => void } | null>(null);
  let renaming = $state(false);
  let draft = $state("");
  let input = $state<HTMLInputElement | null>(null);
  let node = $state<HTMLElement | null>(null);
  /** Edge the drop would use (AC-045-01), or `null` when this row is not the target. */
  let edge = $state<"before" | "after" | null>(null);
  let refused = $state<string | null>(null);

  /** The click belongs to the hover cluster, the menu or the popover, not to the row. */
  function inActions(target: EventTarget | null): boolean {
    return target instanceof Element && target.closest("[data-workspace-actions]") !== null;
  }

  function open(event?: MouseEvent) {
    // AC-025-02/03 and the 035 focus path: the controller switches host without reconnecting and
    // sends `workspace_focus` once. An offline host takes no click.
    if (event && inActions(event.target)) return;
    if (renaming || row.disabled) return;
    if (closed) {
      if (row.projectId) void ctx.controllers.projects.openClosedProject(row.projectId);
      return;
    }
    if (!row.workspaceId) return;
    // AC-052-01: the focused row only opens or closes its tabs — the focus is already here, so
    // nothing is sent to the engine. Any other row is focused and left expanded.
    if (row.active) {
      setExpanded(!expanded);
      return;
    }
    setExpanded(true);
    void ctx.controllers.projects.focusWorkspace(row.endpoint, row.workspaceId);
  }

  export function startRename() {
    if (!row.workspaceId) return;
    draft = row.name;
    renaming = true;
  }

  function commitRename() {
    // AC-044-02: one `workspace_rename` with the trimmed name; an empty name sends nothing and
    // leaves the editor open.
    const label = draft.trim();
    if (!label || !row.workspaceId) return;
    void ctx.controllers.projects.renameWorkspace(row.endpoint, row.workspaceId, label);
    renaming = false;
  }

  function onKeydown(event: KeyboardEvent) {
    if (inActions(event.target) || renaming) return;
    if (event.key === "F2") {
      event.preventDefault();
      startRename();
      return;
    }
    if (event.key !== "Enter" && event.key !== " ") return;
    event.preventDefault();
    open();
  }

  function onContextMenu(event: MouseEvent) {
    event.preventDefault();
    // Spec 049: the menu is placed from the rectangle of what opened it — here, the line itself.
    actions?.openMenu(node);
  }

  function spotOf(event: DragEvent): DropSpot {
    const rect = node?.getBoundingClientRect() ?? { top: 0, height: 0 };
    return {
      kind: "row",
      collectionId,
      projectId: row.projectId,
      endpoint: row.endpoint,
      placement: placementOf(rect, event.clientY),
    };
  }

  function onDragOver(event: DragEvent) {
    const source = activeDrag();
    if (!source || source.rowId === row.id) return;
    const spot = spotOf(event);
    const plan = planDrop(source, spot);
    refused = plan.refused;
    if (plan.refused !== null || plan.steps.length === 0) {
      edge = null;
      return;
    }
    // Only a drop this row can serve gets `preventDefault`, so the cursor tells the truth.
    event.preventDefault();
    edge = spot.kind === "row" ? spot.placement : null;
  }

  function clearDrop() {
    edge = null;
    refused = null;
  }

  function drop(event: DragEvent) {
    event.preventDefault();
    const source = activeDrag();
    const spot = spotOf(event);
    clearDrop();
    if (source) onDrop(spot);
    endDrag();
  }

  $effect(() => {
    if (renaming) input?.select();
  });

  // Esc (or the end of the drag anywhere) takes the insertion point with it.
  $effect(() => onDragChange(clearDrop));

  const label = $derived(
    [
      row.name,
      row.hostBadge,
      row.branch,
      row.hostStateText ?? (row.hostState === "offline" ? t("sidebar.offline") : null),
      row.status.label || null,
    ]
      .filter(Boolean)
      .join(", "),
  );
</script>

<div
  class="row"
  role="button"
  bind:this={node}
  tabindex={row.disabled ? -1 : 0}
  data-workspace-row={row.id}
  data-endpoint={row.endpoint}
  data-drop-refused={refused ?? undefined}
  title={refused ?? undefined}
  ondragover={onDragOver}
  ondragleave={clearDrop}
  ondrop={drop}
  data-workspace={row.workspaceId ?? ""}
  data-active={row.active ? "true" : undefined}
  data-offline={row.offline ? "true" : undefined}
  data-stale={row.stale ? "true" : undefined}
  data-closed={closed ? "true" : undefined}
  aria-current={row.active ? "true" : undefined}
  aria-disabled={row.disabled ? "true" : undefined}
  aria-expanded={closed || !row.workspaceId ? undefined : expanded}
  data-expanded={expanded && row.workspaceId ? "true" : undefined}
  aria-label={label}
  onclick={open}
  onkeydown={onKeydown}
  oncontextmenu={onContextMenu}
>
  <span
    class="tile"
    data-ws-tile
    data-row-icon
    data-row-color={row.color ?? undefined}
    style:color={closed ? "var(--text-dim, #6B6B6B)" : row.tileColor}
    style:background-color={`${row.tileColor}${closed ? "26" : "33"}`}
    aria-hidden="true"
  >
    {initial}
  </span>

  <span class="info">
    <span class="title-line">
      {#if renaming}
        <!-- svelte-ignore a11y_autofocus -->
        <input
          class="rename"
          data-rename-input
          bind:this={input}
          bind:value={draft}
          aria-label={t("sidebar.row.rename", { name: row.name })}
          autofocus
          onclick={(event) => event.stopPropagation()}
          onblur={() => (renaming = false)}
          onkeydown={(event) => {
            event.stopPropagation();
            if (event.key === "Enter") {
              event.preventDefault();
              commitRename();
            } else if (event.key === "Escape") {
              event.preventDefault();
              renaming = false;
            }
          }}
        />
      {:else}
        {#if row.pinned}
          <span class="pin" data-pinned title={t("sidebar.row.pinned")} aria-label={t("sidebar.row.pinned")}>
            <svg viewBox="0 0 24 24" width="11" height="11" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <path d="M9 3h6l-1 5 3 3v2H7v-2l3-3-1-5Z" /><path d="M12 13v8" />
            </svg>
          </span>
        {/if}
        <span class="name">{row.name}</span>
        {#if row.hostBadge}
          <span
            class="host-badge"
            data-host-badge
            data-host-state={row.hostState}
            title={row.hostStateTitle ?? undefined}
          >
            {#if row.stale}<span class="badge-spinner" data-host-spinner aria-hidden="true">⟳</span>{/if}
            <span class="badge-host">{row.hostBadge}</span>
            {#if row.hostStateText}<span class="badge-state">{row.hostStateText}</span>{/if}
          </span>
        {/if}
        {#if row.hostState === "offline"}<span class="offline-tag">{t("sidebar.offline")}</span>{/if}
      {/if}
    </span>
    {#if row.branch}
      <span class="branch" data-branch title={t("sidebar.row.branch", { branch: row.branch })}>
        <span class="branch-icon" data-branch-icon="git-branch" aria-hidden="true">
          <svg viewBox="0 0 24 24" width="11" height="11" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <circle cx="6" cy="5" r="2.2" /><circle cx="6" cy="19" r="2.2" /><circle cx="18" cy="8" r="2.2" />
            <path d="M6 7.2v9.6" /><path d="M18 10.2c0 4-4 3.8-6 5.6" />
          </svg>
        </span>
        {row.branch}
      </span>
    {/if}
  </span>

  <span class="trailing">
    {#if row.status.kind !== "idle"}
      <span class="status" data-row-status={row.status.kind} aria-label={row.status.label} title={row.status.label}>
        {#if row.status.kind === "working"}
          <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" aria-hidden="true">
            <path d="M21 12a9 9 0 1 1-6.2-8.55" />
          </svg>
          <span class="status-count">{row.status.working}</span>
        {:else if row.status.kind === "waiting"}
          <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true">
            <circle cx="12" cy="12" r="9" /><path d="M12 7.5v5" /><path d="M12 16.2v.2" />
          </svg>
        {:else}
          <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <circle cx="12" cy="12" r="9" /><path d="m8.5 12.2 2.4 2.4 4.6-4.9" />
          </svg>
        {/if}
      </span>
    {/if}
    <span class="actions-slot" data-workspace-actions>
      <WorkspaceActions bind:this={actions} {ctx} {row} {collectionId} onrename={startRename} />
    </span>
  </span>

  {#if edge}
    <span class="drop-indicator" data-drop-indicator={edge} aria-hidden="true"></span>
  {/if}
</div>

{#if expanded && row.workspaceId}
  <div class="tabs-slot" data-workspace-tabs={row.workspaceId}><WorkspaceTabList {ctx} {row} /></div>
{/if}

<style>
  .row {
    position: relative;
    display: flex;
    align-items: flex-start;
    gap: 8px;
    padding: 6px 10px 6px 22px;
    border-radius: 8px;
    cursor: pointer;
    user-select: none;
    font-size: 14px;
    color: var(--text, #EDEDED);
    transition: background-color 0.15s ease;
  }
  .row:hover {
    background-color: var(--surface-2, #171717);
  }
  .row:focus-visible {
    outline: 2px solid var(--accent, #D4D4D4);
    outline-offset: -2px;
  }
  .row[data-offline="true"] {
    opacity: 0.55;
  }
  /* AC-060-02: the line is showing the cached snapshot while the host comes back. */
  .row[data-stale="true"] {
    opacity: 0.55;
  }
  .row[data-closed="true"] {
    opacity: 0.7;
  }
  .row[data-active="true"] .name {
    font-weight: 500;
  }
  .tile {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    border-radius: 5px;
    font-size: 11px;
    font-weight: 700;
    flex-shrink: 0;
  }
  .info {
    display: flex;
    flex-direction: column;
    gap: 1px;
    min-width: 0;
    flex: 1;
  }
  .title-line {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }
  .pin {
    display: flex;
    align-items: center;
    color: var(--text-muted, #A3A3A3);
    flex-shrink: 0;
  }
  .name {
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .rename {
    width: 100%;
    min-width: 0;
    padding: 1px 5px;
    border-radius: 4px;
    border: 1px solid var(--accent, #D4D4D4);
    background-color: var(--surface, #0A0A0A);
    font: inherit;
    font-size: 14px;
    color: var(--text, #EDEDED);
  }
  .host-badge {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
    font-size: 10px;
    line-height: 1.5;
    padding: 0 5px;
    border-radius: 4px;
    color: var(--text-muted, #A3A3A3);
    background-color: var(--surface-3, #242424);
  }
  /* AC-060-02: the tag tells the phase of the host the line came from. */
  [data-host-badge][data-host-state="reconnecting"] {
    color: var(--accent, #D4D4D4);
  }
  [data-host-badge][data-host-state="attention"] {
    color: var(--attention, #E9A23B);
  }
  .badge-spinner {
    display: inline-block;
    line-height: 1;
    animation: spin 1.5s linear infinite;
  }
  @keyframes spin {
    100% {
      transform: rotate(360deg);
    }
  }
  .offline-tag {
    flex-shrink: 0;
    font-size: 10px;
    color: var(--text-muted, #A3A3A3);
  }
  .branch {
    display: flex;
    align-items: center;
    gap: 4px;
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--text-dim, #6B6B6B);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .branch-icon {
    display: flex;
    align-items: center;
    flex-shrink: 0;
  }
  .trailing {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
    margin-left: 6px;
    height: 18px;
  }
  .status {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    border-radius: 999px;
    font-size: 11px;
    line-height: 1;
  }
  .status-count {
    font-variant-numeric: tabular-nums;
  }
  [data-row-status="working"] {
    color: var(--working, #4ADE80);
    background-color: var(--surface-3, #242424);
    padding: 3px 7px 3px 5px;
  }
  [data-row-status="waiting"] {
    color: var(--attention, #E9A23B);
  }
  [data-row-status="done"] {
    color: var(--working, #4ADE80);
  }
  /* AC-045-01: a 2 px accent line marks where the row would land. */
  .drop-indicator {
    position: absolute;
    left: 6px;
    right: 6px;
    height: 2px;
    border-radius: 1px;
    background-color: var(--accent, #D4D4D4);
    pointer-events: none;
  }
  [data-drop-indicator="before"] {
    top: -1px;
  }
  [data-drop-indicator="after"] {
    bottom: -1px;
  }
  .row[data-drop-refused] {
    cursor: not-allowed;
  }
  .actions-slot,
  .tabs-slot {
    display: contents;
  }
</style>
