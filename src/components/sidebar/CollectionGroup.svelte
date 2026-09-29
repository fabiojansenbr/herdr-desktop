<script lang="ts">
  // One collection of the WORKSPACES section (spec 041, AC-041-02): the colour square, the name,
  // the number of lines, and a header that collapses only this collection.
  //
  // Spec 045 adds to the header: it is the drop target that moves a workspace into this
  // collection (AC-045-01), it carries the `…` menu with Renomear / Cor / Excluir and the inline
  // rename editor (AC-045-02). The synthetic buckets ("Sem coleção") accept the drop — landing
  // there only drops the association — but have no menu, because there is no store row to edit.
  import type { FrameContext } from "../../shell/frame-context";
  import { t } from "../../i18n/index.svelte";
  import CollectionMenu from "./CollectionMenu.svelte";
  import { activeDrag, endDrag, planDrop, type DropSpot } from "./drag";
  import type { SidebarCollection } from "./sidebar-model";
  import WorkspaceItem from "./WorkspaceItem.svelte";

  let {
    ctx,
    collection,
    collapsed,
    onToggle,
    onDrop,
  }: {
    ctx: FrameContext;
    collection: SidebarCollection;
    collapsed: boolean;
    onToggle: () => void;
    /** Runs the plan of a drop landed on this header or on one of its rows (spec 045). */
    onDrop: (spot: DropSpot) => void;
  } = $props();

  let menu = $state(false);
  let menuButton = $state<HTMLButtonElement | undefined>();
  /** Rectangle the 049 layer places the menu by: the `…`, or the header on a right click. */
  let menuAnchor = $state<HTMLElement | null>(null);
  let headEl = $state<HTMLElement | undefined>();
  let renaming = $state(false);
  let draft = $state("");
  let input = $state<HTMLInputElement | null>(null);
  let over = $state(false);

  /** A stored collection can be renamed, coloured and deleted; the buckets cannot. */
  const editable = $derived(!collection.ungrouped);

  function startRename() {
    menu = false;
    draft = collection.name;
    renaming = true;
  }

  function commitRename() {
    // AC-045-02: an empty name sends nothing and leaves the editor open.
    const name = draft.trim();
    if (!name) return;
    void ctx.controllers.projects.renameGroup(collection.id, name);
    renaming = false;
  }

  function onDragOver(event: DragEvent) {
    const source = activeDrag();
    if (!source) return;
    if (planDrop(source, { kind: "collection", collectionId: collection.id }).steps.length === 0) return;
    event.preventDefault();
    over = true;
  }

  function drop(event: DragEvent) {
    event.preventDefault();
    over = false;
    onDrop({ kind: "collection", collectionId: collection.id });
    endDrag();
  }

  $effect(() => {
    if (renaming) input?.select();
  });
</script>

<section class="collection" data-collection={collection.id}>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="collection-head"
    data-collection-head
    bind:this={headEl}
    data-drop-over={over ? "true" : undefined}
    data-collection-editable={editable ? "true" : undefined}
    data-collection-menu-open={menu && editable ? "true" : undefined}
    ondragover={onDragOver}
    ondragleave={() => (over = false)}
    ondrop={drop}
    oncontextmenu={(event) => {
      if (!editable) return;
      event.preventDefault();
      menuAnchor = headEl ?? null;
      menu = true;
    }}
  >
    <button
      type="button"
      class="toggle"
      data-collection-toggle
      aria-expanded={!collapsed}
      aria-label={t("sidebar.collection.header", { name: collection.name, count: collection.count })}
      onclick={onToggle}
    >
      <!-- AC-044-04: the 12 px chevron of the design, not the text caret the 041 window drew. -->
      <span class="caret" data-collection-caret={collapsed ? "chevron-right" : "chevron-down"} aria-hidden="true">
        {#if collapsed}
          <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="m9 6 6 6-6 6" />
          </svg>
        {:else}
          <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="m6 9 6 6 6-6" />
          </svg>
        {/if}
      </span>
      <span class="swatch" data-collection-color style="background-color: {collection.color};" aria-hidden="true"></span>
      {#if renaming}
        <!-- svelte-ignore a11y_autofocus -->
        <input
          class="rename"
          data-collection-rename-input
          bind:this={input}
          bind:value={draft}
          aria-label={t("sidebar.row.rename", { name: collection.name })}
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
        <span class="collection-name" data-collection-name>{collection.name}</span>
      {/if}
      <!-- AC-054-01: the slot the `…` is laid over, so every header counts on the same edge. -->
      <span class="count-slot" data-collection-count-slot>
        <span class="collection-count" data-collection-count>{collection.count}</span>
      </span>
    </button>

    {#if editable}
      <button
        type="button"
        class="menu-button"
        data-collection-menu-button
        bind:this={menuButton}
        aria-label={t("sidebar.collection.actions", { name: collection.name })}
        aria-haspopup="menu"
        aria-expanded={menu}
        onclick={(event) => {
          event.stopPropagation();
          menuAnchor = event.currentTarget;
          menu = !menu;
        }}
      >
        <svg viewBox="0 0 24 24" width="13" height="13" fill="currentColor" aria-hidden="true">
          <circle cx="5" cy="12" r="1.6" /><circle cx="12" cy="12" r="1.6" /><circle cx="19" cy="12" r="1.6" />
        </svg>
      </button>
    {/if}

    {#if menu && editable}
      <CollectionMenu
        {ctx}
        {collection}
        anchor={menuAnchor}
        trigger={menuButton}
        onclose={() => (menu = false)}
        onrename={startRename}
      />
    {/if}
  </div>

  {#if !collapsed && collection.rows.length > 0}
    <ul class="rows" aria-label={collection.name}>
      {#each collection.rows as row (row.id)}
        <li class="row-item"><WorkspaceItem {ctx} {row} collectionId={collection.id} {onDrop} /></li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  .collection {
    display: flex;
    flex-direction: column;
  }
  .collection-head {
    position: relative;
    display: flex;
    align-items: center;
    gap: 2px;
    width: 100%;
    border-radius: 6px;
    transition: background-color 0.15s ease;
  }
  .collection-head:hover {
    background-color: var(--surface-2, #171a21);
  }
  /* AC-045-01: the header lights up while it is the drop target. */
  .collection-head[data-drop-over="true"] {
    box-shadow: inset 0 0 0 2px var(--accent, #8fa8ff);
  }
  .toggle {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: 1;
    min-width: 0;
    /* AC-054-01: the inset `.hidden-head` of the section already leaves on its right. */
    padding: 6px 10px;
    background: transparent;
    border: none;
    border-radius: 6px;
    font: inherit;
    font-size: 13px;
    font-weight: 500;
    color: var(--text-muted, #a3a3a3);
    cursor: pointer;
    user-select: none;
    text-align: left;
  }
  .toggle:focus-visible,
  .menu-button:focus-visible {
    outline: 2px solid var(--accent, #8fa8ff);
    outline-offset: -2px;
  }
  /* AC-054-01: out of the flow, over the count slot — in the flow its 18 px pushed the count of
     an editable collection left of the count of `Sem coleção`, which has no `…`. */
  .menu-button {
    position: absolute;
    right: 10px;
    top: 50%;
    transform: translateY(-50%);
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
    opacity: 0;
    transition: opacity 0.12s ease;
  }
  .collection-head:hover .menu-button,
  .collection-head:focus-within .menu-button,
  .menu-button[aria-expanded="true"] {
    opacity: 1;
  }
  .menu-button:hover {
    background-color: var(--surface-3, #1e222b);
    color: var(--text, #e7e9ee);
  }
  .caret {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 12px;
    color: var(--text-muted, #8c93a3);
    flex-shrink: 0;
  }
  .swatch {
    width: 8px;
    height: 8px;
    border-radius: 2px;
    flex-shrink: 0;
  }
  .collection-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 13px;
    font-weight: 500;
    color: var(--text-muted, #a3a3a3);
  }
  .rename {
    flex: 1;
    min-width: 0;
    padding: 1px 5px;
    border-radius: 4px;
    border: 1px solid var(--accent, #8fa8ff);
    background-color: var(--surface, #111318);
    font: inherit;
    font-size: 13px;
    color: var(--text, #e7e9ee);
  }
  .count-slot {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    min-width: 18px;
    flex-shrink: 0;
    transition: opacity 0.12s ease;
  }
  /* AC-054-02: on hover or keyboard focus the `…` takes the place of the count. Only an editable
     header swaps; `Sem coleção` and `Ocultos` have no `…` to put there. */
  .collection-head[data-collection-editable]:hover .count-slot,
  .collection-head[data-collection-editable]:focus-within .count-slot,
  .collection-head[data-collection-menu-open] .count-slot {
    opacity: 0;
  }
  .collection-count {
    font-size: 12px;
    color: var(--text-dim, #6b6b6b);
    flex-shrink: 0;
  }
  .rows {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
  }
  .row-item {
    margin: 0;
    padding: 0;
  }
</style>
