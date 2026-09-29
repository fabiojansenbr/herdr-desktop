// Spec 011 — `visual-projects` phase of the native flow (AC-011-01/02/03).
// Observes the real composed window:
// 1. Projects tree (groups with name, color dot, project count; projects with branch, host badge,
//    agent state dots, active project background surface-3 and 2px accent bar, menu "…").
// 2. Connections footer ("Este computador · Local", SSH hosts with latency ms or offline, status dots, "+" button).
// 3. Connection dialog ("Conectar a um servidor herdr": 620px width, 12px radius, Local/SSH cards,
//    fields Host, Porta 22, Autenticação, Nome de exibição, Grupo, Sessão, and 4 progress items in order).

import { composite, parseColor, type Rgba } from "../../components/frame/contrast";
import { waitFor } from "../../harness/dom";
import { parseIdentity } from "./ssh-flow";
import type { AwaitParent } from "./paste-flow";

export const VISUAL_PROJECTS_STEPS = ["visual-projects-sidebar", "visual-projects-dialog", "visual-projects-close"] as const;

export type Identity = { pane_id: string; generation: string; boot_prefix: string; endpoint: string };

export type SmallText = {
  scope: string;
  text: string;
  color: string;
  background: string;
  opacity: number;
  font_size: number;
  font_weight: number;
  visibility: string;
};

export interface GroupSeen {
  name: string;
  color: string;
  count: number;
  collapsed: boolean;
}

export interface ProjectSeen {
  name: string;
  branch: string;
  host_badge: string | null;
  active: boolean;
  bg_color: string;
  accent_bar_width: number;
  accent_bar_color: string;
  agent_dots: { status: string; label: string }[];
  has_menu: boolean;
}

export interface ConnectionSeen {
  label: string;
  type_badge: string;
  latency_text: string;
  status_text: string;
  tone: string;
}

export interface SidebarSeen {
  groups: GroupSeen[];
  projects: ProjectSeen[];
  connections: ConnectionSeen[];
  small_texts: SmallText[];
}

export interface DialogCardSeen {
  kind: string;
  role: string;
  tabindex: number;
  selected: boolean;
}

export interface ProgressItemSeen {
  text: string;
  step: number;
  status: string;
}

export interface DialogSeen {
  open: boolean;
  width: number;
  height: number;
  border_radius: number;
  cards: DialogCardSeen[];
  fields: string[];
  progress_items: ProgressItemSeen[];
  small_texts: SmallText[];
}

export interface VisualProjectsPage {
  identity(): Identity | null;
  settle(): Promise<string[]>;
  waitForAgent?(): Promise<void>;
  sidebar(): SidebarSeen;
  openDialog(): Promise<boolean>;
  dialog(): DialogSeen | null;
  activeId(): string | null;
}

export async function runVisualProjects(
  page: VisualProjectsPage,
  awaitParent: AwaitParent
): Promise<Record<string, unknown>> {
  const identity = page.identity();
  if (!identity || !identity.endpoint.endsWith(" · Local")) {
    throw new Error(
      `precondition: visual-projects needs the confirmed Local identity; got ${JSON.stringify(identity)}`
    );
  }

  const parent: Record<string, Record<string, unknown>> = {};
  const step = async (
    name: (typeof VISUAL_PROJECTS_STEPS)[number],
    detail: Record<string, unknown> = {}
  ) => {
    parent[name] = await awaitParent(name, { ...identity, ...detail });
    return parent[name]!;
  };

  const settled = await page.settle();
  // The parent starts the deterministic agent in this confirmed pane (the active project's
  // workspace) before answering, so the sidebar must be measured only after the dot shows up.
  await step("visual-projects-sidebar", { pane_id: identity.pane_id });
  if (page.waitForAgent) {
    await page.waitForAgent();
  }
  const sidebar = page.sidebar();

  const opened = await page.openDialog();
  if (!opened) {
    throw new Error("visual-projects: connection dialog never opened after clicking '+' button");
  }

  const dialog = page.dialog();
  if (!dialog || !dialog.open) {
    throw new Error("visual-projects: connection dialog not open when measured");
  }

  await step("visual-projects-dialog", { dialog });

  // Parent sends native Escape key to close the dialog and restore focus
  await step("visual-projects-close", { opener: "add-connection-btn" });

  const focusAfter = page.activeId();
  const dialogAfter = page.dialog();

  return {
    identity,
    settled,
    sidebar,
    dialog,
    dialog_closed: dialogAfter === null || !dialogAfter.open,
    focus_after: focusAfter,
    parent,
  };
}

// ------------------------------------------------------------------------------------ DOM page

const rgba = (c: Rgba) =>
  `rgba(${Math.round(c.r)}, ${Math.round(c.g)}, ${Math.round(c.b)}, ${Math.round(c.a * 1000) / 1000})`;

function effectiveBackground(el: Element): Rgba {
  const layers: Rgba[] = [];
  for (let node: Element | null = el; node; node = node.parentElement) {
    const color = parseColor(getComputedStyle(node).backgroundColor);
    if (color && color.a > 0) layers.push(color);
    if (color && color.a >= 1) break;
  }
  return layers.reverse().reduce<Rgba>((under, layer) => composite(layer, under), {
    r: 17,
    g: 19,
    b: 24,
    a: 1,
  });
}

function opacityOf(el: Element): number {
  let acc = 1;
  for (let node: Element | null = el; node; node = node.parentElement) {
    const val = parseFloat(getComputedStyle(node).opacity);
    if (!Number.isNaN(val)) acc *= val;
  }
  return acc;
}

function collectSmallTexts(container: Element, scope: string): SmallText[] {
  const texts: SmallText[] = [];
  const walk = (node: Element) => {
    const style = getComputedStyle(node);
    if (
      style.display === "none" ||
      style.visibility === "hidden" ||
      parseFloat(style.opacity) === 0 ||
      opacityOf(node) === 0
    )
      return;

    // Direct text child check or leaf with non-empty text
    const text = (node.textContent ?? "").trim();
    if (text.length > 0 && node.children.length === 0) {
      const bg = effectiveBackground(node);
      const fontSize = parseFloat(style.fontSize) || 12;
      const fontWeight = parseFloat(style.fontWeight) || 400;
      texts.push({
        scope,
        text: text.slice(0, 60),
        color: style.color,
        background: rgba(bg),
        opacity: opacityOf(node),
        font_size: fontSize,
        font_weight: fontWeight,
        visibility: style.visibility,
      });
    }
    for (const child of Array.from(node.children)) {
      walk(child);
    }
  };
  walk(container);
  return texts;
}

const nextFrame = () => new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));

export function domVisualProjectsPage(root: HTMLElement): VisualProjectsPage {
  const projectsRegion = () =>
    root.querySelector<HTMLElement>('[data-slot="projects"]') ??
    root.querySelector<HTMLElement>(".region[data-slot='projects']");

  const connectionModal = () =>
    root.querySelector<HTMLDialogElement>("dialog.connection-dialog");

  return {
    identity: () => {
      const id = parseIdentity(
        root.querySelector<HTMLElement>(".status-bar [data-phase=live]")?.title ?? ""
      );
      const endpoint =
        Array.from(root.querySelectorAll<HTMLElement>(".status-bar .status-item")).map((i) =>
          i.innerText.trim()
        )[1] ?? "";
      return id && endpoint ? { ...id, endpoint } : null;
    },

    settle: async () => {
      const done: string[] = [];
      const press = async (label: string) => {
        const button = root.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`);
        if (!button) return;
        if (button.getAttribute("aria-pressed") === "true") return;
        button.click();
        done.push(`${label}: click`);
        await waitFor(
          `${label} pressed`,
          () => button.getAttribute("aria-pressed") === "true",
          5000
        );
      };

      await press("Projetos e coleções");
      await waitFor(
        "projects region mounted",
        () => root.querySelector('[data-slot="projects"]') !== null,
        15000
      );
      await nextFrame();
      await nextFrame();
      return done;
    },

    waitForAgent: async () => {
      await waitFor(
        "agent dot in active project row",
        () => {
          const region = projectsRegion();
          const activeRow = region?.querySelector(".project-row.active");
          const dot = activeRow?.querySelector(".agent-dot");
          return dot !== null ? dot : null;
        },
        15000
      );
      await nextFrame();
      await nextFrame();
    },

    sidebar: () => {
      const region = projectsRegion();
      if (!region) {
        return { groups: [], projects: [], connections: [], small_texts: [] };
      }

      // 1. Groups
      const groupEls = Array.from(region.querySelectorAll<HTMLElement>(".group-header"));
      const groups: GroupSeen[] = groupEls.map((el) => {
        const name = el.querySelector<HTMLElement>(".group-name")?.innerText.trim() ?? "";
        const countText = el.querySelector<HTMLElement>(".project-count")?.innerText.trim() ?? "0";
        const dot = el.querySelector<HTMLElement>(".color-dot");
        const color = dot ? getComputedStyle(dot).backgroundColor : "";
        const collapsed = el.getAttribute("aria-expanded") === "false";
        return { name, color, count: parseInt(countText, 10) || 0, collapsed };
      });

      // 2. Projects
      const projectEls = Array.from(region.querySelectorAll<HTMLElement>(".project-row"));
      const projects: ProjectSeen[] = projectEls.map((el) => {
        const name = el.querySelector<HTMLElement>(".project-name")?.innerText.trim() ?? "";
        const branch = el.querySelector<HTMLElement>(".branch-name")?.innerText.trim() ?? "";
        const hostBadge = el.querySelector<HTMLElement>(".host-badge")?.innerText.trim() ?? null;
        const active = el.classList.contains("active");
        const bgStyle = getComputedStyle(el);
        const accentBar = el.querySelector<HTMLElement>(".accent-bar");
        const accentStyle = accentBar ? getComputedStyle(accentBar) : null;
        const accentWidth = accentBar ? accentBar.getBoundingClientRect().width : 0;
        const accentColor = accentStyle ? accentStyle.backgroundColor : "";

        const dotEls = Array.from(el.querySelectorAll<HTMLElement>(".agent-dot"));
        const agentDots = dotEls.map((d) => ({
          status: d.getAttribute("data-status") ?? "",
          label: d.getAttribute("aria-label") ?? "",
        }));

        const menuBtn = el.querySelector<HTMLButtonElement>(".menu-btn");

        return {
          name,
          branch,
          host_badge: hostBadge,
          active,
          bg_color: bgStyle.backgroundColor,
          accent_bar_width: accentWidth,
          accent_bar_color: accentColor,
          agent_dots: agentDots,
          has_menu: menuBtn !== null,
        };
      });

      // 3. Connections footer
      const footer = region.querySelector<HTMLElement>(".connections-footer");
      const connEls = footer ? Array.from(footer.querySelectorAll<HTMLElement>(".connection-row")) : [];
      const connections: ConnectionSeen[] = connEls.map((el) => {
        const label = el.querySelector<HTMLElement>(".name")?.innerText.trim() ?? "";
        const typeBadge = el.querySelector<HTMLElement>(".type-badge")?.innerText.trim() ?? "";
        const latencyText = el.querySelector<HTMLElement>(".status-text")?.innerText.trim() ?? "";
        const statusDot = el.querySelector<HTMLElement>(".status-dot");
        const tone = statusDot?.getAttribute("data-tone") ?? "";
        return { label, type_badge: typeBadge, latency_text: latencyText, status_text: latencyText, tone };
      });

      // 4. Small texts for contrast verification
      const smallTexts = collectSmallTexts(region, "projects-sidebar");

      return { groups, projects, connections, small_texts: smallTexts };
    },

    openDialog: async () => {
      const region = projectsRegion();
      const addBtn = region?.querySelector<HTMLButtonElement>(".add-btn");
      if (!addBtn) return false;
      addBtn.click();

      const modal = await waitFor(
        "connection dialog open",
        () => {
          const d = connectionModal();
          return d && d.open ? d : null;
        },
        5000
      );
      await nextFrame();
      await nextFrame();
      return modal !== null;
    },

    dialog: () => {
      const modal = connectionModal();
      if (!modal || !modal.open) return null;

      const rect = modal.getBoundingClientRect();
      const style = getComputedStyle(modal);
      const borderRadius = parseFloat(style.borderRadius) || 0;

      // Cards (Local / SSH)
      const cardEls = Array.from(modal.querySelectorAll<HTMLElement>(".kind-card"));
      const cards: DialogCardSeen[] = cardEls.map((el) => ({
        kind: el.getAttribute("data-kind") ?? "",
        role: el.getAttribute("role") ?? "",
        tabindex: el.tabIndex,
        selected: el.getAttribute("aria-checked") === "true",
      }));

      // Field labels
      const labels = Array.from(modal.querySelectorAll<HTMLElement>("label, .field-label")).map((l) =>
        l.innerText.trim()
      );

      // Progress items
      const itemEls = Array.from(modal.querySelectorAll<HTMLElement>(".progress-item"));
      const progressItems: ProgressItemSeen[] = itemEls.map((el, i) => {
        const text = el.querySelector<HTMLElement>(".step-label")?.innerText.trim() ?? el.innerText.trim();
        const status = el.getAttribute("data-status") ?? "";
        return { text, step: i + 1, status };
      });

      const smallTexts = collectSmallTexts(modal, "connection-dialog");

      return {
        open: modal.open,
        width: rect.width,
        height: rect.height,
        border_radius: borderRadius,
        cards,
        fields: labels,
        progress_items: progressItems,
        small_texts: smallTexts,
      };
    },

    activeId: () => {
      const el = document.activeElement;
      if (!el) return null;
      return (
        el.getAttribute("data-testid") ??
        el.getAttribute("aria-label") ??
        el.className ??
        el.tagName.toLowerCase()
      );
    },
  };
}
