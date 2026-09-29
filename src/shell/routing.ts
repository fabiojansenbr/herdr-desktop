// Composition glue (spec 007): project open, agents and files follow the one host selection.
// These wrappers only order and fence calls to the existing feature bridges; they add no IPC
// command and never retarget an action to another host.

import { t } from "../i18n/index.svelte";
import type { AgentsBridge } from "../agents/bridge";
import type { AgentsEvent, QualifiedTarget } from "../agents/types";
import type { ConnectionsBridge } from "../connections/bridge";
import type { HostDto } from "../connections/types";
import type { FileTarget } from "../files/types";
import type { ProjectsBridge } from "../projects/bridge";
import type { OpenResponse, ProjectDto, ProjectsSnapshot, RuntimeError } from "../projects/types";
import type { PresentedIdentityDto, SurfaceController } from "./controller";
import type { SurfaceIdentityDto } from "./types";

const error = (code: string, message: string, endpoint?: string): RuntimeError => ({
  code,
  message,
  retryable: false,
  ...(endpoint ? { endpoint } : {}),
});

export interface HostWaitOptions {
  /** Upper bound for the whole wait (default 30 s). */
  timeoutMs?: number;
  signal?: AbortSignal;
}

function hostError(host: HostDto): RuntimeError | null {
  if (host.phase === "attention") {
    return {
      code: host.attention ?? "host_needs_attention",
      message: host.guidance ?? t("shell.routing.needsAttention", { host: host.label }),
      retryable: false,
      endpoint: host.endpoint,
    };
  }
  if (host.phase === "reconnecting") {
    return host.connection_error
      ? { ...host.connection_error, endpoint: host.endpoint }
      : { code: "host_reconnecting", message: t("shell.routing.reconnecting", { host: host.label }), retryable: true, endpoint: host.endpoint };
  }
  if (host.phase === "offline" && (host.cancelled || host.connection_error)) {
    return host.connection_error
      ? { ...host.connection_error, endpoint: host.endpoint }
      : { code: "connection_cancelled", message: t("shell.routing.cancelled", { host: host.label }), retryable: true, endpoint: host.endpoint };
  }
  return null;
}

/**
 * Connects `endpoint` (once, only if offline) and waits until the hub reports it online, following
 * `connections_watch`. Attention, a failed/retrying connection or a cancellation reject with the
 * host's own error; there is no second connect, no session start and no other host. The wait is
 * bounded by `timeoutMs` and ends on `signal` abort.
 */
export async function ensureHostOnline(
  connections: Pick<ConnectionsBridge, "list" | "connect" | "watch">,
  endpoint: string,
  { timeoutMs = 30_000, signal }: HostWaitOptions = {},
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  const cancelled = () => error("open_cancelled", "abertura cancelada", endpoint);
  let timer: ReturnType<typeof setTimeout> | undefined;
  let onAbort: (() => void) | undefined;
  const stop = new Promise<never>((_, reject) => {
    timer = setTimeout(
      () => reject({ ...error("host_connect_timeout", t("shell.routing.connectTimeout"), endpoint), retryable: true }),
      Math.max(0, deadline - Date.now()),
    );
    onAbort = () => reject(cancelled());
    signal?.addEventListener("abort", onAbort, { once: true });
  });
  stop.catch(() => {});
  const bounded = <T>(run: Promise<T>) => Promise.race([run, stop]);
  try {
    if (signal?.aborted) throw cancelled();
    let view = await bounded(connections.list());
    let host = view.hub.hosts.find((h) => h.endpoint === endpoint);
    if (!host) throw error("unknown_endpoint", t("shell.routing.unknownEndpoint"), endpoint);
    if (host.phase === "online") return;
    const initial = host.phase === "attention" || host.phase === "reconnecting" ? hostError(host) : null;
    if (initial) throw initial;
    if (host.phase === "offline") {
      view = await bounded(connections.connect(endpoint));
    }
    for (;;) {
      host = view.hub.hosts.find((h) => h.endpoint === endpoint);
      if (!host) throw error("unknown_endpoint", t("shell.routing.unknownEndpoint"), endpoint);
      if (host.phase === "online") return;
      const failure = hostError(host);
      if (failure) throw failure;
      view = await bounded(connections.watch(view.hub.revision));
    }
  } finally {
    clearTimeout(timer);
    if (onAbort) signal?.removeEventListener("abort", onAbort);
  }
}

export interface ProjectOpenDeps {
  ensureHost(endpoint: string): Promise<void>;
  /** Makes `endpoint` the selected host; rejects when refused or superseded. */
  select(endpoint: string): Promise<void>;
  onOpened?(project: ProjectDto, response: OpenResponse): void;
}

/**
 * ProjectsBridge whose `open` first connects and selects the project's own endpoint. Any
 * failure rejects `open`, so the projects controller shows it on that project only.
 */
export function selectingProjectsBridge(base: ProjectsBridge, deps: ProjectOpenDeps): ProjectsBridge {
  let snapshot: ProjectsSnapshot | null = null;
  const keep = async (run: Promise<ProjectsSnapshot>) => (snapshot = await run);
  return {
    list: () => keep(base.list()),
    createProject: (draft, collectionId) => keep(base.createProject(draft, collectionId)),
    createCollection: (name) => keep(base.createCollection(name)),
    addToCollection: (c, p, i) => keep(base.addToCollection(c, p, i)),
    removeFromCollection: (c, p) => keep(base.removeFromCollection(c, p)),
    moveProject: (c, p, i) => keep(base.moveProject(c, p, i)),
    moveCollection: (c, i) => keep(base.moveCollection(c, i)),
    groupCreate: (name) => keep(base.groupCreate(name)),
    groupAssign: (groupId, entry) => keep(base.groupAssign(groupId, entry)),
    // Spec 044: a client preference touches no host, so it never connects or selects one.
    workspacePrefSet: (input) => keep(base.workspacePrefSet(input)),
    // Spec 045: the collection menu edits the local catalog only — no host is involved.
    groupRename: (groupId, name) => keep(base.groupRename(groupId, name)),
    groupSetColor: (groupId, color) => keep(base.groupSetColor(groupId, color)),
    groupSetCollapsed: (groupId, collapsed) => keep(base.groupSetCollapsed(groupId, collapsed)),
    groupDelete: (groupId) => keep(base.groupDelete(groupId)),
    // Spec 046: the recents are a client list keyed by endpoint; recording one connects nothing.
    recentFolderAdd: (endpointProfileId, path) => keep(base.recentFolderAdd(endpointProfileId, path)),
    // Spec 025 AC-025-03: clicking a workspace of another host selects its host (so the center
    // shows the tabs/panes of that workspace) before the one `workspace_focus` that changes the
    // engine focus. No tab/pane command is ever sent.
    async workspaceFocus(endpoint, workspaceId) {
      await deps.ensureHost(endpoint);
      await deps.select(endpoint);
      await base.workspaceFocus(endpoint, workspaceId);
    },
    // "Abrir projeto…" and a closed project: connect and select the host, then one
    // `workspace.create` with the picked/saved cwd; the engine focuses it (`focus: true`).
    async workspaceCreate(input) {
      await deps.ensureHost(input.endpoint_profile_id);
      await deps.select(input.endpoint_profile_id);
      return base.workspaceCreate(input);
    },
    workspaceRename: (endpoint, workspaceId, label) => base.workspaceRename(endpoint, workspaceId, label),
    workspaceClose: (endpoint, workspaceId) => base.workspaceClose(endpoint, workspaceId),
    async open(projectId) {
      let project = snapshot?.projects.find((p) => p.id === projectId);
      if (!project) project = (await keep(base.list())).projects.find((p) => p.id === projectId);
      if (!project) throw error("project_not_found", t("shell.routing.projectNotFound"));
      const endpoint = project.endpoint_profile_id;
      await deps.ensureHost(endpoint);
      await deps.select(endpoint);
      const response = await base.open(projectId);
      snapshot = response.snapshot;
      deps.onOpened?.(response.snapshot.projects.find((p) => p.id === projectId) ?? project, response);
      return response;
    },
  };
}

export interface TerminalRevealDeps {
  surface: Pick<SurfaceController, "state" | "select" | "setInterest" | "whenInteractive" | "whenPresented" | "episode">;
  /** Closes the files layer so the terminal is the shown view (App: `filesShown = false`). */
  showTerminal(): void;
  /** Document visibility; a hidden document is never treated as visible to send an action. */
  documentVisible(): boolean;
  /** Bound of each wait for the surface (default 30 s). */
  timeoutMs?: number;
}

export interface TerminalReveal {
  /**
   * For an action on the already selected `endpoint` that needs its terminal surface: shows the
   * terminal and resolves with the confirmed identity once the backend acknowledged the interest
   * and a full frame arrived. Rejects (nothing else is sent) when the document is hidden, another
   * host is selected, the show fails, the surface is hidden again or the wait times out.
   */
  reveal(endpoint: string): Promise<SurfaceIdentityDto>;
  /**
   * Project open: selects `endpoint` when needed, shows the terminal, then waits for the presented
   * surface (ack + full of that attach) — not for a focused pane, so an empty session can create
   * its first workspace. Rejects as `reveal`.
   */
  selectAndReveal(endpoint: string): Promise<PresentedIdentityDto>;
}

const surfaceHidden = (endpoint: string) =>
  error("surface_hidden", t("shell.routing.windowHidden"), endpoint);

/** Orders "show the terminal, then wait for its surface" before actions that depend on it. */
export function createTerminalReveal(deps: TerminalRevealDeps): TerminalReveal {
  const { surface } = deps;
  // Both waits are fenced by the attach episode captured when the user acted (sync, before any await).
  /**
   * Shows the terminal now, synchronously (same tick as a selection start, so its attach already
   * runs presenting). The ack, a failed show and the full frame reach the waiter through the
   * controller; nothing is retried.
   */
  const show = () => {
    deps.showTerminal();
    void surface.setInterest(true);
  };
  return {
    async reveal(endpoint) {
      if (!deps.documentVisible()) throw surfaceHidden(endpoint);
      if (surface.state.selection?.endpoint !== endpoint) {
        throw error("selection_changed", t("shell.routing.selectionChanged"), endpoint);
      }
      const episode = surface.episode();
      show();
      return surface.whenInteractive(endpoint, { timeoutMs: deps.timeoutMs, episode });
    },
    async selectAndReveal(endpoint) {
      if (!deps.documentVisible()) throw surfaceHidden(endpoint);
      const state = surface.state;
      const selecting = state.selection?.endpoint !== endpoint || state.phase !== "live" ? surface.select(endpoint) : null;
      // Captured right after the selection began: a later switch (even back to this host) cancels.
      const episode = surface.episode();
      show();
      if (selecting) await selecting;
      return surface.whenPresented(endpoint, { timeoutMs: deps.timeoutMs, episode });
    },
  };
}

export interface ScopedAgentsOptions {
  /**
   * Composed window: awaited before actions that need the terminal surface (focus, split, ratio,
   * tabs, open attention). Agents start/prompt/input never wait for it.
   */
  reveal?(endpoint: string): Promise<SurfaceIdentityDto>;
}

/**
 * AgentsBridge fenced to the selected host: actions addressed to another endpoint are refused
 * locally (never sent), and events of a replaced agents channel are dropped. With `reveal`, a
 * surface action captures its target and agents channel first, waits for the terminal surface and
 * is sent once only if the same host is still selected, the channel was not replaced and the
 * surface confirmed the target's session/boot/generation; otherwise it is refused unsent.
 */
export function selectionScopedAgentsBridge(
  base: AgentsBridge,
  selected: () => string | null,
  options: ScopedAgentsOptions = {},
): AgentsBridge {
  let channel = 0;
  const fenced = <A extends unknown[], R>(run: (target: QualifiedTarget, ...args: A) => Promise<R>) =>
    (target: QualifiedTarget, ...args: A): Promise<R> => {
      if (target.endpoint !== selected()) {
        return Promise.reject(error("selection_changed", t("shell.routing.selectionChanged"), target.endpoint));
      }
      return run(target, ...args);
    };
  const surfaceAction = <A extends unknown[], R>(run: (target: QualifiedTarget, ...args: A) => Promise<R>) => {
    const reveal = options.reveal;
    if (!reveal) return fenced(run);
    return fenced(async (issued: QualifiedTarget, ...args: A): Promise<R> => {
      // Copy captured when the user acted: a caller mutating its object meanwhile re-addresses nothing.
      const target = { ...issued };
      const own = channel;
      const confirmed = await reveal(target.endpoint);
      if (own !== channel || target.endpoint !== selected()) {
        throw error("selection_changed", t("shell.routing.selectionChanged"), target.endpoint);
      }
      if (
        confirmed.endpoint !== target.endpoint ||
        confirmed.session !== target.session ||
        confirmed.boot_id !== target.boot_id ||
        confirmed.connection_generation !== target.connection_generation
      ) {
        throw error("target_stale", t("shell.routing.targetStale"), target.endpoint);
      }
      return run(target, ...args);
    });
  };
  return {
    connect(geometry, onEvent) {
      const own = ++channel;
      return base.connect(geometry, (event: AgentsEvent) => {
        if (own === channel) onEvent(event);
      });
    },
    overview: () => base.overview(),
    detach: () => base.detach(),
    startAgent: fenced((t, kind: string, name: string, autonomous?: boolean) => base.startAgent(t, kind, name, autonomous)),
    prompt: fenced((t, text: string, resend: boolean) => base.prompt(t, text, resend)),
    openAttention: surfaceAction((t) => base.openAttention(t)),
    split: surfaceAction((t, direction: "right" | "down") => base.split(t, direction)),
    focusPane: surfaceAction((t) => base.focusPane(t)),
    setSplitRatio: surfaceAction((t, path: boolean[], ratio: number) => base.setSplitRatio(t, path, ratio)),
    input: fenced((t, events: Parameters<AgentsBridge["input"]>[1]) => base.input(t, events)),
    createTab: surfaceAction((t) => base.createTab(t)),
    focusTab: surfaceAction((t, tabId: string) => base.focusTab(t, tabId)),
    closeTab: surfaceAction((t, tabId: string) => base.closeTab(t, tabId)),
    renameTab: surfaceAction((t, tabId: string, label: string) => base.renameTab(t, tabId, label)),
    zoomPane: surfaceAction((t, mode: Parameters<AgentsBridge["zoomPane"]>[1]) => base.zoomPane(t, mode)),
    closePane: surfaceAction((t) => base.closePane(t)),
  };
}

export const targetKey = (target: FileTarget) => `${target.provider}|${target.host ?? ""}|${target.root}`;

/** One instance per file target, created on first use and kept for the window's lifetime. */
export function createTargetRegistry<T>(factory: (target: FileTarget) => T) {
  const items = new Map<string, T>();
  return {
    /** Existing instance only; never creates (safe to read while rendering). */
    peek(target: FileTarget): T | undefined {
      return items.get(targetKey(target));
    },
    get(target: FileTarget): T {
      const key = targetKey(target);
      let item = items.get(key);
      if (item === undefined) {
        item = factory(target);
        items.set(key, item);
      }
      return item;
    },
  };
}
