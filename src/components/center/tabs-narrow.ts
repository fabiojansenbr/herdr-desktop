// Spec 028 AC-028-02 — tab width follows the content: status icon + glyph + title + × (every tab),
// 10 px padding, min 64 px, max 280 px with the title ellipsized above it. Spec 028 r2 — the CSS
// is the source of truth: the tab measures what it renders (`width: max-content`), and the title
// shrinks only at the 280 px cap (`flex: 0 1 auto; min-width: 0` on the label). `tabWidth` mirrors
// that layout as the numeric oracle the tests use for the 64/280 px limits.

export const TAB_MIN_PX = 64;
export const TAB_MAX_PX = 280;
export const TAB_PADDING_PX = 10;
/** Gap between the parts of one tab inside the padding (`display: inline-flex; gap: 8px`). */
export const TAB_GAP_PX = 8;
/** Sizes of the fixed parts (status icon, glyph disc, close button) at the 13 px UI font. */
export const TAB_STATUS_PX = 14;
const GLYPH_PX = 16;
const CLOSE_PX = 18;
/** `padding-left` of the `+N` inside the label. */
const MORE_PAD_PX = 4;
/** Approximate advance of one character of the label; wide glyphs count double. */
const CHAR_PX = 7;
const WIDE_CHAR_PX = 13;
/** Padding 16 + + control 36 + Comando 96 (the strip is what is left of the bar). */
export const TAB_BAR_CHROME_PX = 148;

const WIDE = /[\u1100-\u115f\u2e80-\ua4cf\ua960-\ua97f\uac00-\ud7a3\uf900-\ufaff\ufe10-\ufe19\ufe30-\ufe6f\uff00-\uff60\uffe0-\uffe6\u{1f300}-\u{1faff}\u{20000}-\u{3fffd}]/u;

/** Estimated advance of `text` at the 13 px UI font. */
export function estimatedTextWidth(text: string): number {
  let width = 0;
  for (const char of text) width += WIDE.test(char) ? WIDE_CHAR_PX : CHAR_PX;
  return width;
}

export interface TabWidthParts {
  /** The tab shows the agent/kind glyph disc. */
  glyph: boolean;
  /** Extra panes past the first two (`+N`). */
  more: number;
}

/**
 * Text a tab renders — and therefore the text its width must measure (spec 028 r2): a tab with a
 * name of its own shows its engine label, never the pane-derived `display` (which for a named tab
 * can be empty or stale), while an unnamed tab shows the pane-derived display.
 */
export function tabText(tab: { label: string; display: string; named: boolean }): string {
  return tab.named ? tab.label : tab.display;
}

/** Width of one tab in px, from the text it renders, clamped to [64, 280] with 10 px padding. */
export function tabWidth(title: string, parts: TabWidthParts): number {
  // Every tab lays the × out, so it always takes one gap and its own width.
  const gaps = 2 + (parts.glyph ? 1 : 0);
  const content =
    TAB_STATUS_PX +
    (parts.glyph ? GLYPH_PX : 0) +
    estimatedTextWidth(title) +
    (parts.more > 0 ? MORE_PAD_PX + estimatedTextWidth(`+${parts.more}`) : 0) +
    CLOSE_PX;
  const width = TAB_PADDING_PX * 2 + content + TAB_GAP_PX * gaps;
  return Math.min(TAB_MAX_PX, Math.max(TAB_MIN_PX, Math.round(width)));
}
