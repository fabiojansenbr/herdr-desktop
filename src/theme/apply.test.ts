// @vitest-environment happy-dom
// Specs 020/061 — neutral UI and terminal, retaining only Omarchy ANSI colors.
// Would catch: Omarchy repainting the UI, a changed default ANSI slot, or low contrast.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { contrastRatio, parseColor } from "../components/frame/contrast";
import { DEFAULT_THEME, themeFromDto } from "../terminal/colors";
import { applyTheme, cssVarsFrom, FALLBACK_CSS, terminalThemeFrom, type ThemeDto } from "./apply";

const css = readFileSync(resolve("src/app.css"), "utf8");
const titleBar = readFileSync(resolve("src/components/frame/TitleBar.svelte"), "utf8");
const sidebar = readFileSync(resolve("src/components/frame/Sidebar.svelte"), "utf8");
const statusBar = readFileSync(resolve("src/components/frame/StatusBar.svelte"), "utf8");
const center = readFileSync(resolve("src/components/center/CenterRegion.svelte"), "utf8");
const workbench = readFileSync(resolve("src/shell/Workbench.svelte"), "utf8");

/** Omarchy tokyo-night fixture (same values as the temp-dir Rust tests, not the machine path). */
const FIXTURE: ThemeDto = {
  source: "omarchy",
  mode: "dark",
  background: "#1a1b26",
  foreground: "#c0caf5",
  cursor: "#c0caf5",
  selection: "#292e42",
  named: [
    "#1a1b26",
    "#f7768e",
    "#9ece6a",
    "#e0af68",
    "#7aa2f7",
    "#ad8ee6",
    "#449dab",
    "#a9b1d6",
    "#414868",
    "#ff7a93",
    "#b9f27c",
    "#ff9e64",
    "#7da6ff",
    "#bb9af7",
    "#0db9d7",
    "#c0caf5",
  ],
  surface: "#1a1b26",
  surface_2: "#24283b",
  surface_3: "#313547",
  border: "#292e42",
  text: "#c0caf5",
  text_muted: "#b4bee6",
  text_dim: "#565f89",
  accent: "#7aa2f7",
  working: "#9ece6a",
  attention: "#e0af68",
  error: "#f7768e",
  idle: "#414868",
};

function mountRegions() {
  document.documentElement.removeAttribute("style");
  document.body.innerHTML = `
    <style>
      :root {
        --bg: #0F0F0F;
        --surface: #0A0A0A;
        --surface-2: #171717;
        --surface-3: #242424;
        --border: #1C1C1C;
        --text: #EDEDED;
        --text-muted: #A3A3A3;
        --text-dim: #6B6B6B;
        --accent: #D4D4D4;
        --accent-soft: #D4D4D41F;
        --working: #4ADE80;
        --attention: #E9A23B;
        --error: #F87171;
        --idle: #6B6B6B;
      }
      [data-region="topbar"], [data-region="sidebar"], [data-region="status"] { background-color: var(--surface); }
      [data-region="panes"], [data-terminal] { background-color: var(--bg); }
    </style>
    <div data-region="topbar"></div>
    <div data-region="sidebar"></div>
    <div data-region="panes"></div>
    <div data-region="status"></div>
    <div data-terminal></div>
  `;
}

afterEach(() => {
  document.documentElement.removeAttribute("style");
  document.body.innerHTML = "";
});

describe("AC-061-01/02 neutral CSS variables from ThemeDto", () => {
  it("keeps all 14 UI tokens neutral with the Omarchy fixture", () => {
    const vars = cssVarsFrom(FIXTURE);
    expect(vars["--bg"]).toBe("#0F0F0F");
    expect(vars["--surface"]).toBe("#0A0A0A");
    expect(vars["--surface"]).not.toBe(vars["--bg"]);
    expect(vars["--surface-2"]).toBe("#171717");
    expect(vars["--surface-3"]).toBe("#242424");
    expect(vars["--border"]).toBe("#1C1C1C");
    expect(vars["--text"]).toBe("#EDEDED");
    expect(vars["--text-muted"]).toBe("#A3A3A3");
    expect(vars["--text-dim"]).toBe("#6B6B6B");
    expect(vars["--accent"]).toBe("#D4D4D4");
    expect(vars["--accent-soft"]).toBe("#D4D4D41F");
    expect(vars["--working"]).toBe("#4ADE80");
    expect(vars["--attention"]).toBe("#E9A23B");
    expect(vars["--error"]).toBe("#F87171");
    expect(vars["--idle"]).toBe("#6B6B6B");
  });

  it("applies neutral surfaces to topbar, sidebar and status, and the neutral background to panes", () => {
    mountRegions();
    applyTheme(FIXTURE);
    const root = getComputedStyle(document.documentElement);
    expect(root.getPropertyValue("--bg").trim()).toBe("#0F0F0F");
    expect(root.getPropertyValue("--surface").trim()).toBe("#0A0A0A");
    expect(root.getPropertyValue("--text-muted").trim()).toBe("#A3A3A3");
    const color = (sel: string) => getComputedStyle(document.querySelector<HTMLElement>(sel)!).backgroundColor;
    const terminal = color("[data-terminal]");
    expect(parseColor(color('[data-region="topbar"]'))).toEqual({ r: 10, g: 10, b: 10, a: 1 });
    expect(parseColor(color('[data-region="sidebar"]'))).toEqual({ r: 10, g: 10, b: 10, a: 1 });
    expect(color('[data-region="panes"]')).toBe(terminal);
    expect(parseColor(color('[data-region="status"]'))).toEqual({ r: 10, g: 10, b: 10, a: 1 });
    const parsed = parseColor(terminal);
    expect(parsed).toEqual({ r: 15, g: 15, b: 15, a: 1 });
  });

  it("keeps --text-muted over --bg at least 4.5:1 on the Omarchy fixture", () => {
    const ratio = contrastRatio(parseColor("#b4bee6")!, parseColor("#1a1b26")!);
    expect(ratio).toBeGreaterThanOrEqual(4.5);
    mountRegions();
    applyTheme(FIXTURE);
    const style = getComputedStyle(document.documentElement);
    const muted = parseColor(style.getPropertyValue("--text-muted").trim())!;
    const bg = parseColor(style.getPropertyValue("--bg").trim())!;
    expect(contrastRatio(muted, bg)).toBeGreaterThanOrEqual(4.5);
  });

  it("restores the app.css fallbacks when the dto is default or absent", () => {
    mountRegions();
    applyTheme(FIXTURE);
    applyTheme(null);
    const style = getComputedStyle(document.documentElement);
    expect(style.getPropertyValue("--bg").trim().toUpperCase()).toBe("#0F0F0F");
    expect(style.getPropertyValue("--surface").trim().toUpperCase()).toBe("#0A0A0A");
    expect(style.getPropertyValue("--text-muted").trim().toUpperCase()).toBe("#A3A3A3");
    const fallback = cssVarsFrom({ ...FIXTURE, source: "default" });
    expect(fallback["--bg"]?.toUpperCase()).toBe("#0F0F0F");
    expect(fallback["--surface"]?.toUpperCase()).toBe("#0A0A0A");
  });

  it("the 16 :root values in app.css are those fallbacks, and the frame regions use the variables", () => {
    for (const [name, value] of Object.entries(FALLBACK_CSS)) {
      expect(css, name).toMatch(new RegExp(`${name.replace("--", "--")}:\\s*${value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}`));
    }
    expect(titleBar).toMatch(/background:\s*var\(--surface\)/);
    expect(sidebar).toMatch(/background-color:\s*var\(--surface\)/);
    expect(statusBar).toMatch(/background:\s*var\(--bg\)/);
    expect(center).toMatch(/background:\s*var\(--bg\)/);
    expect(workbench).toMatch(/background-color:\s*var\(--bg\)/);
  });

  it("the product window is opaque and carries a native backgroundColor", () => {
    const conf = JSON.parse(readFileSync(resolve("src-tauri/tauri.conf.json"), "utf8")) as {
      app: { windows: { transparent?: boolean; backgroundColor?: string }[] };
    };
    const win = conf.app.windows[0];
    expect(win?.transparent).toBe(false);
    expect(win?.backgroundColor).toMatch(/^#[0-9A-Fa-f]{6}$/);
  });

  it("root, body and .terminal have an opaque background after applyTheme", () => {
    mountRegions();
    const terminal = document.createElement("div");
    terminal.className = "terminal";
    terminal.style.backgroundColor = "var(--bg)";
    document.body.appendChild(terminal);
    applyTheme(FIXTURE);
    for (const el of [document.documentElement, document.body, terminal]) {
      const parsed = parseColor(getComputedStyle(el).backgroundColor);
      expect([el.tagName, parsed?.a]).toEqual([el.tagName, 1]);
      expect(parsed).toEqual({ r: 15, g: 15, b: 15, a: 1 });
    }
    expect(css).toMatch(/html,\s*\nbody,\s*\n#app \{[^}]*background:\s*var\(--bg\)/s);
    const view = readFileSync(resolve("src/terminal/TerminalView.svelte"), "utf8");
    expect(view).toMatch(/getContext\(\s*["']2d["']\s*,\s*\{\s*alpha:\s*false\s*\}\s*\)/);
    expect(view).toMatch(/\.terminal\s*\{[^}]*background:\s*var\(--bg\)/s);
  });

  it("keeps color-scheme dark for light Omarchy while the raw DTO converter preserves its values", () => {
    applyTheme({ ...FIXTURE, mode: "light" });
    expect(document.documentElement.style.colorScheme).toBe("dark");
    const theme = themeFromDto(FIXTURE);
    expect(theme.background).toBe("#1a1b26");
    expect(theme.cursor).toBe("#c0caf5");
    expect(theme.named).toHaveLength(16);
    expect(theme.named[15]).toBe("#c0caf5");
    expect(themeFromDto(null)).toEqual(DEFAULT_THEME);
  });
});

// Independent values from PRD visual-neutro, never derived from the implementation.
const NEUTRAL = {
  "--bg": "#0F0F0F", "--surface": "#0A0A0A", "--surface-2": "#171717",
  "--surface-3": "#242424", "--border": "#1C1C1C", "--text": "#EDEDED",
  "--text-muted": "#A3A3A3", "--text-dim": "#6B6B6B", "--accent": "#D4D4D4",
  "--accent-soft": "#D4D4D41F", "--working": "#4ADE80", "--attention": "#E9A23B",
  "--error": "#F87171", "--idle": "#6B6B6B",
};
const DEFAULT_ANSI = [
  "#1b1f24", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#abb2bf",
  "#5c6370", "#f27983", "#a9d48a", "#f0cc8a", "#7cbdf5", "#d38fe6", "#6fc7d3", "#ffffff",
];

function expectNeutralRoot() {
  const style = getComputedStyle(document.documentElement);
  for (const [name, value] of Object.entries(NEUTRAL)) {
    expect(style.getPropertyValue(name).trim(), name).toBe(value);
  }
}

describe("spec 061 palette contract", () => {
  it("AC-061-01: the authored CSS supplies every neutral token before theme loading", () => {
    const style = document.createElement("style");
    style.textContent = css;
    document.body.appendChild(style);
    expectNeutralRoot();
    expect(getComputedStyle(document.documentElement).getPropertyValue("--attention-soft").trim()).toBe("#2B2112");
  });

  it.each([null, { ...FIXTURE, source: "default" }, FIXTURE, { ...FIXTURE, mode: "light" }])(
    "AC-061-01/02: applyTheme keeps every token neutral for %j", (dto) => {
      mountRegions();
      // A stale inline theme must be replaced even when the stylesheet is already neutral.
      for (const name of Object.keys(NEUTRAL)) document.documentElement.style.setProperty(name, "#010203");
      expect(applyTheme(dto)).toEqual(NEUTRAL);
      expectNeutralRoot();
      expect(getComputedStyle(document.documentElement).colorScheme).toBe("dark");
    },
  );

  it.each([null, { ...FIXTURE, source: "default" }, FIXTURE])(
    "AC-061-03: terminal base stays neutral and preserves all 16 ANSI colors for %j", (dto) => {
      const theme = terminalThemeFrom(dto);
      expect(theme).toEqual({
        background: "#0F0F0F", foreground: "#EDEDED", cursor: "#EDEDED",
        named: dto?.source === "omarchy" ? FIXTURE.named : DEFAULT_ANSI,
      });
      expect(DEFAULT_THEME.named).toEqual(DEFAULT_ANSI);
    },
  );

  it("AC-061-02/03: a live light theme update changes only terminal ANSI colors", () => {
    mountRegions();
    applyTheme(FIXTURE);
    const before = terminalThemeFrom(FIXTURE);
    const updated = { ...FIXTURE, mode: "light", background: "#ffffff", accent: "#ff0000",
      named: FIXTURE.named.map((_, i) => `#1234${i.toString(16).padStart(2, "0")}`) };
    applyTheme(updated);
    expectNeutralRoot();
    expect(terminalThemeFrom(updated)).toEqual({ ...before, named: updated.named });
    expect(terminalThemeFrom(null).named).toEqual(DEFAULT_ANSI);
  });
});
