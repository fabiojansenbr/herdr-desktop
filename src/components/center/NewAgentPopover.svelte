<script lang="ts">
  // Spec 017 — popover of "Novo agente": Tipo from the engine list, optional Nome, Iniciar
  // creates/uses a pane in the active workspace and calls the existing agent.start once.
  //
  // Spec 073 replaces the native `<select>` (WebKitGTK draws it with the GTK theme, outside the
  // neutral visual of 061/065) with a searchable list: the recently used kinds first, then the
  // ones this machine has installed, then the rest — dimmed, but still offered, because the
  // engine may launch an agent the host's `PATH` lookup cannot see. Availability is the host's
  // `agent_kinds_available` and only for the Local host; a remote host, or a probe that fails, is
  // "everything available" and the `Not installed` section simply does not exist. Starting is
  // unchanged: the same pane choice, the same `agent.start`, `Shell` still opens an engine tab.
  //
  // Spec 076 adds the auto mode switch: each agent starts with its own autonomy flag
  // (`--dangerously-skip-permissions`, `--yolo`…). The table is the backend's
  // (`agent_autonomy_flags`), read only to show the flag on each row — the WebView sends the
  // switch, never arguments. It is on by default and kept in `localStorage`; a kind the table
  // does not name says so instead of a flag, and `Shell` is not an agent, so it shows nothing.
  import { errorText, t } from "../../i18n/index.svelte";
  import { agentAutonomyFlags } from "../../agents/bridge";
  import type { AgentAutonomyFlagsDto } from "../../agents/types";
  import type { FrameContext } from "../../shell/frame-context";
  import { agentName } from "./model";
  import {
    agentChoices,
    autonomyHint,
    readAgentAutonomy,
    readRecentAgents,
    rememberAgentAutonomy,
    rememberRecentAgent,
    SHELL_KIND,
    type AgentChoice,
  } from "./new-agent-model";

  /** Kinds of `list` this machine can launch; injected so the list is testable without a window. */
  async function hostAvailableKinds(list: string[]): Promise<string[]> {
    const { invoke } = await import("@tauri-apps/api/core");
    const answer = await invoke<{ kind: string; available: boolean }[]>("agent_kinds_available", { kinds: list });
    return answer.filter((entry) => entry.available).map((entry) => entry.kind);
  }

  let {
    ctx,
    onclose,
    probeAvailable = hostAvailableKinds,
    probeAutonomy = agentAutonomyFlags,
  }: {
    ctx: FrameContext;
    onclose: () => void;
    probeAvailable?: (kinds: string[]) => Promise<readonly string[]>;
    probeAutonomy?: (kinds: string[]) => Promise<readonly AgentAutonomyFlagsDto[]>;
  } = $props();

  const kinds = $derived(ctx.agents?.kinds ?? []);
  /** The lookup describes this machine only; a selected SSH host is never asked about it. */
  const localHost = $derived((ctx.selectedEndpoint ?? "local") === "local");
  let available = $state<string[] | null>(null);
  let query = $state("");
  let name = $state("");
  let activeKind = $state("");
  let search = $state<HTMLInputElement | null>(null);
  /** Why the last attempt did not start an agent; the popover stays open while it is set (075). */
  let failure = $state<string | null>(null);
  /** Auto mode as the user left it; on when nothing (or no storage) answered (076). */
  let autonomy = $state(readAgentAutonomy());
  /** Flags per kind as the backend answered; null until it did, or when the read failed (076). */
  let flags = $state<Record<string, string[]> | null>(null);
  const recent = readRecentAgents();

  const rows = $derived(agentChoices({ kinds, available, recent, query }));
  const activeIndex = $derived(Math.max(0, rows.findIndex((row) => row.kind === activeKind)));
  const active = $derived<AgentChoice | null>(rows[activeIndex] ?? null);

  $effect(() => {
    search?.focus();
  });

  $effect(() => {
    const list = [...kinds];
    if (!localHost || list.length === 0) return;
    let cancelled = false;
    void probeAvailable(list).then(
      (answer) => {
        if (!cancelled) available = [...answer];
      },
      () => {
        // The list is drawn without the `Not installed` section; nothing is shown as an error.
      },
    );
    return () => {
      cancelled = true;
    };
  });

  // One read of the table per popover, for every kind the engine published: the flags belong to
  // the agent binary, so a remote host is described by the same table and is asked all the same.
  $effect(() => {
    const list = [...kinds];
    if (list.length === 0) return;
    let cancelled = false;
    void probeAutonomy(list).then(
      (answer) => {
        if (!cancelled) flags = Object.fromEntries(answer.map((entry) => [entry.kind, [...entry.flags]]));
      },
      () => {
        // No flag is announced on any row; the switch still decides how the agent starts.
      },
    );
    return () => {
      cancelled = true;
    };
  });

  function move(delta: number) {
    if (rows.length === 0) return;
    activeKind = rows[(activeIndex + delta + rows.length) % rows.length]!.kind;
  }

  function onkeydown(event: KeyboardEvent) {
    if (event.key === "Escape") {
      event.preventDefault();
      onclose();
      return;
    }
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      move(event.key === "ArrowDown" ? 1 : -1);
      return;
    }
    // The list is the form's default action: Enter starts the active row from any of its fields.
    if (event.key === "Enter" && !event.isComposing) {
      event.preventDefault();
      void start(active?.kind ?? "");
    }
  }

  /**
   * Spec 075: the pane a split creates is only in the topology on the engine's next frame, so the
   * choice is made from the id `split` answers, never from the local topology. Every refusal —
   * no capability, no free pane, a backend that rejected the start — keeps the popover open with
   * its reason; the popover closes only after the agent actually started.
   */
  async function start(chosen: string) {
    const agents = ctx.controllers.agents;
    if (!chosen) return;
    failure = null;
    rememberRecentAgent(chosen);
    if (chosen === SHELL_KIND) {
      await agents.createTab();
      onclose();
      return;
    }
    if (!agents.state.capabilities?.start_agent) {
      failure = t("center.newAgent.cannotStart");
      return;
    }
    const occupied = new Set((ctx.agents?.agents ?? []).map((a) => a.pane_id));
    const topology = ctx.agents?.topology ?? agents.state.topology;
    let paneId = ctx.surface.identity?.pane_id ?? topology?.focused_pane_id ?? null;
    if (!paneId || occupied.has(paneId)) {
      paneId = topology?.panes.find((p) => !occupied.has(p.pane_id))?.pane_id ?? null;
    }
    if (!paneId) {
      // The engine answers the created pane; a split it refused, or a server that does not name
      // the pane, leaves nothing to start in and is reported instead of starting on the occupied one.
      paneId = await agents.split("right");
    }
    if (!paneId || occupied.has(paneId)) {
      failure = t("center.newAgent.noPane");
      return;
    }
    const started = name.trim() || agentName(chosen, paneId);
    agents.editStart("paneId", paneId);
    agents.editStart("kind", chosen);
    agents.editStart("name", started);
    agents.setStartAutonomy(autonomy);
    await agents.startAgent();
    const refused = agents.state.start.error;
    if (refused) {
      failure = errorText(refused);
      return;
    }
    onclose();
  }

  function submit(event: SubmitEvent) {
    event.preventDefault();
    void start(active?.kind ?? "");
  }

  const sectionTitle = (section: AgentChoice["section"]): string =>
    section === "recent" ? t("center.newAgent.recent") : section === "all" ? t("center.newAgent.all") : t("center.newAgent.notInstalled");
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<form class="popover" data-new-agent-popover aria-label={t("center.newAgent.label")} onsubmit={submit} {onkeydown}>
  <p class="head">{t("center.newAgent.label")}</p>
  <input
    class="search"
    data-agent-search
    bind:this={search}
    bind:value={query}
    aria-label={t("center.newAgent.search")}
    placeholder={t("center.newAgent.search")}
  />
  <label class="toggle">
    <input
      type="checkbox"
      data-agent-autonomy
      bind:checked={autonomy}
      onchange={() => rememberAgentAutonomy(autonomy)}
    />
    <span>{t("center.newAgent.autonomy")}</span>
  </label>
  <div class="list" role="listbox" aria-label={t("center.newAgent.kind")}>
    {#each rows as row, index (row.kind)}
      {#if row.section === "shell"}
        <span class="divider" data-agent-divider aria-hidden="true"></span>
      {:else if rows[index - 1]?.section !== row.section}
        <p class="section" data-agent-section-title>{sectionTitle(row.section)}</p>
      {/if}
      <button
        type="button"
        class="row"
        class:dim={!row.installed}
        data-agent-kind={row.kind}
        data-agent-section={row.section}
        data-agent-installed={row.installed}
        data-agent-active={row.kind === active?.kind ? "" : undefined}
        role="option"
        aria-selected={row.kind === active?.kind}
        onclick={() => void start(row.kind)}
        onmouseenter={() => (activeKind = row.kind)}
      >
        <span class="tile" data-agent-tile aria-hidden="true">{row.initial}</span>
        <span class="label" data-agent-label>{row.section === "shell" ? t("center.newAgent.shell") : row.label}</span>
        {#if autonomy && flags && row.section !== "shell"}
          <span class="flags" data-agent-flags>{autonomyHint(flags[row.kind]) ?? t("center.newAgent.noAutonomy")}</span>
        {/if}
      </button>
    {/each}
    {#if rows.length === 0}
      <p class="empty" data-agent-empty>{t("center.newAgent.noMatch")}</p>
    {/if}
  </div>
  <label>
    <span>{t("center.newAgent.name")}</span>
    <input
      data-agent-name
      bind:value={name}
      aria-label={t("center.newAgent.name")}
      placeholder={t("center.newAgent.namePlaceholder")}
    />
  </label>
  {#if failure}
    <p class="failure" data-agent-error role="alert">{failure}</p>
  {/if}
  <button type="submit" data-start-agent>{t("center.newAgent.start")}</button>
</form>

<style>
  .popover {
    position: absolute;
    top: calc(100% + 6px);
    right: 0;
    z-index: 30;
    width: 248px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 10px;
    background: var(--surface-2);
    border: 1px solid var(--surface-3);
    border-radius: 10px;
    box-shadow: 0 12px 32px rgba(0, 0, 0, 0.5);
  }
  .head {
    margin: 0;
    font-size: 11px;
    font-weight: 600;
    color: var(--text-muted);
  }
  label {
    display: grid;
    gap: 4px;
    font-size: 11px;
    color: var(--text-muted);
  }
  .popover input:not([type="checkbox"]):not([type="radio"]) {
    width: 100%;
    min-width: 0;
    background: var(--surface);
    border: 1px solid var(--surface-3);
    border-radius: 8px;
    color: var(--text);
    padding: 6px 8px;
    font: inherit;
    font-size: 14px;
  }
  .popover input:not([type="checkbox"]):not([type="radio"]):hover,
  .popover input:not([type="checkbox"]):not([type="radio"]):focus-visible {
    border-color: var(--surface-3);
  }
  .list {
    display: flex;
    flex-direction: column;
    gap: 1px;
    max-height: 264px;
    overflow-y: auto;
  }
  .section {
    margin: 6px 0 2px;
    padding: 0 6px;
    font-size: 10.5px;
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--text-dim);
  }
  .divider {
    height: 1px;
    margin: 6px 0;
    background: var(--surface-3);
  }
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 4px 6px;
    background: transparent;
    border: 1px solid transparent;
    border-radius: 6px;
    color: var(--text);
    font: inherit;
    font-size: 13.5px;
    text-align: left;
    cursor: pointer;
  }
  .row.dim {
    color: var(--text-dim);
  }
  .row:hover,
  .row:focus-visible,
  .row[data-agent-active] {
    background: var(--surface-3);
  }
  .tile {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    border-radius: 5px;
    background: var(--surface-3);
    color: var(--text-muted);
    font-size: 11px;
    font-weight: 700;
    flex-shrink: 0;
  }
  .row[data-agent-active] .tile {
    background: var(--surface);
  }
  .label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .toggle {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 11px;
    color: var(--text-muted);
    cursor: pointer;
  }
  .flags {
    margin-left: auto;
    padding-left: 6px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 10.5px;
    color: var(--text-dim);
    flex-shrink: 0;
    max-width: 52%;
  }
  .empty {
    margin: 0;
    padding: 6px;
    font-size: 12px;
    color: var(--text-dim);
  }
  .failure {
    margin: 0;
    font-size: 11.5px;
    color: var(--error);
  }
  .popover button[type="submit"],
  .popover button[type="submit"]:hover {
    background: var(--text);
    border: 1px solid var(--text);
    color: var(--surface);
    border-radius: 8px;
    padding: 6px 10px;
    font: inherit;
    font-weight: 600;
    font-size: 13.5px;
    cursor: pointer;
  }
</style>
