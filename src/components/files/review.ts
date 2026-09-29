// Review screen of spec 015: pure projections of the files state into the design's shapes —
// breadcrumb segments, the paired side-by-side rows of a line diff, the windowed slice of a very
// long diff, the file/diff tab entries and the terminal dock label. No editor and no DOM import:
// the review renders (and is tested) without loading CodeMirror.

import { t } from "../../i18n/index.svelte";
import type { AgentDto } from "../../agents/types";
import type { DiffLine } from "../../files/diff";

export type ReviewSelection = "file" | "diff";

export interface SideCell {
  op: "same" | "removed" | "added";
  text: string;
  /** 1-based line of this side's snapshot. */
  line: number;
}

export interface SideRow {
  base: SideCell | null;
  current: SideCell | null;
}

/**
 * Pairs a flat line diff into side-by-side rows: unchanged lines on both sides; each run of
 * removed lines is paired index-wise with the run of added lines that follows it, so a changed
 * line shows its old text on the base side and its new text on the current side of the same row.
 */
export function sideBySide(lines: DiffLine[]): SideRow[] {
  const rows: SideRow[] = [];
  let index = 0;
  while (index < lines.length) {
    const line = lines[index]!;
    if (line.op === "same") {
      const cell: SideCell = { op: "same", text: line.text, line: line.baseLine ?? 0 };
      rows.push({ base: cell, current: { op: "same", text: line.text, line: line.currentLine ?? 0 } });
      index++;
      continue;
    }
    const removed: SideCell[] = [];
    const added: SideCell[] = [];
    while (index < lines.length && lines[index]!.op === "removed") {
      const next = lines[index]!;
      removed.push({ op: "removed", text: next.text, line: next.baseLine ?? 0 });
      index++;
    }
    while (index < lines.length && lines[index]!.op === "added") {
      const next = lines[index]!;
      added.push({ op: "added", text: next.text, line: next.currentLine ?? 0 });
      index++;
    }
    const count = Math.max(removed.length, added.length);
    for (let slot = 0; slot < count; slot++) {
      rows.push({ base: removed[slot] ?? null, current: added[slot] ?? null });
    }
  }
  return rows;
}

export interface DiffWindow {
  start: number;
  end: number;
  /** Pixels of the rows before/after the window (spacers of a virtual list). */
  top: number;
  bottom: number;
}

/** Above this many rows the diff is windowed (fixed row height, overscan rows kept on each side). */
export const VIRTUAL_ROW_LIMIT = 5000;
export const VIRTUAL_OVERSCAN = 8;

export function virtualWindow(
  total: number,
  rowHeight: number,
  scrollTop: number,
  viewport: number,
  overscan = VIRTUAL_OVERSCAN,
): DiffWindow {
  if (total <= 0 || rowHeight <= 0) return { start: 0, end: Math.max(0, total), top: 0, bottom: 0 };
  const first = Math.max(0, Math.floor(scrollTop / rowHeight) - overscan);
  const visible = Math.ceil(viewport / rowHeight) + overscan * 2;
  const start = Math.min(first, Math.max(0, total - visible));
  const end = Math.min(total, start + visible);
  return { start, end, top: start * rowHeight, bottom: (total - end) * rowHeight };
}

/** Trailing slashes removed; the root's own name for the breadcrumb's project segment. */
export function rootName(root: string | null): string {
  if (!root) return "";
  const trimmed = root.replace(/\/+$/, "");
  return trimmed.slice(trimmed.lastIndexOf("/") + 1);
}

/** Path relative to `root` when it is inside it; otherwise the path without leading slashes. */
export function relativePath(root: string | null, path: string): string {
  if (root) {
    const base = root.replace(/\/+$/, "");
    if (path === base) return "";
    if (path.startsWith(`${base}/`)) return path.slice(base.length + 1);
  }
  return path.replace(/^\/+/, "");
}

/** `projeto / caminho / arquivo`: the project root's name then the file's path segments. */
export function breadcrumb(root: string | null, path: string): string[] {
  const parts: string[] = [];
  const name = rootName(root);
  if (name) parts.push(name);
  for (const segment of relativePath(root, path).split("/")) {
    if (segment) parts.push(segment);
  }
  return parts;
}

/** Shape the review tabs need: the local and remote view models both provide it. */
export interface ReviewTab {
  id: string;
  name: string;
  path: string;
  active: boolean;
  diff: unknown | null;
}

export interface TabEntry {
  id: string;
  kind: ReviewSelection;
  label: string;
  path: string;
  active: boolean;
}

/**
 * Tabs of the review screen, in order: each open file's tab and, when its diff is open, the
 * `arquivo · diff` tab right after it. `selection` tells which view of the active file is shown.
 */
export function tabEntries(tabs: readonly ReviewTab[], selection: ReviewSelection): TabEntry[] {
  const entries: TabEntry[] = [];
  for (const tab of tabs) {
    const diffing = tab.active && tab.diff !== null && selection === "diff";
    entries.push({
      id: tab.id,
      kind: "file",
      label: tab.name,
      path: tab.path,
      active: tab.active && !diffing,
    });
    if (tab.diff) {
      entries.push({
        id: tab.id,
        kind: "diff",
        label: `${tab.name} · diff`,
        path: tab.path,
        active: diffing,
      });
    }
  }
  return entries;
}

/** Agent the review dock speaks for: the one focused in the session, else the first by pane id. */
export function activeAgent(agents: readonly AgentDto[]): AgentDto | null {
  const focused = agents.find((agent) => agent.focused);
  if (focused) return focused;
  const sorted = [...agents].sort((a, b) => a.pane_id.localeCompare(b.pane_id));
  return sorted[0] ?? null;
}

export function agentLabel(agent: AgentDto | null): string {
  if (!agent) return t("files.review.noAgent");
  return agent.name?.trim() || agent.kind?.trim() || agent.pane_id;
}

/** `TERMINAL · <agente> / <projeto>` of the dock header. */
export function dockLabel(agent: AgentDto | null, project: string | null): string {
  return `TERMINAL · ${agentLabel(agent)} / ${project?.trim() || t("files.review.noProject")}`;
}
