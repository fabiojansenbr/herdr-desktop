<script lang="ts">
  // Menu of a collection header (spec 045, AC-045-02): Renomear (inline, on the header itself),
  // Cor with the six palette swatches and Excluir coleção… with an inline confirmation.
  //
  // All three edit the desktop catalog only: deleting a collection drops the collection row, its
  // projects stay in the catalog without one ("Sem coleção") and no workspace is closed — the
  // engine is never told. Only stored collections get this menu; the synthetic buckets have no
  // row to edit.
  //
  // Spec 049 (AC-049-01/02): like the workspace menu, it is mounted on the window layer of
  // `menu-layer.ts` — inside the list it was clipped by `WorkspacesSection` and by
  // `Sidebar.svelte` — and it closes on a pointerdown anywhere outside or on Esc.
  import type { FrameContext } from "../../shell/frame-context";
  import { t } from "../../i18n/index.svelte";
  import { GROUP_PALETTE } from "../projects/tree-model";
  import { baseMenuStyle, dismissOn, dropdownStyle, portal, MENU_Z_INDEX } from "./menu-layer";
  import type { SidebarCollection } from "./sidebar-model";

  let {
    ctx,
    collection,
    anchor = null,
    trigger = null,
    onclose,
    onrename,
  }: {
    ctx: FrameContext;
    collection: SidebarCollection;
    /** Button or header the menu was opened from: the rectangle it is placed by (AC-049-01). */
    anchor?: HTMLElement | null;
    /** The `…` that toggles on click: its pointerdown must not close and reopen the menu. */
    trigger?: HTMLElement | null;
    onclose: () => void;
    onrename: () => void;
  } = $props();

  let confirming = $state(false);
  let menuEl = $state<HTMLDivElement | undefined>();
  let menuStyle = $state(baseMenuStyle(MENU_Z_INDEX));

  $effect(() => {
    // The inline confirmation changes the height: re-measure before placing it again.
    void confirming;
    if (!menuEl) return;
    const box = menuEl.getBoundingClientRect();
    menuStyle = dropdownStyle(anchor?.getBoundingClientRect() ?? null, box.width, box.height, MENU_Z_INDEX);
  });

  $effect(() => dismissOn({ inside: () => [menuEl], ignore: () => [trigger], onDismiss: onclose }));

  const projects = $derived(ctx.controllers.projects);

  function pickColor(color: string) {
    void projects.setGroupColor(collection.id, color);
    onclose();
  }

  function remove() {
    void projects.deleteGroup(collection.id);
    onclose();
  }
</script>

<div
  class="menu"
  data-collection-menu
  role="menu"
  aria-label={t("sidebar.collection.actions", { name: collection.name })}
  tabindex="-1"
  style={menuStyle}
  bind:this={menuEl}
  use:portal
>
  <button type="button" class="item" data-collection-menu-item="rename" role="menuitem" onclick={onrename}>
    <span class="glyph" aria-hidden="true">
      <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
        <path d="M12 20h9" /><path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4Z" />
      </svg>
    </span>
    <span class="label">{t("sidebar.rename")}</span>
  </button>

  <div class="item colors" data-collection-menu-item="color" role="group" aria-label={t("sidebar.collection.color")}>
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
          data-collection-swatch={color}
          aria-label={t("sidebar.colorSwatch", { color })}
          aria-pressed={collection.color === color}
          style="background-color: {color};"
          onclick={() => pickColor(color)}
        ></button>
      {/each}
    </span>
  </div>

  <div class="separator" role="none"></div>

  {#if confirming}
    <div class="confirm" role="group" aria-label={t("sidebar.collection.delete")}>
      <span class="confirm-text">{t("sidebar.collection.deleteConfirm", { name: collection.name })}</span>
      <span class="confirm-actions">
        <button type="button" class="confirm-action danger" data-confirm-delete onclick={remove}>{t("sidebar.delete")}</button>
        <button type="button" class="confirm-action" data-cancel-delete onclick={() => (confirming = false)}>{t("sidebar.cancel")}</button>
      </span>
    </div>
  {:else}
    <button type="button" class="item" data-collection-menu-item="delete" role="menuitem" onclick={() => (confirming = true)}>
      <span class="glyph" aria-hidden="true">
        <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
          <path d="M4 7h16" /><path d="M10 11v6" /><path d="M14 11v6" /><path d="M6 7l1 13h10l1-13" /><path d="M9 7V4h6v3" />
        </svg>
      </span>
      <span class="label">{t("sidebar.collection.deleteItem")}</span>
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
  .item:hover {
    background-color: var(--surface-3, #242424);
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
  .separator {
    height: 1px;
    margin: 4px 2px;
    background-color: var(--border, #1c1c1c);
  }
  [data-collection-menu-item="delete"] {
    color: var(--error, #f87171);
  }
  [data-collection-menu-item="delete"] .glyph {
    color: var(--error, #f87171);
  }
  .confirm {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 6px 8px;
  }
  .confirm-text {
    font-size: 12px;
    color: var(--text-muted, #a3a3a3);
  }
  .confirm-actions {
    display: flex;
    align-items: center;
    gap: 6px;
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
