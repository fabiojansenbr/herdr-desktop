// DOM helpers for native harness scenarios: act like a user (type, pick, click) and wait for
// what the user would see. Every wait has a deadline and fails with the current page text.

export async function waitFor<T>(what: string, probe: () => T | null | undefined | false, timeoutMs = 20000): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = probe();
    if (value) return value;
    if (Date.now() > deadline) {
      throw new Error(`timed out waiting for ${what}; page: ${document.body.innerText.slice(0, 1500)}`);
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
}

export function byText<E extends HTMLElement>(root: ParentNode, selector: string, text: string): E | null {
  return (Array.from(root.querySelectorAll<E>(selector)).find((el) => el.textContent?.trim() === text) ?? null);
}

export function type(input: HTMLInputElement, value: string): void {
  input.focus();
  input.value = value;
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

export function choose(select: HTMLSelectElement, value: string): void {
  select.value = value;
  select.dispatchEvent(new Event("change", { bubbles: true }));
}

export function press(button: HTMLButtonElement): void {
  if (button.disabled) throw new Error(`button "${button.textContent?.trim()}" is disabled`);
  button.click();
}
