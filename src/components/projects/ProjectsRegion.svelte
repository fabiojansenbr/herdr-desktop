<script lang="ts">
  // Projects region slot (spec 010 foundation; filled by spec 011, AC-011-01, AC-011-02).
  // Mounts the projects tree at the top and the connections footer at the bottom.
  import { onMount } from "svelte";
  import type { Snippet } from "svelte";
  import type { AgentDto, AgentStatus } from "../../agents/types";
  import type { FrameContext } from "../../shell/frame-context";
  import ConnectionsFooter from "./ConnectionsFooter.svelte";
  import ProjectTree from "./ProjectTree.svelte";
  import { buildConnectionItems } from "./tree-model";

  /** Guide: CONEXÕES stays pinned at the sidebar foot when the window is at least 700 px tall. */
  const CONNECTIONS_PIN_MIN_HEIGHT = 700;

  interface Props {
    ctx: FrameContext;
    children?: Snippet;
  }

  let { ctx, children }: Props = $props();

  let regionEl: HTMLDivElement | undefined = $state();
  let sidebarHeight = $state(800);
  const pinFooter = $derived(sidebarHeight >= CONNECTIONS_PIN_MIN_HEIGHT);

  onMount(() => {
    if (!regionEl) return;
    const measure = () => {
      const height = regionEl?.clientHeight ?? 0;
      if (height > 0) sidebarHeight = height;
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver((entries) => {
      const height = entries[0]?.contentRect.height;
      if (typeof height === "number") sidebarHeight = height;
    });
    ro.observe(regionEl);
    return () => ro.disconnect();
  });

  const hosts = $derived(ctx.connections?.view?.hub.hosts ?? []);
  const connectionItems = $derived(buildConnectionItems(hosts, ctx.selectedEndpoint));
  const effectiveTreeAgents = $derived.by<AgentDto[]>(() => {
    const liveAgents = ctx.agents?.agents ?? [];
    if (liveAgents.length > 0) return liveAgents;
    const derived: AgentDto[] = [];
    for (const h of hosts) {
      if (h.agents && h.agents.length > 0) {
        for (const a of h.agents) {
          derived.push({
            pane_id: a.pane_id,
            workspace_id: a.workspace_id,
            tab_id: a.tab_id,
            name: a.name,
            kind: a.agent ?? a.display_agent ?? a.name,
            status: (a.agent_status as AgentStatus) ?? "idle",
            launch_pending: false,
            ready: true,
            focused: a.focused,
            terminal_title: a.terminal_title_stripped ?? a.terminal_title ?? a.title ?? null,
          });
        }
      } else if (h.panes && h.panes.length > 0) {
        for (const p of h.panes) {
          if (p.agent) {
            derived.push({
              pane_id: p.pane_id,
              workspace_id: p.workspace_id,
              tab_id: p.tab_id ?? "",
              name: p.agent,
              kind: p.agent,
              status: (p.agent_status as AgentStatus) ?? "idle",
              launch_pending: false,
              ready: true,
              focused: p.focused,
              terminal_title: p.terminal_title ?? p.title ?? null,
            });
          }
        }
      }
    }
    return derived;
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

  function openConnectionDialog() {
    ctx.controllers.connections.openDialog();
  }
</script>

<div
  class="region"
  class:scroll-footer={!pinFooter}
  data-slot="projects"
  data-activity={ctx.activity}
  data-footer-pin={pinFooter ? "true" : "false"}
  bind:this={regionEl}
>
  {#if ctx.activity === "connections"}
    {@render children?.()}
  {:else}
    <div class="tree-area">
      <ProjectTree
        controller={ctx.controllers.projects}
        state={ctx.navigator}
        agents={effectiveTreeAgents}
        {hosts}
        selectedEndpoint={ctx.selectedEndpoint}
      />
    </div>

    <ConnectionsFooter
      items={connectionItems}
      onSelectHost={selectHost}
      onOpenDialog={openConnectionDialog}
      onConnect={(endpoint) => void ctx.controllers.connections.connect(endpoint)}
      onDisconnect={(endpoint) => void disconnectHost(endpoint)}
      onReconnect={(endpoint) => void ctx.controllers.connections.reconnect(endpoint)}
      onEdit={(endpoint) => void ctx.controllers.connections.editProfile(endpoint)}
      onRemove={(endpoint) => void removeHost(endpoint)}
    />
  {/if}
</div>

<style>
  .region {
    height: 100%;
    min-height: 0;
    display: flex;
    flex-direction: column;
    background-color: var(--surface, #111318);
  }
  .tree-area {
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }
  .region.scroll-footer {
    overflow: auto;
  }
  .region.scroll-footer .tree-area {
    flex: none;
    overflow: visible;
  }
</style>
