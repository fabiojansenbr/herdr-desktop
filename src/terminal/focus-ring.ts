// Visible keyboard focus of the terminal. The focused element is the invisible IME textarea
// (`.ime-target`, excluded from the global focus styles and without outline: it covers the canvas
// and is the input-method target), so the `.terminal` container shows the ring while that textarea
// matches :focus-visible.

export const FOCUS_RING_CLASS = "focus-visible";

type RingTarget = Pick<EventTarget, "addEventListener" | "removeEventListener"> & { matches(selector: string): boolean };
type RingContainer = { classList: { toggle(token: string, force: boolean): boolean } };

/** Keeps `FOCUS_RING_CLASS` on `container` equal to `target:focus-visible`; returns the disposer. */
export function bindFocusRing(container: RingContainer, target: RingTarget): () => void {
  const update = () => {
    let visible = false;
    try {
      visible = target.matches(":focus-visible");
    } catch {
      visible = false;
    }
    container.classList.toggle(FOCUS_RING_CLASS, visible);
  };
  const hide = () => container.classList.toggle(FOCUS_RING_CLASS, false);
  target.addEventListener("focus", update);
  target.addEventListener("keydown", update);
  target.addEventListener("blur", hide);
  return () => {
    target.removeEventListener("focus", update);
    target.removeEventListener("keydown", update);
    target.removeEventListener("blur", hide);
    hide();
  };
}
