// Spec 010 (AC-010-03) — command palette model: sections fed by engine/app state, empty state, filter
// and focus return on close.
import { describe, expect, it } from "vitest";
import { createPaletteFocus, paletteSections, type PaletteSource } from "./palette";

function source(over: Partial<PaletteSource> = {}): { src: PaletteSource; calls: string[] } {
  const calls: string[] = [];
  return {
    calls,
    src: {
      hasSession: true,
      projects: [
        { id: "p1", label: "erp-api", root: "/work/erp-api", host: "Este computador" },
        { id: "p2", label: "Portal Web", root: "/work/portal", host: "dev-box" },
      ],
      panes: [
        { pane_id: "w1:p1", focused: true, width: 80, height: 24 },
        { pane_id: "w1:p2", focused: false, width: 40, height: 24 },
      ],
      agents: [{ pane_id: "w1:p2", name: "claude", kind: "claude", status: "working" }],
      commands: [
        { id: "split", label: "Dividir pane", shortcut: null, run: () => calls.push("split") },
        { id: "search", label: "Buscar…", shortcut: "Ctrl K", run: () => calls.push("search") },
      ],
      openProject: (id) => calls.push(`project:${id}`),
      focusPane: (id) => calls.push(`pane:${id}`),
      openAgent: (id) => calls.push(`agent:${id}`),
      ...over,
    },
  };
}

describe("palette sections", () => {
  // Would catch: sections reordered or renamed, an engine list dropped, or a pane/agent entry acting
  // on the wrong target.
  it("lists Projetos, Panes, Agentes and Comandos from the sources, each entry acting on its own target", () => {
    const { src, calls } = source();
    const sections = paletteSections(src, "");
    expect(sections.map((s) => s.title)).toEqual(["Projetos", "Panes", "Agentes", "Comandos"]);
    expect(sections.map((s) => s.entries.map((e) => e.id))).toEqual([
      ["project:p1", "project:p2"],
      ["pane:w1:p1", "pane:w1:p2"],
      ["agent:w1:p2"],
      // Spec 046: "Novo workspace" is a command of the palette itself, before the menu ones.
      ["command:new-workspace", "command:split"],
    ]);
    for (const section of sections) for (const entry of section.entries) entry.run();
    expect(calls).toEqual(["project:p1", "project:p2", "pane:w1:p1", "pane:w1:p2", "agent:w1:p2", "split"]);
    expect(sections[2]!.entries[0]!.detail).toContain("Trabalhando");
  });

  // Would catch: the palette listing itself as a command (a loop) — the search entry is excluded
  // — or the 046 "Novo workspace" command disappearing from the list.
  it("does not list the palette as a command", () => {
    expect(paletteSections(source().src, "").at(-1)!.entries.map((e) => e.label)).toEqual(["Novo workspace", "Dividir pane"]);
  });

  // Would catch: stale projects/panes/agents shown without a session (edge case: only Comandos).
  it("without a session lists only Comandos", () => {
    const sections = paletteSections(source({ hasSession: false }).src, "");
    expect(sections.map((s) => s.title)).toEqual(["Comandos"]);
  });

  // Would catch: a case- or accent-sensitive filter, or a filter that hides the section headers'
  // order instead of their entries.
  it("filters entries case- and accent-insensitively keeping section order", () => {
    const sections = paletteSections(source().src, "PORTAL");
    expect(sections.map((s) => [s.title, s.entries.map((e) => e.id)])).toEqual([
      ["Projetos", ["project:p2"]],
      ["Panes", []],
      ["Agentes", []],
      ["Comandos", []],
    ]);
    expect(paletteSections(source().src, "divídir").at(-1)!.entries.map((e) => e.id)).toEqual(["command:split"]);
  });
});

describe("palette focus", () => {
  function el(name: string, log: string[]) {
    return { name, isConnected: true, focus: () => log.push(name) };
  }

  // Would catch: focus left on body after Escape, or returned to an element removed meanwhile.
  it("returns the focus to the element focused before opening", () => {
    const log: string[] = [];
    const opener = el("host", log);
    let active: typeof opener | null = opener;
    const focus = createPaletteFocus(() => active);
    focus.open();
    active = el("input", log);
    expect(focus.close()).toBe(opener);
    expect(log).toEqual(["host"]);

    const gone = { ...el("gone", log), isConnected: false };
    active = gone;
    focus.open();
    log.length = 0;
    expect(focus.close()).toBeNull();
    expect(log).toEqual([]);
    expect(focus.close()).toBeNull();
  });
});
