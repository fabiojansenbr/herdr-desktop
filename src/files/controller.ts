// Drives the explorer, tabs and conflict choices: every user intent becomes one bridge
// command; listing and reading happen in event handlers, never during render. Errors stay on
// the affected resource and no path discards a dirty buffer without an explicit choice.

import { t } from "../i18n/index.svelte";
import type { FilesBridge } from "./bridge";
import {
  initialState,
  reduce,
  targetUri,
  type FileTab,
  type FilesAction,
  type FilesState,
} from "./reducer";
import { uriKey, type FileEntryDto, type FileTarget, type FileUriDto, type RuntimeError } from "./types";

export interface FilesController {
  readonly state: FilesState;
  setTarget(target: FileTarget): Promise<void>;
  loadRoot(): Promise<void>;
  refreshDir(uri: FileUriDto): Promise<void>;
  toggleDir(uri: FileUriDto): Promise<void>;
  loadMore(uri: FileUriDto): Promise<void>;
  openFile(entry: FileEntryDto): Promise<void>;
  focusTab(id: string): void;
  edit(id: string, text: string): void;
  save(id: string): Promise<void>;
  saveCopy(id: string): Promise<void>;
  reload(id: string): Promise<void>;
  compare(id: string): void;
  toggleDiff(id: string, base: "original" | "disk"): void;
  requestClose(id: string): void;
  cancelClose(id: string): void;
  close(id: string, action: "save" | "discard"): Promise<void>;
  /** Watcher: stats only the open files and flips the external notice when needed. */
  pollOpenFiles(): Promise<void>;
}

function asRuntimeError(error: unknown): RuntimeError {
  if (error && typeof error === "object" && "code" in error && "message" in error) {
    return error as RuntimeError;
  }
  return { code: "ipc_error", message: t("shell.error.ipc"), retryable: true };
}

export function createFilesController(
  bridge: FilesBridge,
  onChange: (state: FilesState) => void = () => {},
): FilesController {
  let state = initialState();
  const dispatch = (action: FilesAction) => {
    state = reduce(state, action);
    onChange(state);
  };

  async function loadPage(uri: FileUriDto, append: boolean) {
    const cursor = append ? (state.dirs[uriKey(uri)]?.nextCursor ?? null) : null;
    if (append && cursor === null) return;
    dispatch({ type: "dir_load_started", uri });
    try {
      const page = await bridge.list(uri, cursor);
      dispatch({
        type: "dir_loaded",
        uri,
        entries: page.entries,
        nextCursor: page.next_cursor,
        append,
      });
    } catch (error) {
      dispatch({ type: "dir_failed", uri, error: asRuntimeError(error) });
    }
  }

  async function loadTab(id: string, uri: FileUriDto) {
    const token = state.tabs[id]?.token;
    if (token === undefined) return;
    try {
      const snapshot = await bridge.read(uri);
      dispatch({ type: "tab_loaded", id, token, snapshot });
    } catch (error) {
      dispatch({ type: "tab_load_failed", id, token, error: asRuntimeError(error) });
    }
  }

  async function closeTab(id: string, action: "save" | "discard") {
    if (!state.tabs[id]) return;
    if (action === "save") {
      if (state.tabs[id]!.dirty) await save(id);
      const after = state.tabs[id];
      if (!after || after.conflict || after.error || after.saving) return;
    }
    const base = state.tabs[id]?.base ?? null;
    if (base) {
      try {
        await bridge.release([base.id]);
      } catch {
        // Cleanup only: the tab may still close; snapshots are also bounded in the backend.
      }
    }
    dispatch({ type: "tab_closed", id });
  }

  async function save(id: string) {
    const tab = state.tabs[id];
    if (!tab || !tab.base || tab.status !== "ready") return;
    const snapshotId = tab.base.id;
    const content = tab.buffer;
    dispatch({ type: "tab_save_started", id });
    try {
      const outcome = await bridge.save(snapshotId, content);
      if (outcome.outcome === "saved") {
        dispatch({ type: "tab_saved", id, snapshot: outcome.snapshot });
      } else {
        dispatch({ type: "tab_conflict", id, current: outcome.current, message: outcome.message });
      }
    } catch (error) {
      dispatch({ type: "tab_save_failed", id, error: asRuntimeError(error) });
    }
  }

  return {
    get state() {
      return state;
    },
    async setTarget(target) {
      dispatch({ type: "target_set", target });
      await loadPage(targetUri(target), false);
    },
    async loadRoot() {
      if (state.target) await loadPage(targetUri(state.target), false);
    },
    refreshDir: (uri) => loadPage(uri, false),
    async toggleDir(uri) {
      const dir = state.dirs[uriKey(uri)];
      if (!dir) {
        dispatch({ type: "dir_expanded", uri, expanded: true });
        await loadPage(uri, false);
        return;
      }
      dispatch({ type: "dir_expanded", uri, expanded: !dir.expanded });
    },
    loadMore: (uri) => loadPage(uri, true),
    async openFile(entry) {
      const id = uriKey(entry.uri);
      const existing = state.tabs[id];
      dispatch({ type: "tab_open", id, uri: entry.uri, name: entry.name });
      if (!existing || existing.status === "failed") await loadTab(id, entry.uri);
    },
    focusTab: (id) => dispatch({ type: "tab_focus", id }),
    edit: (id, text) => dispatch({ type: "tab_edited", id, text }),
    save,
    async saveCopy(id) {
      const tab = state.tabs[id];
      if (!tab || !tab.base) return;
      try {
        const copy = await bridge.saveRecovery(tab.base.id, tab.buffer);
        dispatch({ type: "tab_recovery_saved", id, copy });
      } catch (error) {
        dispatch({ type: "tab_save_failed", id, error: asRuntimeError(error) });
      }
    },
    async reload(id) {
      const tab = state.tabs[id];
      if (!tab || tab.status !== "ready") return;
      dispatch({ type: "tab_reload_started", id });
      const token = state.tabs[id]!.token;
      try {
        const snapshot = await bridge.read(tab.uri);
        dispatch({ type: "tab_reloaded", id, token, snapshot });
      } catch (error) {
        dispatch({ type: "tab_error", id, token, error: asRuntimeError(error) });
      }
    },
    compare: (id) => dispatch({ type: "tab_diff", id, base: "disk" }),
    toggleDiff: (id, base) =>
      dispatch({
        type: "tab_diff",
        id,
        base: state.tabs[id]?.diffBase === base ? null : base,
      }),
    requestClose(id) {
      const tab: FileTab | undefined = state.tabs[id];
      if (!tab) return;
      if (tab.dirty) {
        // The prompt belongs to the tab that asked to close; show it focused.
        dispatch({ type: "tab_focus", id });
        dispatch({ type: "tab_close_prompt", id });
        return;
      }
      void closeTab(id, "discard");
    },
    cancelClose: (id) => dispatch({ type: "tab_close_cancelled", id }),
    close: closeTab,
    async pollOpenFiles() {
      const open = state.order
        .map((id) => state.tabs[id])
        .filter((tab): tab is FileTab => tab !== undefined && tab.status === "ready" && tab.base !== null);
      for (const tab of open) {
        let external: boolean;
        try {
          const stat = await bridge.stat(tab.uri);
          const current = state.tabs[tab.id];
          if (!current || current.status !== "ready") continue;
          external =
            stat.size !== current.base?.size ||
            (stat.modified_unix_ms ?? null) !== (current.base?.modified_unix_ms ?? null);
        } catch (error) {
          const current = state.tabs[tab.id];
          if (!current) continue;
          external = asRuntimeError(error).code === "file_not_found";
        }
        const current = state.tabs[tab.id];
        if (current && external !== current.external) {
          dispatch({ type: "tab_external", id: tab.id, external });
        }
      }
    },
  };
}
