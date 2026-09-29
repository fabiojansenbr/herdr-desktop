// Display grid fed by FrameEvents. Keeps the last committed cells, tracks dirty rows so
// the renderer repaints incrementally, and refuses patches that do not chain on the
// current revision (the Rust FrameStore is the authority; this is defence in depth).

import type { CellDto, CursorDto, FrameEvent, PaneDto } from "./types";

export interface GridSnapshot {
  width: number;
  height: number;
  revision: number;
  cells: CellDto[];
  cursor: CursorDto | null;
  panes: PaneDto[];
}

export type ApplyResult =
  | { applied: true; dirtyRows: number[]; full: boolean }
  | { applied: false; reason: string };

const BLANK: CellDto = { s: " ", fg: 0, bg: 0, m: 0 };

export class TerminalGrid {
  width = 0;
  height = 0;
  revision = 0;
  cells: CellDto[] = [];
  cursor: CursorDto | null = null;
  panes: PaneDto[] = [];
  private previousCursorRow: number | null = null;

  get hasSurface(): boolean {
    return this.width > 0 && this.height > 0;
  }

  cellAt(x: number, y: number): CellDto {
    if (x < 0 || y < 0 || x >= this.width || y >= this.height) return BLANK;
    return this.cells[y * this.width + x] ?? BLANK;
  }

  rowText(y: number): string {
    let out = "";
    for (let x = 0; x < this.width; x++) out += this.cellAt(x, y).s;
    return out;
  }

  textRows(): string[] {
    const rows: string[] = [];
    for (let y = 0; y < this.height; y++) rows.push(this.rowText(y));
    return rows;
  }

  apply(event: FrameEvent): ApplyResult {
    if (event.type === "full") {
      if (event.cells.length !== event.width * event.height) {
        return { applied: false, reason: "inconsistent_full_frame" };
      }
      this.width = event.width;
      this.height = event.height;
      this.revision = event.revision;
      this.cells = event.cells.slice();
      this.cursor = event.cursor;
      this.panes = event.panes;
      this.previousCursorRow = event.cursor?.y ?? null;
      const dirtyRows = Array.from({ length: this.height }, (_, i) => i);
      return { applied: true, dirtyRows, full: true };
    }
    if (event.type === "patch") {
      if (!this.hasSurface) return { applied: false, reason: "no_base_surface" };
      if (event.revision !== this.revision + 1) {
        return { applied: false, reason: `revision_gap:${this.revision}->${event.revision}` };
      }
      for (const row of event.rows) {
        if (row.y >= this.height || row.x + row.cells.length > this.width) {
          return { applied: false, reason: "row_out_of_bounds" };
        }
      }
      const dirty = new Set<number>();
      for (const row of event.rows) {
        const base = row.y * this.width + row.x;
        for (let i = 0; i < row.cells.length; i++) {
          this.cells[base + i] = row.cells[i]!;
        }
        dirty.add(row.y);
      }
      if (this.previousCursorRow !== null) dirty.add(this.previousCursorRow);
      if (event.cursor) dirty.add(event.cursor.y);
      this.cursor = event.cursor;
      this.previousCursorRow = event.cursor?.y ?? null;
      this.revision = event.revision;
      return { applied: true, dirtyRows: [...dirty].sort((a, b) => a - b), full: false };
    }
    return { applied: false, reason: "not_a_frame_event" };
  }

  snapshot(): GridSnapshot {
    return {
      width: this.width,
      height: this.height,
      revision: this.revision,
      cells: this.cells,
      cursor: this.cursor,
      panes: this.panes,
    };
  }
}
