// Spec 010 — tokens, fonts, frame dimensions (static probes of the generated CSS and structure) and
// the luminance contrast used by the native visual-frame phase. The native flow measures the real
// window (getBoundingClientRect/getComputedStyle); these tests guard the sources and the math.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { composite, contrastRatio, firstFamily, isLargeText, parseColor, relativeLuminance } from "./contrast";

const sources = import.meta.glob(["../../App.svelte", "../../shell/Workbench.svelte", "../../terminal/TerminalView.svelte", "./*.svelte", "../*/*Region.svelte", "../sidebar/SidebarV2.svelte"], {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;
// CSS is read from disk: vitest does not return stylesheet text through `?raw`.
const css = readFileSync(new URL("../../app.css", import.meta.url), "utf8");
const workbench = sources["../../shell/Workbench.svelte"]!;
const app = sources["../../App.svelte"]!;

/** The 16 variables of design/herdr-desktop.pen, kept as CSS fallbacks when no theme is loaded. */
const GUIDE = {
  "--bg": "#0F0F0F",
  "--surface": "#0A0A0A",
  "--surface-2": "#171717",
  "--surface-3": "#242424",
  "--border": "#1C1C1C",
  "--text": "#EDEDED",
  "--text-muted": "#A3A3A3",
  "--text-dim": "#6B6B6B",
  "--accent": "#D4D4D4",
  "--accent-soft": "#D4D4D41F",
  "--working": "#4ADE80",
  "--attention": "#E9A23B",
  "--error": "#F87171",
  "--idle": "#6B6B6B",
};

function rootBlock(source: string): Record<string, string> {
  const block = /:root\s*\{([^}]*)\}/.exec(source)?.[1] ?? "";
  return Object.fromEntries(Array.from(block.matchAll(/(--[a-z0-9-]+)\s*:\s*([^;]+);/g)).map((m) => [m[1]!, m[2]!.trim()]));
}

/** Declarations of the first rule whose selector list contains `selector` exactly. */
function rule(source: string, selector: string): string {
  const style = (source.includes("<style>") ? source.slice(source.indexOf("<style>") + 7, source.indexOf("</style>")) : source).replace(/\/\*[^]*?\*\//g, "");
  for (const m of style.matchAll(/([^{}]+)\{([^}]*)\}/g)) {
    if (m[1]!.split(",").map((s) => s.trim()).includes(selector)) return m[2]!;
  }
  return "";
}

describe("design tokens (AC-010-01)", () => {
  // Would catch: a token missing, renamed (the 007 --pen-* names only), or a fallback that drifted
  // (e.g. text-dim reused as text-muted, idle #8C93A3, accent-soft as rgba approximations).
  it(":root exposes the 16 guide variables as CSS fallbacks", () => {
    const root = rootBlock(css);
    for (const [name, value] of Object.entries(GUIDE)) expect([name, root[name]?.toUpperCase()]).toEqual([name, value]);
    expect(firstFamily(root["--font-ui"] ?? "")).toBe("Inter");
    expect(firstFamily(root["--font-mono"] ?? "")).toBe("JetBrains Mono");
    expect(Object.keys(root).filter((k) => k in GUIDE || k === "--font-ui" || k === "--font-mono")).toHaveLength(16);
  });

  // Would catch: the UI font left on system-ui or the terminal on a generic monospace.
  it("the document uses the UI font token and the terminal the mono token", () => {
    expect(rule(css, ":root")).toMatch(/font-family:\s*var\(--font-ui\)/);
    expect(rule(app, ".terminal")).toMatch(/font-family:\s*var\(--font-mono\)/);
    // The canvas draws with TerminalView's font (the native phase reads the context font).
    expect(sources["../../terminal/TerminalView.svelte"]).toMatch(/fontFamily = "'JetBrains Mono',/);
  });

  // Would catch (010 r4, regression of 007 mouse-scroll-links): the terminal wrapper's mono font
  // inherited by TerminalView's absolute action toolbar (its status and `.uri { font: inherit }`),
  // which widens it over row 0 so the Ctrl+click on a link lands on the toolbar and never reaches
  // the terminal's pointerdown. The toolbar is UI text: it keeps the UI font of spec 007.
  it("the terminal's action toolbar keeps the UI font, not the terminal mono font", () => {
    expect(rule(app, ".terminal")).toMatch(/font-family:\s*var\(--font-mono\)/);
    expect(rule(app, '.terminal :global([role="toolbar"])')).toMatch(/font-family:\s*var\(--font-ui\)/);
    // TerminalView sets no font of its own on the toolbar, so App's rule is the one that applies.
    expect(rule(sources["../../terminal/TerminalView.svelte"]!, ".actions")).not.toMatch(/font(-family)?:/);
    expect(rule(sources["../../terminal/TerminalView.svelte"]!, ".uri")).toMatch(/font:\s*inherit/);
  });
});

describe("frame dimensions (AC-010-01)", () => {
  const frame = (file: string) => sources[`./${file}.svelte`] ?? "";
  // Would catch: a region drawn at another size (the 007 status bar at 11px text but 24px, a
  // 240px sidebar) or a flexible region that shrinks under the terminal.
  // Spec 047 (AC-047-01/04): the sidebar of the approved design is 288 px open and 56 px as the
  // rail — never 0 px, which is what made the collapsed window lose the whole status column.
  it("declares 48/52/288/56/256/28 px with the regions fixed", () => {
    expect(rule(frame("TitleBar"), ".topbar")).toMatch(/height:\s*48px/);
    expect(rule(frame("ActivityBar"), ".activity")).toMatch(/width:\s*52px/);
    expect(rule(frame("StatusBar"), ".status-bar")).toMatch(/height:\s*28px/);
    expect(rule(sources["./Sidebar.svelte"] ?? "", ".sidebar")).toMatch(/width:\s*288px/);
    expect(rule(sources["./Sidebar.svelte"] ?? "", ".sidebar.rail")).toMatch(/width:\s*56px/);
    expect(rule(sources["../agents/AgentsRegion.svelte"] ?? "", ".region")).toMatch(/width:\s*256px/);
    expect(rule(sources["./Sidebar.svelte"] ?? "", ".sidebar")).toMatch(/flex:\s*none/);
  });

  // Would catch: small text styled with text-dim (3.2:1 on bg) instead of text-muted.
  // Spec 064 uses --text-dim for the discrete status bar text.
  it("frame components never color text with --text-dim, except StatusBar (spec 064)", () => {
    for (const file of ["TitleBar", "ActivityBar", "CommandPalette"]) expect([file, /color:\s*var\(--text-dim\)/.test(frame(file))]).toEqual([file, false]);
    expect(/color:\s*var\(--text-dim\)/.test(frame("StatusBar"))).toBe(true);
  });
});

describe("named slots (spec 010 foundation)", () => {
  // Would catch: Workbench without one of the five regions, or App rendering a later spec's screen
  // inline (which would force 011–015 to edit App.svelte again).
  it("the five region owner components still exist in isolation after the clean casca unmounted them from App", () => {
    for (const slot of ["projects", "center"]) {
      expect([slot, new RegExp(`\\{@render ${slot}Region\\?\\.\\(\\)\\}`).test(workbench)]).toEqual([slot, true]);
      expect([slot, new RegExp(`\\{#snippet ${slot}Region\\(\\)\\}`).test(app)]).toEqual([slot, true]);
    }
    for (const [dir, name] of [["projects", "ProjectsRegion"], ["center", "CenterRegion"], ["agents", "AgentsRegion"], ["files", "FilesRegion"], ["home", "HomeRegion"]] as const) {
      expect(sources[`../${dir}/${name}.svelte`]).toBeDefined();
    }
    // Spec 041: the projects slot is filled by the new sidebar, which keeps the slot contract.
    expect(app).toContain(`import SidebarV2 from "./components/sidebar/SidebarV2.svelte"`);
    expect(app).toContain(`import CenterRegion from "./components/center/CenterRegion.svelte"`);
    expect(app).not.toMatch(/<ActivityBar\b/);
    expect(app).not.toMatch(/<AgentsRegion\b/);
    expect(app).not.toMatch(/<FilesRegion\b/);
    expect(app).not.toMatch(/<HomeRegion\b/);
    // Removed regions stay on the isolated components, not on the composed window.
    expect(sources["./ActivityBar.svelte"]).toContain(`data-region="activity"`);
    expect(sources["../agents/AgentsRegion.svelte"]).toContain(`data-region="agents"`);
    expect(sources["../files/FilesRegion.svelte"]).toContain(`data-region="files"`);
    expect(sources["../home/HomeRegion.svelte"]).toContain(`data-region="home"`);
    expect(sources["../sidebar/SidebarV2.svelte"]).toContain(`data-slot="projects"`);
    expect(sources["../center/CenterRegion.svelte"]).toContain(`data-slot="center"`);
    for (const region of ["topbar", "sidebar", "main", "status"]) expect(workbench + Object.values(sources).join("")).toContain(`data-region="${region}"`);
  });
});

describe("luminance contrast", () => {
  // Would catch: a formula without sRGB linearisation, ignoring alpha, or swapping fg/bg order.
  it("matches the guide's measured ratios", () => {
    const ratio = (fg: string, bg: string) => Math.round(contrastRatio(parseColor(fg)!, parseColor(bg)!) * 1000) / 1000;
    expect(ratio("#5B6272", "#0B0C10")).toBe(3.197);
    expect(ratio("#5B6272", "#1E222B")).toBe(2.604);
    expect(ratio("#6B7280", "#0B0C10")).toBe(4.043);
    expect(ratio("#8C93A3", "#0B0C10")).toBe(6.346);
    expect(ratio("#8C93A3", "#1E222B")).toBe(5.169);
    expect(ratio("#0B0C10", "#8C93A3")).toBe(6.346);
    expect(relativeLuminance(parseColor("#FFFFFF")!)).toBe(1);
  });

  // Would catch: computed colors (rgb()/rgba(), space syntax) unparsed, or a translucent text
  // measured as if opaque (8FA8FF at 12% over bg is nearly bg).
  it("parses computed colors and composites translucent layers", () => {
    expect(parseColor("rgb(140, 147, 163)")).toEqual({ r: 140, g: 147, b: 163, a: 1 });
    expect(parseColor("rgba(143, 168, 255, 0.12)")).toEqual({ r: 143, g: 168, b: 255, a: 0.12 });
    expect(parseColor("rgb(1 2 3 / 50%)")).toEqual({ r: 1, g: 2, b: 3, a: 0.5 });
    expect(parseColor("#8FA8FF1F")!.a).toBeCloseTo(31 / 255, 5);
    expect(parseColor("transparent")).toEqual({ r: 0, g: 0, b: 0, a: 0 });
    expect(parseColor("currentcolor")).toBeNull();
    const over = composite(parseColor("rgba(143, 168, 255, 0.12)")!, parseColor("#0B0C10")!);
    expect(over.a).toBe(1);
    expect(contrastRatio(parseColor("rgba(143, 168, 255, 0.12)")!, parseColor("#0B0C10")!)).toBeLessThan(1.6);
  });

  // Would catch: 14px/600 treated as large text (the guide's explicit correction).
  it("large text is 24px regular or 18.67px bold only", () => {
    expect(isLargeText(24, 400)).toBe(true);
    expect(isLargeText(18.67, 700)).toBe(true);
    expect(isLargeText(14, 600)).toBe(false);
    expect(isLargeText(18.67, 600)).toBe(false);
    expect(firstFamily('"JetBrains Mono", monospace')).toBe("JetBrains Mono");
    expect(firstFamily("Inter, system-ui")).toBe("Inter");
  });
});
