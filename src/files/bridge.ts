// IPC wrapper for the files module. Command names mirror `files_local::COMMANDS` (checked by
// src-tauri/tests/files_local.rs). The commands are registered in the window by spec 007; the
// isolated preview and the controller tests use the fake bridge.

import { invoke } from "@tauri-apps/api/core";
import type {
  FilePageDto,
  FileStatDto,
  FileUriDto,
  RecoveryCopyDto,
  SaveOutcomeDto,
  TextSnapshotDto,
} from "./types";

export interface FilesBridge {
  /** One page (≤128 entries) starting after `cursor`; listing is never recursive. */
  list(uri: FileUriDto, cursor: string | null): Promise<FilePageDto>;
  read(uri: FileUriDto): Promise<TextSnapshotDto>;
  stat(uri: FileUriDto): Promise<FileStatDto>;
  save(snapshotId: string, content: string): Promise<SaveOutcomeDto>;
  saveRecovery(snapshotId: string, content: string): Promise<RecoveryCopyDto>;
  release(snapshotIds: string[]): Promise<void>;
}

export function tauriFilesBridge(): FilesBridge {
  return {
    list: (uri, cursor) => invoke<FilePageDto>("files_list", { uri, cursor }),
    read: (uri) => invoke<TextSnapshotDto>("files_read", { uri }),
    stat: (uri) => invoke<FileStatDto>("files_stat", { uri }),
    save: (snapshotId, content) => invoke<SaveOutcomeDto>("files_save", { snapshotId, content }),
    saveRecovery: (snapshotId, content) =>
      invoke<RecoveryCopyDto>("files_save_recovery", { snapshotId, content }),
    release: (snapshotIds) => invoke<void>("files_release", { snapshotIds }),
  };
}
