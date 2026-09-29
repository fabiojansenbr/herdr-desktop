// Spec 069 (PRD i18n) — the top bar, the centre, the agents panel, the home screen and the
// language selector in English and Spanish. The suite is fixed on `pt` (067), so every test here
// says which language it renders and puts it back afterwards.
//
// Would catch: a label, tooltip or accessible name of these regions still written in Portuguese
// in another language, a count or a greeting that ignores the language's own plural/wording, a
// language option that does not reach `setLocalePreference`, a choice that is not persisted or
// not marked, or an interface that needs a reload to change language.
import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { HostDto } from "../../connections/types";
import type { FrameContext } from "../../shell/frame-context";
import {
  dictionaries,
  LOCALE_STORAGE_KEY,
  locale,
  localePreference,
  setLocalePreference,
  t,
  type Locale,
  type LocalePreference,
  type Message,
} from "../../i18n/index.svelte";
import { untranslated } from "../../i18n/testing";
import AgentPanel from "../agents/AgentPanel.svelte";
import ContextMenu from "../center/ContextMenu.svelte";
import { paneContextMenuItems } from "../center/pane-menu";
import HomeScreen from "../home/HomeScreen.svelte";
import { buildServerCards, greetingFor } from "../home/home-model";
import CommandPalette from "./CommandPalette.svelte";
import { buildMenus, type Menu, type MenuContext } from "./menus";
import { paletteSections, type PaletteEntry, type PaletteSource } from "./palette";
import { statusBarModel } from "./status";
import StatusBar from "./StatusBar.svelte";
import TitleBar from "./TitleBar.svelte";

/**
 * The four language options of the palette name their own language (AC-069-03), so `Português` —
 * and `Automático`, which Spanish spells the same way as Portuguese — are the only texts the
 * Portuguese detector may report from a translated region. Nothing else is allowed.
 */
const ENDONYMS = ["Português", "Automático"];
const leftovers = (root: Element): string[] => untranslated(root).filter((text) => !ENDONYMS.includes(text));

/**
 * `untranslated()` reads the accents Portuguese and Spanish share, so it cannot judge a Spanish
 * render — `Esperándote` is correct Spanish and would be reported. The Spanish proof is the
 * dictionaries themselves: every Portuguese wording the product carries that Spanish writes
 * differently, as whole words, is text that must never reach a Spanish screen.
 */
function portugueseWordings(): string[] {
  const dicts = dictionaries();
  const pieces = (message: Message | undefined): string[] =>
    (typeof message === "string" ? [message] : Object.values(message ?? {}))
      .flatMap((text) => text.split(/\{\w+\}/))
      .map((text) => text.trim())
      .filter((text) => text.length >= 4 && /\p{L}/u.test(text));
  const spanish = new Set(Object.values(dicts.es).flatMap(pieces));
  return [...new Set(Object.values(dicts.pt).flatMap(pieces))].filter((text) => !spanish.has(text));
}

/** Everything a user reads under `root`: the visible texts and the accessible names. */
function readable(root: Element): string {
  const names = Array.from(root.querySelectorAll("*")).flatMap((element) =>
    ["aria-label", "title", "placeholder"].map((attribute) => element.getAttribute(attribute) ?? ""),
  );
  return [root.textContent ?? "", ...names].join("\n");
}

const escaped = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** The Portuguese wordings a render shows, matched as whole words (`pane` is not `panel`). */
function portugueseIn(root: Element): string[] {
  const text = readable(root);
  return portugueseWordings().filter((wording) => new RegExp(`(?<!\\p{L})${escaped(wording)}(?!\\p{L})`, "u").test(text));
}

const mounted: { host: HTMLElement; app: Record<string, unknown> | null }[] = [];

function render(component: unknown, props: Record<string, unknown>): HTMLElement {
  const host = document.createElement("div");
  document.body.append(host);
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const app = mount(component as any, { target: host, props }) as Record<string, unknown>;
  flushSync();
  mounted.push({ host, app });
  return host;
}

beforeEach(() => {
  localStorage.clear();
});

afterEach(() => {
  for (const { host, app } of mounted.splice(0)) {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    if (app) unmount(app as any);
    host.remove();
  }
  localStorage.clear();
  setLocalePreference("pt");
});

// --- fixtures ---------------------------------------------------------------------------------

const AGENTS = [
  { pane_id: "w1:p1", workspace_id: "w1", tab_id: "t1", name: "claude", kind: "claude", status: "blocked", launch_pending: false, ready: true, focused: true, terminal_title: "build", detection_last_line: "waiting" },
  { pane_id: "w1:p2", workspace_id: "w1", tab_id: "t1", name: "codex", kind: "codex", status: "working", launch_pending: false, ready: true, focused: false, terminal_title: "tests" },
];

const TABS = [
  { tab_id: "t1", workspace_id: "w1", number: 1, label: "1", custom_label: null, focused: true, zoomed: false, pane_count: 2, agent_status: "blocked" },
  { tab_id: "t2", workspace_id: "w1", number: 2, label: "deploy", custom_label: "deploy", focused: false, zoomed: false, pane_count: 1, agent_status: "idle" },
];

const TOPOLOGY = {
  focused_pane_id: "w1:p1",
  panes: [
    { pane_id: "w1:p1", x: 0, y: 0, width: 80, height: 24, focused: true, cwd: "/work/erp-api", label: null },
    { pane_id: "w1:p2", x: 80, y: 0, width: 80, height: 24, focused: false, cwd: "/work/erp-api", label: null },
  ],
  splits: [],
};

const SNAPSHOT = {
  // A user-given collection name, kept language-neutral so it is never read as a leftover.
  collections: [{ id: "c1", name: "Acme", project_ids: ["p1"] }],
  projects: [
    {
      id: "p1",
      label: "erp-api",
      root: "/work/erp-api",
      endpoint_profile_id: "local",
      collection_id: "c1",
      binding: { workspace_id: "w1", session: "default" },
    },
  ],
};

function frameContext(): FrameContext {
  const noop = () => {};
  return {
    surface: {
      phase: "live",
      identity: { endpoint: "local", session: "default", connection_generation: 1, boot_id: "b1", pane_id: "w1:p1" },
      selection: { endpoint: "local", session: "default" },
      status: { session: "default", server_version: "0.9.0" },
    },
    agents: {
      phase: "connected",
      agents: AGENTS,
      tabs: TABS,
      topology: TOPOLOGY,
      identity: { endpoint: "local", session: "default", connection_generation: 1, boot_id: "b1" },
      agentsIdentity: { endpoint: "local", session: "default", connection_generation: 1, boot_id: "b1" },
      tabFocus: { tab_id: "t1", endpoint: "local", session: "default", connection_generation: 1, boot_id: "b1" },
      capabilities: { split: true, create_tab: true, start_agent: true, focus: true, focus_tab: true, close_tab: true, rename_tab: true, zoom: true },
      kinds: ["claude", "codex"],
      start: { paneId: "w1:p1", kind: "claude", name: "", busy: false },
      attentionErrors: {},
      prompts: {},
      serverVersion: "0.9.0",
    },
    connections: null,
    navigator: { snapshot: SNAPSHOT },
    selectedEndpoint: "local",
    activeProject: null,
    hostLabels: { local: "Este computador" },
    branch: "main",
    activity: "projects",
    view: "terminal",
    sidebarOpen: true,
    agentsOpen: true,
    controllers: {
      surface: { select: async () => undefined },
      agents: { target: () => null, openAttention: noop, focusTab: noop, editStart: noop, start: async () => undefined },
      projects: { open: noop, openFolder: async () => undefined, focusWorkspace: async () => undefined },
      connections: { openDialog: noop },
    },
    actions: {
      showProjects: noop,
      openConnections: noop,
      paste: noop,
      toggleProjects: noop,
      toggleAgents: noop,
      toggleFiles: noop,
      openPalette: noop,
      newAgent: noop,
      split: noop,
      newTab: noop,
      reconnect: noop,
    },
    unavailable: { split: null, newTab: null, newAgent: null },
  } as unknown as FrameContext;
}

/**
 * Two hosts whose `phase_label` is the engine's own Portuguese: what a server card says must come
 * from the product's phase table (`phaseText`, 067), never from that field.
 */
const PHASE_HOSTS = [
  { endpoint: "local", label: "", phase: "online", phase_label: "conectado", panes: [], server_version: "0.9.0", target: null, latency_ms: null },
  { endpoint: "dev-box", label: "dev-box", phase: "attention", phase_label: "precisa de atenção", panes: [], target: "user@dev-box", latency_ms: null },
] as unknown as HostDto[];

const homeBridge = {
  systemUser: async () => ({ user: "Fábio" }),
  paneRead: async () => ({ pane_id: "w1:p1", text: "", revision: 1, truncated: false }),
};

function menuContext(over: Partial<MenuContext> = {}): MenuContext {
  const noop = () => {};
  return {
    hostSelected: true,
    hostOnline: true,
    canInput: true,
    canRetry: true,
    caps: { split: true, create_tab: true, start_agent: true },
    platform: "linux",
    actions: {
      showProjects: noop,
      openConnections: noop,
      paste: noop,
      toggleProjects: noop,
      toggleAgents: noop,
      toggleFiles: noop,
      openPalette: noop,
      newAgent: noop,
      split: noop,
      newTab: noop,
      reconnect: noop,
    },
    ...over,
  };
}

function paletteSource(): PaletteSource {
  const noop = () => {};
  return {
    hasSession: true,
    projects: [{ id: "p1", label: "erp-api", root: "/work/erp-api", host: "local" }],
    panes: [{ pane_id: "w1:p1", focused: true, width: 80, height: 24 }],
    agents: [{ pane_id: "w1:p2", name: "codex", kind: "codex", status: "working" }],
    commands: [{ id: "split", label: t("frame.menu.split"), shortcut: null, run: noop }],
    openProject: noop,
    focusPane: noop,
    openAgent: noop,
  };
}

/** The main menu as a user reads it: every menu, item, shortcut and disabled reason. */
function renderMainMenu(menus: Menu[]): HTMLElement {
  const root = document.createElement("div");
  root.setAttribute("aria-label", t("frame.menu.file"));
  for (const menu of menus) {
    const section = document.createElement("section");
    section.setAttribute("aria-label", menu.label);
    const heading = document.createElement("h2");
    heading.textContent = menu.label;
    section.append(heading);
    for (const item of menu.items) {
      const button = document.createElement("button");
      button.textContent = item.shortcut ? `${item.label} ${item.shortcut}` : item.label;
      if (item.reason) button.title = item.reason;
      section.append(button);
    }
    root.append(section);
  }
  document.body.append(root);
  mounted.push({ host: root, app: null });
  return root;
}

/** Every region of the spec, rendered in `language`, as `[name, root]` pairs. */
function renderRegions(language: Locale): [string, Element][] {
  setLocalePreference(language);
  const ctx = frameContext();
  return [
    ["TitleBar", render(TitleBar, { host: null, searchHint: "Ctrl K", onOpenPalette: () => {}, onHost: () => {}, ctx, waitingCount: 2, windowApi: { minimize: async () => {}, maximize: async () => {}, close: async () => {} } })],
    [
      "StatusBar",
      render(StatusBar, {
        phase: "live",
        diagnostics: "local/default",
        model: statusBarModel({ phase: "live", hasEndpoint: true, serverVersion: "0.9.0", branch: "main", panes: 1, tabs: 2, agents: 8, cwd: "/work/erp-api" }),
        host: "local",
        session: "default",
        blocked: null,
        readOnly: true,
        notice: null,
      }),
    ],
    ["CommandPalette", render(CommandPalette, { sections: (query: string) => paletteSections(paletteSource(), query), onRun: () => {}, onClose: () => {} })],
    ["main menu", renderMainMenu(buildMenus(menuContext()))],
    [
      "pane menu",
      render(ContextMenu, {
        items: paneContextMenuItems({ paneId: "w1:p1", hasManualLabel: true, swapSourcePaneId: "w1:p2", rightClickPassthrough: false, zoomed: true }),
        x: 10,
        y: 10,
        onselect: () => {},
        onclose: () => {},
      }),
    ],
    ["AgentPanel", render(AgentPanel, { ctx })],
    ["HomeScreen", render(HomeScreen, { ctx, bridge: homeBridge })],
  ];
}

// --- AC-069-01: English ------------------------------------------------------------------------

describe("AC-069-01 inglês", () => {
  // Would catch: any of the seven regions still printing a Portuguese label, tooltip or
  // accessible name when the product runs in English.
  it("renders the whole frame, centre, agents and home with nothing left in Portuguese", () => {
    for (const [name, root] of renderRegions("en")) {
      expect([name, leftovers(root)]).toEqual([name, []]);
    }
  });

  // Would catch: a count glued together in Portuguese wording, or a singular/plural taken from
  // the Portuguese rules instead of the language's own.
  it("counts panes, tabs and active agents in English", () => {
    setLocalePreference("en");
    const model = statusBarModel({ phase: "live", hasEndpoint: true, serverVersion: "0.9.0", branch: "main", panes: 1, tabs: 2, agents: 8 });
    expect(model.counts).toBe("1 pane · 2 tabs");
    expect(model.agents).toBe("8 active agents");
    expect(statusBarModel({ phase: "live", hasEndpoint: true, serverVersion: "0.9.0", branch: "main", panes: 3, tabs: 1, agents: 1 }).agents).toBe("1 active agent");
    expect(model.server).toBe("herdr server 0.9.0 · connected");
    // The scrollback badge of a pane (`↑ N linhas`) counts in the language too.
    expect([t("center.pane.scrollLines", { count: 1 }), t("center.pane.scrollLines", { count: 7 })]).toEqual(["1 line", "7 lines"]);
  });

  // Would catch: a server card showing the engine's `phase_label` instead of the product's own
  // words for the phase.
  it("names a server's state from the phase table in English", () => {
    setLocalePreference("en");
    expect(buildServerCards(PHASE_HOSTS, "local").map((card) => card.state)).toEqual(["Online", "Needs attention"]);
  });

  // Would catch: a greeting fixed on one period, or the Portuguese one shown in English.
  it("greets by the hour in English", () => {
    setLocalePreference("en");
    expect(greetingFor(9, "Fábio")).toBe("Good morning, Fábio");
    expect(greetingFor(15, "Fábio")).toBe("Good afternoon, Fábio");
    expect(greetingFor(21, "Fábio")).toBe("Good evening, Fábio");
  });
});

// --- AC-069-02: Spanish ------------------------------------------------------------------------

describe("AC-069-02 espanhol", () => {
  // Would catch: Spanish falling back to the Portuguese text of a region instead of its own.
  it("renders the same regions in Spanish with no Portuguese left", () => {
    // Sanity check of the detector itself: it must recognise the Portuguese it looks for.
    expect(portugueseWordings()).toContain("Precisa da sua atenção");
    for (const [name, root] of renderRegions("es")) {
      expect([name, portugueseIn(root)]).toEqual([name, []]);
    }
  });

  // Would catch: `panel`/`pestaña` pluralised by the Portuguese rules, or the counters copied
  // from Portuguese.
  it("counts panes, tabs and active agents in Spanish", () => {
    setLocalePreference("es");
    const model = statusBarModel({ phase: "live", hasEndpoint: true, serverVersion: "0.9.0", branch: "main", panes: 1, tabs: 2, agents: 8 });
    expect(model.counts).toBe("1 panel · 2 pestañas");
    expect(model.agents).toBe("8 agentes activos");
    expect(statusBarModel({ phase: "live", hasEndpoint: true, serverVersion: "0.9.0", branch: "main", panes: 1, tabs: 1, agents: 1 }).agents).toBe("1 agente activo");
    expect([t("center.pane.scrollLines", { count: 1 }), t("center.pane.scrollLines", { count: 7 })]).toEqual(["1 línea", "7 líneas"]);
  });

  // Would catch: Spanish server cards falling back to the engine's Portuguese `phase_label`.
  it("names a server's state from the phase table in Spanish", () => {
    setLocalePreference("es");
    expect(buildServerCards(PHASE_HOSTS, "local").map((card) => card.state)).toEqual(["En línea", "Necesita atención"]);
  });

  it("greets by the hour in Spanish", () => {
    setLocalePreference("es");
    expect(greetingFor(9, "Fábio")).toBe("Buenos días, Fábio");
    expect(greetingFor(15, "Fábio")).toBe("Buenas tardes, Fábio");
    expect(greetingFor(21, "Fábio")).toBe("Buenas noches, Fábio");
  });
});

// --- AC-069-03: the language selector -----------------------------------------------------------

describe("AC-069-03 seletor de idioma", () => {
  /** The palette as Ctrl K opens it: the window's own sources, and the owner running the choice. */
  function openPalette(): HTMLElement {
    return render(CommandPalette, {
      sections: (query: string) => paletteSections(paletteSource(), query),
      // What `runEntry` of App.svelte does with the chosen entry.
      onRun: (entry: PaletteEntry) => entry.run(),
      onClose: () => {},
    });
  }

  /** Types `text` in the palette's field, the way the user filters it. */
  function search(palette: HTMLElement, text: string): void {
    const field = palette.querySelector<HTMLInputElement>("[data-palette] input")!;
    field.value = text;
    field.dispatchEvent(new Event("input", { bubbles: true }));
    flushSync();
  }

  const group = (palette: HTMLElement) => palette.querySelector<HTMLElement>('[data-palette-section="language"]')!;
  const groupTitle = (palette: HTMLElement) => group(palette).querySelector("[data-section-title]")!.textContent!.trim();
  const options = (palette: HTMLElement) => Array.from(group(palette).querySelectorAll<HTMLButtonElement>("[data-entry-id]"));
  const option = (palette: HTMLElement, id: LocalePreference) => group(palette).querySelector<HTMLButtonElement>(`[data-entry-id="language:${id}"]`)!;

  // Would catch: the selector missing from the surface Ctrl K opens, an option in the wrong order,
  // a group named something else, or a language named in the interface's language instead of its own.
  it("lists the Idioma/Language group with Automático, English, Português and Español", () => {
    for (const [language, title, auto] of [
      ["en", "Language", "Automatic"],
      ["pt", "Idioma", "Automático"],
      ["es", "Idioma", "Automático"],
    ] as const) {
      setLocalePreference(language);
      const palette = openPalette();
      expect([language, groupTitle(palette)]).toEqual([language, title]);
      expect([language, options(palette).map((button) => button.dataset.entryId)]).toEqual([
        language,
        ["language:auto", "language:en", "language:pt", "language:es"],
      ]);
      expect([language, options(palette).map((button) => button.querySelector("[data-label]")!.textContent!.trim())]).toEqual([
        language,
        [auto, "English", "Português", "Español"],
      ]);
    }
  });

  // Would catch: a group the user cannot reach by searching for it, or a filter that only reads the
  // section title (which the query never matches).
  it("is found by searching Idioma or Language, and the other groups empty out", () => {
    for (const [language, word] of [["pt", "Idioma"], ["en", "Language"], ["es", "idioma"]] as const) {
      setLocalePreference(language);
      const palette = openPalette();
      search(palette, word);
      expect([language, options(palette).length]).toEqual([language, 4]);
      const others = Array.from(palette.querySelectorAll<HTMLElement>("[data-palette-section]")).filter((section) => section.dataset.paletteSection !== "language");
      expect([language, others.flatMap((section) => Array.from(section.querySelectorAll("[data-entry-id]")))]).toEqual([language, []]);
    }
  });

  // Would catch: an option that does not reach `setLocalePreference`, or one that changes the
  // language without recording the choice.
  it("each option chosen in the palette records its preference and persists it", () => {
    for (const choice of ["en", "pt", "es", "auto"] as LocalePreference[]) {
      setLocalePreference("pt");
      const palette = openPalette();
      search(palette, "Idioma");
      option(palette, choice).click();
      flushSync();
      expect([choice, localePreference()]).toEqual([choice, choice]);
      expect([choice, localStorage.getItem(LOCALE_STORAGE_KEY)]).toEqual([choice, choice]);
    }
  });

  // Would catch: an option marked by position instead of by the preference in force, nothing marked
  // at all, or a mark with no visible sign.
  it("marks the option in force, `Automático` included", () => {
    const checked = (palette: HTMLElement) => options(palette).filter((button) => button.getAttribute("aria-checked") === "true").map((button) => button.dataset.entryId);
    for (const choice of ["auto", "es", "pt"] as LocalePreference[]) {
      setLocalePreference(choice);
      const palette = openPalette();
      expect([choice, checked(palette)]).toEqual([choice, [`language:${choice}`]]);
      expect([choice, option(palette, choice).querySelector("[data-entry-check]") !== null]).toEqual([choice, true]);
      expect([choice, options(palette).filter((button) => button.querySelector("[data-entry-check]")).length]).toEqual([choice, 1]);
    }
  });

  // Would catch: an interface that needs a reload — a component keeping the language it was
  // mounted with — or a palette that does not follow the language it has just set.
  it("re-renders a mounted region and the palette itself in the chosen language, with no reload", () => {
    setLocalePreference("pt");
    const ctx = frameContext();
    const host = render(AgentPanel, { ctx });
    const palette = openPalette();
    expect(host.textContent).toContain("Precisa da sua atenção");

    option(palette, "en").click();
    flushSync();
    expect(locale()).toBe("en");
    expect(host.textContent).toContain("Needs your attention");
    expect(host.textContent).not.toContain("Precisa da sua atenção");
    expect(groupTitle(palette)).toBe("Language");
    expect(option(palette, "en").getAttribute("aria-checked")).toBe("true");
    expect(option(palette, "pt").getAttribute("aria-checked")).toBe("false");

    option(palette, "es").click();
    flushSync();
    expect(host.textContent).toContain("Necesita tu atención");
  });

  // Would catch: the group built but never mounted — the palette Ctrl K opens is App.svelte's, and
  // it is the owner that runs the chosen entry. Read-only probe: App.svelte belongs to another spec.
  it("is reached by Ctrl K: App mounts the palette and runs the chosen entry", () => {
    const app = (import.meta.glob("../../App.svelte", { query: "?raw", import: "default", eager: true }) as Record<string, string>)["../../App.svelte"]!;
    expect(app).toMatch(/<CommandPalette\s+sections=\{paletteFor\}\s+onRun=\{runEntry\}/);
    expect(app).toMatch(/function runEntry\(entry: PaletteEntry\) \{[^}]*entry\.run\(\);/);
    expect(app).toMatch(/if \(id === "palette"\) \{[\s\S]*?openPalette\(\);/);
  });
});
