// Spec 007 — static source probes of the composed window (App.svelte). They only guard wiring
// that a unit test cannot see; the real proof (native window, frames, modules loaded) is the
// composed E2E flow, which this file does not replace.
import { describe, expect, it } from "vitest";
import { tauriSurfaceBridge } from "./bridge";

const sources = import.meta.glob(["../App.svelte", "./*.svelte"], {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;
const app = sources["../App.svelte"]!;
const markup = app.slice(app.indexOf("</script>"));

describe("surface bridge host focus (spec 007 r10)", () => {
  // Would catch: the page sending a window label or any argument (the backend focuses only the
  // invoking window), or another command name.
  it("focusHost invokes surface_focus_host without arguments", async () => {
    const calls: unknown[][] = [];
    const bridge = tauriSurfaceBridge({ invoke: async <T,>(...args: [string, Record<string, unknown>?]) => (calls.push(args), undefined as T), channel: () => null });
    await bridge.focusHost();
    expect(calls).toEqual([["surface_focus_host"]]);
  });
});

describe("composed window wiring", () => {
  // Would catch: a second terminal (fake HTML surface or another canvas) mounted next to the
  // real one, or the AgentPanel drawing its standalone keyboard surface in the window.
  it("mounts exactly one TerminalView and does not mount the standalone AgentPanel", () => {
    expect(markup.match(/<TerminalView\b/g)).toHaveLength(1);
    // Spec 018 unmounted AgentsRegion from the window; the designed panel stays in the repo
    // (src/components/agents/AgentsRegion.svelte) and is tested in isolation (014 wiring.test.ts).
    expect(markup).not.toMatch(/<AgentsRegion\b/);
    expect(markup).not.toMatch(/<AgentPanel\b/);
  });

  // Would catch: the window intercepting terminal keys (Ctrl+C, Tab, Escape) globally.
  it("installs no global keyboard listener", () => {
    for (const [file, source] of Object.entries(sources)) {
      expect([file, /<svelte:(window|document|body)[^>]*onkey/.test(source)]).toEqual([file, false]);
      expect([file, /(window|document)\.addEventListener\(\s*["']key/.test(source)]).toEqual([file, false]);
    }
  });

  // Would catch: the window using the pre-007 Local-only terminal commands instead of the
  // selected-host surface, or creating a session during mount.
  it("drives the terminal through the selection surface and starts sessions only on demand", () => {
    expect(app).toMatch(/tauriSurfaceBridge\(\)/);
    expect(app).not.toMatch(/tauriBridge\(\)/);
    expect(app).not.toMatch(/onMount\([^]*?startSession\(\)[^]*?\}\);/);
  });

  // Would catch: agents attached when a selection is merely requested or when attach returns
  // (possibly still connecting), instead of on the confirmed live connection; and the surface
  // attaching before the remounted view subscribed.
  it("attaches agents only from onLive and orders surface attach after a render flush", () => {
    expect(app.match(/agents\.connect\(/g)).toHaveLength(1);
    expect(app).toMatch(/onLive:\s*\(\)\s*=>\s*\{\s*agents\.invalidate\(\);\s*void agents\.connect\(/);
    expect(app).toMatch(/onSelectionStart:\s*\(\)\s*=>\s*\{[^}]*agents\.invalidate\(\)/);
    expect(app).toMatch(/ready:\s*\(\)\s*=>\s*tick\(\)/);
  });

  // Would catch: Ctrl+Shift+V left to the browser (a fabricated paste instead of the Rust read)
  // or wired to anything other than the surface controller's native paste; the report of what
  // was pasted (spec 028) must come from the same call.
  it("wires the native paste gesture to the surface controller", () => {
    expect(app).toMatch(/onNativePaste=\{nativePasteWithReport\}/);
    expect(app).toMatch(/async function nativePasteWithReport\(\) \{\s*await surfaceController\.nativePaste\(\);\s*return surfaceController\.state\.paste;/);
  });

  // Spec 028 AC-028-01. Would catch: Ctrl+T claimed but opening the palette, splitting a pane or
  // creating the tab through another action.
  it("wires Ctrl+T to one tab.create like the + button", () => {
    expect(app).toMatch(/if \(id === "new-tab"\) \{\s*\/\/[^]*?event\.preventDefault\(\);\s*void agents\.createTab\(\);/);
  });

  // Would catch: controllers created inside the sidebar/agents snippets, which the Workbench
  // unmounts when a panel collapses.
  it("keeps feature controllers outside collapsible snippets", () => {
    expect(markup).not.toMatch(/create(Projects|Agents|Files|RemoteFiles|Connections)Controller\(/);
    expect(app).toMatch(/createProjectsController\(/);
    expect(app).toMatch(/createAgentsController\(/);
    expect(app).toMatch(/createConnectionsController\(/);
    // Spec 041: the projects slot is the new sidebar; it still receives the frame context here.
    expect(markup).toMatch(/<SidebarV2\b[^>]*\bctx=/);
    // Creating a per-target controller writes state: never from a template expression.
    expect(markup).not.toMatch(/localFiles\.get\(/);
  });
});
