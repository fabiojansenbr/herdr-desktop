// Detector used by the area specs (068–071) to prove a region is translated: it reads a rendered
// tree the way a user does — the visible texts plus the accessible names (`aria-label`, `title`,
// `placeholder`) — and reports what still looks like Portuguese.
//
// It is a heuristic on purpose: accented letters the three languages do not share, plus the whole
// words that appear most in this UI. A Spanish or English text has none of them, so an area that
// has been migrated reports an empty list; a forgotten label reports itself, with its own words,
// which is what makes the failure readable.

/** Letters that only the Portuguese texts of this product use. */
const MARKS = /[ãõçáéíóúâêô]/i;

/** Whole words of the current interface; `sem` must not match inside `assembly`. */
const WORDS =
  /(?<![\p{L}])(?:agora|fechado|carregando|novo|nova|precisa|coleção|abas|aba|conectando|reconectando|sem|fechar|abrir|buscar|arquivos|ações|você)(?![\p{L}])/iu;

/** Attributes a screen reader would announce, in the order they are reported. */
const NAMED_ATTRIBUTES = ["aria-label", "title", "placeholder"] as const;

/** Elements whose text is never shown to the user. */
const IGNORED_TAGS = new Set(["SCRIPT", "STYLE", "TEMPLATE"]);

function looksPortuguese(text: string): boolean {
  return MARKS.test(text) || WORDS.test(text);
}

/**
 * Every visible text and accessible name under `root` (including `root` itself) that still reads
 * as Portuguese, in document order: an element's names come before its content.
 */
export function untranslated(root: Element): string[] {
  const found: string[] = [];
  const visit = (node: Node): void => {
    if (node.nodeType === Node.ELEMENT_NODE) {
      const element = node as Element;
      if (IGNORED_TAGS.has(element.tagName)) return;
      for (const attribute of NAMED_ATTRIBUTES) {
        const value = element.getAttribute(attribute)?.trim() ?? "";
        if (value !== "" && looksPortuguese(value)) found.push(value);
      }
      for (const child of Array.from(element.childNodes)) visit(child);
      return;
    }
    if (node.nodeType === Node.TEXT_NODE) {
      const text = (node.textContent ?? "").trim();
      if (text !== "" && looksPortuguese(text)) found.push(text);
    }
  };
  visit(root);
  return found;
}
