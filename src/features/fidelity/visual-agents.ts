// Spec 014 — `visual-agents` phase of the native flow (AC-014-01/02/03). The page observes the
// designed agents panel in the real composed window: counters, attention cards, running rows,
// the trusted Enter on a card and the widths before/after collapsing the panel. The parent puts
// a nonce on the agent's screen line, reports the engine state, presses the native key and reads
// the engine; every check is computed there (tests/fidelity-native/visual_agents.rs).
import { waitFor } from "../../harness/dom";
import { composite, parseColor, type Rgba } from "../../components/frame/contrast";
import { parseIdentity } from "./ssh-flow";
import type { AwaitParent } from "./paste-flow";

export const AGENTS_STEPS = ["visual-agents-arm", "visual-agents-block", "visual-agents-focus", "visual-agents-collapse"] as const;

export type Identity = { pane_id: string; generation: string; boot_prefix: string; endpoint: string };
export type SmallText = { text: string; color: string; background: string; opacity: number; font_size: number; font_weight: number; visibility: string };
export type CardSeen = { pane: string; name: string; path: string; time: string; status: string; label: string; last_line: string | null; disabled: boolean };
export type RowSeen = { pane: string; name: string; path: string; summary: string; time: string; status: string; label: string };
export type KeySeen = { trusted: boolean; key: string; card: string };
export type Layout = { agents_width_before: number; center_width_before: number; agents_present_after: boolean; center_width_after: number; collapse_label: string };

export interface VisualAgentsPage {
  identity(): Identity | null;
  /** Opens the agents panel and waits for the engine agent of `pane` to be listed in it. */
  settle(pane: string): Promise<string[]>;
  counters(): { active: number; waiting: number; idle: number; unknown: number };
  counterLabels(): string[];
  /** Resolves with the epoch ms of the first sight of the attention card of `pane`. */
  watchCard(pane: string, timeoutMs: number): Promise<number>;
  cards(): CardSeen[];
  overflow(): number | null;
  rows(): RowSeen[];
  smallTexts(): SmallText[];
  /** Focuses the attention card of `pane`; records the keys that reach it. */
  focusCard(pane: string): { focused: boolean; take(): { keys: KeySeen[]; activations: number } };
  /** Collapses the panel with its own header button and measures the regions. */
  collapse(): Promise<Layout>;
}

export async function runVisualAgents(page: VisualAgentsPage, awaitParent: AwaitParent): Promise<Record<string, unknown>> {
  const identity = page.identity();
  if (!identity || !identity.endpoint.endsWith(" · Local")) throw new Error(`precondition: visual-agents needs the confirmed Local identity; got ${JSON.stringify(identity)}`);
  const parent: Record<string, Record<string, unknown>> = {};
  const step = async (name: (typeof AGENTS_STEPS)[number], detail: Record<string, unknown> = {}) => {
    const answer = await awaitParent(name, { ...identity, ...detail });
    // A step the parent could not perform ends the phase here: the report must never look like a
    // complete observation of a step that never happened.
    if (answer.error !== undefined) throw new Error(`${name}: parent failed: ${JSON.stringify(answer.error)}`);
    parent[name] = answer;
    return answer;
  };

  const armed = await step("visual-agents-arm");
  const pane = String(armed.agent_pane ?? "");
  if (pane === "") throw new Error("visual-agents: the parent named no agent pane");
  const settled = await page.settle(pane);

  // The watch starts before the parent reports the state, so the first sight of the card is
  // measured against the engine change and never after it.
  const seen = page.watchCard(pane, 15000);
  await step("visual-agents-block", { pane_agent: pane });
  const cardSeenAt = await seen;
  const cards = page.cards();
  const counters = page.counters();
  // Read with the panel still mounted: `collapse()` unmounts it at the end of the phase.
  const counterLabels = page.counterLabels();
  const overflow = page.overflow();
  const rows = page.rows();
  const smallTexts = page.smallTexts();

  const card = page.focusCard(pane);
  // Every step after the first names the same pane, so the parent never re-derives it.
  await step("visual-agents-focus", { pane_agent: pane, focused_card: card.focused });
  const enter = card.take();

  const layout = await page.collapse();
  await step("visual-agents-collapse", { pane_agent: pane });
  return {
    identity,
    agent_pane: pane,
    settled,
    counters,
    counter_labels: counterLabels,
    card_seen_at_ms: cardSeenAt,
    attention: { cards, overflow },
    running: rows,
    enter: { ...enter, focused: card.focused },
    layout,
    small_texts: smallTexts,
    parent,
  };
}

// ------------------------------------------------------------------------------------ DOM page

const rgba = (c: Rgba) => `rgba(${Math.round(c.r)}, ${Math.round(c.g)}, ${Math.round(c.b)}, ${Math.round(c.a * 1000) / 1000})`;

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

function measuredTexts(root: Element | null): SmallText[] {
  if (!root) return [];
  return Array.from(root.querySelectorAll<HTMLElement>("*"))
    .filter((el) => Array.from(el.childNodes).some((n) => n.nodeType === Node.TEXT_NODE && (n.textContent ?? "").trim() !== "") && el.getClientRects().length > 0)
    .map((el) => {
      const style = getComputedStyle(el);
      return {
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

const width = (el: Element | null) => (el ? el.getBoundingClientRect().width : 0);
const nextFrame = () => new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
const textOf = (scope: Element | null, selector: string) => scope?.querySelector<HTMLElement>(selector)?.innerText.trim() ?? "";

export function domVisualAgentsPage(root: HTMLElement): VisualAgentsPage {
  const panel = () => root.querySelector<HTMLElement>("[data-agents-panel]");
  const region = (name: string) => root.querySelector<HTMLElement>(`[data-region="${name}"]`);
  const counterBox = (name: string) => panel()?.querySelector<HTMLElement>(`[data-counter="${name}"]`) ?? null;
  const count = (name: string) => {
    const box = counterBox(name);
    if (!box) return 0;
    return Number.parseInt(box.querySelector("strong")?.textContent?.trim() ?? "0", 10) || 0;
  };
  const cardOf = (pane: string) => panel()?.querySelector<HTMLButtonElement>(`[data-attention-pane="${pane}"]`) ?? null;
  const card = (el: HTMLElement): CardSeen => ({
    pane: el.dataset.attentionPane ?? "",
    name: textOf(el, ".name"),
    path: `${textOf(el, ".path")}`,
    time: textOf(el, "[data-time]"),
    status: el.dataset.status ?? "",
    label: (el.querySelector<HTMLElement>(".state")?.innerText ?? "").replace(/\s+/g, " ").trim().replace(/^\S+\s/, ""),
    last_line: el.querySelector<HTMLElement>("[data-last-line]")?.innerText.trim() ?? null,
    disabled: (el as HTMLButtonElement).disabled === true,
  });
  return {
    identity: () => {
      const id = parseIdentity(root.querySelector<HTMLElement>(".status-bar [data-phase=live]")?.title ?? "");
      const endpoint = Array.from(root.querySelectorAll<HTMLElement>(".status-bar .status-item")).map((i) => i.innerText.trim())[1] ?? "";
      return id && endpoint ? { ...id, endpoint } : null;
    },
    settle: async (pane) => {
      const done: string[] = [];
      const button = root.querySelector<HTMLButtonElement>('button[aria-label="Painel de agentes"]');
      if (!button) throw new Error("visual-agents: the agents activity button is missing");
      if (button.getAttribute("aria-pressed") !== "true") {
        button.click();
        done.push("Painel de agentes: click");
        await waitFor("agents panel open", () => button.getAttribute("aria-pressed") === "true", 5000);
      }
      await waitFor("designed agents panel", panel, 30000);
      await waitFor(`engine agent ${pane} in the panel`, () => panel()?.querySelector(`[data-running-pane="${pane}"]`), 60000);
      await nextFrame();
      await nextFrame();
      return done;
    },
    counters: () => ({ active: count("active"), waiting: count("waiting"), idle: count("idle"), unknown: count("unknown") }),
    counterLabels: () => ["active", "waiting", "idle"].map((name) => counterBox(name)?.querySelector("span")?.textContent?.trim() ?? ""),
    watchCard: (pane, timeoutMs) =>
      new Promise<number>((resolve, reject) => {
        const deadline = Date.now() + timeoutMs;
        const timer = setInterval(() => {
          if (cardOf(pane)) {
            clearInterval(timer);
            // Epoch ms, the same clock the parent stamps its engine command with.
            resolve(performance.timeOrigin + performance.now());
          } else if (Date.now() > deadline) {
            clearInterval(timer);
            reject(new Error(`visual-agents: no attention card for ${pane} within ${timeoutMs} ms`));
          }
        }, 10);
      }),
    cards: () => Array.from(panel()?.querySelectorAll<HTMLElement>("[data-attention-pane]") ?? []).map(card),
    overflow: () => {
      const shown = panel()?.querySelector<HTMLElement>("[data-attention-overflow]")?.innerText.trim();
      return shown ? Number.parseInt(shown.replace("+", ""), 10) : null;
    },
    rows: () =>
      Array.from(panel()?.querySelectorAll<HTMLElement>("[data-running-pane]") ?? []).map((el) => ({
        pane: el.dataset.runningPane ?? "",
        name: textOf(el, ".name"),
        path: textOf(el, ".path"),
        summary: textOf(el, "[data-summary]"),
        time: textOf(el, "[data-time]"),
        status: el.dataset.status ?? "",
        label: textOf(el, "[data-state]").replace(/^\S+\s/, ""),
      })),
    smallTexts: () => measuredTexts(panel()),
    focusCard: (pane) => {
      const el = cardOf(pane);
      el?.focus();
      const keys: KeySeen[] = [];
      let activations = 0;
      const onKey = (event: KeyboardEvent) => {
        if (event.key !== "Enter") return;
        keys.push({ trusted: event.isTrusted, key: event.key, card: (event.currentTarget as HTMLElement).dataset.attentionPane ?? "" });
      };
      const onClick = () => {
        activations += 1;
      };
      el?.addEventListener("keydown", onKey);
      el?.addEventListener("click", onClick);
      return {
        focused: !!el && document.activeElement === el,
        take: () => {
          el?.removeEventListener("keydown", onKey);
          el?.removeEventListener("click", onClick);
          return { keys: keys.splice(0), activations };
        },
      };
    },
    collapse: async () => {
      const before = { agents: width(region("agents")), center: width(region("center")) };
      const button = panel()?.querySelector<HTMLButtonElement>("[data-collapse-agents]");
      if (!button) throw new Error("visual-agents: the panel has no collapse button");
      const label = button.getAttribute("aria-label") ?? "";
      button.click();
      await waitFor("agents panel collapsed", () => !root.querySelector("[data-agents-panel]"), 5000);
      await nextFrame();
      await nextFrame();
      return {
        agents_width_before: before.agents,
        center_width_before: before.center,
        agents_present_after: root.querySelector("[data-agents-panel]") !== null,
        center_width_after: width(region("center")),
        collapse_label: label,
      };
    },
  };
}
