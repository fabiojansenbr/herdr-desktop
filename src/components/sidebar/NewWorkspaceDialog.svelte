<script lang="ts">
  // Spec 046 — "Novo workspace" (`design/v2-03-novo-workspace.png`). One modal: the host as a
  // segmented control with its status (offline listed and disabled with its motive), the folder
  // (browsed with the 016 native dialog only on this computer — P3 — typed on an SSH host, with
  // the recents of the marked host), the collection and "Iniciar com".
  //
  // Criar runs the chain of AC-046-03 in order, each step an existing command of the chosen host:
  // the 037 switch, `workspace_create`, `agent_start` on the pane the host confirmed for the new
  // workspace and `group_assign`. A refusal stays in line here and nothing is tried elsewhere.
  import { untrack } from "svelte";
  import type { FrameContext } from "../../shell/frame-context";
  import { errorText, t } from "../../i18n/index.svelte";
  import { pickProjectFolder } from "../../projects/bridge";
  import {
    createWorkspaceFlow,
    hostChoices,
    pollFor,
    recentFoldersFor,
    SHELL_START,
    type NewWorkspaceDeps,
  } from "../../projects/new-workspace";
  import type { RuntimeError } from "../../projects/types";
  import { agentName } from "../center/model";

  let {
    ctx,
    endpoint = null,
    onclose,
    pickFolder = pickProjectFolder,
  }: {
    ctx: FrameContext;
    /** Host to mark when the dialog was asked for by another region; the selected one by default. */
    endpoint?: string | null;
    onclose: () => void;
    pickFolder?: () => Promise<string | null>;
  } = $props();

  const hosts = $derived(ctx.connections?.view?.hub.hosts ?? []);
  const choices = $derived(hostChoices(hosts));
  const kinds = $derived(ctx.agents?.kinds ?? []);
  const collections = $derived(ctx.navigator?.snapshot?.collections ?? []);

  // The host the dialog opened on; from here it is the user's choice, not the window's.
  let host = $state(untrack(() => endpoint ?? ctx.selectedEndpoint ?? "local"));
  let folder = $state("");
  let collectionId = $state("");
  let start = $state<string | null>(null);
  let busy = $state(false);
  let error = $state<RuntimeError | null>(null);

  const chosenStart = $derived(start ?? kinds[0] ?? SHELL_START);
  const isLocal = $derived((hosts.find((h) => h.endpoint === host)?.kind ?? (host === "local" ? "local" : "ssh")) === "local");
  const recents = $derived(recentFoldersFor(ctx.navigator?.snapshot?.recent_folders, host));

  function markHost(choice: { endpoint: string; reason: string | null }) {
    if (choice.reason !== null) return;
    host = choice.endpoint;
  }

  async function browse() {
    const picked = await pickFolder();
    if (picked === null) return;
    folder = picked;
  }

  /** The window's own agent start (017), on the pane of the workspace the host confirmed. */
  async function startAgent(paneId: string, kind: string) {
    const agents = ctx.controllers.agents;
    if (ctx.agents?.capabilities?.start_agent === false) {
      throw { code: "start_agent_unavailable", message: t("sidebar.newWorkspace.noAgentStart"), retryable: false } satisfies RuntimeError;
    }
    agents.editStart("paneId", paneId);
    agents.editStart("kind", kind);
    agents.editStart("name", agentName(kind, paneId));
    await agents.startAgent();
    const failure = agents.state.start.error;
    if (failure) throw failure;
  }

  const deps = (): NewWorkspaceDeps => ({
    selectedEndpoint: () => ctx.selectedEndpoint,
    selectHost: (target) => ctx.controllers.surface.select(target),
    createWorkspace: (input) => ctx.controllers.projects.createWorkspace(input),
    // The engine publishes the new workspace in the host snapshot; only then does a pane of it
    // exist to receive an agent. Nothing here polls `pane.read`.
    confirmPane: (target, workspaceId) =>
      pollFor(() => {
        const live = (ctx.connections?.view?.hub.hosts ?? []).find((h) => h.endpoint === target);
        if (!live?.workspaces?.some((w) => w.workspace_id === workspaceId)) return null;
        return live.panes.find((p) => p.workspace_id === workspaceId)?.pane_id ?? null;
      }),
    startAgent,
    assignToCollection: (group, entry) => ctx.controllers.projects.assignToGroupChecked(group, entry),
    recordRecent: (target, path) => ctx.controllers.projects.recordRecentFolder(target, path),
    sessionOf: (target) => hosts.find((h) => h.endpoint === target)?.session ?? "default",
  });

  async function create() {
    if (busy) return;
    busy = true;
    error = null;
    try {
      const failure = await createWorkspaceFlow(deps(), {
        endpoint: host,
        folder,
        collectionId: collectionId || null,
        start: chosenStart,
      });
      if (failure) {
        error = failure;
        return;
      }
      onclose();
    } finally {
      busy = false;
    }
  }

  /**
   * Esc closes and Ctrl/⌘+Enter creates from wherever the focus is — the same window listener
   * `HostPopover.svelte` uses. A handler on the backdrop only ever fired when the backdrop itself
   * held the focus, which never happened: the modal is opened from the sidebar, from the palette
   * and from the title bar, and the focus starts in the folder field.
   */
  $effect(() => {
    const onKeydown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onclose();
        return;
      }
      if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
        event.preventDefault();
        void create();
      }
    };
    window.addEventListener("keydown", onKeydown);
    return () => window.removeEventListener("keydown", onKeydown);
  });

  let folderInput: HTMLInputElement | undefined = $state();
  let focused = false;

  /** The first field of the design takes the focus, so the path is typed without a click. */
  $effect(() => {
    if (!folderInput || focused) return;
    focused = true;
    folderInput.focus();
  });
</script>

<div class="backdrop" data-new-workspace-dialog role="dialog" aria-modal="true" aria-label={t("sidebar.workspaces.new")}>
  <div class="sheet">
    <header class="head">
      <div class="title">
        <h2>{t("sidebar.workspaces.new")}</h2>
        <p>{t("sidebar.newWorkspace.subtitle")}</p>
      </div>
      <button type="button" class="close" data-dialog-close aria-label={t("sidebar.close")} onclick={onclose}>✕</button>
    </header>

    <div class="body">
      <section class="field">
        <span class="label">{t("sidebar.newWorkspace.host")}</span>
        <div class="segmented" role="group" aria-label={t("sidebar.newWorkspace.host")}>
          {#each choices as choice (choice.endpoint)}
            <button
              type="button"
              class="segment"
              data-host-option={choice.endpoint}
              data-reason={choice.reason}
              aria-pressed={choice.endpoint === host}
              aria-disabled={choice.reason !== null ? "true" : undefined}
              title={choice.reason === null ? choice.name : t("sidebar.newWorkspace.hostOffline", { name: choice.name })}
              onclick={() => markHost(choice)}
            >
              <span class="segment-name">{choice.name}</span>
              <span class="dot" data-tone={choice.online ? "online" : "offline"} aria-hidden="true"></span>
              {#if choice.reason !== null}<span class="segment-reason">{t("sidebar.newWorkspace.disconnected")}</span>{/if}
            </button>
          {/each}
        </div>
      </section>

      <section class="field">
        <span class="label" id="new-workspace-folder">{t("sidebar.newWorkspace.folder")}</span>
        <div class="folder">
          <input
            bind:this={folderInput}
            data-folder-input
            aria-labelledby="new-workspace-folder"
            placeholder={isLocal ? t("sidebar.newWorkspace.localPath") : t("sidebar.newWorkspace.remotePath")}
            bind:value={folder}
          />
          {#if isLocal}
            <button type="button" class="ghost" data-pick-folder onclick={() => void browse()}>{t("sidebar.newWorkspace.browse")}</button>
          {/if}
        </div>
        {#if recents.length > 0}
          <span class="hint">{t("sidebar.newWorkspace.recents")}</span>
          <ul class="recents">
            {#each recents as path (path)}
              <li>
                <button type="button" class="recent" data-recent={path} onclick={() => (folder = path)}>{path}</button>
              </li>
            {/each}
          </ul>
        {/if}
      </section>

      <div class="row">
        <section class="field">
          <span class="label" id="new-workspace-collection">{t("sidebar.newWorkspace.collection")}</span>
          <select
            data-collection
            aria-labelledby="new-workspace-collection"
            value={collectionId}
            onchange={(event) => (collectionId = event.currentTarget.value)}
          >
            <option value="">{t("sidebar.ungrouped")}</option>
            {#each collections as collection (collection.id)}
              <option value={collection.id}>{collection.name}</option>
            {/each}
          </select>
        </section>

        <section class="field">
          <span class="label">{t("sidebar.newWorkspace.startWith")}</span>
          <div class="segmented" role="group" aria-label={t("sidebar.newWorkspace.startWith")}>
            <button
              type="button"
              class="segment"
              data-start={SHELL_START}
              aria-pressed={chosenStart === SHELL_START}
              onclick={() => (start = SHELL_START)}
            >
              {t("sidebar.newWorkspace.shell")}
            </button>
            {#each kinds as kind (kind)}
              <button type="button" class="segment" data-start={kind} aria-pressed={chosenStart === kind} onclick={() => (start = kind)}>
                {kind}
              </button>
            {/each}
          </div>
        </section>
      </div>

      {#if error}
        <p class="error" data-new-workspace-error role="alert">{errorText(error)}</p>
      {/if}
    </div>

    <footer class="foot">
      <span class="note">{t("sidebar.newWorkspace.note")}</span>
      <div class="buttons">
        <button type="button" class="ghost" data-cancel onclick={onclose}>{t("sidebar.cancel")}</button>
        <button type="button" class="primary" data-submit disabled={busy} onclick={() => void create()}>{t("sidebar.newWorkspace.submit")}</button>
      </div>
    </footer>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 60;
    display: flex;
    align-items: center;
    justify-content: center;
    background-color: rgba(0, 0, 0, 0.45);
  }
  .sheet {
    width: min(560px, calc(100vw - 32px));
    max-height: calc(100vh - 48px);
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    background-color: var(--surface-2, #171717);
    border: 1px solid var(--surface-3, #242424);
    border-radius: 12px;
    box-shadow: 0 24px 64px rgba(0, 0, 0, 0.55);
    color: var(--text, #ededed);
  }
  .head {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    padding: 16px 16px 8px;
  }
  .title {
    flex: 1;
    min-width: 0;
  }
  h2 {
    margin: 0;
    font-size: 15px;
    font-weight: 600;
  }
  .head p {
    margin: 4px 0 0;
    font-size: 12px;
    color: var(--text-muted, #a3a3a3);
  }
  .close {
    flex-shrink: 0;
    background: transparent;
    border: none;
    font: inherit;
    font-size: 13px;
    color: var(--text-muted, #a3a3a3);
    cursor: pointer;
    padding: 2px 6px;
    border-radius: 6px;
  }
  .close:hover {
    background-color: var(--surface-3, #242424);
  }
  .body {
    display: flex;
    flex-direction: column;
    gap: 14px;
    padding: 8px 16px 16px;
  }
  .row {
    display: flex;
    gap: 12px;
    flex-wrap: wrap;
  }
  .row .field {
    flex: 1;
    min-width: 180px;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .label {
    font-size: 12px;
    color: var(--text-muted, #a3a3a3);
  }
  .hint {
    font-size: 11px;
    color: var(--text-muted, #a3a3a3);
  }
  .segmented {
    display: flex;
    gap: 4px;
    padding: 4px;
    border: 1px solid var(--surface-3, #242424);
    border-radius: 10px;
    background-color: var(--surface, #0a0a0a);
    flex-wrap: wrap;
  }
  .segment {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 10px;
    border: 1px solid transparent;
    border-radius: 8px;
    background: transparent;
    font: inherit;
    font-size: 12px;
    color: var(--text, #ededed);
    cursor: pointer;
  }
  .segment:hover {
    background-color: var(--surface-3, #242424);
  }
  .segment[aria-pressed="true"] {
    background-color: var(--surface-3, #242424);
    border-color: var(--accent, #d4d4d4);
  }
  .segment[aria-disabled="true"] {
    color: var(--text-muted, #a3a3a3);
    cursor: not-allowed;
  }
  .segment[aria-disabled="true"]:hover {
    background: transparent;
  }
  .segment-reason {
    font-size: 10px;
    color: var(--text-muted, #a3a3a3);
  }
  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    flex-shrink: 0;
    background-color: var(--idle, #6b7280);
  }
  .dot[data-tone="online"] {
    background-color: var(--working, #5bd68a);
  }
  .folder {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .sheet .folder input[data-folder-input],
  .sheet select {
    flex: 1;
    min-width: 0;
    background-color: var(--surface, #0a0a0a);
    border: 1px solid var(--surface-3, #242424);
    border-radius: 8px;
    font: inherit;
    font-size: 14px;
    color: var(--text, #ededed);
    padding: 8px 10px;
  }
  .sheet .folder input[data-folder-input]:hover,
  .sheet .folder input[data-folder-input]:focus-visible,
  .sheet select:hover,
  .sheet select:focus-visible {
    border-color: var(--surface-3, #242424);
  }
  .sheet input:focus-visible,
  .sheet select:focus-visible {
    outline: 2px solid var(--accent, #d4d4d4);
    outline-offset: -2px;
  }
  .recents {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .recent {
    display: block;
    width: 100%;
    text-align: left;
    padding: 6px 10px;
    border: none;
    border-radius: 8px;
    background: transparent;
    font: inherit;
    font-size: 12px;
    color: var(--text, #ededed);
    cursor: pointer;
  }
  .recent:hover {
    background-color: var(--surface-3, #242424);
  }
  .error {
    margin: 0;
    font-size: 12px;
    color: var(--attention-strong, #f87171);
  }
  .foot {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 12px 16px;
    border-top: 1px solid var(--surface-3, #242424);
  }
  .note {
    font-size: 11px;
    color: var(--text-muted, #a3a3a3);
  }
  .buttons {
    display: flex;
    gap: 8px;
  }
  .ghost,
  .primary {
    padding: 7px 12px;
    border-radius: 8px;
    font: inherit;
    font-size: 12px;
    cursor: pointer;
  }
  .ghost {
    border: 1px solid var(--surface-3, #242424);
    background: transparent;
    color: var(--text, #ededed);
  }
  .ghost:hover {
    background-color: var(--surface-3, #242424);
  }
  .sheet .primary,
  .sheet .primary:hover {
    border: 1px solid var(--text, #ededed);
    background: var(--text, #ededed);
    color: var(--surface, #0a0a0a);
    font-weight: 600;
  }
  .primary[disabled] {
    opacity: 0.6;
    cursor: default;
  }
</style>
