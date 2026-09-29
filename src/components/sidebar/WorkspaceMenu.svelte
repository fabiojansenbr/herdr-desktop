<script lang="ts">
  // Menu of one workspace row (spec 044, AC-044-02), in the order of
  // `design/v2-02-organizar-workspaces.png`: Renomear (F2), Cor with the six palette swatches,
  // Fixar no topo, Mover para coleção ▸ (the store collections plus Nova coleção…), Copiar
  // caminho, Ocultar da lateral and Fechar workspace… in red.
  //
  // Colour, pinning and hiding are client preferences: they go to `workspace_pref_set` and the
  // engine is never told. Renaming and closing are the engine actions 025 already exposes, each
  // sent once through the controller. Moving reuses `group_assign` by root cwd (AC-025-02).
  //
  // Spec 049 (AC-049-01/02): the menu and its submenu are mounted on the window layer of
  // `menu-layer.ts` instead of inside the scrolling list, which clipped the submenu at the
  // sidebar edge, and both close on a pointerdown anywhere outside or on Esc.
  import type { FrameContext } from "../../shell/frame-context";
  import { t } from "../../i18n/index.svelte";
  import { GROUP_PALETTE } from "../projects/tree-model";
  import {
    baseMenuStyle,
    dismissOn,
    dropdownStyle,
    flyoutStyle,
    portal,
    MENU_Z_INDEX,
    SUBMENU_Z_INDEX,
  } from "./menu-layer";
  import type { SidebarRow } from "./sidebar-model";

  let {
    ctx,
    row,
    anchor = null,
    trigger = null,
    onclose,
    onrename,
  }: {
    ctx: FrameContext;
    row: SidebarRow;
    /** Button or row the menu was opened from: the rectangle it is placed by (AC-049-01). */
    anchor?: HTMLElement | null;
    /** The `…` that toggles on click: its pointerdown must not close and reopen the menu. */
    trigger?: HTMLElement | null;
    onclose: () => void;
    onrename: () => void;
  } = $props();

  let submenu = $state(false);
  let creating = $state(false);
  let newName = $state("");
  let confirming = $state(false);

  let menuEl = $state<HTMLDivElement | undefined>();
  let submenuEl = $state<HTMLDivElement | undefined>();
  let moveEl = $state<HTMLButtonElement | undefined>();
  let menuStyle = $state(baseMenuStyle(MENU_Z_INDEX));
  let submenuStyle = $state(baseMenuStyle(SUBMENU_Z_INDEX));

  // Measured like the host overlays (032/048): the layer shrinks to its content first, then is
  // placed from the anchor and clamped, so it never leaves the window.
  $effect(() => {
    // Re-measure whenever the menu changes height.
    void confirming;
    void submenu;
    if (!menuEl) return;
    const box = menuEl.getBoundingClientRect();
    menuStyle = dropdownStyle(anchor?.getBoundingClientRect() ?? null, box.width, box.height, MENU_Z_INDEX);
  });

  $effect(() => {
    void creating;
    if (!submenu || !submenuEl) return;
    const box = submenuEl.getBoundingClientRect();
    submenuStyle = flyoutStyle(moveEl?.getBoundingClientRect() ?? null, box.width, box.height, SUBMENU_Z_INDEX);
  });

  $effect(() =>
    dismissOn({
      inside: () => [menuEl, submenuEl],
      ignore: () => [trigger],
      onDismiss: onclose,
    }),
  );

  const projects = $derived(ctx.controllers.projects);
  const collections = $derived(ctx.navigator?.snapshot?.collections ?? []);
  const current = $derived(collections.find((c) => row.projectId !== null && c.project_ids.includes(row.projectId))?.id ?? null);
  /** Preferences need the root the engine reported; a row without one only renames and closes. */
  const keyed = $derived(row.prefRoot !== null);
  const live = $derived(row.workspaceId !== null);

  function pref(patch: { color?: string; pinned?: boolean; hidden?: boolean }) {
    if (row.prefRoot === null) return;
    void projects.setWorkspacePref({ endpoint_profile_id: row.endpoint, root: row.prefRoot, ...patch });
    onclose();
  }

  function moveTo(groupId: string) {
    if (!row.cwd) return;
    void projects.assignToGroup(groupId, {
      endpoint_profile_id: row.endpoint,
      session_name: row.session,
      cwd: row.cwd,
      label: row.name,
    });
    onclose();
  }

  /** Nova coleção…: the group is created first, then the row moves into the one just created. */
  async function createAndMove() {
    const name = newName.trim();
    if (!name) return;
    const before = new Set(collections.map((c) => c.id));
    await projects.createGroup(name);
    const created = (ctx.navigator?.snapshot?.collections ?? []).find((c) => !before.has(c.id) && c.name === name);
    if (created) moveTo(created.id);
    else onclose();
  }

  async function copyPath() {
    if (row.cwd) await navigator.clipboard?.writeText(row.cwd);
    onclose();
  }

  function closeWorkspace() {
    if (row.workspaceId) void projects.closeWorkspace(row.endpoint, row.workspaceId);
    onclose();
  }
</script>

<div
  class="menu"
  data-workspace-menu
  role="menu"
  aria-label={t("sidebar.row.actions", { name: row.name })}
  tabindex="-1"
  style={menuStyle}
  bind:this={menuEl}
  use:portal
>
  <button type="button" class="item" data-menu-item="rename" role="menuitem" disabled={!live} onclick={onrename}>
    <span class="glyph" aria-hidden="true">
      <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
        <path d="M12 20h9" /><path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4Z" />
      </svg>
    </span>
    <span class="label">{t("sidebar.rename")}</span>
    <kbd class="hint">F2</kbd>
  </button>

  <div class="item colors" data-menu-item="color" role="group" aria-label={t("sidebar.color")}>
    <span class="glyph" aria-hidden="true">
      <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
        <path d="M12 3a9 9 0 1 0 0 18c1 0 1.6-.7 1.6-1.5 0-.5-.2-.8-.5-1.1-.3-.3-.5-.7-.5-1.1 0-.8.7-1.5 1.6-1.5H16a5 5 0 0 0 5-5c0-4.4-4-8-9-8Z" /><circle cx="7.5" cy="12" r="1" /><circle cx="9.5" cy="8" r="1" /><circle cx="14.5" cy="7.5" r="1" />
      </svg>
    </span>
    <span class="label">{t("sidebar.color")}</span>
    <span class="swatches">
      {#each GROUP_PALETTE as color (color)}
        <button
          type="button"
          class="swatch"
          data-color-swatch={color}
          aria-label={t("sidebar.colorSwatch", { color })}
          aria-pressed={row.color === color}
          disabled={!keyed}
          style="background-color: {color};"
          onclick={() => pref({ color })}
        ></button>
      {/each}
    </span>
  </div>

  <button type="button" class="item" data-menu-item="pin" role="menuitem" disabled={!keyed} onclick={() => pref({ pinned: !row.pinned })}>
    <span class="glyph" aria-hidden="true">
      <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
        <path d="M9 3h6l-1 5 3 3v2H7v-2l3-3-1-5Z" /><path d="M12 13v8" />
      </svg>
    </span>
    <span class="label">{row.pinned ? t("sidebar.menu.unpin") : t("sidebar.menu.pin")}</span>
  </button>

  <div class="submenu-anchor">
    <button
      type="button"
      class="item"
      data-menu-item="move"
      role="menuitem"
      aria-haspopup="menu"
      aria-expanded={submenu}
      disabled={!row.cwd}
      bind:this={moveEl}
      onclick={() => (submenu = !submenu)}
    >
      <span class="glyph" aria-hidden="true">
        <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
          <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7Z" /><path d="m12 11 3 3-3 3" />
        </svg>
      </span>
      <span class="label">{t("sidebar.menu.move")}</span>
      <span class="caret" aria-hidden="true">
        <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="m9 6 6 6-6 6" />
        </svg>
      </span>
    </button>

    {#if submenu}
      <div
        class="menu submenu"
        data-collection-submenu
        role="menu"
        aria-label={t("sidebar.menu.move")}
        style={submenuStyle}
        bind:this={submenuEl}
        use:portal
      >
        {#each collections as collection, position (collection.id)}
          <button
            type="button"
            class="item"
            role="menuitemradio"
            aria-checked={current === collection.id}
            data-move-collection={collection.id}
            data-current={current === collection.id ? "true" : undefined}
            onclick={() => moveTo(collection.id)}
          >
            <span class="dot" style="background-color: {GROUP_PALETTE[position % GROUP_PALETTE.length]};" aria-hidden="true"></span>
            <span class="label">{collection.name}</span>
            {#if current === collection.id}<span class="check" aria-hidden="true">✓</span>{/if}
          </button>
        {/each}

        <div class="separator" role="none"></div>

        {#if creating}
          <input
            class="inline-input"
            data-new-collection-input
            aria-label={t("sidebar.menu.newCollectionName")}
            placeholder={t("sidebar.menu.collectionName")}
            bind:value={newName}
            onkeydown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                void createAndMove();
              } else if (event.key === "Escape") {
                event.preventDefault();
                creating = false;
              }
            }}
          />
        {:else}
          <button type="button" class="item" role="menuitem" data-new-collection onclick={() => (creating = true)}>
            <span class="glyph" aria-hidden="true">+</span>
            <span class="label">{t("sidebar.menu.newCollection")}</span>
          </button>
        {/if}
      </div>
    {/if}
  </div>

  <button type="button" class="item" data-menu-item="copy-path" role="menuitem" disabled={!row.cwd} onclick={copyPath}>
    <span class="glyph" aria-hidden="true">
      <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linejoin="round">
        <rect x="9" y="9" width="11" height="11" rx="2" /><path d="M5 15V5a2 2 0 0 1 2-2h8" />
      </svg>
    </span>
    <span class="label">{t("sidebar.menu.copyPath")}</span>
  </button>

  <div class="separator" role="none"></div>

  <button type="button" class="item" data-menu-item="hide" role="menuitem" disabled={!keyed} onclick={() => pref({ hidden: true })}>
    <span class="glyph" aria-hidden="true">
      <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
        <path d="M3 3l18 18" /><path d="M10.6 5.2A9.6 9.6 0 0 1 12 5c5 0 9 4.5 9 7 0 .9-.6 2.1-1.6 3.2" /><path d="M6.3 6.7C4 8.2 3 10.3 3 12c0 2.5 4 7 9 7 1.5 0 2.9-.4 4.1-1" /><path d="M9.9 9.9a3 3 0 0 0 4.2 4.2" />
      </svg>
    </span>
    <span class="label">{t("sidebar.menu.hide")}</span>
  </button>

  {#if confirming}
    <div class="confirm" role="group" aria-label={t("sidebar.menu.close")}>
      <span class="confirm-text">{t("sidebar.menu.closeConfirm", { name: row.name })}</span>
      <button type="button" class="confirm-action danger" data-confirm-close onclick={closeWorkspace}>{t("sidebar.close")}</button>
      <button type="button" class="confirm-action" data-cancel-close onclick={() => (confirming = false)}>{t("sidebar.cancel")}</button>
    </div>
  {:else}
    <button type="button" class="item" data-menu-item="close" role="menuitem" disabled={!live} onclick={() => (confirming = true)}>
      <span class="glyph" aria-hidden="true">
        <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
          <path d="M4 7h16" /><path d="M10 11v6" /><path d="M14 11v6" /><path d="M6 7l1 13h10l1-13" /><path d="M9 7V4h6v3" />
        </svg>
      </span>
      <span class="label">{t("sidebar.menu.closeItem")}</span>
    </button>
  {/if}
</div>

<style>
  /* AC-049-01: a layer of the window, placed by `menu-layer.ts`, never a child of the list. */
  .menu {
    position: fixed;
    min-width: 232px;
    padding: 6px;
    display: flex;
    flex-direction: column;
    gap: 1px;
    border-radius: 10px;
    border: 1px solid var(--surface-3, #242424);
    background-color: var(--surface-2, #171717);
    box-shadow: 0 12px 32px rgb(0 0 0 / 45%);
    font-size: 13.5px;
    color: var(--text, #ededed);
    text-align: left;
  }
  .item {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 6px 8px;
    background: transparent;
    border: none;
    border-radius: 6px;
    font: inherit;
    font-size: 13.5px;
    color: inherit;
    text-align: left;
    cursor: pointer;
  }
  .item:hover:not(:disabled) {
    background-color: var(--surface-3, #242424);
  }
  .item:disabled {
    opacity: 0.4;
    cursor: default;
  }
  .item:focus-visible {
    background-color: var(--surface-3, #242424);
    outline: 2px solid var(--accent, #d4d4d4);
    outline-offset: -2px;
  }
  .glyph {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 16px;
    color: var(--text-muted, #a3a3a3);
    flex-shrink: 0;
  }
  .label {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .hint,
  .check {
    flex-shrink: 0;
    font-family: inherit;
    font-size: 11px;
    color: var(--text-muted, #a3a3a3);
  }
  .check {
    color: var(--text, #ededed);
  }
  .caret {
    display: flex;
    align-items: center;
    color: var(--text-muted, #a3a3a3);
    flex-shrink: 0;
  }
  .colors {
    cursor: default;
  }
  .swatches {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
  }
  .swatch {
    width: 14px;
    height: 14px;
    padding: 0;
    border: none;
    border-radius: 999px;
    cursor: pointer;
  }
  .swatch[aria-pressed="true"] {
    box-shadow: 0 0 0 2px var(--surface-2, #171717), 0 0 0 3px currentColor;
  }
  .swatch:focus-visible {
    outline: 2px solid var(--accent, #d4d4d4);
    outline-offset: 1px;
  }
  .dot {
    width: 9px;
    height: 9px;
    border-radius: 2px;
    flex-shrink: 0;
  }
  .separator {
    height: 1px;
    margin: 4px 2px;
    background-color: var(--border, #1c1c1c);
  }
  .inline-input {
    width: 100%;
    padding: 5px 8px;
    border-radius: 6px;
    border: 1px solid var(--accent, #d4d4d4);
    background-color: var(--surface, #0a0a0a);
    font: inherit;
    font-size: 13.5px;
    color: var(--text, #ededed);
  }
  [data-menu-item="close"] {
    color: var(--error, #f87171);
  }
  [data-menu-item="close"] .glyph {
    color: var(--error, #f87171);
  }
  .confirm {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 8px;
  }
  .confirm-text {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12px;
    color: var(--text-muted, #a3a3a3);
  }
  .confirm-action {
    flex-shrink: 0;
    padding: 3px 8px;
    border-radius: 6px;
    border: 1px solid var(--surface-3, #242424);
    background: transparent;
    font: inherit;
    font-size: 13.5px;
    color: var(--text, #ededed);
    cursor: pointer;
  }
  .confirm-action:hover,
  .confirm-action:focus-visible {
    background-color: var(--surface-3, #242424);
  }
  .confirm-action:focus-visible {
    outline: 2px solid var(--accent, #d4d4d4);
  }
  .confirm-action.danger {
    color: var(--error, #f87171);
    border-color: var(--error, #f87171);
  }
</style>
