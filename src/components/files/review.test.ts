import { describe, expect, it } from "vitest";
import type { AgentDto } from "../../agents/types";
import { diffLines } from "../../files/diff";
import type { DiffView, TabView } from "../../files/reducer";
import {
  activeAgent,
  breadcrumb,
  dockLabel,
  relativePath,
  rootName,
  sideBySide,
  tabEntries,
  virtualWindow,
} from "./review";

function agent(fields: Partial<AgentDto>): AgentDto {
  return {
    pane_id: "w1:p1",
    workspace_id: "w1",
    tab_id: "w1:t1",
    name: null,
    kind: null,
    status: "idle",
    launch_pending: false,
    ready: true,
    focused: false,
    ...fields,
  };
}

function diff(base: string, current: string): DiffView {
  const lines = diffLines(base, current);
  return {
    base: "original",
    baseLabel: "conteúdo-base 11111111",
    currentLabel: "buffer atual",
    lines,
    added: lines.filter((line) => line.op === "added").length,
    removed: lines.filter((line) => line.op === "removed").length,
    identical: false,
  };
}

function tab(fields: Partial<TabView>): TabView {
  return {
    id: "t1",
    name: "notas.txt",
    path: "/tmp/work/notas.txt",
    uri: { provider: "local", host: null, path: "/tmp/work/notas.txt" },
    active: true,
    status: "ready",
    dirty: false,
    saving: false,
    error: null,
    conflict: null,
    closePrompt: false,
    external: false,
    notice: null,
    recovery: null,
    canSave: true,
    diff: null,
    ...fields,
  };
}

describe("sideBySide", () => {
  // Would catch: a changed line shown on the wrong side, a lost unchanged line or a run whose
  // removed/added lines were paired by position across the whole diff instead of within the block.
  it("pairs each removed run with the added run that follows it", () => {
    const view = diff(
      ["pub fn total() {", "    let mut sum = 0;", "    sum += item.price;", "}", "// fim"].join("\n"),
      ["pub fn total() {", "    let mut sum = 0;", "    sum += item.subtotal();", "}", "// fim"].join("\n"),
    );
    const rows = sideBySide(view.lines);
    expect(rows).toHaveLength(5);
    expect(rows[0]).toEqual({
      base: { op: "same", text: "pub fn total() {", line: 1 },
      current: { op: "same", text: "pub fn total() {", line: 1 },
    });
    expect(rows[2]).toEqual({
      base: { op: "removed", text: "    sum += item.price;", line: 3 },
      current: { op: "added", text: "    sum += item.subtotal();", line: 3 },
    });
    expect(rows[4]!.base!.line).toBe(5);
    expect(rows[4]!.current!.line).toBe(5);
  });

  it("keeps a deleted line without a current side and an added line without a base side", () => {
    const view = diff("um\ndois\ntres\n", "um\ntres\nnovo\n");
    const rows = sideBySide(view.lines);
    const baseOnly = rows.filter((row) => row.base && !row.current);
    const currentOnly = rows.filter((row) => !row.base && row.current);
    expect(baseOnly.map((row) => row.base!.text)).toEqual(["dois"]);
    expect(currentOnly.map((row) => row.current!.text)).toEqual(["novo"]);
    expect(rows[0]!.base!.line).toBe(1);
    expect(rows[1]).toEqual({ base: { op: "removed", text: "dois", line: 2 }, current: null });
    expect(rows[2]!.base!.text).toBe("tres");
    expect(rows[3]).toEqual({ base: null, current: { op: "added", text: "novo", line: 3 } });
  });

  it("adds no row for an empty diff", () => {
    expect(sideBySide(diffLines("um\ndois\n", "um\ndois\n"))).toHaveLength(2);
  });
});

describe("breadcrumb", () => {
  // Would catch: the root name lost, a duplicated segment or an absolute path leaking into the
  // review's `projeto / caminho / arquivo`.
  it("starts at the project root's name and lists the file's segments", () => {
    expect(breadcrumb("/var/tmp/work", "/var/tmp/work/src/billing/invoice.rs")).toEqual([
      "work",
      "src",
      "billing",
      "invoice.rs",
    ]);
    expect(breadcrumb("/srv/projeto/", "/srv/projeto/notas.txt")).toEqual(["projeto", "notas.txt"]);
  });

  it("falls back to the path without slashes when it is outside the root", () => {
    expect(breadcrumb("/srv/projeto", "/outro/lugar/nota.txt")).toEqual(["projeto", "outro", "lugar", "nota.txt"]);
    expect(rootName("/srv/projeto/")).toBe("projeto");
    expect(relativePath(null, "/a/b.txt")).toBe("a/b.txt");
  });
});

describe("virtualWindow", () => {
  // Would catch: a window that drops visible rows, negative spans or a slice beyond the total.
  it("keeps the visible slice with overscan inside the row count", () => {
    const window = virtualWindow(6000, 18, 18 * 3000, 190, 8);
    expect(window.start).toBe(2992);
    expect(window.end).toBe(3019);
    expect(window.top).toBe(2992 * 18);
    expect(window.bottom).toBe((6000 - window.end) * 18);
  });

  it("shows the whole short list and clamps the end of a long one", () => {
    expect(virtualWindow(10, 18, 0, 190)).toEqual({ start: 0, end: 10, top: 0, bottom: 0 });
    const last = virtualWindow(6000, 18, 18 * 5999, 190, 8);
    expect(last.end).toBe(6000);
    expect(last.bottom).toBe(0);
    expect(virtualWindow(0, 18, 0, 190)).toEqual({ start: 0, end: 0, top: 0, bottom: 0 });
  });
});

describe("tabEntries", () => {
  // Would catch: the diff tab missing, out of order, or both entries of a file active at once.
  it("lists file then `arquivo · diff` with a single active entry", () => {
    const entries = tabEntries([tab({ diff: diff("um\n", "dois\n") })], "diff");
    expect(entries.map((entry) => [entry.kind, entry.label, entry.active])).toEqual([
      ["file", "notas.txt", false],
      ["diff", "notas.txt · diff", true],
    ]);
    const file = tabEntries([tab({ diff: diff("um\n", "dois\n") })], "file");
    expect(file.map((entry) => entry.active)).toEqual([true, false]);
  });

  it("lists only the file tab when no diff is open", () => {
    const entries = tabEntries([tab({}), tab({ id: "t2", name: "outro.txt", path: "/tmp/work/outro.txt", active: false })], "file");
    expect(entries.map((entry) => entry.label)).toEqual(["notas.txt", "outro.txt"]);
  });
});

describe("dockLabel", () => {
  // Would catch: a dock that names an inactive agent or hides the project.
  it("names the focused agent, else the first by pane id, with the project", () => {
    const idle = agent({ pane_id: "w2:p1", kind: "pi" });
    const focused = agent({ pane_id: "w1:p1", kind: "claude", name: "Claude", focused: true });
    expect(activeAgent([focused, idle])?.pane_id).toBe("w1:p1");
    expect(activeAgent([idle])?.kind).toBe("pi");
    expect(activeAgent([])).toBeNull();
    expect(dockLabel(focused, "erp-api")).toBe("TERMINAL · Claude / erp-api");
    expect(dockLabel(idle, "erp-api")).toBe("TERMINAL · pi / erp-api");
    expect(dockLabel(null, null)).toBe("TERMINAL · sem agente / sem projeto");
  });
});
