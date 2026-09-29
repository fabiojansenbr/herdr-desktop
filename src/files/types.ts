// DTOs shared with src-tauri/src/files/local.rs (FilePage / TextSnapshot / SaveOutcome /
// RecoveryCopy). Spec 006 reuses these shapes for the remote provider; the frontend never
// re-implements detection: size/NUL/UTF-8 rules and limits live in the backend.

import type { RuntimeError } from "../terminal/types";

export type { RuntimeError };

/** Identity of the explorer target: provider + host + authorized root. */
export interface FileTarget {
  provider: string;
  /** null for the local host. A remote host never falls back to the local filesystem. */
  host: string | null;
  root: string;
}

export interface FileUriDto {
  provider: string;
  host?: string | null;
  path: string;
}

export type FileKind = "File" | "Directory" | "Symlink" | "Other";

export interface FileEntryDto {
  uri: FileUriDto;
  name: string;
  kind: FileKind;
}

export interface FilePageDto {
  uri: FileUriDto;
  entries: FileEntryDto[];
  /** Opaque continuation token; null on the last page. */
  next_cursor: string | null;
}

export interface FileStatDto {
  uri: FileUriDto;
  kind: FileKind;
  size: number;
  modified_unix_ms: number | null;
  read_only: boolean;
}

export type LineEnding = "lf" | "crlf";

export interface TextSnapshotDto {
  id: string;
  uri: FileUriDto;
  /** Normalised to \n; the backend re-applies BOM/CRLF when saving. */
  content: string;
  bom: boolean;
  eol: LineEnding;
  size: number;
  modified_unix_ms: number | null;
}

export type SaveOutcomeDto =
  | { outcome: "saved"; snapshot: TextSnapshotDto }
  | { outcome: "conflict"; current: TextSnapshotDto | null; message: string };

export interface RecoveryCopyDto {
  path: string;
  uri: FileUriDto;
  bytes: number;
}

export function localUri(path: string): FileUriDto {
  return { provider: "local", host: null, path };
}

export function uriKey(uri: FileUriDto): string {
  return `${uri.provider}|${uri.host ?? ""}|${uri.path}`;
}

export function targetKey(target: FileTarget): string {
  return `${target.provider}|${target.host ?? ""}|${target.root}`;
}
