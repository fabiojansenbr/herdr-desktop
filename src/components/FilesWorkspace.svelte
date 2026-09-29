<script lang="ts">
  // Files workspace (spec 005) in the review screen of spec 015: paged explorer, one tab per file
  // plus its `arquivo · diff` entry, the side-by-side diff of the open base and the editor loaded
  // only on demand. The editor entry point is a dynamic import reached only when the first text
  // tab is ready and its file view is selected (AC-005-03, AC-015-01); the final window
  // composition belongs to spec 007.
  import { onMount, tick } from "svelte";
  import { errorText, t } from "../i18n/index.svelte";
  import type { FilesBridge } from "../files/bridge";
  import { createFilesController, type FilesController } from "../files/controller";
  import { targetUri, viewModel, type ExplorerNode, type FilesState } from "../files/reducer";
  import type { FileTarget } from "../files/types";
  import type { EditorHandle } from "../editor/editor";
  import FileTabs from "./files/FileTabs.svelte";
  import ReviewHeader from "./files/ReviewHeader.svelte";
  import SideBySideDiff from "./files/SideBySideDiff.svelte";
  import { breadcrumb, tabEntries, type ReviewSelection } from "./files/review";

  // Standalone (harness): pass `bridge`. Composed window (spec 007): pass `controller` + `state`
  // owned per target outside, so hiding the workspace or switching projects never discards
  // open (dirty) buffers; the editor view itself is recreated on demand.
  interface Props {
    bridge?: FilesBridge;
    target: FileTarget;
    controller?: FilesController;
    state?: FilesState;
  }

  let { bridge, target, controller: external, state: controlled }: Props = $props();

  let own = $state<FilesState | null>(null);
  // svelte-ignore state_referenced_locally
  const controller = external ?? createFilesController(bridge!, (next) => (own = next));
  // svelte-ignore state_referenced_locally
  if (!external) own = controller.state;
  const files = $derived(controlled ?? own ?? controller.state);
  const view = $derived(viewModel(files ?? controller.state));
  const active = $derived(view.active);
  let selection = $state<ReviewSelection>("file");
  const tabs = $derived(tabEntries(view.tabs, selection));
  const segments = $derived(breadcrumb(target.root, active?.path ?? ""));

  let explorer = $state<HTMLElement | null>(null);
  let editorHost = $state<HTMLElement | null>(null);
  let mounted: { host: HTMLElement; promise: Promise<EditorHandle> } | null = null;
  let editorChain: Promise<void> = Promise.resolve();

  function editorFor(host: HTMLElement): Promise<EditorHandle> {
    if (!mounted || mounted.host !== host) {
      mounted = { host, promise: import("../editor/editor").then((module) => module.mountEditor(host)) };
    }
    return mounted.promise;
  }

  $effect(() => {
    const currentTarget = target;
    const loaded = controller.state.target;
    // A controller that already shows this target keeps its explorer as it was.
    if (
      loaded &&
      loaded.provider === currentTarget.provider &&
      loaded.host === currentTarget.host &&
      loaded.root === currentTarget.root
    ) {
      return;
    }
    void controller.setTarget(currentTarget);
  });

  // The diff entry is only meaningful while the tab has a diff; without one the file view returns.
  $effect(() => {
    if (!active?.diff) selection = "file";
  });

  // The editor loads only for a ready tab shown as the file view; a tab with a message (too
  // large/binary) never mounts it, and leaving the file view tears the view down.
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
    if (!tab || tab.status !== "ready") return;
    const tabId = tab.id;
    // The view model is presentation-only; the buffer lives in the raw tab state.
    const buffer = files?.tabs[tabId]?.buffer ?? "";
    const name = tab.name;
    editorChain = editorChain.then(async () => {
      const handle = await editorFor(host);
      if (!host.isConnected) return;
      await handle.show(tabId, buffer, name, (text) => controller.edit(tabId, text));
    });
  });

  onMount(() => {
    // Watcher: only the open files are stat-ed, never the tree.
    const timer = window.setInterval(() => {
      if (document.visibilityState === "visible") void controller.pollOpenFiles();
    }, 1500);
    return () => window.clearInterval(timer);
  });

  function openNode(node: ExplorerNode) {
    if (node.kind === "Directory") void controller.toggleDir(node.uri);
    else void controller.openFile({ uri: node.uri, name: node.name, kind: node.kind });
  }

  /** `+ Abrir arquivo`: brings the explorer up and lands the focus on its first entry. */
  async function requestOpenFile() {
    if (!target.root) return;
    if (view.nodes.length === 0 && !view.rootLoading && !view.rootError) await controller.loadRoot();
    await tick();
    explorer?.querySelector<HTMLButtonElement>("button.entry")?.focus();
  }

  function focusTab(id: string, kind: ReviewSelection) {
    controller.focusTab(id);
    selection = kind;
  }

  function toggleDiff(id: string, base: "original" | "disk") {
    controller.toggleDiff(id, base);
    selection = "diff";
  }

  function compare(id: string) {
    controller.compare(id);
    selection = "diff";
  }
</script>

{#snippet tree(nodes: ExplorerNode[])}
  {#each nodes as node (node.key)}
    <div class="node" style={`--depth:${node.depth}`}>
      <button
        type="button"
        class="entry"
        data-path={node.uri.path}
        data-name={node.name}
        data-kind={node.kind}
        aria-expanded={node.kind === "Directory" ? node.expanded : undefined}
        onclick={() => openNode(node)}
      >
        <span class="icon">{node.kind === "Directory" ? (node.expanded ? "▾" : "▸") : "•"}</span>
        {node.name}
      </button>
      {#if node.loading}<span class="muted" data-dir-loading={node.uri.path}>{t("files.explorer.loadingDir")}</span>{/if}
      {#if node.error}
        <span class="error" data-dir-error={node.uri.path}>{node.error.code} — {errorText(node.error)}</span>
      {/if}
      {#if node.expanded}
        {#if node.children.length > 0}
          <div class="children">{@render tree(node.children)}</div>
        {:else if !node.loading && !node.hasMore}
          <span class="muted">{t("files.explorer.emptyFolder")}</span>
        {/if}
        {#if node.hasMore}
          <button
            type="button"
            class="more"
            data-load-more={node.uri.path}
            onclick={() => controller.loadMore(node.uri)}>{t("files.explorer.loadMore")}</button
          >
        {/if}
      {/if}
    </div>
  {/each}
{/snippet}

<section class="files" data-files-workspace>
  <aside class="explorer" aria-label={t("files.explorer.label")} bind:this={explorer}>
    <header>
      <h2>{t("files.explorer.title")}</h2>
      <button type="button" data-refresh onclick={() => controller.loadRoot()}>{t("files.explorer.refresh")}</button>
    </header>
    {#if !target.root}
      <p class="muted" data-explorer-empty>{t("files.explorer.noProject")}</p>
    {:else if view.rootLoading && view.nodes.length === 0}
      <p class="muted" aria-live="polite">{t("files.explorer.loading")}</p>
    {:else if view.rootError}
      <p class="error" role="alert" data-explorer-error={view.rootError.code}>
        {view.rootError.code} — {errorText(view.rootError)}
      </p>
    {:else if view.empty}
      <div class="onboarding" data-explorer-empty>
        <p><b>{t("files.explorer.emptyFolder")}</b></p>
        <p>{t("files.explorer.emptyRoot")}</p>
      </div>
    {/if}
    <div class="tree" role="tree" aria-label={t("files.explorer.tree")}>
      {@render tree(view.nodes)}
    </div>
    {#if view.rootHasMore}
      <button
        type="button"
        class="more"
        data-load-more={target.root}
        onclick={() => controller.loadMore(targetUri(target))}>{t("files.explorer.loadMore")}</button
      >
    {/if}
  </aside>

  <section class="workspace" aria-label={t("files.workspace.label")} data-review-workspace>
    <ReviewHeader {segments} />
    <FileTabs {tabs} dirtyOf={(entry) => files?.tabs[entry.id]?.dirty ?? false} onFocus={focusTab} onClose={(id) => controller.requestClose(id)} onOpenFile={requestOpenFile} />

    {#if active}
      <div class="document" data-active-tab={active.id}>
        {#if active.status === "failed"}
          <p class="error" role="alert" data-file-error={active.error?.code}>
            <b>{active.error?.code}</b> — {active.error ? errorText(active.error) : ""}
          </p>
          <p class="muted">{t("files.workspace.editorNotLoaded")}</p>
        {:else if active.status === "loading"}
          <p class="muted" data-file-loading>{t("files.workspace.loadingFile")}</p>
        {:else}
          <div class="toolbar">
            <span class="path" data-file-path title={active.path}>{active.path}</span>
            <button
              type="button"
              data-save
              disabled={!active.canSave || !active.dirty}
              onclick={() => controller.save(active.id)}>{t("files.workspace.save")}</button
            >
            <button
              type="button"
              data-diff-original
              onclick={() => toggleDiff(active.id, "original")}>{t("files.workspace.diffOriginal")}</button
            >
            {#if active.external}
              <span class="notice" data-external="true">{t("files.workspace.external")}</span>
            {/if}
            {#if active.saving}<span class="muted" data-saving>{t("files.workspace.saving")}</span>{/if}
            {#if active.notice}<span class="notice" data-notice>{active.notice}</span>{/if}
          </div>

          {#if active.conflict}
            <div class="conflict" role="alert" data-conflict>
              <p><b>{t("files.workspace.conflict")}</b> — {active.conflict.message}</p>
              <div class="actions">
                <button type="button" data-reload onclick={() => controller.reload(active.id)}>{t("files.workspace.reload")}</button>
                <button type="button" data-compare onclick={() => compare(active.id)}>{t("files.workspace.compare")}</button>
                <button type="button" data-save-copy onclick={() => controller.saveCopy(active.id)}>{t("files.workspace.saveCopy")}</button>
              </div>
              {#if active.recovery}
                <p class="muted" data-recovery={active.recovery.path}>{t("files.workspace.recovery", { path: active.recovery.path })}</p>
              {/if}
            </div>
          {/if}

          {#if active.closePrompt}
            <div class="confirm" role="alertdialog" aria-label={t("files.workspace.closePrompt")} data-close-prompt>
              <p>{t("files.workspace.unsaved")}</p>
              <div class="actions">
                <button type="button" data-close-save onclick={() => controller.close(active.id, "save")}>{t("files.workspace.saveAndClose")}</button>
                <button type="button" data-close-discard onclick={() => controller.close(active.id, "discard")}>{t("files.workspace.discard")}</button>
                <button type="button" data-close-cancel onclick={() => controller.cancelClose(active.id)}>{t("files.workspace.cancel")}</button>
              </div>
            </div>
          {/if}

          {#if selection === "diff" && active.diff}
            <SideBySideDiff diff={active.diff} />
          {:else}
            <div class="editor" data-editor-host bind:this={editorHost}></div>
          {/if}
        {/if}
      </div>
    {:else}
      <p class="onboarding" data-editor-empty data-review-empty>
        {t("files.workspace.empty")}
      </p>
    {/if}
  </section>
</section>

<style>
  .files {
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
  .explorer header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }
  h2 {
    margin: 0;
    font-size: 13px;
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
  .entry {
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
  .entry:hover {
    background: #171a21;
    border-color: #242833;
  }
  .entry:focus-visible {
    outline: 2px solid #8fa8ff;
    outline-offset: 1px;
  }
  .children {
    display: grid;
    gap: 2px;
  }
  .more {
    justify-self: start;
  }
  .toolbar,
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    align-items: center;
  }
  .path {
    font-size: 11px;
    color: #8c93a3;
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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
  .conflict,
  .confirm {
    margin-top: 8px;
    padding: 8px;
    border: 1px solid #5a2a30;
    border-radius: 6px;
    background: #201417;
  }
  .confirm {
    border-color: #5a4a24;
    background: #1f1b12;
  }
  .notice {
    font-size: 11px;
    color: #f4b454;
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
