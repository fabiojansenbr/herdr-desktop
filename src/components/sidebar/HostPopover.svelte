<script lang="ts">
  // Host popover of the new sidebar (spec 048, AC-048-02). Opens above the compact line with every
  // host (Local first, the order of the 011 list), each with its status dot, the latency when the
  // connection is live and a ✓ on the selected one. Picking a host goes through the 037 path
  // (`controllers.surface.select`, wired by HostSwitcher) and closes. The `…` of each SSH host
  // carries the same menu as the CONEXÕES footer — the very entries and confirmation of
  // `connection-actions.ts` — and the last entry opens the connections view.
  import {
    baseStyle,
    clampedStyle,
    confirmRemoveText,
    hostActions,
    CONFIRM_Z_INDEX,
    MENU_Z_INDEX,
    type HostActionId,
  } from "../projects/connection-actions";
  import type { ConnectionItem } from "../projects/tree-model";
  import { t } from "../../i18n/index.svelte";

  interface HostOption extends ConnectionItem {
    /** Round trip of a live connection (`42 ms`); null when the host is not online. */
    readonly latency: string | null;
    /** "Conectar ao abrir" of the saved profile (spec 058); SSH hosts only. */
    readonly connectOnOpen?: boolean;
  }

  interface Props {
    items: readonly HostOption[];
    onSelect: (endpoint: string) => void;
    onDisconnect: (endpoint: string) => void;
    onReconnect: (endpoint: string) => void;
    onEdit: (endpoint: string) => void;
    onRemove: (endpoint: string) => void;
    onSetConnectOnOpen: (endpoint: string, enabled: boolean) => void;
    onNewConnection: () => void;
    onClose: () => void;
  }

  let { items, onSelect, onDisconnect, onReconnect, onEdit, onRemove, onSetConnectOnOpen, onNewConnection, onClose }: Props = $props();

  let menuFor = $state<string | null>(null);
  let confirmFor = $state<string | null>(null);
  let rootEl: HTMLDivElement | undefined = $state();
  let menuEl: HTMLDivElement | undefined = $state();
  let confirmEl: HTMLDivElement | undefined = $state();
  let menuStyle = $state("");
  let confirmStyle = $state("");
  let anchorEl: HTMLElement | null = null;

  // Same overlay rule as the footer (032): the popover sits at the window foot, so the host menu
  // and the confirmation are fixed, measured and clamped instead of overflowing the sidebar.
  $effect(() => {
    if (menuFor === null || !menuEl) return;
    const rect = menuEl.getBoundingClientRect();
    menuStyle = clampedStyle(anchorEl?.getBoundingClientRect() ?? null, rect.width, rect.height, MENU_Z_INDEX);
  });

  $effect(() => {
    if (confirmFor === null || !confirmEl) return;
    const rect = confirmEl.getBoundingClientRect();
    confirmStyle = clampedStyle(anchorEl?.getBoundingClientRect() ?? null, rect.width, rect.height, CONFIRM_Z_INDEX);
  });

  const overlayOpen = $derived(menuFor !== null || confirmFor !== null);

  function closeMenus() {
    menuFor = null;
    confirmFor = null;
  }

  $effect(() => {
    const onPointerDown = (event: Event) => {
      const target = event.target;
      if (!(target instanceof Element)) return;
      if (menuEl?.contains(target) || confirmEl?.contains(target)) return;
      // The `…` button toggles on click; closing on its pointerdown would reopen it.
      if (target.closest("[data-host-menu]")) return;
      // Inside the popover (or on the line that owns it): only the overlays close.
      if (rootEl?.contains(target) || target.closest("[data-host-trigger]")) {
        closeMenus();
        return;
      }
      closeMenus();
      onClose();
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      // Escape peels one layer: first the host menu/confirmation, then the popover itself.
      if (overlayOpen) {
        closeMenus();
        return;
      }
      onClose();
    };
    window.addEventListener("pointerdown", onPointerDown, true);
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("pointerdown", onPointerDown, true);
      window.removeEventListener("keydown", onKeyDown);
    };
  });

  function pick(endpoint: string) {
    closeMenus();
    onSelect(endpoint);
    onClose();
  }

  function onKeydown(e: KeyboardEvent, endpoint: string) {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      pick(endpoint);
    }
  }

  function toggleMenu(e: MouseEvent, endpoint: string) {
    e.stopPropagation();
    if (menuFor === endpoint) {
      closeMenus();
      return;
    }
    anchorEl = e.currentTarget as HTMLElement;
    menuStyle = baseStyle(MENU_Z_INDEX);
    menuFor = endpoint;
    confirmFor = null;
  }

  function onContextMenu(e: MouseEvent, endpoint: string) {
    if (endpoint === "local") return;
    e.preventDefault();
    anchorEl = e.currentTarget as HTMLElement;
    menuStyle = baseStyle(MENU_Z_INDEX);
    menuFor = endpoint;
    confirmFor = null;
  }

  function askRemove(e: MouseEvent, endpoint: string) {
    e.stopPropagation();
    menuFor = null;
    confirmStyle = baseStyle(CONFIRM_Z_INDEX);
    confirmFor = endpoint;
  }

  /**
   * Handler of each menu entry of `hostActions` (Remover asks for the confirmation first).
   * `Conectar ao abrir` writes the saved profile through the connections controller (spec 058);
   * `checked` is the value the menu is showing, so the click sends its opposite.
   */
  function runAction(e: MouseEvent, endpoint: string, id: HostActionId, checked: boolean | undefined) {
    if (id === "remove") {
      askRemove(e, endpoint);
      return;
    }
    e.stopPropagation();
    menuFor = null;
    if (id === "connect_on_open") {
      onSetConnectOnOpen(endpoint, checked !== true);
      return;
    }
    (id === "disconnect" ? onDisconnect : id === "reconnect" ? onReconnect : onEdit)(endpoint);
  }
</script>

<div class="popover" data-host-popover role="dialog" aria-label={t("sidebar.host.select")} bind:this={rootEl}>
  <div class="options" role="list">
    {#each items as item (item.endpoint)}
      <div class="option-wrap">
        <div
          class="option"
          class:active={item.active}
          role="button"
          tabindex="0"
          data-host-option={item.endpoint}
          aria-current={item.active ? "true" : undefined}
          aria-label={`${item.name}, ${item.typeBadge}, ${item.statusText}`}
          title={item.tooltip}
          onclick={() => pick(item.endpoint)}
          onkeydown={(e) => onKeydown(e, item.endpoint)}
          oncontextmenu={(e) => onContextMenu(e, item.endpoint)}
        >
          <span class="dot" data-host-dot data-tone={item.tone === "ok" ? "online" : item.tone === "warn" ? "connecting" : "offline"} title={item.statusText} aria-label={item.statusText}></span>
          <span class="name">{item.name}</span>
          {#if item.latency}
            <span class="latency" data-host-latency>{item.latency}</span>
          {/if}
          {#if item.active}
            <span class="check" data-host-check aria-label={t("sidebar.host.selected")}>✓</span>
          {/if}
          {#if item.endpoint !== "local"}
            <button
              type="button"
              class="host-menu"
              data-host-menu={item.endpoint}
              aria-haspopup="menu"
              aria-expanded={menuFor === item.endpoint}
              aria-label={t("sidebar.row.actions", { name: item.name })}
              title={t("sidebar.host.actions")}
              onclick={(e) => toggleMenu(e, item.endpoint)}
            >
              …
            </button>
          {/if}
        </div>

        {#if menuFor === item.endpoint}
          <div
            class="context-menu"
            role="menu"
            data-host-menu-for={item.endpoint}
            aria-label={t("sidebar.row.actions", { name: item.name })}
            style={menuStyle}
            bind:this={menuEl}
          >
            {#each hostActions(item) as action (action.id)}
              <button
                type="button"
                role={action.checked === undefined ? "menuitem" : "menuitemcheckbox"}
                aria-checked={action.checked === undefined ? undefined : action.checked}
                data-action={action.id}
                onclick={(e) => runAction(e, item.endpoint, action.id, action.checked)}
              >
                <span class="entry-label">{action.label}</span>
                {#if action.checked}
                  <span class="entry-check" data-action-check aria-hidden="true">✓</span>
                {/if}
              </button>
            {/each}
          </div>
        {/if}

        {#if confirmFor === item.endpoint}
          <div class="confirm-remove" data-host-confirm={item.endpoint} role="alert" style={confirmStyle} bind:this={confirmEl}>
            <span>{confirmRemoveText(item.name)}</span>
            <button
              type="button"
              data-host-confirm-remove={item.endpoint}
              onclick={(e) => {
                e.stopPropagation();
                confirmFor = null;
                onRemove(item.endpoint);
              }}
            >
              {t("sidebar.remove")}
            </button>
            <button
              type="button"
              data-host-cancel-remove={item.endpoint}
              onclick={(e) => {
                e.stopPropagation();
                confirmFor = null;
              }}
            >
              {t("sidebar.cancel")}
            </button>
          </div>
        {/if}
      </div>
    {/each}
  </div>

  <button type="button" class="new-connection" data-host-new onclick={() => onNewConnection()}>
    <span aria-hidden="true">+</span>
    {t("sidebar.host.newConnection")}
  </button>
</div>

<style>
  /* Above the compact line (the sidebar foot); the line is the positioned ancestor. */
  .popover {
    position: absolute;
    bottom: 100%;
    left: 8px;
    right: 8px;
    margin-bottom: 6px;
    z-index: 90;
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 6px;
    background-color: var(--surface-2, #171717);
    border: 1px solid var(--surface-3, #242424);
    border-radius: 10px;
    box-shadow: 0 10px 24px rgba(0, 0, 0, 0.45);
  }
  .options {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .option-wrap {
    position: relative;
  }
  .option {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 8px;
    border-radius: 6px;
    cursor: pointer;
    font-size: 13.5px;
    color: var(--text, #ededed);
  }
  .option:hover,
  .option:focus-visible {
    background-color: var(--surface-3, #242424);
  }
  .option.active {
    background-color: var(--surface-3, #242424);
  }
  .option:focus-visible {
    outline: 2px solid var(--accent, #d4d4d4);
    outline-offset: -1px;
  }
  .name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .latency {
    font-size: 11px;
    color: var(--text-muted, #a3a3a3);
    flex-shrink: 0;
  }
  .check {
    font-size: 11px;
    color: var(--text, #ededed);
    flex-shrink: 0;
  }
  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    flex-shrink: 0;
  }
  [data-host-dot][data-tone="online"] {
    background-color: var(--working, #5bd68a);
  }
  [data-host-dot][data-tone="connecting"] {
    background-color: var(--attention, #f4b454);
  }
  [data-host-dot][data-tone="offline"] {
    background-color: var(--idle, #6b7280);
  }
  .host-menu {
    background: transparent;
    border: none;
    color: var(--text-muted, #a3a3a3);
    font-size: 13.5px;
    line-height: 1;
    padding: 1px 4px;
    border-radius: 6px;
    cursor: pointer;
    opacity: 0;
    flex-shrink: 0;
  }
  .option:hover .host-menu,
  .host-menu:focus-visible,
  .host-menu[aria-expanded="true"] {
    opacity: 1;
  }
  .host-menu:hover,
  .host-menu:focus-visible {
    color: var(--text, #ededed);
    background-color: var(--surface-3, #242424);
  }
  .host-menu:focus-visible {
    outline: 2px solid var(--accent, #d4d4d4);
  }
  .new-connection {
    border-radius: 6px;
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    background: transparent;
    border: none;
    border-top: 1px solid var(--surface-3, #242424);
    color: var(--text-muted, #a3a3a3);
    font: inherit;
    font-size: 13.5px;
    text-align: left;
    padding: 7px 8px 3px;
    margin-top: 2px;
    cursor: pointer;
  }
  .new-connection:hover,
  .new-connection:focus-visible {
    background-color: var(--surface-3, #242424);
    color: var(--text, #ededed);
  }
  .new-connection:focus-visible {
    outline: 2px solid var(--accent, #d4d4d4);
  }
  .context-menu {
    display: flex;
    flex-direction: column;
    min-width: 148px;
    padding: 4px;
    background-color: var(--surface-2, #171717);
    border: 1px solid var(--surface-3, #242424);
    border-radius: 10px;
    box-shadow: 0 8px 20px rgba(0, 0, 0, 0.45);
  }
  .context-menu button {
    display: flex;
    align-items: center;
    gap: 8px;
    background: transparent;
    border: none;
    color: var(--text, #ededed);
    text-align: left;
    font: inherit;
    font-size: 13.5px;
    padding: 5px 8px;
    border-radius: 6px;
    cursor: pointer;
  }
  .entry-label {
    flex: 1;
    min-width: 0;
  }
  .entry-check {
    font-size: 11px;
    color: var(--text, #ededed);
    flex-shrink: 0;
  }
  .context-menu button:hover,
  .context-menu button:focus-visible {
    background-color: var(--surface-3, #242424);
  }
  .context-menu button:focus-visible {
    outline: 2px solid var(--accent, #d4d4d4);
  }
  .confirm-remove {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 8px;
    background-color: var(--surface-2, #171717);
    border: 1px solid var(--surface-3, #242424);
    border-radius: 10px;
    font-size: 11px;
    color: var(--text, #ededed);
  }
  .confirm-remove button {
    background: transparent;
    border: 1px solid var(--surface-3, #242424);
    color: var(--text, #ededed);
    font-size: 13.5px;
    padding: 2px 6px;
    border-radius: 6px;
    cursor: pointer;
  }
  .confirm-remove button:hover,
  .confirm-remove button:focus-visible {
    background-color: var(--surface-3, #242424);
  }
  .confirm-remove button:focus-visible {
    outline: 2px solid var(--accent, #d4d4d4);
  }
  .context-menu button[data-action="remove"],
  .confirm-remove button[data-host-confirm-remove] {
    border-color: var(--error, #f87171);
    color: var(--error, #f87171);
  }
</style>
