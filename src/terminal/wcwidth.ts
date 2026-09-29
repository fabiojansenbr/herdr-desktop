// Display width of one grapheme cluster as the server's grid lays it out:
// 0 for pure combining clusters, 2 for East Asian wide/fullwidth and emoji
// presentation, 1 otherwise. The server already places continuation cells with an
// empty symbol, so the renderer only needs this to size the glyph run.

const COMBINING = /^\p{M}+$/u;
const VARIATION_SELECTOR_16 = 0xfe0f;

function isWideCodePoint(cp: number): boolean {
  return (
    (cp >= 0x1100 && cp <= 0x115f) || // Hangul Jamo
    cp === 0x2329 ||
    cp === 0x232a ||
    (cp >= 0x2e80 && cp <= 0x303e) || // CJK radicals, punctuation
    (cp >= 0x3041 && cp <= 0x33ff) || // Hiragana, Katakana, CJK compat
    (cp >= 0x3400 && cp <= 0x4dbf) || // CJK ext A
    (cp >= 0x4e00 && cp <= 0x9fff) || // CJK unified
    (cp >= 0xa000 && cp <= 0xa4cf) || // Yi
    (cp >= 0xac00 && cp <= 0xd7a3) || // Hangul syllables
    (cp >= 0xf900 && cp <= 0xfaff) || // CJK compat ideographs
    (cp >= 0xfe30 && cp <= 0xfe4f) || // CJK compat forms
    (cp >= 0xff00 && cp <= 0xff60) || // Fullwidth forms
    (cp >= 0xffe0 && cp <= 0xffe6) ||
    (cp >= 0x1f300 && cp <= 0x1f64f) || // Misc symbols & pictographs, emoticons
    (cp >= 0x1f680 && cp <= 0x1f6ff) || // Transport
    (cp >= 0x1f900 && cp <= 0x1f9ff) || // Supplemental symbols
    (cp >= 0x1fa70 && cp <= 0x1faff) ||
    (cp >= 0x20000 && cp <= 0x3fffd) // CJK ext B..
  );
}

export function graphemeWidth(symbol: string): number {
  if (symbol.length === 0) return 0;
  if (COMBINING.test(symbol)) return 0;
  let width = 1;
  let hasVs16 = false;
  for (const ch of symbol) {
    const cp = ch.codePointAt(0) ?? 0;
    if (cp === VARIATION_SELECTOR_16) hasVs16 = true;
    if (isWideCodePoint(cp)) {
      width = 2;
    }
  }
  if (hasVs16) width = 2;
  return width;
}
