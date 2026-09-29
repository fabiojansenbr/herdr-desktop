<script lang="ts">
  // Footer section "Conexões" (spec 011, AC-011-02; spec 029, AC-029-02/03).
  // Lists "Este computador · Local" and each saved host with type, latency or the failure reason,
  // state dot with text, and "+" button that opens the dialog. Each SSH host has a context menu
  // (right click or `…` on hover): Desconectar (online), Reconectar (offline/error), Editar… and
  // Remover (with an inline confirmation). A failed host shows the reason, the error dot and
  // Tentar novamente; Local never offers Desconectar/Remover.
  import type { ConnectionItem } from "./tree-model";
  import {
    baseStyle,
    clampedStyle,
    confirmRemoveText,
    hostActions,
    CONFIRM_Z_INDEX,
    MENU_Z_INDEX,
    type HostActionId,
  } from "./connection-actions";
  import { t } from "../../i18n/index.svelte";

  interface Props {
    items: readonly ConnectionItem[];
    onSelectHost: (endpoint: string) => void;
    onOpenDialog: () => void;
    onConnect: (endpoint: string) => void;
    onDisconnect: (endpoint: string) => void;
    onReconnect: (endpoint: string) => void;
    onEdit: (endpoint: string) => void;
    onRemove: (endpoint: string) => void;
  }

  let {
    items,
    onSelectHost,
    onOpenDialog,
    onConnect,
    onDisconnect,
    onReconnect,
    onEdit,
    onRemove,
  }: Props = $props();

  // Spec 032 (AC-032-01): the footer sits at the end of the sidebar, so the host menu and the
  // remove confirmation are viewport overlays (`position: fixed`), measured from the button/row
  // rect, flipped upward when there is no room below and clamped to 8 px of the window edges.
  // The placement and the menu itself live in `connection-actions.ts` (spec 048), shared with the
  // host popover of the new sidebar; the behaviour here is unchanged.

  let menuFor = $state<string | null>(null);
  let confirmFor = $state<string | null>(null);
  let menuEl: HTMLDivElement | undefined = $state();
  let confirmEl: HTMLDivElement | undefined = $state();
  let menuStyle = $state("");
  let confirmStyle = $state("");
  let anchorEl: HTMLElement | null = null;

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

  function closeMenus() {
    menuFor = null;
    confirmFor = null;
  }

  $effect(() => {
    const onPointerDown = (event: Event) => {
      if (menuFor === null && confirmFor === null) return;
      const target = event.target;
      if (!(target instanceof Element)) return;
      if (menuEl?.contains(target) || confirmEl?.contains(target)) return;
      // The `…` button toggles on click; closing on its pointerdown would reopen it.
      if (target.closest("[data-menu]")) return;
      closeMenus();
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || (menuFor === null && confirmFor === null)) return;
      closeMenus();
    };
    window.addEventListener("pointerdown", onPointerDown, true);
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("pointerdown", onPointerDown, true);
      window.removeEventListener("keydown", onKeyDown);
    };
  });

  function onKeydown(e: KeyboardEvent, endpoint: string) {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      onSelectHost(endpoint);
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

  function run(e: MouseEvent, endpoint: string, action: (endpoint: string) => void) {
    e.stopPropagation();
    menuFor = null;
    action(endpoint);
  }

  /** Handler of each menu entry of `hostActions` (Remover asks for the confirmation first). */
  function runAction(e: MouseEvent, endpoint: string, id: HostActionId) {
    if (id === "remove") {
      askRemove(e, endpoint);
      return;
    }
    run(e, endpoint, id === "disconnect" ? onDisconnect : id === "reconnect" ? onReconnect : onEdit);
  }

  function askRemove(e: MouseEvent, endpoint: string) {
    e.stopPropagation();
    menuFor = null;
    confirmStyle = baseStyle(CONFIRM_Z_INDEX);
    confirmFor = endpoint;
  }
</script>

<footer class="connections-footer" aria-label={t("projects.connections.label")}>
  <div class="connections-header">
    <span class="title">{t("projects.connections.title")}</span>
    <button
      type="button"
      class="add-btn"
      id="add-connection-btn"
      aria-label={t("projects.connections.add")}
      title={t("projects.connections.add")}
      onclick={onOpenDialog}
    >
      +
    </button>
  </div>

  <div class="connections-list" role="list">
    {#each items as item (item.endpoint)}
      <div class="connection-item">
        <div
          class="connection-row"
          class:active={item.active}
          role="button"
          tabindex="0"
          aria-label={`${item.name}, ${item.typeBadge}, ${item.latencyText}, ${item.statusText}`}
          aria-current={item.active ? "true" : undefined}
          data-endpoint={item.endpoint}
          data-active={item.active ? "true" : undefined}
          title={item.tooltip}
          onclick={() => onSelectHost(item.endpoint)}
          onkeydown={(e) => onKeydown(e, item.endpoint)}
          oncontextmenu={(e) => onContextMenu(e, item.endpoint)}
        >
          <div class="host-main">
            <span class="host-icon" aria-hidden="true">
              {#if item.endpoint === "local"}
                🖥️
              {:else if item.tone === "idle"}
                ☁️
              {:else}
                📟
              {/if}
            </span>
            <span class="host-name name">{item.name}</span>
            <span class="kind-badge type-badge">{item.typeBadge}</span>
          </div>

          <div class="host-status">
            {#if item.retryable}
              <span class="failure-reason" data-reason={item.endpoint}>{item.statusText}</span>
              <button
                type="button"
                class="retry-btn"
                data-retry={item.endpoint}
                onclick={(e) => {
                  e.stopPropagation();
                  onConnect(item.endpoint);
                }}
              >
                {t("projects.connections.retry")}
              </button>
            {/if}
            <span class="latency status-text" class:offline={item.latencyText === t("projects.offline")}>{item.latencyText}</span>
            <span
              class="state-dot status-dot"
              data-tone={item.tone}
              class:ok={item.tone === "ok"}
              class:idle={item.tone === "idle"}
              class:warn={item.tone === "warn"}
              class:attention={item.tone === "attention"}
              title={item.statusText}
              aria-label={item.statusText}
            ></span>
            {#if item.endpoint !== "local"}
              <button
                type="button"
                class="host-menu"
                data-menu={item.endpoint}
                aria-haspopup="menu"
                aria-expanded={menuFor === item.endpoint}
                aria-label={t("projects.host.actionsOf", { name: item.name })}
                title={t("projects.host.actions")}
                onclick={(e) => toggleMenu(e, item.endpoint)}
              >
                …
              </button>
            {/if}
          </div>
        </div>

        {#if menuFor === item.endpoint && item.endpoint !== "local"}
          <div
            class="context-menu"
            role="menu"
            data-menu-for={item.endpoint}
            aria-label={t("projects.host.actionsOf", { name: item.name })}
            style={menuStyle}
            bind:this={menuEl}
          >
            {#each hostActions(item) as action (action.id)}
              <button type="button" role="menuitem" data-action={action.id} onclick={(e) => runAction(e, item.endpoint, action.id)}>
                {action.label}
              </button>
            {/each}
          </div>
        {/if}

        {#if confirmFor === item.endpoint}
          <div
            class="confirm-remove"
            data-confirm={item.endpoint}
            role="alert"
            style={confirmStyle}
            bind:this={confirmEl}
          >
            <span>{confirmRemoveText(item.name)}</span>
            <button
              type="button"
              data-confirm-remove={item.endpoint}
              onclick={(e) => {
                e.stopPropagation();
                confirmFor = null;
                onRemove(item.endpoint);
              }}
            >
              {t("projects.host.remove")}
            </button>
            <button
              type="button"
              data-cancel-remove={item.endpoint}
              onclick={(e) => {
                e.stopPropagation();
                confirmFor = null;
              }}
            >
              {t("projects.cancel")}
            </button>
          </div>
        {/if}
      </div>
    {/each}
  </div>
</footer>

<style>
  .connections-footer {
    border-top: 1px solid var(--border, #242833);
    padding: 12px 12px 16px;
    background-color: var(--surface, #111318);
    flex-shrink: 0;
  }
  .connections-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 8px;
    padding: 0 4px;
  }
  .title {
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.05em;
    color: var(--text-muted, #8c93a3);
  }
  .add-btn {
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
  .add-btn:hover {
    color: var(--text, #e7e9ee);
    background-color: var(--surface-2, #171a21);
  }
  .add-btn:focus-visible {
    outline: 2px solid var(--accent, #8fa8ff);
  }
  .connections-list {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .connection-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 6px 8px;
    border-radius: 6px;
    cursor: pointer;
    font-size: 12px;
    color: var(--text, #e7e9ee);
    transition: background-color 0.15s ease;
  }
  .connection-row:hover {
    background-color: var(--surface-2, #171a21);
  }
  .connection-row.active {
    background-color: var(--surface-3, #1e222b);
  }
  .connection-row:focus-visible {
    outline: 2px solid var(--accent, #8fa8ff);
    outline-offset: -1px;
  }
  .host-main {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    flex: 1;
  }
  .host-icon {
    font-size: 12px;
    line-height: 1;
    flex-shrink: 0;
    opacity: 1;
  }
  .host-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .kind-badge {
    font-size: 10px;
    font-weight: 500;
    padding: 1px 4px;
    border-radius: 3px;
    background-color: var(--surface-2, #171a21);
    color: var(--text-muted, #8c93a3);
    border: 1px solid var(--border, #242833);
    flex-shrink: 0;
  }
  .host-status {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
  }
  .latency {
    font-size: 11px;
    color: var(--text-muted, #8c93a3);
  }
  .latency.offline {
    color: var(--text-muted, #8c93a3);
  }
  .state-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    flex-shrink: 0;
  }
  .state-dot.ok {
    background-color: var(--working, #5bd68a);
  }
  .state-dot.idle {
    background-color: var(--idle, #6b7280);
  }
  .state-dot.warn {
    background-color: var(--attention, #f4b454);
  }
  .state-dot.attention {
    background-color: var(--error, #f2777a);
  }
  .connection-item {
    position: relative;
  }
  .host-menu {
    background: transparent;
    border: none;
    color: var(--text-muted, #8c93a3);
    font-size: 13px;
    line-height: 1;
    padding: 1px 4px;
    border-radius: 4px;
    cursor: pointer;
    opacity: 0;
    transition: opacity 0.15s ease, color 0.15s ease, background-color 0.15s ease;
  }
  .connection-row:hover .host-menu,
  .host-menu:focus-visible,
  .host-menu[aria-expanded="true"] {
    opacity: 1;
  }
  .host-menu:hover {
    color: var(--text, #e7e9ee);
    background-color: var(--surface-3, #1e222b);
  }
  .host-menu:focus-visible {
    outline: 2px solid var(--accent, #8fa8ff);
  }
  .failure-reason {
    font-size: 11px;
    color: var(--error, #f2777a);
    white-space: nowrap;
  }
  .retry-btn {
    background: transparent;
    border: 1px solid var(--error, #f2777a);
    color: var(--error, #f2777a);
    font-size: 11px;
    line-height: 1;
    padding: 2px 6px;
    border-radius: 4px;
    cursor: pointer;
  }
  .retry-btn:hover {
    background-color: #f2777a1f;
  }
  .retry-btn:focus-visible {
    outline: 2px solid var(--accent, #8fa8ff);
  }
  .context-menu {
    display: flex;
    flex-direction: column;
    min-width: 148px;
    padding: 4px;
    background-color: var(--surface-2, #171a21);
    border: 1px solid var(--border, #242833);
    border-radius: 6px;
    box-shadow: 0 8px 20px rgba(0, 0, 0, 0.45);
  }
  .context-menu button {
    background: transparent;
    border: none;
    color: var(--text, #e7e9ee);
    text-align: left;
    font: inherit;
    font-size: 12px;
    padding: 5px 8px;
    border-radius: 4px;
    cursor: pointer;
  }
  .context-menu button:hover {
    background-color: var(--surface-3, #1e222b);
  }
  .context-menu button:focus-visible {
    outline: 2px solid var(--accent, #8fa8ff);
  }
  .confirm-remove {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 8px;
    background-color: var(--surface-2, #171a21);
    border: 1px solid var(--error, #f2777a);
    border-radius: 6px;
    font-size: 11px;
    color: var(--text, #e7e9ee);
  }
  .confirm-remove button {
    background: transparent;
    border: 1px solid var(--border, #242833);
    color: var(--text, #e7e9ee);
    font-size: 11px;
    padding: 2px 6px;
    border-radius: 4px;
    cursor: pointer;
  }
  .confirm-remove button[data-confirm-remove] {
    border-color: var(--error, #f2777a);
    color: var(--error, #f2777a);
  }
</style>
