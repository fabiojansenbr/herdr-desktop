// Remote files (spec 006): read-only explorer and text snapshots per SSH host over the SFTP
// provider (src-tauri/src/files/sftp.rs). DTOs reuse the 005 shapes (FilePage / TextSnapshot)
// plus the connection that produced them. The backend validates identity, roots, limits and
// encoding; this module only decides what the user sees: host badge, Somente leitura,
// Desatualizado when the cached read no longer matches the live connection, explicit reload on
// the new connection, cancel of the tab's own operation and the diff of two identified reads.
// Command names mirror `files::sftp::COMMANDS` (checked by src-tauri/tests/files_remote.rs).

import { invoke } from "@tauri-apps/api/core";
import { t } from "../i18n/index.svelte";
import { diffLines, diffSummary, type DiffLine } from "./diff";
import {
  uriKey,
  type FileEntryDto,
  type FileKind,
  type FilePageDto,
  type FileStatDto,
  type FileUriDto,
  type RuntimeError,
  type TextSnapshotDto,
} from "./types";

export const REMOTE_PROVIDER = "sftp";

/** Connection an operation is addressed to (qualified target without a pane). */
export interface RemoteFileTarget {
  endpoint: string;
  session: string;
  connection_generation: number;
  boot_id: string;
}

export interface RemoteConnectionStamp extends RemoteFileTarget {
  channel: number;
}

export type RemoteTextSnapshotDto = TextSnapshotDto & { connection: RemoteConnectionStamp };
export type RemoteFilePageDto = FilePageDto & { connection: RemoteConnectionStamp };

export interface RemoteHostDto {
  endpoint: string;
  label: string;
  session: string;
  phase: string;
  phase_label: string;
  online: boolean;
  connection_generation: number | null;
  boot_id: string | null;
  roots: string[];
  provider: string;
  capabilities: { list: boolean; read: boolean; stat: boolean; write: boolean };
}

export interface RemoteHostsView {
  revision: number;
  hosts: RemoteHostDto[];
}

export interface RemoteFilesBridge {
  hosts(): Promise<RemoteHostsView>;
  /** Resolves when hosts changed after `revision` (or after a short timeout). */
  watch(revision: number): Promise<RemoteHostsView>;
  list(opId: string, target: RemoteFileTarget, uri: FileUriDto, cursor: string | null): Promise<RemoteFilePageDto>;
  read(opId: string, target: RemoteFileTarget, uri: FileUriDto): Promise<RemoteTextSnapshotDto>;
  stat(opId: string, target: RemoteFileTarget, uri: FileUriDto): Promise<FileStatDto>;
  /** Cancels a pending operation; the backend ends only that host's SFTP channel. */
  cancel(opId: string): Promise<boolean>;
}

export function tauriRemoteFilesBridge(): RemoteFilesBridge {
  return {
    hosts: () => invoke<RemoteHostsView>("remote_files_hosts"),
    watch: (revision) => invoke<RemoteHostsView>("remote_files_watch", { revision }),
    list: (opId, target, uri, cursor) =>
      invoke<RemoteFilePageDto>("remote_files_list", { opId, target, uri, cursor }),
    read: (opId, target, uri) => invoke<RemoteTextSnapshotDto>("remote_files_read", { opId, target, uri }),
    stat: (opId, target, uri) => invoke<FileStatDto>("remote_files_stat", { opId, target, uri }),
    cancel: (opId) => invoke<boolean>("remote_files_cancel", { opId }),
  };
}

/** Explanations shown next to specific errors (the backend message stays the primary text). */
export function remoteErrorHint(code: string): string | null {
  switch (code) {
    case "sftp_unavailable":
    case "host_unavailable":
    case "remote_name_unsupported":
      return t(`files.hint.${code}`);
    case "timeout":
    case "operation_cancelled":
      return t("files.hint.cancelled");
    case "ssh_host_key_unknown":
    case "ssh_host_key_changed":
    case "ssh_authentication_required":
      return t("files.hint.ssh");
    default:
      return null;
  }
}

// ---------------------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------------------

interface RemoteDir {
  uri: FileUriDto;
  entries: FileEntryDto[];
  nextCursor: string | null;
  loading: boolean;
  error: RuntimeError | null;
  expanded: boolean;
  token: number;
  connection: RemoteConnectionStamp | null;
}

interface RemoteTab {
  id: string;
  uri: FileUriDto;
  name: string;
  endpoint: string;
  token: number;
  opId: string | null;
  snapshot: RemoteTextSnapshotDto | null;
  previous: RemoteTextSnapshotDto | null;
  error: RuntimeError | null;
  compare: boolean;
}

export interface RemoteFilesState {
  revision: number;
  hosts: RemoteHostDto[];
  endpoint: string | null;
  dirs: Record<string, RemoteDir>;
  tabs: Record<string, RemoteTab>;
  order: string[];
  active: string | null;
  nextToken: number;
}

export function initialRemoteState(): RemoteFilesState {
  return { revision: 0, hosts: [], endpoint: null, dirs: {}, tabs: {}, order: [], active: null, nextToken: 0 };
}

function hostOf(state: RemoteFilesState, endpoint: string | null): RemoteHostDto | null {
  return state.hosts.find((h) => h.endpoint === endpoint) ?? null;
}

/** Live target of a host, or null while it is not online. */
export function liveTarget(host: RemoteHostDto | null): RemoteFileTarget | null {
  if (!host || !host.online || host.connection_generation === null || host.boot_id === null) return null;
  return {
    endpoint: host.endpoint,
    session: host.session,
    connection_generation: host.connection_generation,
    boot_id: host.boot_id,
  };
}

function rootUri(host: RemoteHostDto): FileUriDto | null {
  const root = host.roots[0];
  return root ? { provider: REMOTE_PROVIDER, host: host.endpoint, path: root } : null;
}

type StaleReason = "host_offline" | "connection_renewed";

function staleReason(host: RemoteHostDto | null, stamp: RemoteConnectionStamp | null): StaleReason | null {
  if (!stamp) return null;
  const live = liveTarget(host);
  if (!live) return "host_offline";
  if (
    live.endpoint !== stamp.endpoint ||
    live.session !== stamp.session ||
    live.connection_generation !== stamp.connection_generation ||
    live.boot_id !== stamp.boot_id
  ) {
    return "connection_renewed";
  }
  return null;
}

// ---------------------------------------------------------------------------------------
// View model
// ---------------------------------------------------------------------------------------

export interface RemoteNode {
  key: string;
  uri: FileUriDto;
  name: string;
  kind: FileKind;
  depth: number;
  expanded: boolean;
  loading: boolean;
  error: RuntimeError | null;
  hint: string | null;
  hasMore: boolean;
  children: RemoteNode[];
}

export interface RemoteDiffView {
  baseLabel: string;
  currentLabel: string;
  lines: DiffLine[];
  added: number;
  removed: number;
  identical: boolean;
}

export interface RemoteTabView {
  id: string;
  name: string;
  path: string;
  endpoint: string;
  active: boolean;
  status: "loading" | "ready" | "failed";
  loading: boolean;
  content: string | null;
  snapshotLabel: string | null;
  stale: boolean;
  staleReason: StaleReason | null;
  badge: { host: string; readOnly: true; stale: boolean };
  error: RuntimeError | null;
  hint: string | null;
  canReload: boolean;
  canCancel: boolean;
  canCompare: boolean;
  diff: RemoteDiffView | null;
}

export interface RemoteFilesView {
  hosts: RemoteHostDto[];
  host: RemoteHostDto | null;
  /** Remote files are read-only in this version. */
  readOnly: true;
  online: boolean;
  onboarding: "no_host" | "host_offline" | "no_root" | null;
  canList: boolean;
  treeStale: boolean;
  rootLoading: boolean;
  rootError: RuntimeError | null;
  rootHint: string | null;
  rootHasMore: boolean;
  empty: boolean;
  nodes: RemoteNode[];
  tabs: RemoteTabView[];
  active: RemoteTabView | null;
}

function snapshotLabel(snapshot: RemoteTextSnapshotDto): string {
  return t("files.remote.snapshot", {
    id: snapshot.id.slice(0, 8),
    generation: snapshot.connection.connection_generation,
  });
}

function nodesFor(state: RemoteFilesState, key: string, depth: number): RemoteNode[] {
  const dir = state.dirs[key];
  if (!dir) return [];
  return dir.entries.map((entry) => {
    const entryKey = uriKey(entry.uri);
    const child = state.dirs[entryKey];
    const expanded = entry.kind === "Directory" && child?.expanded === true;
    return {
      key: entryKey,
      uri: entry.uri,
      name: entry.name,
      kind: entry.kind,
      depth,
      expanded,
      loading: child?.loading === true,
      error: child?.error ?? null,
      hint: child?.error ? remoteErrorHint(child.error.code) : null,
      hasMore: child?.nextCursor != null,
      children: expanded ? nodesFor(state, entryKey, depth + 1) : [],
    };
  });
}

function tabView(state: RemoteFilesState, tab: RemoteTab): RemoteTabView {
  const host = hostOf(state, tab.endpoint);
  const reason = staleReason(host, tab.snapshot?.connection ?? null);
  const loading = tab.opId !== null;
  const diff =
    tab.compare && tab.snapshot && tab.previous
      ? (() => {
          const lines = diffLines(tab.previous.content, tab.snapshot.content);
          const summary = diffSummary(lines);
          return {
            baseLabel: snapshotLabel(tab.previous),
            currentLabel: snapshotLabel(tab.snapshot),
            lines,
            ...summary,
            identical: summary.added === 0 && summary.removed === 0,
          };
        })()
      : null;
  return {
    id: tab.id,
    name: tab.name,
    path: tab.uri.path,
    endpoint: tab.endpoint,
    active: state.active === tab.id,
    status: tab.snapshot ? "ready" : loading ? "loading" : "failed",
    loading,
    content: tab.snapshot?.content ?? null,
    snapshotLabel: tab.snapshot ? snapshotLabel(tab.snapshot) : null,
    stale: reason !== null,
    staleReason: reason,
    badge: { host: host?.label ?? tab.endpoint, readOnly: true, stale: reason !== null },
    error: tab.error,
    hint: tab.error ? remoteErrorHint(tab.error.code) : null,
    canReload: liveTarget(host) !== null && !loading,
    canCancel: loading,
    canCompare: tab.snapshot !== null && tab.previous !== null,
    diff,
  };
}

/** Pure projection of the state: never issues commands. */
export function remoteViewModel(state: RemoteFilesState): RemoteFilesView {
  const host = hostOf(state, state.endpoint);
  const live = liveTarget(host);
  const root = host ? rootUri(host) : null;
  const rootKey = root ? uriKey(root) : null;
  const rootDir = rootKey ? state.dirs[rootKey] : undefined;
  const tabs = state.order
    .map((id) => state.tabs[id])
    .filter((tab): tab is RemoteTab => tab !== undefined)
    .map((tab) => tabView(state, tab));
  const onboarding = !host ? "no_host" : !live ? "host_offline" : !root ? "no_root" : null;
  const treeStale = rootDir?.connection ? staleReason(host, rootDir.connection) !== null : false;
  return {
    hosts: state.hosts,
    host,
    readOnly: true,
    online: live !== null,
    onboarding,
    canList: onboarding === null,
    treeStale,
    rootLoading: rootDir?.loading ?? false,
    rootError: rootDir?.error ?? null,
    rootHint: rootDir?.error ? remoteErrorHint(rootDir.error.code) : null,
    rootHasMore: !treeStale && rootDir?.nextCursor != null,
    empty:
      rootDir !== undefined &&
      !rootDir.loading &&
      rootDir.error === null &&
      rootDir.entries.length === 0 &&
      rootDir.nextCursor === null,
    nodes: rootKey ? nodesFor(state, rootKey, 0) : [],
    tabs,
    active: tabs.find((tab) => tab.active) ?? null,
  };
}

// ---------------------------------------------------------------------------------------
// Controller
// ---------------------------------------------------------------------------------------

export interface RemoteFilesController {
  readonly state: RemoteFilesState;
  applyHosts(view: RemoteHostsView): Promise<void>;
  refreshHosts(): Promise<void>;
  /** One watch round (the component loops while mounted). */
  watchOnce(): Promise<void>;
  selectHost(endpoint: string): Promise<void>;
  loadRoot(): Promise<void>;
  toggleDir(uri: FileUriDto): Promise<void>;
  loadMore(uri: FileUriDto): Promise<void>;
  openFile(entry: FileEntryDto): Promise<void>;
  reload(id: string): Promise<void>;
  cancel(id: string): Promise<void>;
  focusTab(id: string): void;
  closeTab(id: string): void;
  toggleCompare(id: string): void;
}

function asRuntimeError(error: unknown): RuntimeError {
  if (error && typeof error === "object" && "code" in error && "message" in error) {
    return error as RuntimeError;
  }
  return { code: "ipc_error", message: t("shell.error.ipc"), retryable: true };
}

export function defaultOperationIds(): () => string {
  let counter = 0;
  const session = Math.random().toString(36).slice(2, 10);
  return () => `rf-${session}-${++counter}`;
}

export function createRemoteFilesController(
  bridge: RemoteFilesBridge,
  onChange: (state: RemoteFilesState) => void = () => {},
  nextOpId: () => string = defaultOperationIds(),
): RemoteFilesController {
  let state = initialRemoteState();
  const commit = (next: RemoteFilesState) => {
    state = next;
    onChange(state);
  };
  const withDir = (key: string, patch: Partial<RemoteDir> & { uri: FileUriDto }) => {
    const current: RemoteDir = state.dirs[key] ?? {
      uri: patch.uri,
      entries: [],
      nextCursor: null,
      loading: false,
      error: null,
      expanded: false,
      token: 0,
      connection: null,
    };
    commit({ ...state, dirs: { ...state.dirs, [key]: { ...current, ...patch } } });
  };
  const withTab = (id: string, patch: Partial<RemoteTab>) => {
    const tab = state.tabs[id];
    if (!tab) return;
    commit({ ...state, tabs: { ...state.tabs, [id]: { ...tab, ...patch } } });
  };

  async function loadPage(uri: FileUriDto, append: boolean) {
    const host = hostOf(state, uri.host ?? null);
    const target = liveTarget(host);
    if (!target) return;
    const key = uriKey(uri);
    const dir = state.dirs[key];
    const renewed = dir?.connection ? staleReason(host, dir.connection) !== null : false;
    const cursor = append && !renewed ? (dir?.nextCursor ?? null) : null;
    if (append && cursor === null) return;
    const token = state.nextToken + 1;
    commit({ ...state, nextToken: token });
    withDir(key, { uri, loading: true, error: null, token });
    try {
      const page = await bridge.list(nextOpId(), target, uri, cursor);
      if (state.dirs[key]?.token !== token) return;
      const entries = cursor ? [...(state.dirs[key]?.entries ?? []), ...page.entries] : page.entries;
      withDir(key, { uri, entries, nextCursor: page.next_cursor, loading: false, error: null, connection: page.connection });
    } catch (error) {
      if (state.dirs[key]?.token !== token) return;
      // Never keep a partial listing that could pass for the whole directory.
      withDir(key, { uri, entries: [], nextCursor: null, loading: false, error: asRuntimeError(error), connection: null });
    }
  }

  async function readTab(id: string) {
    const tab = state.tabs[id];
    if (!tab) return;
    const target = liveTarget(hostOf(state, tab.endpoint));
    if (!target) return;
    const opId = nextOpId();
    const token = state.nextToken + 1;
    commit({ ...state, nextToken: token });
    withTab(id, { token, opId, error: null });
    try {
      const snapshot = await bridge.read(opId, target, tab.uri);
      const current = state.tabs[id];
      if (!current || current.token !== token) return;
      const previous =
        current.snapshot && current.snapshot.id !== snapshot.id ? current.snapshot : current.previous;
      withTab(id, { snapshot, previous, opId: null, error: null });
    } catch (error) {
      const current = state.tabs[id];
      if (!current || current.token !== token) return;
      // The cached snapshot (if any) is kept: a failed or cancelled reload never discards it.
      withTab(id, { opId: null, error: asRuntimeError(error) });
    }
  }

  const controller: RemoteFilesController = {
    get state() {
      return state;
    },
    async applyHosts(view) {
      if (view.revision === state.revision && view.hosts === state.hosts) return;
      commit({ ...state, revision: view.revision, hosts: view.hosts });
    },
    async refreshHosts() {
      await controller.applyHosts(await bridge.hosts());
    },
    async watchOnce() {
      await controller.applyHosts(await bridge.watch(state.revision));
    },
    async selectHost(endpoint) {
      if (state.endpoint !== endpoint) {
        commit({ ...state, endpoint, dirs: {} });
      }
      await controller.loadRoot();
    },
    async loadRoot() {
      const host = hostOf(state, state.endpoint);
      const root = host ? rootUri(host) : null;
      if (!root || !liveTarget(host)) return;
      withDir(uriKey(root), { uri: root, expanded: true });
      await loadPage(root, false);
    },
    async toggleDir(uri) {
      const key = uriKey(uri);
      const dir = state.dirs[key];
      if (!dir) {
        withDir(key, { uri, expanded: true });
        await loadPage(uri, false);
        return;
      }
      withDir(key, { uri, expanded: !dir.expanded });
    },
    loadMore: (uri) => loadPage(uri, true),
    async openFile(entry) {
      const id = uriKey(entry.uri);
      const existing = state.tabs[id];
      if (!existing) {
        const tab: RemoteTab = {
          id,
          uri: entry.uri,
          name: entry.name,
          endpoint: entry.uri.host ?? "",
          token: 0,
          opId: null,
          snapshot: null,
          previous: null,
          error: null,
          compare: false,
        };
        commit({ ...state, tabs: { ...state.tabs, [id]: tab }, order: [...state.order, id], active: id });
        await readTab(id);
        return;
      }
      commit({ ...state, active: id });
      if (!existing.snapshot && existing.opId === null) await readTab(id);
    },
    async reload(id) {
      const tab = state.tabs[id];
      if (!tab) return;
      await readTab(id);
    },
    async cancel(id) {
      const opId = state.tabs[id]?.opId;
      if (!opId) return;
      await bridge.cancel(opId);
    },
    focusTab(id) {
      if (state.tabs[id]) commit({ ...state, active: id });
    },
    closeTab(id) {
      if (!state.tabs[id]) return;
      const index = state.order.indexOf(id);
      const order = state.order.filter((other) => other !== id);
      const tabs = { ...state.tabs };
      delete tabs[id];
      const active = state.active === id ? (order[index] ?? order[index - 1] ?? null) : state.active;
      commit({ ...state, tabs, order, active });
    },
    toggleCompare(id) {
      const tab = state.tabs[id];
      if (tab) withTab(id, { compare: !tab.compare });
    },
  };
  return controller;
}

// ---------------------------------------------------------------------------------------
// In-memory bridge for the isolated browser preview
// ---------------------------------------------------------------------------------------

export function createFakeRemoteFilesBridge(): RemoteFilesBridge & { dropConnection(): void; reconnect(): void } {
  const endpoint = "0123456789abcdef0123456789abcdef";
  let host: RemoteHostDto = {
    endpoint,
    label: "servidor-exemplo",
    session: "trabalho",
    phase: "online",
    phase_label: "Online",
    online: true,
    connection_generation: 1,
    boot_id: "boot-exemplo",
    roots: ["/srv/projeto"],
    provider: REMOTE_PROVIDER,
    capabilities: { list: true, read: true, stat: true, write: false },
  };
  let revision = 1;
  const files: Record<string, string> = {
    "/srv/projeto/README.md": "# Projeto remoto\n\nSomente leitura nesta versão.\n",
    "/srv/projeto/src/main.rs": "fn main() {\n    println!(\"remoto\");\n}\n",
  };
  let reads = 0;
  const stamp = () => ({
    endpoint,
    session: host.session,
    connection_generation: host.connection_generation ?? 0,
    boot_id: host.boot_id ?? "",
    channel: host.connection_generation ?? 0,
  });
  const check = (target: RemoteFileTarget) => {
    if (!host.online) throw { code: "host_unavailable", message: "o host SSH não está conectado", retryable: true, endpoint };
    if (target.connection_generation !== host.connection_generation) {
      throw { code: "target_generation_stale", message: "a conexão foi renovada", retryable: false, endpoint };
    }
  };
  return {
    hosts: async () => ({ revision, hosts: [host] }),
    watch: async () => {
      await new Promise((resolve) => setTimeout(resolve, 500));
      return { revision, hosts: [host] };
    },
    async list(_opId, target, uri) {
      check(target);
      const prefix = uri.path.endsWith("/") ? uri.path : `${uri.path}/`;
      const names = new Map<string, FileKind>();
      for (const path of Object.keys(files)) {
        if (!path.startsWith(prefix)) continue;
        const [first, ...rest] = path.slice(prefix.length).split("/");
        names.set(first!, rest.length > 0 ? "Directory" : "File");
      }
      const entries = [...names].map(([name, kind]) => ({
        uri: { provider: REMOTE_PROVIDER, host: endpoint, path: `${prefix}${name}` },
        name,
        kind,
      }));
      return { uri, entries, next_cursor: null, connection: stamp() };
    },
    async read(_opId, target, uri) {
      check(target);
      const content = files[uri.path];
      if (content === undefined) throw { code: "file_not_found", message: "recurso não encontrado", retryable: false, endpoint };
      reads += 1;
      return {
        id: `fake${reads}-${Date.now().toString(16)}`,
        uri,
        content: reads > 1 ? `${content}alterado na leitura ${reads}\n` : content,
        bom: false,
        eol: "lf",
        size: content.length,
        modified_unix_ms: null,
        connection: stamp(),
      };
    },
    async stat(_opId, target, uri) {
      check(target);
      return { uri, kind: "File", size: files[uri.path]?.length ?? 0, modified_unix_ms: null, read_only: true };
    },
    cancel: async () => false,
    dropConnection() {
      host = { ...host, phase: "reconnecting", phase_label: "Reconectando", online: false };
      revision += 1;
    },
    reconnect() {
      host = { ...host, phase: "online", phase_label: "Online", online: true, connection_generation: (host.connection_generation ?? 0) + 1 };
      revision += 1;
    },
  };
}
