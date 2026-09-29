// Spec 010 — `visual-frame` phase of the native flow (AC-010-01/02/03). The page observes the real
// composed window: region rects (getBoundingClientRect), computed tokens/fonts/colors
// (getComputedStyle), the top bar, menus, status bar, palette DOM and the trusted Ctrl+K key events.
// The parent resizes the private output to a 1440×900 CSS viewport, presses the native keys, reads
// the PTY bytes and the engine data, and computes every check (tests/fidelity-native/visual_frame.rs).
import { composite, parseColor, type Rgba } from "../../components/frame/contrast";
import { waitFor } from "../../harness/dom";
import { parseIdentity } from "./ssh-flow";
import type { AwaitParent } from "./paste-flow";

export const VISUAL_STEPS = ["visual-viewport", "visual-engine", "visual-ctrlk-outside", "visual-escape", "visual-ctrlk-terminal", "visual-restore"] as const;
export const TARGET_VIEWPORT = { width: 1440, height: 900 } as const;
export const GUIDE_TOKEN_NAMES = ["--bg", "--surface", "--surface-2", "--surface-3", "--border", "--text", "--text-muted", "--text-dim", "--accent", "--accent-soft", "--working", "--attention", "--error", "--idle", "--font-ui", "--font-mono"] as const;

export type Identity = { pane_id: string; generation: string; boot_prefix: string; endpoint: string };
export type Viewport = { inner_width: number; inner_height: number; dpr: number };
export type KeySeen = { trusted: boolean; target: string; palette_at_first_frame: boolean; frames_waited: number };
export type SmallText = { scope: string; text: string; color: string; background: string; opacity: number; font_size: number; font_weight: number; visibility: string };
/** Palette DOM with its small texts measured while open (contrast covers the palette, not only the bars). */
export type PaletteSeen = { sections: { title: string; entries: { id: string; label: string }[] }[]; input_focused: boolean; small_texts?: SmallText[] };

export interface VisualFramePage {
  identity(): Identity | null;
  viewport(): Viewport;
  waitViewport(width: number, height: number, timeoutMs: number): Promise<boolean>;
  /**
   * Default composed state before measuring: projects and agents panels open (earlier phases may
   * have collapsed them), the session's agents attached (status counts shown) and the agent the
   * parent started (visual-viewport) listed in the agents panel. Returns the actions taken.
   */
  settle(): Promise<string[]>;
  frame(): Record<string, unknown>;
  /** Each top-bar menu opened in turn: items and the small texts measured while it is open. */
  menus(): Promise<unknown[]>;
  palette(): PaletteSeen | null;
  /** Focuses the host selector of the top bar (outside the terminal); returns the focused id. */
  focusOutside(): string | null;
  focusTerminal(): boolean;
  activeId(): string | null;
  /** Records trusted Ctrl/Cmd+K keydowns and whether the palette exists at the next frame. */
  recordKeys(): { take(): KeySeen[] };
}

export async function runVisualFrame(page: VisualFramePage, awaitParent: AwaitParent): Promise<Record<string, unknown>> {
  const identity = page.identity();
  if (!identity || !identity.endpoint.endsWith(" · Local")) throw new Error(`precondition: visual-frame needs the confirmed Local identity; got ${JSON.stringify(identity)}`);
  const parent: Record<string, Record<string, unknown>> = {};
  const step = async (name: (typeof VISUAL_STEPS)[number], detail: Record<string, unknown> = {}) => {
    parent[name] = await awaitParent(name, { ...identity, ...detail });
    return parent[name]!;
  };
  const before = page.viewport();
  await step("visual-viewport", { ...before, target_width: TARGET_VIEWPORT.width, target_height: TARGET_VIEWPORT.height });
  if (!(await page.waitViewport(TARGET_VIEWPORT.width, TARGET_VIEWPORT.height, 15000))) {
    throw new Error(`viewport never reached 1440×900 CSS px: ${JSON.stringify(page.viewport())}`);
  }
  const settled = await page.settle();
  const viewport = page.viewport();
  const frame = page.frame();
  const menus = await page.menus();
  await step("visual-engine");

  const opener = page.focusOutside();
  const outsideKeys = page.recordKeys();
  await step("visual-ctrlk-outside", { opener });
  const ctrlkOutside = { opener, keys: outsideKeys.take(), palette: page.palette(), focus_in_palette: page.activeId() };

  await step("visual-escape");
  const escape = { palette: page.palette(), focus_after: page.activeId() };

  const focused = page.focusTerminal();
  const terminalKeys = page.recordKeys();
  await step("visual-ctrlk-terminal", { focused });
  const ctrlkTerminal = { focused, keys: terminalKeys.take(), palette: page.palette(), focus_after: page.activeId() };
  await step("visual-restore");
  return { identity, viewport_before: before, settled, viewport, frame, menus, parent, ctrlk_outside: ctrlkOutside, escape, ctrlk_terminal: ctrlkTerminal };
}

// ------------------------------------------------------------------------------------ DOM page

const rectOf = (el: Element | null) => {
  if (!el) return null;
  const r = el.getBoundingClientRect();
  return { left: r.left, top: r.top, width: r.width, height: r.height };
};

const rgba = (c: Rgba) => `rgba(${Math.round(c.r)}, ${Math.round(c.g)}, ${Math.round(c.b)}, ${Math.round(c.a * 1000) / 1000})`;

/** Background actually under `el`: its own and its ancestors' computed backgrounds, composited. */
function effectiveBackground(el: Element): Rgba {
  const layers: Rgba[] = [];
  for (let node: Element | null = el; node; node = node.parentElement) {
    const color = parseColor(getComputedStyle(node).backgroundColor);
    if (color && color.a > 0) layers.push(color);
    if (color && color.a >= 1) break;
  }
  return layers.reverse().reduce<Rgba>((under, layer) => composite(layer, under), { r: 255, g: 255, b: 255, a: 1 });
}

function opacityOf(el: Element): number {
  let product = 1;
  for (let node: Element | null = el; node; node = node.parentElement) product *= parseFloat(getComputedStyle(node).opacity) || 0;
  return product;
}

/** Visible elements carrying their own text in `scope`, with the colors needed to measure contrast. */
function smallTexts(scope: string, root: Element | null): SmallText[] {
  if (!root) return [];
  return Array.from(root.querySelectorAll<HTMLElement>("*"))
    .filter((el) => Array.from(el.childNodes).some((n) => n.nodeType === Node.TEXT_NODE && (n.textContent ?? "").trim() !== "") && el.getClientRects().length > 0)
    .map((el) => {
      const style = getComputedStyle(el);
      return {
        scope,
        text: (el.innerText || el.textContent || "").trim().slice(0, 60),
        color: style.color,
        background: rgba(effectiveBackground(el)),
        opacity: opacityOf(el),
        font_size: parseFloat(style.fontSize),
        font_weight: parseInt(style.fontWeight, 10) || 400,
        visibility: style.visibility,
      };
    });
}

const idOf = (el: Element | null): string | null =>
  el ? (el.getAttribute("data-topbar-item") ?? el.getAttribute("aria-label") ?? el.tagName.toLowerCase()) : null;

const nextFrame = () => new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));

export function domVisualFramePage(root: HTMLElement): VisualFramePage {
  const topbar = () => root.querySelector<HTMLElement>('[data-region="topbar"]');
  const status = () => root.querySelector<HTMLElement>('[data-region="status"]');
  return {
    identity: () => {
      const id = parseIdentity(root.querySelector<HTMLElement>(".status-bar [data-phase=live]")?.title ?? "");
      const endpoint = Array.from(root.querySelectorAll<HTMLElement>(".status-bar .status-item")).map((i) => i.innerText.trim())[1] ?? "";
      return id && endpoint ? { ...id, endpoint } : null;
    },
    viewport: () => ({ inner_width: window.innerWidth, inner_height: window.innerHeight, dpr: window.devicePixelRatio }),
    waitViewport: async (width, height, timeoutMs) => {
      const deadline = performance.now() + timeoutMs;
      while (performance.now() < deadline) {
        if (window.innerWidth === width && window.innerHeight === height) {
          await nextFrame();
          await nextFrame();
          return true;
        }
        await new Promise((resolve) => setTimeout(resolve, 50));
      }
      return false;
    },
    settle: async () => {
      const done: string[] = [];
      await waitFor("session counts in the status bar", () => root.querySelector('[data-region="status"] [data-item="counts"]'), 60000);
      await waitFor("workspace tabs", () => root.querySelector("[data-center-tabs]"), 60000);
      await nextFrame();
      await nextFrame();
      return done;
    },
    frame: () => {
      const rootStyle = getComputedStyle(document.documentElement);
      const terminal = root.querySelector<HTMLTextAreaElement>("textarea.ime-target")?.parentElement ?? null;
      const canvas = terminal?.querySelector("canvas") ?? null;
      const host = root.querySelector<HTMLElement>('[data-topbar-item="host"]');
      const hostText = host?.querySelector<HTMLElement>("[data-host-text]") ?? null;
      return {
        regions: Object.fromEntries(Array.from(root.querySelectorAll<HTMLElement>("[data-region]")).filter((el) => el.getClientRects().length > 0).map((el) => [el.dataset.region!, rectOf(el)])),
        tokens: Object.fromEntries(GUIDE_TOKEN_NAMES.map((name) => [name, rootStyle.getPropertyValue(name).trim()])),
        fonts: {
          topbar: topbar() ? getComputedStyle(topbar()!).fontFamily : null,
          status: status() ? getComputedStyle(status()!).fontFamily : null,
          terminal: terminal ? getComputedStyle(terminal).fontFamily : null,
          canvas: canvas?.getContext("2d")?.font ?? null,
        },
        topbar_items: Array.from(topbar()?.querySelectorAll<HTMLElement>("[data-topbar-item]") ?? []).map((el) => ({
          item: el.dataset.topbarItem,
          text: el.innerText.trim(),
          label: el.getAttribute("aria-label"),
          title: el.getAttribute("title"),
          disabled: (el as HTMLButtonElement).disabled === true,
        })),
        host: host && hostText ? { title: host.getAttribute("title"), text: hostText.innerText.trim(), text_overflow: getComputedStyle(hostText).textOverflow, overflow: getComputedStyle(hostText).overflow, white_space: getComputedStyle(hostText).whiteSpace, dot: host.querySelector("[data-host-dot]") !== null } : null,
        status_items: Array.from(status()?.querySelectorAll<HTMLElement>(".status-item") ?? []).map((el) => ({ item: el.dataset.item ?? "", text: el.innerText.trim(), phase: el.dataset.phase ?? null })),
        small_texts: [...smallTexts("topbar", topbar()), ...smallTexts("status", status()), ...smallTexts("activity", root.querySelector('[data-region="activity"]'))],
      };
    },
    menus: async () => {
      const out: unknown[] = [];
      for (const button of Array.from(topbar()?.querySelectorAll<HTMLButtonElement>('[data-topbar-item="menu"]') ?? [])) {
        button.click();
        await nextFrame();
        const menu = root.querySelector<HTMLElement>(`[role="menu"][aria-label="${button.innerText.trim()}"]`);
        out.push({
          label: button.innerText.trim(),
          expanded: button.getAttribute("aria-expanded"),
          items: Array.from(menu?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? []).map((el) => ({
            id: el.dataset.item ?? "",
            label: el.querySelector("[data-label]")?.textContent?.trim() ?? el.innerText.trim(),
            action: el.dataset.action ?? null,
            disabled: el.getAttribute("aria-disabled") === "true" || (el as HTMLButtonElement).disabled === true,
            reason: el.getAttribute("title"),
          })),
          small_texts: smallTexts(`menu:${button.innerText.trim()}`, menu),
        });
        button.click();
        await nextFrame();
      }
      return out;
    },
    palette: () => {
      const palette = root.querySelector<HTMLElement>("[data-palette]");
      if (!palette) return null;
      return {
        sections: Array.from(palette.querySelectorAll<HTMLElement>("[data-palette-section]")).map((s) => ({
          title: s.querySelector("[data-section-title]")?.textContent?.trim() ?? "",
          entries: Array.from(s.querySelectorAll<HTMLElement>("[data-entry-id]")).map((e) => ({ id: e.dataset.entryId!, label: e.querySelector("[data-label]")?.textContent?.trim() ?? "" })),
        })),
        input_focused: document.activeElement === palette.querySelector("input"),
        small_texts: smallTexts("palette", palette),
      };
    },
    focusOutside: () => {
      const host = root.querySelector<HTMLElement>('[data-topbar-item="host"]');
      host?.focus();
      return host && document.activeElement === host ? idOf(host) : null;
    },
    focusTerminal: () => {
      const target = root.querySelector<HTMLTextAreaElement>("textarea.ime-target");
      target?.focus();
      return !!target && document.activeElement === target;
    },
    activeId: () => idOf(document.activeElement),
    recordKeys: () => {
      const seen: KeySeen[] = [];
      const listener = (e: KeyboardEvent) => {
        if (e.key.toLowerCase() !== "k" || !(e.ctrlKey || e.metaKey)) return;
        const record: KeySeen = { trusted: e.isTrusted, target: idOf(e.target as Element) ?? "", palette_at_first_frame: false, frames_waited: 0 };
        seen.push(record);
        requestAnimationFrame(() => {
          record.frames_waited = 1;
          record.palette_at_first_frame = root.querySelector("[data-palette]") !== null;
        });
      };
      window.addEventListener("keydown", listener, { capture: true });
      return {
        take: () => {
          window.removeEventListener("keydown", listener, { capture: true });
          return seen.splice(0);
        },
      };
    },
  };
}
