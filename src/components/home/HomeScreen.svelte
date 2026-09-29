<script lang="ts">
  // Spec 012 — the "Boa tarde, Fábio" home screen of the design. It appears when the window
  // opens without an active project and through the Início item of the activity bar; the sidebar
  // tree, the center and the agents panel keep working around it.
  //
  // Every value comes from the 011 tree and the 014 agent states through `home-model`; the
  // greeting uses the system user read once through IPC. Thumbnails are static `pane.read`
  // snapshots taken once per card when the screen mounts (a visit); leaving and coming back is
  // a new visit and a new read. Nothing here renders a terminal, keeps a frame loop or sends a
  // key to an agent: the waiting chips open the project and focus the pane once.
  import { onDestroy, onMount, tick } from "svelte";
  import AttentionStrip from "./AttentionStrip.svelte";
  import ProjectCard from "./ProjectCard.svelte";
  import ServerCard from "./ServerCard.svelte";
  import {
    buildHomeView,
    collapsedGroupText,
    createChipDispatcher,
    createThumbnailSnapshots,
    startsCollapsed,
    toggledCollapse,
    type AttentionChip,
    type HomeGroup,
  } from "./home-model";
  import { createActivityLedger } from "../agents/panel";
  import { t } from "../../i18n/index.svelte";
  import { tauriHomeBridge, type HomeBridge } from "../../home/bridge";
  import { pickProjectFolder } from "../../projects/bridge";
  import type { FrameContext } from "../../shell/frame-context";

  interface Props {
    ctx: FrameContext;
    /** Injectable for tests/harnesses; the window uses the IPC bridge. */
    bridge?: HomeBridge;
  }

  let { ctx, bridge = tauriHomeBridge() }: Props = $props();

  // The visit starts when this component mounts (the Workbench removes the layer when Início
  // closes); the store schedules at most one read per card, never a frame.
  const snapshots = createThumbnailSnapshots({
    read: async (paneId, lines) => {
      const target = ctx.controllers.agents.target(paneId);
      if (!target) throw new Error(t("home.read.noConnection"));
      return (await bridge.paneRead(target, paneId, lines)).text;
    },
  });
  snapshots.beginVisit();

  const ledger = createActivityLedger();
  let user = $state("");
  let now = $state(Date.now());
  let snapshotsTick = $state(0);
  let collapsed = $state<Record<string, boolean>>({});
  let ensured = false;

  const ticker = setInterval(() => (now = Date.now()), 30_000);
  onDestroy(() => clearInterval(ticker));

  // The system user of the greeting, read once from the backend (never a shell, never a secret).
  onMount(() => {
    void bridge
      .systemUser()
      .then((dto) => (user = dto.user))
      .catch(() => (user = ""));
  });

  const connected = $derived(ctx.agents?.phase === "connected" && ctx.surface.phase === "live");
  const hosts = $derived(ctx.connections?.view?.hub.hosts ?? []);
  const view = $derived.by(() => {
    // Re-derive when a static snapshot lands; the text itself is read from the store.
    void snapshotsTick;
    return buildHomeView({
      hour: new Date(now).getHours(),
      user,
      navigator: ctx.navigator,
      agents: ctx.agents,
      hosts,
      selectedEndpoint: ctx.selectedEndpoint,
      hostLabels: ctx.hostLabels,
      snapshots,
      ledger,
      now,
      connected,
    });
  });
  const readPlan = $derived(
    view.groups.flatMap((group) =>
      group.cards.flatMap((card) => card.thumbnails.map((thumb) => ({ paneId: thumb.paneId, endpoint: card.endpoint }))),
    ),
  );

  // Observing the engine list is what dates each transition (the engine publishes no timestamp).
  $effect(() => {
    const agents = ctx.agents?.agents ?? [];
    const at = Date.now();
    ledger.observe(agents, at);
    now = at;
  });

  // One snapshot per card, once, as soon as the tree is known; no interval, no frame.
  $effect(() => {
    const plan = readPlan;
    if (ensured || plan.length === 0) return;
    ensured = true;
    const canRead = (endpoint: string) => connected && endpoint === ctx.selectedEndpoint;
    void snapshots.ensure(plan, canRead).then(() => (snapshotsTick += 1));
  });

  function isCollapsed(group: HomeGroup): boolean {
    return collapsed[group.id] ?? startsCollapsed(group.count);
  }

  function toggleGroup(group: HomeGroup) {
    collapsed = toggledCollapse(collapsed, group.id, isCollapsed(group));
  }

  const activateChip = createChipDispatcher({
    openProject: (projectId) => ctx.controllers.projects.open(projectId),
    focusPane: (paneId) => ctx.controllers.agents.openAttention(paneId),
  });

  function connectServer() {
    ctx.controllers.connections.openDialog();
  }

  function newGroup() {
    // The existing group creation lives in the projects tree: show it and focus its name field.
    ctx.actions.showProjects();
    void tick().then(() => {
      document.querySelector<HTMLButtonElement>("[data-slot='projects'] [data-new-group]")?.click();
      // The field is found by its own attribute, never by a placeholder: the text is translated.
      document.querySelector<HTMLInputElement>("[data-slot='projects'] [data-new-group-input]")?.focus();
    });
  }

  function openProject() {
    // Native folder picker: creates the Local project in "Meus projetos" and opens it.
    const session = ctx.surface.selection?.session ?? ctx.surface.status?.session ?? "default";
    void ctx.controllers.projects.openFolder(pickProjectFolder, session);
  }

  function selectServer(endpoint: string) {
    void ctx.controllers.surface.select(endpoint).catch(() => {
      // The refusal is shown on the surface; no fallback host is chosen.
    });
  }

  function selectChip(chip: AttentionChip) {
    void activateChip(chip);
  }
</script>

<section class="home" data-home-screen data-empty={view.empty ? "true" : "false"} data-cache={view.cache ? "true" : "false"}>
  <header class="intro">
    <div class="headline">
      <h1 data-greeting>{view.greeting}</h1>
      <p class="summary" data-summary>{view.summary.text}</p>
    </div>
    <div class="buttons">
      <button type="button" class="ghost" data-home-connect onclick={connectServer}>{t("home.connectServer")}</button>
      <button type="button" class="ghost" data-home-new-group onclick={newGroup}>{t("home.newGroup")}</button>
      <button type="button" class="primary" data-home-open-project onclick={openProject}>{t("home.openProject")}</button>
    </div>
  </header>

  {#if view.empty}
    <p class="empty" data-home-empty>{view.emptyText}</p>
  {:else}
    <AttentionStrip chips={view.attention} cache={view.cache} onselect={selectChip} />

    {#each view.groups as group (group.id)}
      <section class="group" data-home-group={group.id}>
        <header class="group-head">
          <button
            type="button"
            class="group-toggle"
            data-group-toggle={group.id}
            aria-expanded={!isCollapsed(group)}
            title={t(isCollapsed(group) ? "home.group.expand" : "home.group.collapse", { name: group.name })}
            onclick={() => toggleGroup(group)}
          >
            <span class="chevron" aria-hidden="true">{isCollapsed(group) ? "›" : "⌄"}</span>
            <span class="color-dot" style={`--group-color:${group.color}`} aria-hidden="true"></span>
            <span class="group-name">{group.name}</span>
            <span class="group-count">{group.count}</span>
          </button>
          {#if isCollapsed(group)}
            <span class="collapsed-text" data-collapsed-text>{collapsedGroupText(group.count, group.agentSummary)}</span>
          {:else}
            <span class="group-detail">{t("home.group.detail", { projects: t("home.group.projects", { count: group.count }), hosts: group.hostSummary })}</span>
          {/if}
        </header>

        {#if !isCollapsed(group)}
          <div class="cards">
            {#each group.cards as card (card.id)}
              <ProjectCard {card} cache={view.cache} />
            {/each}
            <button
              type="button"
              class="add-card"
              data-add-project={group.id}
              title={t("home.group.addProject", { name: group.name })}
              onclick={() => ctx.controllers.projects.openForm(group.id)}
            >
              <span class="plus" aria-hidden="true">+</span>
              {t("home.group.addProjectLabel")}
            </button>
          </div>
        {/if}
      </section>
    {/each}

    <section class="servers" aria-label={t("home.servers")}>
      <h2>{t("home.servers")}</h2>
      <div class="server-grid">
        {#each view.servers as server (server.endpoint)}
          <ServerCard {server} onselect={selectServer} />
        {/each}
      </div>
    </section>
  {/if}
</section>

<style>
  .home {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 16px;
    padding: 20px 24px 32px;
    background: var(--bg);
    color: var(--text);
    font-family: var(--font-ui);
  }
  .intro {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 16px;
    flex-wrap: wrap;
  }
  .headline {
    display: grid;
    gap: 4px;
    min-width: 0;
  }
  h1 {
    margin: 0;
    font-size: 26px;
    font-weight: 600;
    color: var(--text);
  }
  .summary {
    margin: 0;
    font-size: 13px;
    color: var(--text-muted);
  }
  .buttons {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  button {
    font: inherit;
  }
  .buttons button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 34px;
    padding: 0 12px;
    border-radius: 6px;
    font-size: 13px;
    cursor: pointer;
  }
  .ghost {
    border: 1px solid var(--border);
    background: var(--surface-2);
    color: var(--text);
  }
  .ghost:hover {
    background: var(--surface-3);
  }
  .primary {
    border: 1px solid var(--accent);
    background: var(--accent);
    color: var(--bg);
    font-weight: 600;
  }
  .buttons button:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  .empty {
    margin: 0;
    font-size: 13px;
    color: var(--text-muted);
  }
  .group {
    display: grid;
    gap: 8px;
    min-width: 0;
  }
  .group-head {
    display: flex;
    align-items: baseline;
    gap: 10px;
    min-width: 0;
  }
  .group-toggle {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 2px 4px;
    border: none;
    border-radius: 6px;
    background: none;
    color: var(--text);
    cursor: pointer;
  }
  .group-toggle:hover {
    background: var(--surface-2);
  }
  .group-toggle:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  .chevron {
    width: 10px;
    color: var(--text-muted);
  }
  .color-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--group-color, var(--accent));
  }
  .group-name {
    font-size: 13px;
    font-weight: 600;
  }
  .group-count {
    font-size: 11px;
    color: var(--text-muted);
  }
  .group-detail,
  .collapsed-text {
    font-size: 11px;
    color: var(--text-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .collapsed-text {
    min-width: 0;
  }
  .cards {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
    gap: 12px;
  }
  .add-card {
    display: grid;
    place-content: center;
    gap: 6px;
    min-height: 120px;
    border: 1px dashed var(--border);
    border-radius: 10px;
    background: none;
    color: var(--text-muted);
    font-size: 12px;
    cursor: pointer;
  }
  .add-card:hover {
    border-color: var(--accent);
    color: var(--text);
  }
  .add-card:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  .plus {
    font-size: 18px;
  }
  .servers {
    display: grid;
    gap: 8px;
    margin-top: 4px;
  }
  .servers h2 {
    margin: 0;
    font-size: 13px;
    font-weight: 600;
    color: var(--text);
  }
  .server-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
    gap: 10px;
  }
</style>
