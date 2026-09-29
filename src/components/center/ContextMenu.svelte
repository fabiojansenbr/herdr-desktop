<script lang="ts">
  // Spec 028 AC-028-03 — generic context menu of the center region (tabs bar and pane overlay):
  // a list of items positioned at the click, one callback per chosen item, keyboard navigation,
  // Escape and a click outside closing it. The owner decides the items and the actions; this
  // component never talks to a controller.
  import { onMount } from "svelte";
  import { t } from "../../i18n/index.svelte";
  import { menuRectLine, uiTrace } from "../../shell/ui-trace";

  export interface ContextMenuItem {
    id: string;
    label: string;
    disabled?: boolean;
    /** Explanation shown when the item is disabled (never a silent refusal). */
    reason?: string;
  }

  let {
    items,
    x,
    y,
    label = t("center.contextMenu.label"),
    onselect,
    onclose,
  }: {
    items: readonly ContextMenuItem[];
    x: number;
    y: number;
    label?: string;
    onselect: (id: string) => void;
    onclose: () => void;
  } = $props();

  let menuEl: HTMLDivElement | undefined = $state();
  let highlighted = $state(0);

  $effect(() => {
    if (highlighted >= items.length) highlighted = Math.max(0, items.length - 1);
  });

  function move(delta: number) {
    if (items.length === 0) return;
    highlighted = (highlighted + delta + items.length) % items.length;
  }

  function activate(entry: ContextMenuItem | undefined) {
    if (!entry || entry.disabled) return;
    onselect(entry.id);
  }

  function onKeyDown(event: KeyboardEvent) {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      onclose();
      return;
    }
    if (event.key === "ArrowDown") {
      event.preventDefault();
      move(1);
      return;
    }
    if (event.key === "ArrowUp") {
      event.preventDefault();
      move(-1);
      return;
    }
    if (event.key === "Home" || event.key === "End") {
      event.preventDefault();
      highlighted = event.key === "Home" ? 0 : items.length - 1;
      return;
    }
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      activate(items[highlighted]);
    }
  }

  onMount(() => {
    // Debug trail (spec 028 r3): where the menu really landed and which stacking it got, so a
    // window test distinguishes "never opened" from "open but clipped/behind the canvas".
    const rect = menuEl ? menuEl.getBoundingClientRect() : null;
    const zIndex = menuEl ? getComputedStyle(menuEl).zIndex : "none";
    uiTrace(menuRectLine({ label, rect, zIndex, items: items.length }));
    menuEl?.focus();
    const outside = (event: PointerEvent) => {
      const target = event.target;
      if (menuEl && (!(target instanceof Node) || !menuEl.contains(target))) onclose();
    };
    window.addEventListener("pointerdown", outside, true);
    return () => {
      window.removeEventListener("pointerdown", outside, true);
    };
  });
</script>

<div
  class="context-menu"
  role="menu"
  aria-label={label}
  data-context-menu
  tabindex="-1"
  style={`left:${x}px;top:${y}px`}
  bind:this={menuEl}
  onkeydown={onKeyDown}
>
  {#each items as entry, index (entry.id)}
    <button
      type="button"
      role="menuitem"
      data-menu-item={entry.id}
      data-highlighted={highlighted === index}
      aria-disabled={entry.disabled ?? false}
      title={entry.disabled ? (entry.reason ?? entry.label) : entry.label}
      onclick={() => activate(entry)}
      onmouseenter={() => (highlighted = index)}
    >{entry.label}</button>
  {/each}
</div>

<style>
  .context-menu {
    position: absolute;
    z-index: 8;
    display: flex;
    flex-direction: column;
    min-width: 12ch;
    padding: 4px;
    border: 1px solid var(--surface-3);
    border-radius: 10px;
    background: var(--surface-2);
    box-shadow: 0 6px 18px rgb(0 0 0 / 35%);
    outline: none;
    pointer-events: auto;
  }
  .context-menu button {
    padding: 4px 10px;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--text);
    text-align: left;
    font: inherit;
    font-size: 13.5px;
    white-space: nowrap;
  }
  .context-menu button:hover:not([aria-disabled="true"]),
  .context-menu button:focus-visible:not([aria-disabled="true"]),
  .context-menu button[data-highlighted="true"]:not([aria-disabled="true"]) {
    background: var(--surface-3);
  }
  .context-menu button[data-menu-item="close"] {
    color: var(--error);
  }
  .context-menu button:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }
  .context-menu button[aria-disabled="true"] {
    color: var(--text-muted);
    cursor: default;
    opacity: 0.6;
  }
</style>
