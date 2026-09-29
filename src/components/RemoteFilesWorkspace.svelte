<script lang="ts">
  // Remote files workspace (spec 006) in the review screen of spec 015: explorer and read-only tabs
  // of one SSH host over the SFTP provider, with the host badge, Somente leitura, Desatualizado for
  // cached reads, explicit reload/cancel and the side-by-side diff of two identified reads. No save
  // action exists. The editor is a dynamic import mounted read-only only when a tab has content and
  // its file view is selected; the review banner marks the read-only snapshot comparison.
  import { onMount, tick } from "svelte";
  import { errorText, phaseText, t } from "../i18n/index.svelte";
  import type { EditorHandle } from "../editor/editor";
  import RemoteFileBadge from "./RemoteFileBadge.svelte";
  import FileTabs from "./files/FileTabs.svelte";
  import ReviewHeader from "./files/ReviewHeader.svelte";
  import SideBySideDiff from "./files/SideBySideDiff.svelte";
  import { breadcrumb, tabEntries, type ReviewSelection } from "./files/review";
  import {
    createRemoteFilesController,
    remoteViewModel,
    type RemoteFilesBridge,
    type RemoteFilesController,
    type RemoteFilesState,
    type RemoteNode,
  } from "../files/remote";

  // Standalone (harness): pass `bridge`; the component lists hosts, lets the user pick one and
  // watches host changes. Composed window (spec 007): pass `controller` + `state` owned (and
  // watched) outside and the selected `endpoint`; the host is then the window's selection, so no
  // second host picker is shown.
  interface Props {
    bridge?: RemoteFilesBridge;
    controller?: RemoteFilesController;
    state?: RemoteFilesState;
    endpoint?: string | null;
  }

  let { bridge, controller: external, state: controlled, endpoint }: Props = $props();

  let own = $state<RemoteFilesState | null>(null);
  // svelte-ignore state_referenced_locally
  const controller = external ?? createRemoteFilesController(bridge!, (next) => (own = next));
  // svelte-ignore state_referenced_locally
  if (!external) own = controller.state;
  const files = $derived(controlled ?? own ?? controller.state);
  const view = $derived(remoteViewModel(files ?? controller.state));
  const selectable = $derived(endpoint === undefined);

  $effect(() => {
    const selected = endpoint;
    const known = files.hosts.some((host) => host.endpoint === selected);
    if (selected && known && controller.state.endpoint !== selected) void controller.selectHost(selected);
  });
  const active = $derived(view.active);
  const tabOf = (id: string) => view.tabs.find((tab) => tab.id === id) ?? null;
  let selection = $state<ReviewSelection>("file");
  // The diff entry is only meaningful while the tab has a diff; without one the file view returns.
  $effect(() => {
    if (!active?.diff) selection = "file";
  });
  const tabs = $derived(tabEntries(view.tabs, selection));
  const root = $derived(view.host?.roots[0] ?? null);
  const segments = $derived(breadcrumb(root, active?.path ?? ""));
  const banner = $derived(view.host ? t("files.remote.banner", { host: view.host.label }) : null);

  /** The five link phases carry the product's own words (spec 067); anything else is the engine's. */
  const PHASES = ["offline", "connecting", "online", "reconnecting", "attention"] as const;
  const hostPhase = (host: { phase: string; phase_label: string }) =>
    (PHASES as readonly string[]).includes(host.phase) ? phaseText(host.phase as (typeof PHASES)[number]) : host.phase_label;

  let explorer = $state<HTMLElement | null>(null);
  let editorHost = $state<HTMLElement | null>(null);
  let mounted: { host: HTMLElement; promise: Promise<EditorHandle> } | null = null;
  let editorChain: Promise<void> = Promise.resolve();

  function editorFor(host: HTMLElement): Promise<EditorHandle> {
    if (!mounted || mounted.host !== host) {
      mounted = { host, promise: import("../editor/editor").then((module) => module.mountEditor(host, { readOnly: true })) };
    }
    return mounted.promise;
  }

  $effect(() => {
    const host = editorHost;
    if (!host) {
      if (mounted) {
        const current = mounted;
        mounted = null;
        void current.promise.then((handle) => handle.destroy());
      }
      return;
    }
    const tab = active;
    if (!tab || tab.content === null) return;
    const key = `${tab.id}#${tab.snapshotLabel ?? ""}`;
    const content = tab.content;
    const name = tab.name;
    editorChain = editorChain.then(async () => {
      const handle = await editorFor(host);
      if (!host.isConnected) return;
      await handle.show(key, content, name, () => {});
    });
  });

  onMount(() => {
    // svelte-ignore state_referenced_locally
    if (external) return;
    let alive = true;
    void (async () => {
      try {
        await controller.refreshHosts();
      } catch {
        // The watch loop below retries; the explorer stays in its onboarding state meanwhile.
      }
      while (alive) {
        try {
          await controller.watchOnce();
        } catch {
          await new Promise((resolve) => setTimeout(resolve, 2000));
        }
      }
    })();
    return () => {
      alive = false;
    };
  });

  function openNode(node: RemoteNode) {
    if (node.kind === "Directory") void controller.toggleDir(node.uri);
    else void controller.openFile({ uri: node.uri, name: node.name, kind: node.kind });
  }

  /** `+ Abrir arquivo`: brings the explorer up and lands the focus on its first entry. */
  async function requestOpenFile() {
    if (!root || !view.canList) return;
    if (view.nodes.length === 0 && !view.rootLoading && !view.rootError) await controller.loadRoot();
    await tick();
    explorer?.querySelector<HTMLButtonElement>("button.remote-entry")?.focus();
  }

  function focusTab(id: string, kind: ReviewSelection) {
    controller.focusTab(id);
    selection = kind;
  }

  function compare(id: string) {
    controller.toggleCompare(id);
    selection = "diff";
  }
</script>

{#snippet tree(nodes: RemoteNode[])}
  {#each nodes as node (node.key)}
    <div class="node" style={`--depth:${node.depth}`}>
      <button
        type="button"
        class="remote-entry"
        data-path={node.uri.path}
        data-name={node.name}
        data-kind={node.kind}
        aria-expanded={node.kind === "Directory" ? node.expanded : undefined}
        onclick={() => openNode(node)}
      >
        <span class="icon">{node.kind === "Directory" ? (node.expanded ? "▾" : "▸") : "•"}</span>
        {node.name}
      </button>
      {#if node.loading}<span class="muted">{t("files.explorer.loadingDir")}</span>{/if}
      {#if node.error}
        <span class="error" data-remote-dir-error={node.error.code}>{node.error.code} — {errorText(node.error)}</span>
        {#if node.hint}<span class="hint">{node.hint}</span>{/if}
      {/if}
      {#if node.expanded}
        {#if node.children.length > 0}
          <div class="children">{@render tree(node.children)}</div>
        {:else if !node.loading && !node.hasMore && !node.error}
          <span class="muted">{t("files.explorer.emptyFolder")}</span>
        {/if}
        {#if node.hasMore}
          <button type="button" class="more" data-remote-load-more={node.uri.path} onclick={() => controller.loadMore(node.uri)}
            >{t("files.explorer.loadMore")}</button
          >
        {/if}
      {/if}
    </div>
  {/each}
{/snippet}

<section class="remote-files" data-remote-files-workspace>
  <aside class="explorer" aria-label={t("files.remote.title")} bind:this={explorer}>
    <header>
      <h2>{t("files.remote.title")}</h2>
      <button type="button" data-remote-refresh disabled={!view.canList} onclick={() => controller.loadRoot()}>{t("files.explorer.refresh")}</button>
    </header>

    {#if selectable}
    <div class="hosts" role="group" aria-label={t("files.remote.hosts")}>
      {#each view.hosts as host (host.endpoint)}
        <button
          type="button"
          class="host"
          class:selected={view.host?.endpoint === host.endpoint}
          data-remote-host={host.endpoint}
          data-online={host.online}
          data-phase={host.phase}
          data-generation={host.connection_generation ?? ""}
          aria-pressed={view.host?.endpoint === host.endpoint}
          onclick={() => controller.selectHost(host.endpoint)}
        >
          {host.label} <span class="phase">{hostPhase(host)}</span>
        </button>
      {/each}
    </div>
    {/if}

    {#if view.host}
      <div class="header" data-remote-header data-endpoint={view.host.endpoint}>
        <RemoteFileBadge host={view.host.label} />
        {#if view.host.roots[0]}<span class="root" title={view.host.roots[0]}>{view.host.roots[0]}</span>{/if}
      </div>
    {/if}

    {#if view.onboarding === "no_host"}
      <p class="muted" data-remote-onboarding="no_host">{t("files.remote.noHost")}</p>
    {:else if view.onboarding === "host_offline"}
      <p class="muted" data-remote-onboarding="host_offline">
        {t("files.remote.hostOffline", { host: view.host?.label ?? "", phase: view.host ? hostPhase(view.host) : "" })}
      </p>
    {:else if view.onboarding === "no_root"}
      <p class="muted" data-remote-onboarding="no_root">{t("files.remote.noRoot")}</p>
    {:else if view.rootLoading && view.nodes.length === 0}
      <p class="muted" aria-live="polite">{t("files.remote.loading")}</p>
    {:else if view.rootError}
      <p class="error" role="alert" data-remote-root-error={view.rootError.code}>
        {view.rootError.code} — {errorText(view.rootError)}
      </p>
      {#if view.rootHint}<p class="hint" data-remote-root-hint>{view.rootHint}</p>{/if}
    {:else if view.empty}
      <p class="muted" data-remote-onboarding="empty">{t("files.explorer.emptyFolder")}</p>
    {/if}
    {#if view.treeStale}
      <p class="notice" data-tree-stale>{t("files.remote.treeStale")}</p>
    {/if}

    <div class="tree" role="tree" aria-label={t("files.remote.tree")}>
      {@render tree(view.nodes)}
    </div>
    {#if view.rootHasMore && view.host?.roots[0]}
      <button
        type="button"
        class="more"
        data-remote-load-more={view.host.roots[0]}
        onclick={() => view.host && controller.loadMore({ provider: view.host.provider, host: view.host.endpoint, path: view.host.roots[0]! })}
        >{t("files.explorer.loadMore")}</button
      >
    {/if}
  </aside>

  <section class="workspace" aria-label={t("files.remote.workspace")} data-remote-review-workspace>
    <ReviewHeader {segments} {banner} />
    <FileTabs
      {tabs}
      variant="remote"
      staleOf={(entry) => tabOf(entry.id)?.stale ?? false}
      loadingOf={(entry) => tabOf(entry.id)?.loading ?? false}
      onFocus={focusTab}
      onClose={(id) => controller.closeTab(id)}
      onOpenFile={requestOpenFile}
    >
      {#snippet badge(entry)}
        <RemoteFileBadge host={tabOf(entry.id)?.badge.host ?? ""} stale={tabOf(entry.id)?.badge.stale ?? false} />
      {/snippet}
    </FileTabs>

    {#if active}
      <div class="document" data-remote-document={active.id}>
        <div class="toolbar">
          <span class="path" data-file-path title={active.path}>{active.path}</span>
          {#if active.snapshotLabel}<span class="muted" data-snapshot-label>{active.snapshotLabel}</span>{/if}
          <button type="button" data-reload disabled={!active.canReload} onclick={() => controller.reload(active.id)}>{t("files.workspace.reload")}</button>
          {#if active.canCancel}
            <button type="button" data-cancel onclick={() => controller.cancel(active.id)}>{t("files.workspace.cancel")}</button>
          {/if}
          <button type="button" data-compare disabled={!active.canCompare} onclick={() => compare(active.id)}
            >{t("files.remote.compare")}</button
          >
        </div>

        {#if active.stale}
          <p class="notice" role="status" data-stale-notice={active.staleReason}>
            {t("files.remote.stale", {
              snapshot: active.snapshotLabel ?? "",
              reason: active.staleReason === "host_offline" ? t("files.remote.staleOffline") : t("files.remote.stalePrevious"),
            })}
            {#if active.canReload}{t("files.remote.staleReload")}{/if}
          </p>
        {/if}

        {#if active.error}
          <p class="error" role="alert" data-remote-file-error={active.error.code}>
            <b>{active.error.code}</b> — {errorText(active.error)}
          </p>
          {#if active.hint}<p class="hint" data-remote-hint>{active.hint}</p>{/if}
        {/if}

        {#if active.status === "loading"}
          <p class="muted" data-remote-loading>{t("files.remote.loadingFile")}</p>
        {:else if active.status === "failed"}
          <p class="muted">{t("files.workspace.editorNotLoaded")}</p>
        {/if}

        {#if selection === "diff" && active.diff}
          <SideBySideDiff diff={active.diff} variant="remote" />
        {:else if active.content !== null}
          <div class="editor" data-remote-editor bind:this={editorHost}></div>
        {/if}
      </div>
    {:else}
      <p class="onboarding" data-remote-editor-empty data-review-empty>
        {t("files.remote.empty")}
      </p>
    {/if}
  </section>
</section>

<style>
  .remote-files {
    display: grid;
    grid-template-columns: 272px 1fr;
    gap: 12px;
    padding: 12px;
    align-items: start;
    color: #e7e9ee;
    font-family: Inter, system-ui, sans-serif;
  }
  .explorer,
  .workspace {
    background: #111318;
    border: 1px solid #242833;
    border-radius: 6px;
    padding: 10px;
    min-width: 0;
  }
  .explorer header,
  .toolbar,
  .hosts,
  .header {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
  }
  .explorer header {
    justify-content: space-between;
  }
  h2 {
    margin: 0;
    font-size: 13px;
  }
  .hosts {
    margin: 8px 0;
  }
  .host {
    background: #171a21;
    border: 1px solid #242833;
    border-radius: 6px;
    padding: 3px 8px;
    font-size: 11px;
    color: #e7e9ee;
    cursor: pointer;
    transition: border-color 0.15s ease, background-color 0.15s ease;
  }
  .host:hover {
    border-color: #8fa8ff;
    background: #242a36;
  }
  .host.selected {
    border-color: #8fa8ff;
    background: #1e222b;
  }
  .phase,
  .root,
  .path {
    font-size: 11px;
    color: #8c93a3;
  }
  .path {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tree {
    display: grid;
    gap: 2px;
    margin-top: 8px;
  }
  .node {
    padding-left: calc(var(--depth) * 12px);
    display: grid;
    gap: 2px;
  }
  .remote-entry {
    text-align: left;
    background: transparent;
    border: 1px solid transparent;
    color: inherit;
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 3px 6px;
    border-radius: 4px;
    width: 100%;
    cursor: pointer;
    transition: background-color 0.15s ease;
  }
  .remote-entry:hover {
    background: #171a21;
    border-color: #242833;
  }
  .remote-entry:focus-visible {
    outline: 2px solid #8fa8ff;
    outline-offset: 1px;
  }
  .children {
    display: grid;
    gap: 2px;
  }
  .editor {
    margin-top: 8px;
    min-height: 200px;
    border: 1px solid #242833;
    border-radius: 6px;
    overflow: auto;
  }
  .editor :global(.cm-editor) {
    min-height: 200px;
  }
  .notice {
    font-size: 11px;
    color: #f4b454;
  }
  .hint {
    font-size: 11px;
    color: #b8c2cc;
    margin: 2px 0;
  }
  .error {
    color: #f2777a;
    font-size: 12px;
    margin: 0;
  }
  .muted {
    color: #8c93a3;
    font-size: 11px;
  }
  .onboarding {
    color: #b8c2cc;
    font-size: 12px;
    max-width: 460px;
  }
  button {
    font: inherit;
    font-size: 12px;
    border-radius: 6px;
    background: #1e222b;
    border: 1px solid #242833;
    color: #e7e9ee;
    padding: 3px 10px;
    cursor: pointer;
    transition: border-color 0.15s ease, background-color 0.15s ease;
  }
  button:hover:not(:disabled) {
    border-color: #8fa8ff;
    background: #242a36;
  }
  button:disabled {
    opacity: 0.55;
    cursor: default;
  }
  button:focus-visible {
    outline: 2px solid #8fa8ff;
    outline-offset: 1px;
  }
</style>
