// Spec 061 — neutral UI and terminal base; Omarchy supplies only terminal ANSI colors.

import { DEFAULT_THEME, themeFromDto, type Theme } from "../terminal/colors";

export interface ThemeDto {
  source: "omarchy" | "default" | string;
  mode: string;
  background: string;
  foreground: string;
  cursor: string;
  selection: string;
  named: string[];
  surface: string;
  surface_2: string;
  surface_3: string;
  border: string;
  text: string;
  text_muted: string;
  text_dim: string;
  accent: string;
  working: string;
  attention: string;
  error: string;
  idle: string;
}

/** Neutral `:root` values of `src/app.css`, applied regardless of the theme source. */
export const FALLBACK_CSS: Record<string, string> = {
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

const TOKEN_NAMES = Object.keys(FALLBACK_CSS);

/** `accent` at 12 % (31/255 ≈ 0x1F), matching the Pen `--accent-soft` encoding. */
export function accentSoft(accent: string): string {
  const hex = accent.trim();
  if (/^#[0-9a-fA-F]{6}$/.test(hex)) return `${hex}1F`;
  return FALLBACK_CSS["--accent-soft"]!;
}

export function cssVarsFrom(_dto: ThemeDto | null | undefined): Record<string, string> {
  return { ...FALLBACK_CSS };
}

export function terminalThemeFrom(dto: ThemeDto | null | undefined): Theme {
  if (!dto || dto.source !== "omarchy") return DEFAULT_THEME;
  return themeFromDto({ named: dto.named });
}

export function applyTheme(dto: ThemeDto | null | undefined, root: HTMLElement | null = typeof document === "undefined" ? null : document.documentElement): Record<string, string> {
  const vars = cssVarsFrom(dto);
  if (root) {
    for (const name of TOKEN_NAMES) root.style.setProperty(name, vars[name]!);
    const mode = "dark";
    root.style.colorScheme = mode;
    root.style.setProperty("color-scheme", mode);
    root.style.backgroundColor = vars["--bg"]!;
    const body = root.ownerDocument?.body;
    if (body) body.style.backgroundColor = vars["--bg"]!;
  }
  return vars;
}
