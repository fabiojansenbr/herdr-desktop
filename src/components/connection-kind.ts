// Local/SSH kind cards of the connection dialog (spec 011, AC-011-03): the chosen kind is the
// only one checked and tabbable, and it moves with arrows (both axes) as a radio group; Enter and
// Space activate the card that has focus. Pure so the DOM wiring stays a one-liner.

export type Kind = "local" | "ssh";

export interface KindCard {
  checked: boolean;
  tabindex: 0 | -1;
}

/** Arrow keys of the radiogroup: left/up go to Local, right/down to SSH; null = not a move. */
export function kindForKey(key: string): Kind | null {
  if (key === "ArrowLeft" || key === "ArrowUp") return "local";
  if (key === "ArrowRight" || key === "ArrowDown") return "ssh";
  return null;
}

/** Keys that activate the focused card (button semantics). */
export function isKindActivation(key: string): boolean {
  return key === "Enter" || key === " " || key === "Spacebar";
}

/** Rendered card state of `kind` for the chosen kind: aria-checked follows the selection. */
export function kindCard(kind: Kind, selected: Kind): KindCard {
  const checked = kind === selected;
  return { checked, tabindex: checked ? 0 : -1 };
}
