// Files state (spec 005): paged directory listings, one buffer per open file, the conflict
// choice and the diff selection. The backend stays the authority for limits/encoding/conflict;
// nothing here re-implements detection, and no function in this module issues a command.

import { t } from "../i18n/index.svelte";
import { diffLines, diffSummary, type DiffLine } from "./diff";
import {
  uriKey,
  type FileEntryDto,
  type FileKind,
  type FileTarget,
  type FileUriDto,
  type RecoveryCopyDto,
  type RuntimeError,
  type TextSnapshotDto,
} from "./types";

/** URI of the explorer root for one target. */
export function targetUri(target: FileTarget): FileUriDto {
  return { provider: target.provider, host: target.host, path: target.root };
}

export interface ConflictState {
  /** Fresh snapshot of the disk version, when it is still readable text. */
  current: TextSnapshotDto | null;
  message: string;
}

export interface FileTab {
  id: string;
  uri: FileUriDto;
  name: string;
  path: string;
  /** Correlation of the latest load/reload; responses with another token are dropped. */
  token: number;
  status: "loading" | "ready" | "failed";
  base: TextSnapshotDto | null;
  buffer: string;
  dirty: boolean;
  saving: boolean;
  reloading: boolean;
  error: RuntimeError | null;
  conflict: ConflictState | null;
  closePrompt: boolean;
  diffBase: "original" | "disk" | null;
  external: boolean;
  notice: string | null;
  recovery: RecoveryCopyDto | null;
}

export interface DirState {
  uri: FileUriDto;
  entries: FileEntryDto[];
  nextCursor: string | null;
  loading: boolean;
  error: RuntimeError | null;
  expanded: boolean;
}

export interface FilesState {
  target: FileTarget | null;
  /** Loaded directory pages, keyed by uri; only expanded directories are rendered. */
  dirs: Record<string, DirState>;
  tabs: Record<string, FileTab>;
  order: string[];
  active: string | null;
  nextToken: number;
}

export type FilesAction =
  | { type: "target_set"; target: FileTarget }
  | { type: "dir_load_started"; uri: FileUriDto }
  | { type: "dir_loaded"; uri: FileUriDto; entries: FileEntryDto[]; nextCursor: string | null; append: boolean }
  | { type: "dir_failed"; uri: FileUriDto; error: RuntimeError }
  | { type: "dir_expanded"; uri: FileUriDto; expanded: boolean }
  | { type: "tab_open"; id: string; uri: FileUriDto; name: string }
  | { type: "tab_loaded"; id: string; token: number; snapshot: TextSnapshotDto }
  | { type: "tab_load_failed"; id: string; token: number; error: RuntimeError }
  | { type: "tab_focus"; id: string }
  | { type: "tab_edited"; id: string; text: string }
  | { type: "tab_save_started"; id: string }
  | { type: "tab_saved"; id: string; snapshot: TextSnapshotDto }
  | { type: "tab_conflict"; id: string; current: TextSnapshotDto | null; message: string }
  | { type: "tab_save_failed"; id: string; error: RuntimeError }
  | { type: "tab_recovery_saved"; id: string; copy: RecoveryCopyDto }
  | { type: "tab_reload_started"; id: string }
  | { type: "tab_reloaded"; id: string; token: number; snapshot: TextSnapshotDto }
  | { type: "tab_error"; id: string; token: number; error: RuntimeError }
  | { type: "tab_external"; id: string; external: boolean }
  | { type: "tab_close_prompt"; id: string }
  | { type: "tab_close_cancelled"; id: string }
  | { type: "tab_closed"; id: string }
  | { type: "tab_diff"; id: string; base: "original" | "disk" | null };

export function initialState(): FilesState {
  return { target: null, dirs: {}, tabs: {}, order: [], active: null, nextToken: 0 };
}

function dirOf(state: FilesState, uri: FileUriDto): DirState {
  return (
    state.dirs[uriKey(uri)] ?? {
      uri,
      entries: [],
      nextCursor: null,
      loading: false,
      error: null,
      expanded: false,
    }
  );
}

function withDir(state: FilesState, uri: FileUriDto, patch: Partial<DirState>): FilesState {
  return { ...state, dirs: { ...state.dirs, [uriKey(uri)]: { ...dirOf(state, uri), ...patch } } };
}

function withTab(state: FilesState, id: string, patch: Partial<FileTab>): FilesState {
  const tab = state.tabs[id];
  if (!tab) return state;
  return { ...state, tabs: { ...state.tabs, [id]: { ...tab, ...patch } } };
}

function dedupe(entries: FileEntryDto[]): FileEntryDto[] {
  const seen = new Set<string>();
  return entries.filter((entry) => {
    const key = uriKey(entry.uri);
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

export function reduce(state: FilesState, action: FilesAction): FilesState {
  switch (action.type) {
    case "target_set":
      // Pages belong to one target; open buffers never do.
      return { ...state, target: action.target, dirs: {} };
    case "dir_load_started":
      return withDir(state, action.uri, { loading: true, error: null });
    case "dir_loaded": {
      const current = dirOf(state, action.uri);
      const entries = action.append ? dedupe([...current.entries, ...action.entries]) : dedupe(action.entries);
      return withDir(state, action.uri, {
        entries,
        nextCursor: action.nextCursor,
        loading: false,
        error: null,
      });
    }
    case "dir_failed":
      return withDir(state, action.uri, { loading: false, error: action.error });
    case "dir_expanded":
      return withDir(state, action.uri, { expanded: action.expanded });
    case "tab_open": {
      const existing = state.tabs[action.id];
      if (existing) {
        if (existing.status === "failed") {
          const token = state.nextToken + 1;
          return {
            ...state,
            active: action.id,
            nextToken: token,
            tabs: {
              ...state.tabs,
              [action.id]: { ...existing, token, status: "loading", error: null, base: null, buffer: "" },
            },
          };
        }
        return { ...state, active: action.id };
      }
      const token = state.nextToken + 1;
      const tab: FileTab = {
        id: action.id,
        uri: action.uri,
        name: action.name,
        path: action.uri.path,
        token,
        status: "loading",
        base: null,
        buffer: "",
        dirty: false,
        saving: false,
        reloading: false,
        error: null,
        conflict: null,
        closePrompt: false,
        diffBase: null,
        external: false,
        notice: null,
        recovery: null,
      };
      return {
        ...state,
        nextToken: token,
        tabs: { ...state.tabs, [action.id]: tab },
        order: [...state.order, action.id],
        active: action.id,
      };
    }
    case "tab_loaded": {
      const tab = state.tabs[action.id];
      if (!tab || tab.status !== "loading" || tab.token !== action.token) return state;
      return withTab(state, action.id, {
        status: "ready",
        base: action.snapshot,
        buffer: action.snapshot.content,
        dirty: false,
        error: null,
        conflict: null,
        external: false,
        notice: null,
        recovery: null,
        diffBase: null,
      });
    }
    case "tab_load_failed": {
      const tab = state.tabs[action.id];
      if (!tab || tab.status !== "loading" || tab.token !== action.token) return state;
      return withTab(state, action.id, { status: "failed", error: action.error });
    }
    case "tab_focus":
      return state.tabs[action.id] ? { ...state, active: action.id } : state;
    case "tab_edited": {
      const tab = state.tabs[action.id];
      if (!tab || tab.status !== "ready") return state;
      return withTab(state, action.id, {
        buffer: action.text,
        dirty: action.text !== (tab.base?.content ?? ""),
      });
    }
    case "tab_save_started":
      return withTab(state, action.id, { saving: true, error: null });
    case "tab_saved":
      return withTab(state, action.id, {
        base: action.snapshot,
        dirty: state.tabs[action.id]!.buffer !== action.snapshot.content,
        saving: false,
        conflict: null,
        external: false,
        error: null,
        notice: t("files.notice.saved"),
      });
    case "tab_conflict":
      return withTab(state, action.id, {
        saving: false,
        conflict: { current: action.current, message: action.message },
        notice: null,
      });
    case "tab_save_failed":
      return withTab(state, action.id, { saving: false, error: action.error });
    case "tab_recovery_saved":
      return withTab(state, action.id, {
        recovery: action.copy,
        notice: t("files.notice.recoveryCopy", { path: action.copy.path }),
      });
    case "tab_reload_started": {
      const tab = state.tabs[action.id];
      if (!tab) return state;
      const token = state.nextToken + 1;
      return {
        ...state,
        nextToken: token,
        tabs: { ...state.tabs, [action.id]: { ...tab, token, reloading: true, error: null } },
      };
    }
    case "tab_reloaded": {
      const tab = state.tabs[action.id];
      if (!tab || tab.token !== action.token) return state;
      return withTab(state, action.id, {
        base: action.snapshot,
        buffer: action.snapshot.content,
        dirty: false,
        reloading: false,
        conflict: null,
        external: false,
        diffBase: null,
        error: null,
        notice: t("files.notice.reloaded"),
      });
    }
    case "tab_error": {
      const tab = state.tabs[action.id];
      if (!tab || tab.token !== action.token) return state;
      return withTab(state, action.id, { reloading: false, saving: false, error: action.error });
    }
    case "tab_external":
      return withTab(state, action.id, { external: action.external });
    case "tab_close_prompt":
      return withTab(state, action.id, { closePrompt: true });
    case "tab_close_cancelled":
      return withTab(state, action.id, { closePrompt: false });
    case "tab_closed": {
      if (!state.tabs[action.id]) return state;
      const index = state.order.indexOf(action.id);
      const order = state.order.filter((id) => id !== action.id);
      const tabs = { ...state.tabs };
      delete tabs[action.id];
      const active =
        state.active === action.id
          ? (order[index] ?? order[index - 1] ?? null)
          : state.active;
      return { ...state, tabs, order, active };
    }
    case "tab_diff":
      return withTab(state, action.id, { diffBase: action.base });
  }
}

// ---------------------------------------------------------------------------------------
// View model
// ---------------------------------------------------------------------------------------

export interface ExplorerNode {
  key: string;
  uri: FileUriDto;
  name: string;
  kind: FileKind;
  depth: number;
  expanded: boolean;
  loading: boolean;
  error: RuntimeError | null;
  hasMore: boolean;
  children: ExplorerNode[];
}

export interface DiffView {
  base: "original" | "disk";
  baseLabel: string;
  currentLabel: string;
  lines: DiffLine[];
  added: number;
  removed: number;
  identical: boolean;
}

export interface TabView {
  id: string;
  name: string;
  path: string;
  uri: FileUriDto;
  active: boolean;
  status: FileTab["status"];
  dirty: boolean;
  saving: boolean;
  error: RuntimeError | null;
  conflict: ConflictState | null;
  closePrompt: boolean;
  external: boolean;
  notice: string | null;
  recovery: RecoveryCopyDto | null;
  canSave: boolean;
  diff: DiffView | null;
}

export interface FilesView {
  empty: boolean;
  rootLoading: boolean;
  rootError: RuntimeError | null;
  rootHasMore: boolean;
  canList: boolean;
  nodes: ExplorerNode[];
  tabs: TabView[];
  active: TabView | null;
}

function diffView(tab: FileTab): DiffView | null {
  if (!tab.diffBase) return null;
  const base = tab.diffBase === "disk" ? (tab.conflict?.current ?? null) : tab.base;
  if (!base) return null;
  const lines = diffLines(base.content, tab.buffer);
  const summary = diffSummary(lines);
  return {
    base: tab.diffBase,
    baseLabel:
      tab.diffBase === "disk"
        ? t("files.diff.baseDisk", { id: base.id.slice(0, 8) })
        : t("files.diff.baseOriginal", { id: base.id.slice(0, 8) }),
    currentLabel: t("files.diff.current"),
    lines,
    ...summary,
    identical: summary.added === 0 && summary.removed === 0,
  };
}

function tabView(tab: FileTab, active: boolean): TabView {
  return {
    id: tab.id,
    name: tab.name,
    path: tab.path,
    uri: tab.uri,
    active,
    status: tab.status,
    dirty: tab.dirty,
    saving: tab.saving,
    error: tab.error,
    conflict: tab.conflict,
    closePrompt: tab.closePrompt,
    external: tab.external,
    notice: tab.notice,
    recovery: tab.recovery,
    canSave: tab.status === "ready" && tab.base !== null && !tab.saving && !tab.reloading,
    diff: diffView(tab),
  };
}

function nodesFor(state: FilesState, key: string, depth: number): ExplorerNode[] {
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
      hasMore: child?.nextCursor != null,
      children: expanded ? nodesFor(state, entryKey, depth + 1) : [],
    };
  });
}

/** Pure projection of the state: renders only expanded pages, never issues commands. */
export function viewModel(state: FilesState): FilesView {
  const rootKey = state.target ? uriKey(targetUri(state.target)) : null;
  const root = rootKey ? state.dirs[rootKey] : undefined;
  const tabs = state.order
    .map((id) => state.tabs[id])
    .filter((tab): tab is FileTab => tab !== undefined)
    .map((tab) => tabView(tab, state.active === tab.id));
  return {
    empty:
      root !== undefined &&
      !root.loading &&
      root.error === null &&
      root.entries.length === 0 &&
      root.nextCursor === null,
    rootLoading: root?.loading ?? false,
    rootError: root?.error ?? null,
    rootHasMore: root?.nextCursor != null,
    canList: rootKey !== null,
    nodes: rootKey ? nodesFor(state, rootKey, 0) : [],
    tabs,
    active: tabs.find((tab) => tab.active) ?? null,
  };
}
