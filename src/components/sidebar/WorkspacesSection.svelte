<script lang="ts">
  // WORKSPACES section (spec 041, AC-041-02): every workspace of every host, regrouped by the
  // store's collections and then "Sem coleção" (P6).
  // Spec 044 (AC-044-03, P5): the workspaces hidden by preference leave their collection for the
  // collapsible `Ocultos (N)` item that closes the section, where each line can be shown again.
  // Spec 045: the collapse survives a reload (AC-045-03) — a stored collection keeps it in the
  // store, the two synthetic buckets in `localStorage` — and this section runs the plan of a
  // drag (AC-045-01), so a drop is one chain of store commands wherever it landed.
  // Spec 060 (AC-060-03): a host that is still opening its connection and has no workspace in the
  // snapshot yet has no line to dim, so `Sem coleção` carries one waiting line for it — with the
  // host name and a spinner, no action at all — which leaves as soon as the host answers.
  // Spec 046: the `+` of the header asks for the "Novo workspace" modal, exactly like the palette
  // command and the title bar's "Abrir projeto…" on an SSH host. The modal itself belongs to the
  // window (`NewWorkspaceHost.svelte`), never to this section: the 047 rail replaces the section
  // whenever the sidebar is collapsed, and a request must not be lost then.
  import type { FrameContext } from "../../shell/frame-context";
  import { t } from "../../i18n/index.svelte";
  import { requestNewWorkspace } from "../../projects/new-workspace";
  import CollectionGroup from "./CollectionGroup.svelte";
  import type { DropSpot } from "./drag";
  import {
    buildSidebarSections,
    effectiveAgents,
    HIDDEN_ID,
    readCollapsed,
    runDropPlan,
    UNGROUPED_ID,
    writeCollapsed,
    type SidebarRow,
  } from "./sidebar-model";

  let { ctx }: { ctx: FrameContext } = $props();

  /** Collapse of the two synthetic buckets, seeded from this browser's storage. */
  let bucketCollapsed = $state<Record<string, boolean>>({
    [UNGROUPED_ID]: readCollapsed(UNGROUPED_ID, false),
    [HIDDEN_ID]: readCollapsed(HIDDEN_ID, true),
  });
  const hiddenOpen = $derived(!bucketCollapsed[HIDDEN_ID]);

  function toggleBucket(id: string) {
    const next = !bucketCollapsed[id];
    bucketCollapsed[id] = next;
    writeCollapsed(id, next);
  }

  /**
   * Collapse a stored collection asked for but the store has not confirmed yet. The header must
   * answer the click at once, so the toggle is optimistic — and the entry is dropped as soon as
   * the write comes back, whatever it answered: the store is the truth again, so a refused write
   * falls back to the recorded state instead of leaving the header claiming something else.
   */
  let pending = $state<Record<string, boolean>>({});

  /** A stored collection records its collapse in the store, so the next window opens the same. */
  async function toggleCollection(id: string, collapsed: boolean) {
    if (id === UNGROUPED_ID) {
      toggleBucket(id);
      return;
    }
    const next = !collapsed;
    pending[id] = next;
    await ctx.controllers.projects.setGroupCollapsed(id, next);
    // A newer click already asked for something else: that one owns the optimistic state now.
    if (pending[id] === next) delete pending[id];
  }

  /** Runs the plan of the drop in order; every step is an existing catalog command (spec 045). */
  const runDrop = (spot: DropSpot) => runDropPlan(ctx.controllers.projects, spot);

  const hosts = $derived(ctx.connections?.view?.hub.hosts ?? []);
  const agents = $derived(effectiveAgents(ctx.agents?.agents ?? [], hosts));
  const sections = $derived(
    buildSidebarSections({
      hosts,
      groups: ctx.navigator?.snapshot?.collections ?? [],
      projects: ctx.navigator?.snapshot?.projects ?? [],
      agents,
      opening: ctx.navigator?.opening ?? {},
      selectedEndpoint: ctx.selectedEndpoint,
      prefs: ctx.navigator?.snapshot?.workspace_prefs ?? [],
    }),
  );
  const collections = $derived(sections.collections);
  const hidden = $derived(sections.hidden);
  /** AC-060-03: the hosts `Sem coleção` is waiting for; never `Nenhum workspace` over them. */
  const waiting = $derived(sections.waiting);
  const empty = $derived(
    collections.every((collection) => collection.rows.length === 0) && hidden.length === 0 && waiting.length === 0,
  );

  function unhide(row: SidebarRow) {
    if (row.prefRoot === null) return;
    void ctx.controllers.projects.setWorkspacePref({ endpoint_profile_id: row.endpoint, root: row.prefRoot, hidden: false });
  }
</script>

<section class="workspaces" data-sidebar-workspaces aria-label={t("sidebar.workspaces")}>
  <header class="section-head">
    <span class="section-title">{t("sidebar.workspaces")}</span>
    <button
      type="button"
      class="new-workspace"
      data-new-workspace
      data-icon="folder-plus"
      aria-label={t("sidebar.workspaces.new")}
      title={t("sidebar.workspaces.new")}
      onclick={() => requestNewWorkspace(null)}
    >
      <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <path d="M4 20h16a1 1 0 0 0 1-1v-9a1 1 0 0 0-1-1H12l-2-3H4a1 1 0 0 0-1 1v12a1 1 0 0 0 1 1Z" />
        <path d="M12 12v6" /><path d="M9 15h6" />
      </svg>
    </button>
  </header>

  <div class="section-body">
    {#each collections as collection (collection.id)}
      {@const isCollapsed = collection.ungrouped
        ? Boolean(bucketCollapsed[UNGROUPED_ID])
        : (pending[collection.id] ?? collection.collapsed)}
      <CollectionGroup
        {ctx}
        {collection}
        collapsed={isCollapsed}
        onToggle={() => void toggleCollection(collection.id, isCollapsed)}
        onDrop={(spot) => void runDrop(spot)}
      />
      {#if collection.ungrouped && !isCollapsed && waiting.length > 0}
        <ul class="waiting-rows" data-ungrouped-waiting aria-label={t("sidebar.waitingHosts")}>
          {#each waiting as host (host.endpoint)}
            <li class="waiting-row" data-host-waiting={host.endpoint}>
              <span class="waiting-spinner" data-host-waiting-spinner aria-hidden="true">⟳</span>
              <span class="waiting-label">{host.label}</span>
            </li>
          {/each}
        </ul>
      {/if}
    {/each}

    {#if hidden.length > 0}
      <section class="hidden-group" data-hidden-group={hidden.length}>
        <button
          type="button"
          class="hidden-head"
          data-hidden-toggle
          aria-expanded={hiddenOpen}
          onclick={() => toggleBucket(HIDDEN_ID)}
        >
          <span class="caret" data-hidden-caret={hiddenOpen ? "chevron-down" : "chevron-right"} aria-hidden="true">
            {#if hiddenOpen}
              <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <path d="m6 9 6 6 6-6" />
              </svg>
            {:else}
              <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <path d="m9 6 6 6-6 6" />
              </svg>
            {/if}
          </span>
          <span class="hidden-name">{t("sidebar.hidden.title", { count: hidden.length })}</span>
        </button>

        {#if hiddenOpen}
          <ul class="hidden-rows" aria-label={t("sidebar.hidden.list")}>
            {#each hidden as row (row.id)}
              <li class="hidden-row" data-hidden-row={row.id}>
                <span class="hidden-label" title={row.cwd ?? row.name}>{row.name}</span>
                {#if row.hostBadge}<span class="hidden-badge">{row.hostBadge}</span>{/if}
                <button type="button" class="unhide" data-unhide onclick={() => unhide(row)}>{t("sidebar.hidden.unhide")}</button>
              </li>
            {/each}
          </ul>
        {/if}
      </section>
    {/if}

    {#if empty}
      <p class="empty">{t("sidebar.workspaces.empty")}</p>
    {/if}
  </div>
</section>

<style>
  .workspaces {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    padding-top: 6px;
  }
  .section-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    height: 26px;
    padding: 0 12px;
    flex-shrink: 0;
  }
  .new-workspace {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    padding: 0;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--text-muted, #8c93a3);
    cursor: pointer;
  }
  .new-workspace:hover {
    background-color: var(--surface-2, #171a21);
    color: var(--text, #e7e9ee);
  }
  .new-workspace:focus-visible {
    outline: 2px solid var(--accent, #8fa8ff);
    outline-offset: -2px;
  }
  .section-title {
    font-size: 13px;
    font-weight: 500;
    color: var(--text-dim, #6b6b6b);
  }
  .section-body {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 0 4px 8px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  /* AC-060-03: the waiting line of `Sem coleção`, indented like the rows of the bucket. */
  .waiting-rows {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
  }
  .waiting-row {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 10px 6px 22px;
    font-size: 13px;
    color: var(--text-dim, #6b6b6b);
  }
  .waiting-spinner {
    display: inline-block;
    font-size: 11px;
    line-height: 1;
    flex-shrink: 0;
    color: var(--accent, #8fa8ff);
    animation: spin 1.5s linear infinite;
  }
  @keyframes spin {
    100% {
      transform: rotate(360deg);
    }
  }
  .waiting-label {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .hidden-group {
    display: flex;
    flex-direction: column;
    margin-top: 4px;
  }
  .hidden-head {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 6px 10px;
    background: transparent;
    border: none;
    border-radius: 6px;
    font: inherit;
    font-size: 12px;
    color: var(--text-muted, #8c93a3);
    cursor: pointer;
    text-align: left;
  }
  .hidden-head:hover {
    background-color: var(--surface-2, #171a21);
  }
  .hidden-head:focus-visible {
    outline: 2px solid var(--accent, #8fa8ff);
    outline-offset: -2px;
  }
  .caret {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 12px;
    flex-shrink: 0;
  }
  .hidden-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .hidden-rows {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
  }
  .hidden-row {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 10px 4px 22px;
    font-size: 12px;
    color: var(--text-muted, #8c93a3);
  }
  .hidden-label {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .hidden-badge {
    flex-shrink: 0;
    font-size: 10px;
    line-height: 1.5;
    padding: 0 5px;
    border-radius: 4px;
    background-color: var(--surface-3, #1e222b);
  }
  .unhide {
    flex-shrink: 0;
    padding: 2px 7px;
    border-radius: 6px;
    border: 1px solid var(--border, #242833);
    background: transparent;
    font: inherit;
    font-size: 11px;
    color: var(--text, #e7e9ee);
    cursor: pointer;
  }
  .unhide:hover {
    background-color: var(--surface-3, #1e222b);
  }
  .empty {
    margin: 0;
    padding: 20px 16px;
    text-align: center;
    font-size: 13px;
    color: var(--text-muted, #8c93a3);
  }
</style>
