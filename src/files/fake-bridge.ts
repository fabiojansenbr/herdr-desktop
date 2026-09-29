// In-memory FilesBridge for the isolated preview and the controller tests. It records every
// command with the exact argument object the Tauri bridge would send and mimics the backend
// rules that the frontend depends on (opaque snapshot ids, content conflict, recovery copy).
// It is not the provider: limits/encoding/symlink rules live in src-tauri/src/files/local.rs.

import type { FilesBridge } from "./bridge";
import {
  localUri,
  type FileEntryDto,
  type FileKind,
  type FilePageDto,
  type FileStatDto,
  type FileUriDto,
  type RecoveryCopyDto,
  type RuntimeError,
  type SaveOutcomeDto,
  type TextSnapshotDto,
} from "./types";

export interface RecordedCall {
  command: string;
  args: Record<string, unknown>;
}

export interface FakeFilesBridge extends FilesBridge {
  calls: RecordedCall[];
  files: Map<string, string>;
  /** Simulates another process writing the file. */
  writeExternal(path: string, content: string): void;
  failRead(path: string, error: RuntimeError): void;
}

const PAGE_SIZE = 128;

export function createFakeFilesBridge(
  options: { root?: string; files?: Record<string, string> } = {},
): FakeFilesBridge {
  const root = options.root ?? "/projeto";
  const files = new Map<string, string>(
    Object.entries(
      options.files ?? {
        [`${root}/notas.txt`]: "linha um\nlinha dois\n",
        [`${root}/outro.txt`]: "outro arquivo\n",
        [`${root}/sub/um.txt`]: "dentro do subdiretorio\n",
      },
    ),
  );
  const snapshots = new Map<string, { path: string; content: string }>();
  const failures = new Map<string, RuntimeError>();
  const modified = new Map<string, number>();
  const calls: RecordedCall[] = [];
  let nextSnapshot = 1;
  let nextRecovery = 1;
  let clock = 1;

  const record = (command: string, args: Record<string, unknown> = {}) => {
    calls.push({ command, args });
  };

  const fail = (code: string, message: string): never => {
    throw { code, message, retryable: false } satisfies RuntimeError;
  };

  const kindOf = (path: string): FileKind => {
    for (const candidate of files.keys()) {
      if (candidate.startsWith(`${path}/`)) return "Directory";
    }
    return "File";
  };

  const mtimeOf = (path: string): number => modified.get(path) ?? 1;

  const snapshot = (path: string, content: string): TextSnapshotDto => {
    const id = `snap-${nextSnapshot++}`;
    snapshots.set(id, { path, content });
    return {
      id,
      uri: localUri(path),
      content,
      bom: false,
      eol: "lf",
      size: content.length,
      modified_unix_ms: mtimeOf(path),
    };
  };

  const readFile = (path: string): string => {
    const failure = failures.get(path);
    if (failure) throw failure;
    const content = files.get(path);
    if (content === undefined) {
      if (kindOf(path) === "Directory") return fail("not_a_file", "o caminho não é um arquivo");
      return fail("file_not_found", "recurso não encontrado");
    }
    return content;
  };

  const writeFile = (path: string, content: string) => {
    files.set(path, content);
    modified.set(path, ++clock);
  };

  return {
    calls,
    files,
    writeExternal(path, content) {
      writeFile(path, content);
    },
    failRead(path, error) {
      failures.set(path, error);
    },
    async list(uri: FileUriDto, cursor: string | null): Promise<FilePageDto> {
      record("files_list", { uri, cursor });
      const prefix = `${uri.path}/`;
      const entries: FileEntryDto[] = [...files.keys()]
        .filter((path) => path.startsWith(prefix) && !path.slice(prefix.length).includes("/"))
        .sort()
        .map((path) => ({
          uri: localUri(path),
          name: path.slice(prefix.length),
          kind: kindOf(path),
        }));
      const offset = cursor ? Number(cursor) : 0;
      const page = entries.slice(offset, offset + PAGE_SIZE);
      return {
        uri,
        entries: page,
        next_cursor: offset + PAGE_SIZE < entries.length ? String(offset + PAGE_SIZE) : null,
      };
    },
    async read(uri: FileUriDto): Promise<TextSnapshotDto> {
      record("files_read", { uri });
      return snapshot(uri.path, readFile(uri.path));
    },
    async stat(uri: FileUriDto): Promise<FileStatDto> {
      record("files_stat", { uri });
      const failure = failures.get(uri.path);
      if (failure) throw failure;
      const kind = kindOf(uri.path);
      if (kind === "Directory") {
        if (![...files.keys()].some((path) => path.startsWith(`${uri.path}/`))) {
          fail("file_not_found", "recurso não encontrado");
        }
        return { uri, kind, size: 0, modified_unix_ms: mtimeOf(uri.path), read_only: false };
      }
      const content = readFile(uri.path);
      return {
        uri,
        kind,
        size: content.length,
        modified_unix_ms: mtimeOf(uri.path),
        read_only: false,
      };
    },
    async save(snapshotId: string, content: string): Promise<SaveOutcomeDto> {
      record("files_save", { snapshotId, content });
      const base = snapshots.get(snapshotId) ?? fail("snapshot_unknown", "leitura de origem indisponível");
      const disk = files.get(base.path);
      if (disk === undefined) {
        return { outcome: "conflict", current: null, message: "o arquivo não existe mais no disco" };
      }
      if (disk !== base.content) {
        return {
          outcome: "conflict",
          current: snapshot(base.path, disk),
          message: "o arquivo mudou no disco depois da leitura; recarregue, compare ou salve uma cópia",
        };
      }
      writeFile(base.path, content);
      return { outcome: "saved", snapshot: snapshot(base.path, content) };
    },
    async saveRecovery(snapshotId: string, content: string): Promise<RecoveryCopyDto> {
      record("files_save_recovery", { snapshotId, content });
      const base = snapshots.get(snapshotId) ?? fail("snapshot_unknown", "leitura de origem indisponível");
      const path = `${base.path}.herdr-recuperacao-${nextRecovery++}`;
      writeFile(path, content);
      return { path, uri: localUri(path), bytes: content.length };
    },
    async release(snapshotIds: string[]): Promise<void> {
      record("files_release", { snapshotIds });
      for (const id of snapshotIds) snapshots.delete(id);
    },
  };
}
