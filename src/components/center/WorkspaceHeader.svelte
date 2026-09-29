<script lang="ts">
  // Spec 013 header, spec 017 shell: crumbs of the bound project, or `sessão › workspace` and the
  // focused pane cwd when none is bound. Novo agente is the primary control on a live Local host
  // and opens NewAgentPopover; it does not start on render.
  import { t } from "../../i18n/index.svelte";
  import type { FrameContext } from "../../shell/frame-context";
  import { actionAvailability, confirmedTarget, projectHeader, type HeaderAction } from "./model";
  import NewAgentPopover from "./NewAgentPopover.svelte";

  let { ctx }: { ctx: FrameContext } = $props();

  const project = $derived(ctx.activeProject);
  const collection = $derived(
    project
      ? (ctx.navigator?.snapshot?.collections.find((c) => c.project_ids.includes(project.id))?.name ?? null)
      : null,
  );
  const tabs = $derived(ctx.agents?.tabs.length ?? 0);
  const session = $derived(ctx.surface.selection?.session ?? ctx.surface.status?.session ?? null);
  const focused = $derived(
    ctx.agents?.topology?.panes.find((p) => p.focused) ??
      ctx.agents?.topology?.panes.find((p) => p.pane_id === ctx.surface.identity?.pane_id) ??
      null,
  );
  const workspace = $derived(focused?.pane_id.split(":")[0] ?? ctx.surface.identity?.pane_id.split(":")[0] ?? null);
  const cwd = $derived(focused?.cwd ?? null);
  const model = $derived(
    projectHeader({
      project,
      collection,
      branch: ctx.branch,
      tabs,
      session,
      workspace,
      cwd,
    }),
  );
  const shows = (action: HeaderAction) => model.actions.includes(action);
  const kinds = $derived(ctx.agents?.kinds ?? []);
  const target = $derived(confirmedTarget({ identity: ctx.surface.identity, agents: ctx.agents }));
  const available = $derived(actionAvailability({ target, unavailable: ctx.unavailable }));
  const localLive = $derived(ctx.surface.selection?.kind === "local" && ctx.surface.phase === "live");
  const reasons = $derived.by(() => {
    const localReason = t("center.reason.localHost");
    return {
      split: available.split,
      newTab: localLive ? available.newTab : localReason,
      newAgent: localLive
        ? (ctx.unavailable.newAgent ?? (kinds.length === 0 ? t("center.reason.noAgentKinds") : null))
        : localReason,
    };
  });
  let popover = $state(false);

  function run(action: HeaderAction, perform: () => void) {
    if (reasons[action] !== null) return;
    perform();
  }

  function togglePopover() {
    if (reasons.newAgent !== null) return;
    popover = !popover;
  }
</script>

<div class="header" data-center-header>
  <div class="crumbs">
    {#if !ctx.sidebarOpen}
      <button type="button" class="subtle" onclick={() => ctx.actions.toggleProjects()} aria-label={t("center.header.expandSidebar")} title={t("center.header.expandSidebar")}>
        <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <rect x="3" y="3" width="18" height="18" rx="2" ry="2" /><line x1="9" y1="3" x2="9" y2="21" /><path d="m11 9 3 3-3 3" />
        </svg>
      </button>
    {/if}
    {#each model.crumbs as crumb, i (crumb)}
      {#if i > 0}<span class="sep" aria-hidden="true">›</span>{/if}
      <span class="crumb" class:project={i === model.crumbs.length - 1} data-crumb={i} title={crumb}>{crumb}</span>
    {/each}
    {#if model.branch}
      <span class="branch" data-branch title={t("center.header.branchTitle", { name: model.branch })}>⎇ {model.branch}</span>
    {/if}
    {#if model.path}
      <span class="path" data-path title={model.path}>{model.path}</span>
    {/if}
    {#if model.empty}
      <span class="empty" data-empty>{model.emptyText}</span>
    {/if}
  </div>
  <div class="actions">
    {#if shows("split") && ctx.unavailable.split === null}
      <button
        type="button"
        data-action="split"
        onclick={() => run("split", () => ctx.actions.split())}
        aria-disabled={reasons.split !== null}
        aria-label={t("center.header.split")}
        title={reasons.split ?? t("center.header.splitTitle")}
      >
        {t("center.header.splitButton")}
      </button>
    {/if}
    {#if shows("newTab")}
      <button
        type="button"
        data-action="newTab"
        onclick={() => run("newTab", () => ctx.actions.newTab())}
        aria-disabled={reasons.newTab !== null}
        aria-label={t("center.header.newShell")}
        title={reasons.newTab ?? t("center.header.newShellTitle")}
      >
        Shell
      </button>
    {/if}
    {#if shows("newAgent")}
      <div class="new-agent">
        <button
          type="button"
          class="primary"
          data-action="newAgent"
          onclick={togglePopover}
          aria-disabled={reasons.newAgent !== null}
          aria-expanded={popover}
          aria-label={t("center.header.newAgent")}
          title={reasons.newAgent ?? t("center.header.newAgentTitle")}
        >
          {t("center.header.newAgentTitle")}
        </button>
        {#if popover}
          <NewAgentPopover {ctx} onclose={() => (popover = false)} />
        {/if}
      </div>
    {/if}
    {#if !ctx.agentsOpen}
      <button type="button" class="subtle" onclick={() => ctx.actions.toggleAgents()} aria-label={t("center.header.expandAgents")} title={t("center.header.expandAgentsTitle")}>
        <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <rect x="3" y="3" width="18" height="18" rx="2" ry="2" /><line x1="15" y1="3" x2="15" y2="21" /><path d="m13 9-3 3 3 3" />
        </svg>
      </button>
    {/if}
  </div>
</div>

<style>
  .header {
    flex: none;
    height: 40px;
    padding: 0 12px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    border-bottom: 1px solid var(--border);
  }
  .crumbs,
  .actions {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }
  .crumb {
    color: var(--text-muted);
    max-width: 220px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .crumb.project {
    font-weight: 600;
    color: var(--text);
  }
  .sep {
    color: var(--text-muted);
  }
  .branch {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 2px 8px;
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--text-muted);
    font-size: 12px;
    white-space: nowrap;
  }
  .path,
  .empty {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12px;
    color: var(--text-muted);
  }
  .path {
    font-family: var(--font-mono);
  }
  .new-agent {
    position: relative;
  }
  .primary {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--bg);
  }
  .primary[aria-disabled="true"] {
    opacity: 0.6;
  }
  button[aria-disabled="true"] {
    color: var(--text-muted);
    cursor: default;
  }
  .subtle {
    background: transparent;
    border: none;
    padding: 4px;
    color: var(--text-muted);
    display: flex;
  }
</style>
