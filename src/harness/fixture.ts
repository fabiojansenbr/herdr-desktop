// Converts tests/fixtures/surface-cells-v1.json into FrameEvents (shared by the harness
// and the vitest suites so both sides of the seam read the same expectations).

import raw from "../../tests/fixtures/surface-cells-v1.json";
import type { CellDto, CursorDto, FrameEvent } from "../terminal/types";

interface FixtureCell {
  s: string;
  fg?: number;
  bg?: number;
  m?: number;
}

interface FixturePatch {
  projection_revision: number;
  base_surface_revision: number;
  surface_revision: number;
  cursor: CursorDto | null;
  rows: { x: number; y: number; cells: FixtureCell[] }[];
  boot_id?: string;
}

interface Fixture {
  boot_id: string;
  width: number;
  height: number;
  full: {
    surface_revision: number;
    cursor: CursorDto;
    pane: { pane_id: string; inner_rect: number[] };
    rows: FixtureCell[][];
  };
  patches: FixturePatch[];
  stale_patch: FixturePatch;
  foreign_boot_patch: FixturePatch;
  expected_after_patches: {
    surface_revision: number;
    cursor: CursorDto;
    text_rows: string[];
    cells: Record<string, FixtureCell>;
  };
}

export const fixture = raw as unknown as Fixture;

function cell(c: FixtureCell): CellDto {
  return { s: c.s, fg: c.fg ?? 0, bg: c.bg ?? 0, m: c.m ?? 0 };
}

function patchEvent(p: FixturePatch): FrameEvent {
  return {
    type: "patch",
    revision: p.surface_revision,
    rows: p.rows.map((r) => ({ x: r.x, y: r.y, cells: r.cells.map(cell) })),
    cursor: p.cursor,
  };
}

export function fixtureEvents(): { full: FrameEvent; patches: FrameEvent[]; stale: FrameEvent } {
  const inner = fixture.full.pane.inner_rect;
  const full: FrameEvent = {
    type: "full",
    revision: fixture.full.surface_revision,
    width: fixture.width,
    height: fixture.height,
    cells: fixture.full.rows.flat().map(cell),
    cursor: fixture.full.cursor,
    panes: [
      {
        pane_id: fixture.full.pane.pane_id,
        x: inner[0] ?? 0,
        y: inner[1] ?? 0,
        width: inner[2] ?? fixture.width,
        height: inner[3] ?? fixture.height,
        focused: true,
      },
    ],
  };
  return {
    full,
    patches: fixture.patches.map(patchEvent),
    stale: patchEvent(fixture.stale_patch),
  };
}
