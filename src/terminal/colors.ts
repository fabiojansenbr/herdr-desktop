// Packed u32 colour decoding (mirror of wire.rs color_to_u32 / u32_to_color).
// 0x00_00_00_XX named (0 reset .. 16 white), 0x01_00_00_XX indexed, 0x02_RR_GG_BB rgb.

export interface Theme {
  foreground: string;
  background: string;
  cursor: string;
  named: string[]; // 16 entries: black, red, green, yellow, blue, magenta, cyan, gray(white), darkgray, light*...
}

/** Builds a renderer theme from a dto (or any partial); missing slots use `DEFAULT_THEME`. */
export function themeFromDto(dto: { foreground?: string; background?: string; cursor?: string; named?: string[] } | null | undefined): Theme {
  if (!dto) return DEFAULT_THEME;
  return {
    foreground: dto.foreground ?? DEFAULT_THEME.foreground,
    background: dto.background ?? DEFAULT_THEME.background,
    cursor: dto.cursor ?? DEFAULT_THEME.cursor,
    named: Array.from({ length: 16 }, (_, i) => dto.named?.[i] ?? DEFAULT_THEME.named[i]!),
  };
}

export const DEFAULT_THEME: Theme = {
  foreground: "#EDEDED",
  background: "#0F0F0F",
  cursor: "#EDEDED",
  named: [
    "#1b1f24", // Black
    "#e06c75", // Red
    "#98c379", // Green
    "#e5c07b", // Yellow
    "#61afef", // Blue
    "#c678dd", // Magenta
    "#56b6c2", // Cyan
    "#abb2bf", // Gray (white)
    "#5c6370", // DarkGray
    "#f27983", // LightRed
    "#a9d48a", // LightGreen
    "#f0cc8a", // LightYellow
    "#7cbdf5", // LightBlue
    "#d38fe6", // LightMagenta
    "#6fc7d3", // LightCyan
    "#ffffff", // White
  ],
};

function hex2(n: number): string {
  return n.toString(16).padStart(2, "0");
}

/** 256-colour palette index → CSS colour (xterm layout). */
export function indexedToCss(index: number, theme: Theme): string {
  if (index < 16) return theme.named[index] ?? theme.foreground;
  if (index < 232) {
    const i = index - 16;
    const r = Math.floor(i / 36);
    const g = Math.floor((i % 36) / 6);
    const b = i % 6;
    const level = (v: number) => (v === 0 ? 0 : 55 + v * 40);
    return `#${hex2(level(r))}${hex2(level(g))}${hex2(level(b))}`;
  }
  const gray = 8 + (index - 232) * 10;
  return `#${hex2(gray)}${hex2(gray)}${hex2(gray)}`;
}

/** Decodes a packed wire colour. `role` decides what "reset" (0) means. */
export function packedToCss(packed: number, role: "fg" | "bg", theme: Theme): string {
  const tag = packed >>> 24;
  if (tag === 0x00) {
    const named = packed & 0xff;
    if (named === 0 || named > 16) return role === "fg" ? theme.foreground : theme.background;
    return theme.named[named - 1] ?? theme.foreground;
  }
  if (tag === 0x01) return indexedToCss(packed & 0xff, theme);
  if (tag === 0x02) {
    return `#${hex2((packed >>> 16) & 0xff)}${hex2((packed >>> 8) & 0xff)}${hex2(packed & 0xff)}`;
  }
  return role === "fg" ? theme.foreground : theme.background;
}
