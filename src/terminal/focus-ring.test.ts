// AC-030-03: terminal focus never paints a frame around the terminal or pane. The split divider is
// the focus affordance; this contract prevents the old CSS ring from returning.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
describe("terminal focus presentation", () => {
  it("does not draw an outline or box-shadow for focus on the terminal frame", () => {
    const view = readFileSync(new URL("./TerminalView.svelte", import.meta.url), "utf8");
    const style = view.slice(view.indexOf("<style>"));
    expect(style).not.toMatch(/\.terminal:focus-within/);
    expect(style).not.toMatch(/\.terminal:global\(\.focus-visible\)/);
    const terminal = /\.terminal\s*\{([^}]*)\}/s.exec(style)?.[1] ?? "";
    expect(terminal.replace(/outline\s*:\s*none\s*;/g, "")).not.toMatch(/outline\s*:/);
    expect(style).toContain(".ime-target");
    const ime = /\.ime-target\s*\{([^}]*)\}/s.exec(style)?.[1] ?? "";
    expect(ime).toMatch(/outline:\s*none;/);
  });
});
