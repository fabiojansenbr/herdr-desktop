<script lang="ts">
  // Host selector at the sidebar foot (spec 048, `design/v2-01-workspace.png`). One compact line —
  // icon, name, status dot and `Local · herdr 0.9.0 · 2 hosts SSH` — replaces the CONEXÕES list of
  // 011/029 that the 041 slot still rendered; everything else (the per-host actions, the switch
  // without reconnecting of 035/037, the fallback to Local of 029) moves into the popover.
  import HostPopover from "./HostPopover.svelte";
  import { buildConnectionItems } from "../projects/tree-model";
  import type { HostDto } from "../../connections/types";
  import type { FrameContext } from "../../shell/frame-context";
  import { phaseText, t } from "../../i18n/index.svelte";

  let { ctx }: { ctx: FrameContext } = $props();

  let open = $state(false);

  const hosts = $derived(ctx.connections?.view?.hub.hosts ?? []);
  const profiles = $derived(ctx.connections?.view?.profiles ?? []);
  const items = $derived(buildConnectionItems(hosts, ctx.selectedEndpoint));
  /**
   * Latency of a live connection (the popover shows it only for a host that is online) and the
   * host's saved `Conectar ao abrir` (spec 058), which only an SSH profile has.
   */
  const options = $derived(
    items.map((item) => {
      const host = hosts.find((h) => h.endpoint === item.endpoint);
      const profile = profiles.find((p) => p.id === item.endpoint);
      return {
        ...item,
        latency: item.online && host?.latency_ms != null ? `${host.latency_ms} ms` : null,
        connectOnOpen: profile === undefined ? undefined : profile.connect_on_open,
      };
    }),
  );

  const selected = $derived<HostDto | null>(hosts.find((h) => h.endpoint === (ctx.selectedEndpoint ?? "local")) ?? null);
  const isLocal = $derived((ctx.selectedEndpoint ?? "local") === "local");
  const sshCount = $derived(hosts.filter((h) => h.endpoint !== "local").length);

  /** Green online, amber while (re)connecting, grey otherwise — the three states of the design. */
  const tone = $derived(
    selected?.phase === "online" ? "online" : selected?.phase === "connecting" || selected?.phase === "reconnecting" ? "connecting" : "offline",
  );

  const name = $derived(isLocal ? t("sidebar.host.thisComputer") : (selected?.label ?? ctx.selectedEndpoint ?? ""));

  /**
   * `Local · herdr 0.9.0 · 2 hosts SSH`: kind of the selected host, the engine version when it is
   * known (the host reports it; the agents state is the fallback) and how many SSH hosts are saved.
   */
  const summary = $derived.by(() => {
    const version = selected?.server_version?.trim() || ctx.agents?.serverVersion?.trim() || null;
    const parts = [isLocal ? "Local" : "SSH"];
    if (version) parts.push(`herdr ${version}`);
    if (sshCount > 0) parts.push(t("sidebar.host.sshCount", { count: sshCount }));
    return parts.join(" · ");
  });

  function selectHost(endpoint: string) {
    void ctx.controllers.surface.select(endpoint).catch(() => {
      // The refusal is shown on the surface; no fallback host is chosen.
    });
  }

  /** Disconnecting/removing the selected host returns the selection to Local (AC-029-03). */
  function returnToLocal() {
    void ctx.controllers.surface.select("local").catch(() => {
      // No Local host configured: nothing is selected, never another SSH host.
    });
  }

  async function disconnectHost(endpoint: string) {
    await ctx.controllers.connections.disconnect(endpoint);
    if (ctx.selectedEndpoint === endpoint) returnToLocal();
  }

  async function removeHost(endpoint: string) {
    await ctx.controllers.connections.removeProfile(endpoint);
    if (ctx.selectedEndpoint === endpoint) returnToLocal();
  }
</script>

<div class="hosts" data-sidebar-hosts>
  {#if open}
    <HostPopover
      items={options}
      onSelect={selectHost}
      onDisconnect={(endpoint) => void disconnectHost(endpoint)}
      onReconnect={(endpoint) => void ctx.controllers.connections.reconnect(endpoint)}
      onEdit={(endpoint) => void ctx.controllers.connections.editProfile(endpoint)}
      onRemove={(endpoint) => void removeHost(endpoint)}
      onSetConnectOnOpen={(endpoint, enabled) => void ctx.controllers.connections.setConnectOnOpen(endpoint, enabled)}
      onNewConnection={() => {
        open = false;
        ctx.actions.openConnections();
      }}
      onClose={() => (open = false)}
    />
  {/if}

  <button
    type="button"
    class="host-line"
    data-host-trigger
    aria-haspopup="dialog"
    aria-expanded={open}
    aria-label={t("sidebar.host.label", { name, summary })}
    title={selected ? phaseText(selected.phase) : summary}
    onclick={() => (open = !open)}
  >
    <span class="icon" data-host-icon={isLocal ? "monitor" : "server"} aria-hidden="true">
      {#if isLocal}
        <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
          <rect x="3" y="4" width="18" height="12" rx="2" /><path d="M9 20h6" /><path d="M12 16v4" />
        </svg>
      {:else}
        <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
          <rect x="3" y="4" width="18" height="7" rx="1.8" /><rect x="3" y="13" width="18" height="7" rx="1.8" />
          <path d="M7 7.5h.01" /><path d="M7 16.5h.01" />
        </svg>
      {/if}
    </span>
    <span class="text">
      <span class="name" data-host-name>{name}</span>
      <span class="summary" data-host-summary>
        <span class="dot" data-host-dot data-tone={tone}></span>
        {summary}
      </span>
    </span>
    <span class="chevrons" aria-hidden="true">⌃⌄</span>
  </button>
</div>

<style>
  .hosts {
    position: relative;
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    padding: 8px;
  }
  .host-line {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    background-color: var(--surface-2, #171717);
    border: none;
    border-radius: 8px;
    padding: 8px 10px;
    font: inherit;
    text-align: left;
    color: var(--text, #ededed);
    cursor: pointer;
    transition: background-color 0.15s ease;
  }
  .host-line:hover {
    background-color: var(--surface-3, #242424);
  }
  .host-line:focus-visible {
    outline: 2px solid var(--accent, #d4d4d4);
    outline-offset: -1px;
  }
  .host-line[aria-expanded="true"] {
    background-color: var(--surface-3, #242424);
  }
  /* AC-047-05: the `monitor`/`server` line icon of the design, never an emoji. */
  .icon {
    display: flex;
    align-items: center;
    justify-content: center;
    line-height: 1;
    flex-shrink: 0;
    color: var(--text-muted, #8c93a3);
  }
  .text {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    flex: 1;
  }
  .name {
    font-size: 13.5px;
    font-weight: 500;
    color: var(--text, #ededed);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .summary {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--text-dim, #6b6b6b);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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
  .chevrons {
    font-size: 10px;
    line-height: 1;
    color: var(--text-muted, #8c93a3);
    flex-shrink: 0;
    writing-mode: vertical-rl;
    letter-spacing: -2px;
  }
</style>
