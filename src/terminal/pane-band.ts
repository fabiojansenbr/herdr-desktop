// Spec 013 — the band of a pane: the strip of the surface right above its `inner_rect`, used by the
// pane frames of the center region and by the terminal's own action toolbar. One definition, so the
// frame header and the toolbar never disagree and neither of them ever covers a content cell (the
// toolbar over row 0 was the debt the 010 r4 found).

import type { RectDto } from "./types";

export interface CellSize {
  cellWidth: number;
  cellHeight: number;
}
export interface BandBox {
  left: number;
  top: number;
  width: number;
  height: number;
}

/** Rows of the surface a band occupies (the engine's border row, or the strip the region reserves
 * above the canvas for the panes that start at row 0). */
export const HEADER_ROWS = 1;

export function headerHeightPx(cellHeight: number): number {
  return cellHeight * HEADER_ROWS;
}

/** Band of `inner`, in CSS px relative to the canvas origin (negative top for a pane at row 0). */
export function bandOffset(inner: RectDto, cell: CellSize): BandBox {
  const height = headerHeightPx(cell.cellHeight);
  return {
    left: inner.x * cell.cellWidth,
    top: inner.y * cell.cellHeight - height,
    width: inner.width * cell.cellWidth,
    height,
  };
}
