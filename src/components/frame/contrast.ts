// Luminance contrast (WCAG 2.2 relative luminance) of computed colors. Used by the spec 010 native
// visual-frame phase to measure small text; the parent test recomputes the ratio from the raw colors.

export interface Rgba {
  r: number;
  g: number;
  b: number;
  a: number;
}

/** Parses `#rgb`, `#rrggbb`, `#rrggbbaa`, `rgb()`/`rgba()` (comma or space syntax) and `transparent`. */
export function parseColor(css: string): Rgba | null {
  const text = css.trim().toLowerCase();
  if (text === "transparent") return { r: 0, g: 0, b: 0, a: 0 };
  const hex = /^#([0-9a-f]{3}|[0-9a-f]{6}|[0-9a-f]{8})$/.exec(text)?.[1];
  if (hex) {
    const full = hex.length === 3 ? hex.split("").map((c) => c + c).join("") : hex;
    const byte = (i: number) => parseInt(full.slice(i, i + 2), 16);
    return { r: byte(0), g: byte(2), b: byte(4), a: full.length === 8 ? byte(6) / 255 : 1 };
  }
  const fn = /^rgba?\((.*)\)$/.exec(text)?.[1];
  if (!fn) return null;
  const parts = fn.split(/[\s,/]+/).filter(Boolean);
  if (parts.length !== 3 && parts.length !== 4) return null;
  const channel = (p: string) => (p.endsWith("%") ? (parseFloat(p) * 255) / 100 : parseFloat(p));
  const alpha = parts[3] === undefined ? 1 : parts[3].endsWith("%") ? parseFloat(parts[3]) / 100 : parseFloat(parts[3]);
  const [r, g, b] = parts.slice(0, 3).map(channel) as [number, number, number];
  if ([r, g, b, alpha].some((n) => Number.isNaN(n))) return null;
  return { r, g, b, a: alpha };
}

/** `fg` drawn over `bg` (source-over); the result is opaque when `bg` is. */
export function composite(fg: Rgba, bg: Rgba): Rgba {
  const a = fg.a + bg.a * (1 - fg.a);
  if (a === 0) return { r: 0, g: 0, b: 0, a: 0 };
  const mix = (f: number, b: number) => (f * fg.a + b * bg.a * (1 - fg.a)) / a;
  return { r: mix(fg.r, bg.r), g: mix(fg.g, bg.g), b: mix(fg.b, bg.b), a };
}

export function relativeLuminance(c: Rgba): number {
  const lin = (v: number) => {
    const s = v / 255;
    return s <= 0.04045 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b);
}

/** Ratio of `fg` composited over `bg` (itself treated as opaque) against `bg`. */
export function contrastRatio(fg: Rgba, bg: Rgba): number {
  const base = { ...bg, a: 1 };
  const a = relativeLuminance(composite(fg, base));
  const b = relativeLuminance(base);
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
}

/** WCAG large text: 24 px regular, or 18.67 px (14 pt) bold. */
export function isLargeText(fontSizePx: number, weight: number): boolean {
  return fontSizePx >= 24 || (fontSizePx >= 18.66 && weight >= 700);
}

/** First family of a CSS `font-family` list, unquoted. */
export function firstFamily(list: string): string {
  return (list.split(",")[0] ?? "").trim().replace(/^["']|["']$/g, "");
}
