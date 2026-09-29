// Page menu bar of the frame (spec 010, premise 1): Arquivo, Editar, Ver, Agentes, Janela. Each enabled
// item calls exactly one action the window already has; what the window cannot do, or what depends
// on a host that is not available, is listed disabled with its reason. No native GTK menu.
//
// Spec 069 (AC-069-03): the language selector is not here. Every item of a menu maps to exactly one
// `MenuAction` of the window, and choosing a language is not one; it lives in the command palette
// (`palette.ts:languageSection`), the menu surface this app actually renders.
import { t } from "../../i18n/index.svelte";
import { shortcutLabel, type Platform } from "../../shell/shortcuts";

export const MENU_ACTIONS = [
  "showProjects",
  "openConnections",
  "paste",
  "toggleProjects",
  "toggleAgents",
  "toggleFiles",
  "openPalette",
  "newAgent",
  "split",
  "newTab",
  "reconnect",
] as const;
export type MenuAction = (typeof MENU_ACTIONS)[number];

export interface MenuContext {
  hostSelected: boolean;
  hostOnline: boolean;
  /** Terminal input gate of the selected surface (live identity + interest). */
  canInput: boolean;
  canRetry: boolean;
  caps: { split: boolean; create_tab: boolean; start_agent: boolean } | null;
  platform: Platform;
  actions: Record<MenuAction, () => void>;
}

export interface MenuItem {
  id: string;
  label: string;
  shortcut: string | null;
  action: MenuAction | null;
  run: (() => void) | null;
  reason: string | null;
}

export interface Menu {
  id: string;
  label: string;
  items: MenuItem[];
}

export function buildMenus(ctx: MenuContext): Menu[] {
  const item = (id: string, label: string, action: MenuAction | null, reason: string | null, shortcut: string | null = null): MenuItem => {
    const enabled = action !== null && reason === null;
    return { id, label, shortcut, action: enabled ? action : null, run: enabled ? () => ctx.actions[action]() : null, reason: enabled ? null : (reason ?? t("frame.menu.reason.unavailable")) };
  };
  const hostReason = (): string | null => {
    if (!ctx.hostSelected) return t("frame.menu.reason.noHost");
    if (!ctx.hostOnline) return t("frame.menu.reason.hostOffline");
    return null;
  };
  const capability = (available: boolean | undefined): string | null => hostReason() ?? (available ? null : t("frame.menu.reason.noCapability"));
  const reconnect = ctx.canRetry ? null : ctx.hostSelected ? t("frame.menu.reason.hostConnected") : t("frame.menu.reason.noHost");
  const windowControls = t("frame.menu.reason.windowControls");
  return [
    {
      id: "file",
      label: t("frame.menu.file"),
      items: [
        item("projects", t("frame.menu.projects"), "showProjects", null),
        item("connect", t("frame.menu.connect"), "openConnections", null),
        item("open-file", t("frame.menu.openFile"), null, t("frame.menu.reason.openFile")),
        item("close-window", t("frame.menu.closeWindow"), null, windowControls),
      ],
    },
    {
      id: "edit",
      label: t("frame.menu.edit"),
      items: [
        item("undo", t("frame.menu.undo"), null, t("frame.menu.reason.noHistory")),
        item("redo", t("frame.menu.redo"), null, t("frame.menu.reason.noHistory")),
        item("copy", t("frame.menu.copy"), null, t("frame.menu.reason.copy")),
        item("paste", t("frame.menu.paste"), "paste", hostReason() ?? (ctx.canInput ? null : t("frame.menu.reason.noInput")), "Ctrl+Shift+V"),
      ],
    },
    {
      id: "view",
      label: t("frame.menu.view"),
      items: [
        item("toggle-projects", t("frame.menu.projectsPanel"), "toggleProjects", null),
        item("toggle-agents", t("frame.menu.agentsPanel"), "toggleAgents", null),
        item("files", t("frame.menu.files"), "toggleFiles", null),
        item("search", t("frame.menu.search"), "openPalette", null, shortcutLabel("palette", ctx.platform)),
      ],
    },
    {
      id: "agents",
      label: t("frame.menu.agents"),
      items: [
        item("new-agent", t("frame.menu.newAgent"), "newAgent", capability(ctx.caps?.start_agent)),
        item("split", t("frame.menu.split"), "split", capability(ctx.caps?.split)),
        item("new-tab", t("frame.menu.newTab"), "newTab", capability(ctx.caps?.create_tab)),
      ],
    },
    {
      id: "window",
      label: t("frame.menu.window"),
      items: [
        item("reconnect", t("frame.menu.reconnect"), "reconnect", reconnect),
        item("minimize", t("frame.menu.minimize"), null, windowControls),
        item("maximize", t("frame.menu.maximize"), null, windowControls),
      ],
    },
  ];
}
