<script lang="ts">
  // Collections and projects (spec 002). Every row shows its own endpoint (Local/SSH id),
  // session and workspace state in text. "Remover da coleção" only edits the collection;
  // "Abrir" is the only action that reaches the engine. Loading never opens anything.
  // Reorder: Alt+↑/Alt+↓ on a focused row or collection header, or drag a row.
  import { onMount } from "svelte";
  import { errorText, t } from "../i18n/index.svelte";
  import type { ProjectsBridge } from "../projects/bridge";
  import { createProjectsController, type ProjectsController } from "../projects/controller";
  import { viewModel, type HostLabels, type NavigatorState } from "../projects/reducer";
  import type { ProjectDraft } from "../projects/types";

  // Standalone (harness): pass `bridge`; the component owns and loads its controller.
  // Composed window (spec 007): pass `controller` + `state` owned outside, so collapsing the
  // sidebar never loses navigator state; the owner loads it.
  interface Props {
    bridge?: ProjectsBridge;
    controller?: ProjectsController;
    state?: NavigatorState;
    /** Display names of endpoints already loaded by the App (identity stays the endpoint id). */
    hostLabels?: HostLabels;
  }

  let { bridge, controller: external, state: controlled, hostLabels }: Props = $props();

  let own = $state<NavigatorState | null>(null);
  // svelte-ignore state_referenced_locally
  const controller = external ?? createProjectsController(bridge!, (next) => (own = next));
  // svelte-ignore state_referenced_locally
  if (!external) own = controller.state;
  const nav = $derived(controlled ?? own ?? controller.state);
  const view = $derived(nav ? viewModel(nav, hostLabels) : null);

  let newCollection = $state("");
  let dragging = $state<{ collectionId: string; projectId: string } | null>(null);
  let addTarget = $state<Record<string, string>>({});

  // Read at render time (a getter per field), so a language change reaches the open form.
  const fieldLabels: Record<keyof ProjectDraft, string> = {
    get label() {
      return t("shell.navigator.field.label");
    },
    get endpoint_profile_id() {
      return t("shell.navigator.field.endpoint");
    },
    get session_name() {
      return t("shell.navigator.field.session");
    },
    get root() {
      return t("shell.navigator.field.root");
    },
  };
  const fields: (keyof ProjectDraft)[] = ["label", "endpoint_profile_id", "session_name", "root"];

  onMount(() => {
    // svelte-ignore state_referenced_locally
    if (!external) void controller.load();
  });

  async function submitCollection(event: SubmitEvent) {
    event.preventDefault();
    const name = newCollection;
    await controller.createCollection(name);
    if (!nav?.globalError) newCollection = "";
  }

  async function submitProject(event: SubmitEvent) {
    event.preventDefault();
    await controller.submitProject();
  }

  function rowKeydown(event: KeyboardEvent, collectionId: string, projectId: string) {
    if (!event.altKey || (event.key !== "ArrowUp" && event.key !== "ArrowDown")) return;
    event.preventDefault();
    const target = event.currentTarget as HTMLElement;
    void controller.moveByKeyboard(collectionId, projectId, event.key === "ArrowUp" ? "up" : "down").then(() => {
      // Keep keyboard focus on the moved row.
      queueMicrotask(() =>
        target.closest("ul")?.querySelector<HTMLElement>(`[data-project="${projectId}"] .handle`)?.focus(),
      );
    });
  }

  function collectionKeydown(event: KeyboardEvent, collectionId: string) {
    if (!event.altKey || (event.key !== "ArrowUp" && event.key !== "ArrowDown")) return;
    event.preventDefault();
    void controller.moveCollection(collectionId, event.key === "ArrowUp" ? "up" : "down");
  }

  function drop(event: DragEvent, collectionId: string, overId: string) {
    event.preventDefault();
    const drag = dragging;
    dragging = null;
    if (!drag || drag.collectionId !== collectionId) return;
    const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
    const placement = event.clientY < rect.top + rect.height / 2 ? "before" : "after";
    void controller.moveByDrag(collectionId, drag.projectId, overId, placement);
  }

  function addable(collectionId: string) {
    const members = new Set(nav?.snapshot?.collections.find((c) => c.id === collectionId)?.project_ids ?? []);
    return (nav?.snapshot?.projects ?? []).filter((p) => !members.has(p.id));
  }
</script>

<nav class="navigator" aria-label={t("shell.navigator.label")}>
  <header>
    <h2>{t("shell.navigator.collections")}</h2>
  </header>

  {#if nav?.globalError}
    <p class="error" role="alert"><b>{nav.globalError.code}</b> — {errorText(nav.globalError)}</p>
  {/if}

  {#if !view || !nav?.snapshot}
    <p class="muted" aria-live="polite">{nav?.loading ? t("shell.navigator.loading") : ""}</p>
  {:else}
    {#if view.empty}
      <div class="onboarding">
        <p><b>{t("shell.navigator.emptyTitle")}</b></p>
        <p>{t("shell.navigator.emptyBody")} <em>{t("shell.navigator.emptyBodyOpen")}</em>.</p>
      </div>
    {/if}

    <form class="inline" onsubmit={submitCollection}>
      <label>
        <span class="sr">{t("shell.navigator.newCollection")}</span>
        <input placeholder={t("shell.navigator.newCollection")} bind:value={newCollection} />
      </label>
      <button type="submit" disabled={newCollection.trim() === ""}>{t("shell.navigator.createCollection")}</button>
    </form>

    {#each view.collections as collection (collection.id)}
      <section class="collection" aria-label={t("shell.navigator.collection", { name: collection.name })} data-collection={collection.id}>
        <h3>
          {collection.name} <span class="count">{collection.projects.length}</span>
          <button
            class="handle"
            aria-label={t("shell.navigator.reorderCollection", { name: collection.name })}
            title={t("shell.navigator.reorderHint")}
            onkeydown={(e) => collectionKeydown(e, collection.id)}>⇅</button
          >
        </h3>
        {#if collection.error}
          <p class="error" role="alert"><b>{collection.error.code}</b> — {errorText(collection.error)}</p>
        {/if}
        {#if collection.projects.length === 0}
          <p class="muted">{t("shell.navigator.emptyCollection")}</p>
        {/if}
        <ul>
          {#each collection.projects as project (project.id)}
            <li
              data-project={project.id}
              class:dragging={dragging?.projectId === project.id && dragging?.collectionId === collection.id}
              aria-label={`${project.label}, ${project.endpointLabel}, ${project.statusText}`}
              ondragover={(e) => e.preventDefault()}
              ondrop={(e) => drop(e, collection.id, project.id)}
            >
              <div class="row">
                <button
                  class="handle"
                  draggable="true"
                  aria-label={t("shell.navigator.moveProject", { name: project.label })}
                  title={t("shell.navigator.moveHint")}
                  onkeydown={(e) => rowKeydown(e, collection.id, project.id)}
                  ondragstart={() => (dragging = { collectionId: collection.id, projectId: project.id })}
                  ondragend={() => (dragging = null)}>⠿</button
                >
                <span class="label">{project.label}</span>
                <span class="badge" title={t("shell.navigator.endpointTitle")}>{project.endpointLabel}</span>
              </div>
              <div class="meta">{t("shell.navigator.meta", { session: project.session, root: project.root })}</div>
              <div
                class="status"
                data-status={project.status}
                data-workspace={project.workspaceId ?? ""}
                data-outcome={project.outcome ?? ""}
                aria-live="polite"
              >
                {project.statusText}{project.outcomeText ? ` · ${project.outcomeText}` : ""}
              </div>
              {#if project.notice}<div class="notice">{project.notice}</div>{/if}
              {#if project.error}
                <div class="error" role="alert"><b>{project.error.code}</b> — {errorText(project.error)}</div>
              {/if}
              <div class="actions">
                <button onclick={() => controller.open(project.id)} disabled={project.opening}>
                  {project.error?.retryable ? t("connections.action.retry") : t("shell.navigator.open")}
                </button>
                <button onclick={() => controller.removeFromCollection(collection.id, project.id)}>{t("shell.navigator.remove")}</button>
              </div>
            </li>
          {/each}
        </ul>
        <div class="collection-actions">
          <button onclick={() => controller.openForm(collection.id)}>{t("shell.navigator.newProjectHere")}</button>
          {#if addable(collection.id).length > 0}
            <label>
              <span class="sr">{t("shell.navigator.addExistingTo", { name: collection.name })}</span>
              <select bind:value={addTarget[collection.id]}>
                <option value="">{t("shell.navigator.addExisting")}</option>
                {#each addable(collection.id) as candidate (candidate.id)}
                  <option value={candidate.id}>{candidate.label}</option>
                {/each}
              </select>
            </label>
            <button
              disabled={!addTarget[collection.id]}
              onclick={async () => {
                const projectId = addTarget[collection.id];
                if (projectId) await controller.addToCollection(collection.id, projectId);
                addTarget[collection.id] = "";
              }}>{t("shell.navigator.add")}</button
            >
          {/if}
        </div>
      </section>
    {/each}

    {#if view.unassigned.length > 0}
      <section class="collection" aria-label={t("shell.navigator.unassignedLabel")}>
        <h3>{t("shell.navigator.unassigned")}</h3>
        <ul>
          {#each view.unassigned as project (project.id)}
            <li data-project={project.id}>
              <div class="row">
                <span class="label">{project.label}</span>
                <span class="badge">{project.endpointLabel}</span>
              </div>
              <div class="status" data-status={project.status}>{project.statusText}</div>
              {#if project.error}
                <div class="error" role="alert"><b>{project.error.code}</b> — {errorText(project.error)}</div>
              {/if}
            </li>
          {/each}
        </ul>
      </section>
    {/if}

    {#if nav.form.open}
      <form class="project-form" onsubmit={submitProject} aria-label={t("shell.navigator.newProject")}>
        <h3>{t("shell.navigator.newProject")}</h3>
        {#each fields as field (field)}
          <label>
            <span>{fieldLabels[field]}</span>
            <input
              value={nav.form.draft[field]}
              aria-invalid={nav.form.missing.includes(field)}
              oninput={(e) => controller.editForm(field, (e.currentTarget as HTMLInputElement).value)}
            />
            {#if nav.form.missing.includes(field)}<small class="error">{t("shell.navigator.required")}</small>{/if}
          </label>
        {/each}
        {#if nav.form.error}
          <p class="error" role="alert"><b>{nav.form.error.code}</b> — {errorText(nav.form.error)}</p>
        {/if}
        <div class="actions">
          <button type="submit">{t("shell.navigator.saveProject")}</button>
          <button type="button" onclick={() => controller.cancelForm()}>{t("connections.action.cancel")}</button>
        </div>
      </form>
    {/if}
  {/if}
</nav>

<style>
  .navigator {
    display: grid;
    gap: 10px;
    width: 272px;
    max-width: 100%;
    box-sizing: border-box;
    padding: 10px;
    background: #111318;
    color: #e7e9ee;
    font-family: Inter, system-ui, sans-serif;
  }
  h2,
  h3 {
    margin: 0;
    font-size: 13px;
  }
  h3 {
    padding: 4px 0;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 6px;
  }
  li {
    padding: 8px;
    border: 1px solid #242833;
    border-radius: 6px;
    background: #171a21;
    display: grid;
    gap: 4px;
  }
  .handle {
    cursor: grab;
    padding: 0 4px;
    background: transparent;
    color: #8c93a3;
    border: 1px solid transparent;
  }
  .handle:focus-visible {
    outline: 2px solid #8fa8ff;
    outline-offset: 1px;
  }
  li.dragging {
    opacity: 0.5;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .label {
    flex: 1;
  }
  .badge {
    font-size: 11px;
    padding: 0 6px;
    border-radius: 6px;
    background: #1e222b;
    color: #e7e9ee;
  }
  .meta,
  .muted,
  .count,
  .status {
    font-size: 11px;
    color: #8c93a3;
  }
  .status[data-status="open"] {
    color: #5bd68a;
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
  .actions,
  .collection-actions,
  .inline {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .inline label {
    flex: 1;
    min-width: 0;
  }
  .inline input {
    width: 100%;
  }
  .collection-actions select {
    max-width: 100%;
  }
  .project-form {
    display: grid;
    gap: 6px;
  }
  .project-form label {
    display: grid;
    gap: 2px;
    font-size: 11px;
    color: #8c93a3;
  }
  .project-form input {
    width: 100%;
  }
  .sr {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
  }
  button,
  input,
  select {
    font: inherit;
    font-size: 12px;
    border-radius: 6px;
    box-sizing: border-box;
  }
  button {
    background: #1e222b;
    border: 1px solid #242833;
    color: #e7e9ee;
    padding: 4px 8px;
    cursor: pointer;
    transition: border-color 0.15s ease, background-color 0.15s ease;
  }
  button:hover:not(:disabled):not(.handle) {
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
  input,
  select {
    background: #171a21;
    border: 1px solid #242833;
    color: #e7e9ee;
    padding: 4px 8px;
    transition: border-color 0.15s ease;
  }
  input:focus-visible,
  select:focus-visible {
    outline: 2px solid #8fa8ff;
    outline-offset: 1px;
    border-color: #8fa8ff;
  }
  input::placeholder {
    color: #8c93a3;
    opacity: 1;
  }
  input[aria-invalid="true"] {
    border-color: #f2777a;
  }
</style>
