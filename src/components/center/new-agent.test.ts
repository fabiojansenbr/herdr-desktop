// @vitest-environment happy-dom
// Spec 040 AC-040-01 (replaces Spec 017 AC-017-03): Novo agente is removed from the bar;
// agents start from +/shell.
import { mount, unmount, flushSync } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createAgentsController } from "../../agents/controller";
import { createFakeAgentsBridge } from "../../agents/fake-bridge";
import type { FrameContext } from "../../shell/frame-context";
import type { SurfaceState } from "../../shell/controller";
import type { MenuAction } from "../frame/menus";
import TitleBar from "../frame/TitleBar.svelte";

const GEOMETRY = { cols: 120, rows: 40, cell_width_px: 9, cell_height_px: 18 };
const hosts: { el: HTMLElement; app: ReturnType<typeof mount> }[] = [];

const ACTIONS = Object.fromEntries(
  ["showProjects", "openConnections", "paste", "toggleProjects", "toggleAgents", "toggleFiles", "openPalette", "newAgent", "split", "newTab", "reconnect"].map(
    (name) => [name, () => {}],
  ),
) as Record<MenuAction, () => void>;

function liveSurface(over: Partial<SurfaceState["selection"]> & { phase?: SurfaceState["phase"] } = {}): SurfaceState {
  const phase = over.phase ?? "live";
  const { phase: _p, ...selection } = over as { phase?: SurfaceState["phase"] } & Partial<NonNullable<SurfaceState["selection"]>>;
  return {
    selection: {
      endpoint: "local",
      kind: "local",
      label: "Local",
      session: "hd017",
      online: true,
      identity: null,
      ...selection,
    },
    status: { session: "hd017" } as SurfaceState["status"],
    phase,
    reason: null,
    error: null,
    identity: {
      endpoint: "local",
      session: "hd017",
      connection_generation: 1,
      boot_id: "boot-017",
      pane_id: "w1:p2",
    },
    surfaceKey: 1,
    busy: false,
  };
}

async function renderHeader(opts: { local?: boolean; agents?: [string, "idle" | "working"][] } = {}) {
  const local = opts.local !== false;
  const bridge = createFakeAgentsBridge({
    bootId: "boot-017",
    generation: 1,
    session: "hd017",
    kinds: ["claude", "codex", "opencode", "grok", "pi"],
    agents: opts.agents ?? [],
  });
  const controller = createAgentsController(bridge);
  await controller.connect(GEOMETRY);
  bridge.calls.length = 0;
  const surface = liveSurface(local ? {} : { endpoint: "dev-box", kind: "ssh", label: "dev-box", phase: "live" });
  if (!local) {
    surface.identity = {
      endpoint: "dev-box",
      session: "hd017",
      connection_generation: 1,
      boot_id: "boot-017",
      pane_id: "w1:p2",
    };
  }
  const ctx: FrameContext = {
    surface,
    agents: controller.state,
    connections: null,
    navigator: null,
    selectedEndpoint: local ? "local" : "dev-box",
    activeProject: null,
    hostLabels: {},
    branch: "master",
    activity: "projects",
    view: "terminal",
    sidebarOpen: true,
    agentsOpen: true,
    controllers: {
      surface: {} as FrameContext["controllers"]["surface"],
      agents: controller,
      projects: {} as FrameContext["controllers"]["projects"],
      connections: {} as FrameContext["controllers"]["connections"],
    },
    actions: ACTIONS,
    unavailable: { split: null, newTab: null, newAgent: null },
  };
  const el = document.createElement("div");
  document.body.appendChild(el);
  const app = mount(TitleBar, {
    target: el,
    props: {
      host: { label: local ? "Este computador" : "dev-box", kind: local ? "Local" : "SSH", state: "conectado", tone: "live" },
      searchHint: "Ctrl K",
      onOpenPalette: () => {},
      onHost: () => {},
      ctx,
      projectLabel: "sessão hd017 › w1",
    },
  });
  flushSync();
  hosts.push({ el, app });
  return { el, bridge, controller };
}

afterEach(() => {
  for (const host of hosts.splice(0)) {
    unmount(host.app);
    host.el.remove();
  }
});

describe("AC-040-01 (replaces AC-017-03) Novo agente absent from topbar", () => {
  it("does not render Novo agente button or popover on a live Local host", async () => {
    const { el } = await renderHeader();
    expect(el.querySelector('[data-action="newAgent"]')).toBeNull();
    expect(el.querySelector('[data-topbar-item="new-agent"]')).toBeNull();
    expect(el.querySelector("[data-new-agent-popover]")).toBeNull();
    expect(el.textContent).not.toContain("Novo agente");
  });

  it("does not render Novo agente button without a Local host", async () => {
    const { el } = await renderHeader({ local: false });
    expect(el.querySelector('[data-action="newAgent"]')).toBeNull();
    expect(el.querySelector('[data-topbar-item="new-agent"]')).toBeNull();
    expect(el.querySelector("[data-new-agent-popover]")).toBeNull();
    expect(el.textContent).not.toContain("Novo agente");
  });

  // Spec 047 AC-047-04: the search and the host pill moved into the sidebar; spec 049 AC-049-03
  // takes the panel button out too. Spec 055 AC-055-01 takes the breadcrumb out as well — the
  // session label it fell back to is gone, and the bar carries the workspace tabs instead.
  it("shows the workspace tabs and carries no panel, search, host control or session label", async () => {
    const { el } = await renderHeader();
    expect(el.querySelector("[data-center-tabs]"), "the bar draws the tab strip").toBeTruthy();
    expect(el.textContent).not.toMatch(/sessão hd017/);
    expect(el.querySelector('[data-topbar-item="project"]')).toBeNull();
    expect(el.querySelector('[data-topbar-item="sidebar"]')).toBeNull();
    expect(el.querySelector('[data-topbar-item="search"]')).toBeNull();
    expect(el.querySelector('[data-topbar-item="host"]')).toBeNull();
  });
});

// ---------------------------------------------------------------------------------------------
// Spec 073 — the popover is a searchable list (no native `<select>`), installed agents first.
// Would catch: the GTK-themed `<select>` coming back, a section out of order, an unavailable
// agent hidden instead of dimmed, the search ignoring the type, a row that does not start the
// agent, keyboard selection missing, or the recent list growing past three entries.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { compile } from "svelte/compiler";
import { errorText, t, setLocalePreference } from "../../i18n/index.svelte";
import type { AgentStatus, QualifiedTarget, RuntimeError } from "../../agents/types";
import { agentName } from "./model";
import { untranslated } from "../../i18n/testing";
import NewAgentPopover from "./NewAgentPopover.svelte";
import {
  AGENT_AUTONOMY_KEY,
  agentChoices,
  agentLabel,
  autonomyHint,
  MAX_RECENT_AGENTS,
  RECENT_AGENTS_KEY,
  readAgentAutonomy,
  readRecentAgents,
  rememberAgentAutonomy,
  rememberRecentAgent,
  SHELL_KIND,
} from "./new-agent-model";

const injected: HTMLStyleElement[] = [];

function injectPopoverStyle() {
  const filename = resolve("src/components/center/NewAgentPopover.svelte");
  const source = readFileSync(filename, "utf8");
  const style = document.createElement("style");
  style.textContent = compile(source, { filename, css: "external", dev: true }).css!.code;
  document.head.appendChild(style);
  injected.push(style);
}

afterEach(() => {
  for (const style of injected.splice(0)) style.remove();
  document.documentElement.removeAttribute("style");
  try {
    localStorage.removeItem(RECENT_AGENTS_KEY);
    localStorage.removeItem(AGENT_AUTONOMY_KEY);
  } catch {
    // no storage in this environment
  }
  setLocalePreference("pt");
});

async function renderPopover(
  opts: {
    kinds?: string[];
    available?: string[] | "fail";
    recent?: string[];
    local?: boolean;
    /** Agents already running, so the focused pane can be occupied (spec 075). */
    agents?: [string, AgentStatus][];
    /** Answer of `split`, which never updates the local topology (spec 075 AC-075-02). */
    split?: string | null | "reject";
    /** Refusal `startAgent` throws instead of starting (spec 075 AC-075-03). */
    failStart?: RuntimeError;
    /** Server that announces the kinds but not `agent.start` (spec 075 AC-075-03). */
    noStart?: boolean;
    /** Stored answer of the auto mode switch before the popover opens (spec 076). */
    autonomy?: "on" | "off";
    /** The autonomy read fails, like a host that does not answer it (spec 076). */
    flagsFail?: boolean;
  } = {},
) {
  const kinds = opts.kinds ?? ["claude", "codex"];
  localStorage.removeItem(RECENT_AGENTS_KEY);
  localStorage.removeItem(AGENT_AUTONOMY_KEY);
  if (opts.autonomy) localStorage.setItem(AGENT_AUTONOMY_KEY, opts.autonomy);
  if (opts.recent) localStorage.setItem(RECENT_AGENTS_KEY, JSON.stringify(opts.recent));
  const bridge = createFakeAgentsBridge({ bootId: "boot-073", generation: 1, session: "hd073", kinds, agents: opts.agents ?? [] });
  if (opts.noStart) {
    const connect = bridge.connect;
    bridge.connect = async (geometry, onEvent) => {
      const overview = await connect(geometry, onEvent);
      return { ...overview, kinds, capabilities: { ...overview.capabilities!, start_agent: false } };
    };
  }
  if (opts.split !== undefined) {
    bridge.split = async (target, direction) => {
      bridge.calls.push({ command: "pane_split", args: structuredClone({ target, direction }) });
      if (opts.split === "reject") throw { code: "pane_split_failed", message: "sem espaço", retryable: false } satisfies RuntimeError;
      // The engine answers before its frame arrives: the topology here stays behind on purpose.
      return opts.split;
    };
  }
  if (opts.failStart) {
    bridge.startAgent = async (target, kind, name, autonomous) => {
      bridge.calls.push({ command: "agent_start", args: structuredClone({ target, kind, name, autonomous: autonomous ?? false }) });
      throw opts.failStart;
    };
  }
  const controller = createAgentsController(bridge);
  await controller.connect(GEOMETRY);
  bridge.calls.length = 0;
  const local = opts.local !== false;
  const asked: string[][] = [];
  const askedFlags: string[][] = [];
  let closes = 0;
  const ctx: FrameContext = {
    surface: liveSurface(local ? {} : { endpoint: "dev-box", kind: "ssh", label: "dev-box" }),
    agents: controller.state,
    connections: null,
    navigator: null,
    selectedEndpoint: local ? "local" : "dev-box",
    activeProject: null,
    hostLabels: {},
    branch: null,
    activity: "projects",
    view: "terminal",
    sidebarOpen: true,
    agentsOpen: false,
    controllers: {
      surface: {} as FrameContext["controllers"]["surface"],
      agents: controller,
      projects: {} as FrameContext["controllers"]["projects"],
      connections: {} as FrameContext["controllers"]["connections"],
    },
    actions: ACTIONS,
    unavailable: { split: null, newTab: null, newAgent: null },
  };
  injectPopoverStyle();
  document.documentElement.style.setProperty("--surface-3", "rgb(36, 36, 36)");
  const el = document.createElement("div");
  document.body.appendChild(el);
  const app = mount(NewAgentPopover, {
    target: el,
    props: {
      ctx,
      onclose: () => {
        closes += 1;
      },
      probeAvailable: async (list: string[]) => {
        asked.push([...list]);
        if (opts.available === "fail") throw new Error("no host");
        return opts.available ?? list;
      },
      probeAutonomy: async (list: string[]) => {
        askedFlags.push([...list]);
        if (opts.flagsFail) throw new Error("no host");
        return list.map((kind) => ({ kind, flags: FLAGS[kind] ?? [] }));
      },
    },
  });
  hosts.push({ el, app });
  flushSync();
  await Promise.resolve();
  await Promise.resolve();
  flushSync();
  return { el, bridge, controller, asked, askedFlags, closes: () => closes };
}

/**
 * Flags the backend table answers for the kinds under test (spec 076); the table itself is fixed
 * by the Rust test, this only feeds the popover's read.
 */
const FLAGS: Record<string, string[]> = {
  claude: ["--dangerously-skip-permissions"],
  codex: ["--dangerously-bypass-approvals-and-sandbox"],
  cursor: ["--yolo", "--trust"],
  gemini: ["--yolo"],
  agy: ["--dangerously-skip-permissions"],
  pi: ["--approve"],
  devin: [],
};

const rowsOf = (el: HTMLElement) => Array.from(el.querySelectorAll<HTMLButtonElement>("[data-agent-kind]"));
const kindsOf = (el: HTMLElement) => rowsOf(el).map((row) => row.dataset.agentKind);
const labelsOf = (el: HTMLElement) => rowsOf(el).map((row) => row.querySelector("[data-agent-label]")!.textContent);
const headersOf = (el: HTMLElement) =>
  Array.from(el.querySelectorAll("[data-agent-section-title]")).map((node) => node.textContent);

function type(el: HTMLElement, value: string) {
  const search = el.querySelector<HTMLInputElement>("[data-agent-search]")!;
  search.value = value;
  search.dispatchEvent(new Event("input", { bubbles: true }));
  flushSync();
}

function press(el: HTMLElement, key: string) {
  el.querySelector("[data-new-agent-popover]")!.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
  flushSync();
}

describe("AC-073-02 the popover is a searchable list with installed agents first", () => {
  it("drops the native select, focuses the search and orders Recent, All agents, Not installed, Shell", async () => {
    const { el } = await renderPopover({
      kinds: ["pi", "claude", "codex", "gemini", "agy"],
      available: ["claude", "codex", "agy"],
      recent: ["codex"],
    });
    expect(el.querySelector("select"), "the GTK-themed native select is gone").toBeNull();
    const search = el.querySelector<HTMLInputElement>("[data-agent-search]")!;
    expect(search, "search field").toBeTruthy();
    expect(document.activeElement, "the search field takes focus on open").toBe(search);

    expect(headersOf(el)).toEqual([
      t("center.newAgent.recent"),
      t("center.newAgent.all"),
      t("center.newAgent.notInstalled"),
    ]);
    expect(kindsOf(el)).toEqual(["codex", "agy", "claude", "gemini", "pi", SHELL_KIND]);
    expect(labelsOf(el)).toEqual(["Codex", "Antigravity", "Claude Code", "Gemini", "Pi", "Shell"]);
    expect(rowsOf(el).map((row) => row.querySelector("[data-agent-tile]")!.textContent)).toEqual([
      "C",
      "A",
      "C",
      "G",
      "P",
      "S",
    ]);

    const notInstalled = rowsOf(el).filter((row) => row.dataset.agentInstalled === "false");
    expect(notInstalled.map((row) => row.dataset.agentKind)).toEqual(["gemini", "pi"]);
    for (const row of notInstalled) {
      expect(row.classList.contains("dim"), "not installed rows are dimmed").toBe(true);
      expect(row.disabled, "and still clickable").toBe(false);
    }

    const divider = el.querySelector("[data-agent-divider]")!;
    const shell = rowsOf(el).at(-1)!;
    expect(divider, "divider before Shell").toBeTruthy();
    expect(divider.compareDocumentPosition(shell) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("searches the display name and the type, case-insensitively, and says when nothing matches", async () => {
    const { el } = await renderPopover({
      kinds: ["pi", "claude", "codex", "gemini", "agy"],
      available: ["claude", "codex", "agy"],
      recent: ["codex"],
    });
    type(el, "cla");
    expect(labelsOf(el)).toEqual(["Claude Code"]);
    type(el, "AGY");
    expect(labelsOf(el)).toEqual(["Antigravity"]);
    expect(el.querySelector("[data-agent-empty]")).toBeNull();
    type(el, "zzz");
    expect(rowsOf(el)).toHaveLength(0);
    expect(el.querySelector("[data-agent-empty]")!.textContent).toBe(t("center.newAgent.noMatch"));
  });

  it("asks the host only for the Local list and shows no Not installed section otherwise", async () => {
    const remote = await renderPopover({ kinds: ["claude", "codex"], local: false });
    expect(remote.asked, "an SSH host is never asked about local binaries").toEqual([]);
    expect(headersOf(remote.el)).toEqual([t("center.newAgent.all")]);
    expect(kindsOf(remote.el)).toEqual(["claude", "codex", SHELL_KIND]);

    const failed = await renderPopover({ kinds: ["claude", "codex"], available: "fail" });
    expect(failed.asked).toEqual([["claude", "codex"]]);
    expect(headersOf(failed.el), "a failed probe dims nothing").toEqual([t("center.newAgent.all")]);
    expect(kindsOf(failed.el)).toEqual(["claude", "codex", SHELL_KIND]);
  });
});

describe("AC-073-03 choosing and starting from the list", () => {
  it("starts the clicked agent with the optional name, records it as recent and closes", async () => {
    const { el, bridge, closes } = await renderPopover({ kinds: ["claude", "codex"] });
    const name = el.querySelector<HTMLInputElement>('[data-new-agent-popover] input[data-agent-name]')!;
    name.value = "revisor";
    name.dispatchEvent(new Event("input", { bubbles: true }));
    flushSync();
    rowsOf(el).find((row) => row.dataset.agentKind === "codex")!.click();
    await vi.waitFor(() => {
      expect(bridge.calls.filter((c) => c.command === "agent_start")).toHaveLength(1);
      expect(closes()).toBe(1);
    });
    const call = bridge.calls.find((c) => c.command === "agent_start")!;
    expect(call.args.kind).toBe("codex");
    expect(call.args.name).toBe("revisor");
    expect(readRecentAgents()).toEqual(["codex"]);
  });

  it("moves the active row with the arrows, starts it on Enter and closes on Esc without starting", async () => {
    const { el, bridge, closes } = await renderPopover({ kinds: ["claude", "codex"] });
    const active = () => el.querySelector<HTMLElement>("[data-agent-active]")!;
    expect(active().dataset.agentKind).toBe("claude");
    expect(getComputedStyle(active()).backgroundColor, "the active row is --surface-3").toBe("rgb(36, 36, 36)");
    press(el, "ArrowDown");
    expect(active().dataset.agentKind).toBe("codex");
    press(el, "ArrowDown");
    expect(active().dataset.agentKind).toBe(SHELL_KIND);
    press(el, "ArrowUp");
    expect(active().dataset.agentKind).toBe("codex");
    press(el, "Enter");
    await vi.waitFor(() => {
      expect(bridge.calls.filter((c) => c.command === "agent_start")).toHaveLength(1);
      expect(closes()).toBe(1);
    });
    expect(bridge.calls.find((c) => c.command === "agent_start")!.args.kind).toBe("codex");

    const escaped = await renderPopover({ kinds: ["claude", "codex"] });
    press(escaped.el, "Escape");
    expect(escaped.closes()).toBe(1);
    expect(escaped.bridge.calls.filter((c) => c.command === "agent_start")).toHaveLength(0);
    expect(escaped.bridge.calls.filter((c) => c.command === "tab_create")).toHaveLength(0);
  });

  it("starts Shell through the engine tab and keeps the recent list at three entries without repeats", async () => {
    const { el, bridge } = await renderPopover({ kinds: ["claude", "codex"] });
    rowsOf(el).find((row) => row.dataset.agentKind === SHELL_KIND)!.click();
    await vi.waitFor(() => {
      expect(bridge.calls.filter((c) => c.command === "tab_create")).toHaveLength(1);
    });

    localStorage.removeItem(RECENT_AGENTS_KEY);
    expect(rememberRecentAgent("claude")).toEqual(["claude"]);
    expect(rememberRecentAgent("codex")).toEqual(["codex", "claude"]);
    expect(rememberRecentAgent("claude")).toEqual(["claude", "codex"]);
    expect(rememberRecentAgent("agy")).toEqual(["agy", "claude", "codex"]);
    expect(rememberRecentAgent("pi")).toEqual(["pi", "agy", "claude"]);
    expect(MAX_RECENT_AGENTS).toBe(3);
    expect(readRecentAgents()).toEqual(["pi", "agy", "claude"]);
  });

  it("has no Recent section when localStorage is unavailable", async () => {
    const disabled = () => {
      throw new Error("storage disabled");
    };
    const getItem = vi.spyOn(window.localStorage, "getItem").mockImplementation(disabled);
    const setItem = vi.spyOn(window.localStorage, "setItem").mockImplementation(disabled);
    try {
      expect(readRecentAgents()).toEqual([]);
      expect(rememberRecentAgent("claude")).toEqual(["claude"]);
      expect(
        agentChoices({ kinds: ["claude", "codex"], available: null, recent: readRecentAgents() }).map((c) => c.section),
      ).toEqual(["all", "all", "shell"]);
    } finally {
      getItem.mockRestore();
      setItem.mockRestore();
    }
  });

  it("writes every visible text through t(): English leaves nothing Portuguese", async () => {
    setLocalePreference("en");
    const { el } = await renderPopover({
      kinds: ["pi", "claude", "codex", "gemini", "agy"],
      available: ["claude", "codex", "agy"],
      recent: ["codex"],
    });
    expect(untranslated(el.querySelector("[data-new-agent-popover]")!)).toEqual([]);
    expect(agentLabel("claude")).toBe("Claude Code");
    expect(agentLabel("opencode")).toBe("OpenCode");
    expect(agentLabel("hermes")).toBe("Hermes");
  });
});

// ---------------------------------------------------------------------------------------------
// Spec 075 — the chosen agent actually starts, in the pane the split created.
// Would catch: the popup reading the new pane from a topology the split has not reached yet (so
// `agent.start` is aimed at the occupied pane and the controller drops it in silence), a start
// that cannot happen closing the popup with nothing said, or an untranslated failure message.

const targetOf = (call: { args: Record<string, unknown> }) => call.args.target as QualifiedTarget;
const startsOf = (bridge: { calls: { command: string; args: Record<string, unknown> }[] }) =>
  bridge.calls.filter((c) => c.command === "agent_start");
const errorOf = (el: HTMLElement) => el.querySelector("[data-agent-error]")?.textContent ?? null;

describe("AC-075-02 starts in the pane the split answered, with the topology still behind", () => {
  it("aims agent.start at the created pane, names it after that pane and closes", async () => {
    const { el, bridge, closes, controller } = await renderPopover({
      kinds: ["claude", "codex"],
      agents: [
        ["w1:p1", "idle"],
        ["w1:p2", "idle"],
      ],
      split: "w1:p9",
    });
    rowsOf(el).find((row) => row.dataset.agentKind === "claude")!.click();
    await vi.waitFor(() => {
      expect(startsOf(bridge)).toHaveLength(1);
      expect(closes()).toBe(1);
    });
    expect(bridge.calls.filter((c) => c.command === "pane_split")).toHaveLength(1);
    const start = startsOf(bridge)[0]!;
    expect(targetOf(start).pane_id, "the pane the engine created, not the occupied one").toBe("w1:p9");
    expect(controller.state.topology?.panes.map((p) => p.pane_id), "the topology never received it").toEqual(["w1:p1", "w1:p2"]);
    expect(start.args.kind).toBe("claude");
    expect(start.args.name).toBe(agentName("claude", "w1:p9"));
    expect(errorOf(el)).toBeNull();
  });
});

describe("AC-075-03 a start that cannot happen keeps the popup open and says why", () => {
  it("refuses without the start_agent capability, before any split", async () => {
    const { el, bridge, closes } = await renderPopover({ kinds: ["claude", "codex"], noStart: true });
    rowsOf(el).find((row) => row.dataset.agentKind === "claude")!.click();
    await vi.waitFor(() => expect(errorOf(el)).toBe(t("center.newAgent.cannotStart")));
    expect(closes(), "the popup stays open").toBe(0);
    expect(startsOf(bridge)).toHaveLength(0);
    expect(bridge.calls.filter((c) => c.command === "pane_split")).toHaveLength(0);
  });

  it("says so when no pane is free after the split, and starts nothing", async () => {
    for (const split of ["reject", null] as const) {
      const { el, bridge, closes } = await renderPopover({
        kinds: ["claude", "codex"],
        agents: [
          ["w1:p1", "idle"],
          ["w1:p2", "idle"],
        ],
        split,
      });
      rowsOf(el).find((row) => row.dataset.agentKind === "claude")!.click();
      await vi.waitFor(() => expect(errorOf(el)).toBe(t("center.newAgent.noPane")));
      expect(closes()).toBe(0);
      expect(startsOf(bridge), "no agent.start on the occupied pane").toHaveLength(0);
    }
  });

  it("shows the backend refusal through errorText and keeps the popup open", async () => {
    const failure: RuntimeError = { code: "host_offline", message: "engine crua", retryable: true };
    const { el, bridge, closes } = await renderPopover({
      kinds: ["claude", "codex"],
      agents: [
        ["w1:p1", "idle"],
        ["w1:p2", "idle"],
      ],
      split: "w1:p9",
      failStart: failure,
    });
    rowsOf(el).find((row) => row.dataset.agentKind === "claude")!.click();
    await vi.waitFor(() => expect(errorOf(el)).toBe(errorText(failure)));
    expect(errorOf(el), "the code has a dictionary entry: the raw message is not shown").not.toBe(failure.message);
    expect(closes()).toBe(0);
    expect(startsOf(bridge)).toHaveLength(1);
    expect(targetOf(startsOf(bridge)[0]!).pane_id).toBe("w1:p9");
  });

  it("writes the failures through t(), one message per locale", async () => {
    const seen = new Set<string>();
    for (const locale of ["en", "pt", "es"] as const) {
      setLocalePreference(locale);
      const { el } = await renderPopover({ kinds: ["claude", "codex"], noStart: true });
      rowsOf(el).find((row) => row.dataset.agentKind === "claude")!.click();
      await vi.waitFor(() => expect(errorOf(el)).toBe(t("center.newAgent.cannotStart")));
      seen.add(errorOf(el)!);
      // English is the locale whose popover may carry no Portuguese at all (spec 073).
      if (locale === "en") expect(untranslated(el.querySelector("[data-new-agent-popover]")!)).toEqual([]);
    }
    expect(seen.size, "the three dictionaries answer with their own message").toBe(3);
  });
});

// ---------------------------------------------------------------------------------------------
// Spec 076 — every agent starts in its own auto mode, from one switch in the popup.
// Would catch: the switch missing or starting off, the choice not surviving a reopen, a row
// showing a flag the backend table does not have (or none showing at all), the flags still shown
// with the switch off, or the start not carrying the choice the user made.

const autonomySwitch = (el: HTMLElement) => el.querySelector<HTMLInputElement>("[data-agent-autonomy]")!;
const flagsOf = (el: HTMLElement) => rowsOf(el).map((row) => row.querySelector("[data-agent-flags]")?.textContent ?? null);

describe("AC-076-02 the auto mode switch and the flag of each row", () => {
  it("is on by default, names itself and shows each kind's flags, or that it has none", async () => {
    const { el, askedFlags } = await renderPopover({ kinds: ["claude", "cursor", "devin"], available: ["claude", "cursor", "devin"] });
    const toggle = autonomySwitch(el);
    expect(toggle, "the auto mode switch").toBeTruthy();
    expect(toggle.type).toBe("checkbox");
    expect(toggle.checked, "on by default").toBe(true);
    expect(toggle.closest("label")!.textContent).toContain(t("center.newAgent.autonomy"));
    expect(askedFlags, "the table is read once, for the published kinds").toEqual([["claude", "cursor", "devin"]]);
    expect(kindsOf(el)).toEqual(["claude", "cursor", "devin", SHELL_KIND]);
    expect(flagsOf(el)).toEqual([
      "--dangerously-skip-permissions",
      "--yolo --trust",
      t("center.newAgent.noAutonomy"),
      null,
    ]);
  });

  it("remembers the choice in localStorage and shows no flags while it is off", async () => {
    const { el } = await renderPopover({ kinds: ["claude", "devin"] });
    autonomySwitch(el).click();
    flushSync();
    expect(autonomySwitch(el).checked).toBe(false);
    expect(flagsOf(el), "no flag is announced with auto mode off").toEqual([null, null, null]);
    expect(localStorage.getItem(AGENT_AUTONOMY_KEY)).toBe("off");

    const reopened = await renderPopover({ kinds: ["claude", "devin"], autonomy: "off" });
    expect(autonomySwitch(reopened.el).checked, "the switch reopens as the user left it").toBe(false);
    expect(flagsOf(reopened.el)).toEqual([null, null, null]);
    autonomySwitch(reopened.el).click();
    flushSync();
    expect(localStorage.getItem(AGENT_AUTONOMY_KEY)).toBe("on");
    expect(flagsOf(reopened.el)).toEqual(["--dangerously-skip-permissions", t("center.newAgent.noAutonomy"), null]);
  });

  it("starts on when localStorage is unavailable and shows nothing when the read fails", async () => {
    const disabled = () => {
      throw new Error("storage disabled");
    };
    const getItem = vi.spyOn(window.localStorage, "getItem").mockImplementation(disabled);
    const setItem = vi.spyOn(window.localStorage, "setItem").mockImplementation(disabled);
    try {
      expect(readAgentAutonomy(), "no storage: auto mode is on").toBe(true);
      expect(rememberAgentAutonomy(false)).toBe(false);
      expect(readAgentAutonomy()).toBe(true);
    } finally {
      getItem.mockRestore();
      setItem.mockRestore();
    }
    expect(autonomyHint(["--yolo", "--trust"])).toBe("--yolo --trust");
    expect(autonomyHint([])).toBeNull();
    expect(autonomyHint(undefined)).toBeNull();

    const failed = await renderPopover({ kinds: ["claude", "devin"], flagsFail: true });
    expect(autonomySwitch(failed.el).checked).toBe(true);
    expect(flagsOf(failed.el), "a failed read announces no flag at all").toEqual([null, null, null]);
  });

  it("names the switch and the missing-flag hint in the three dictionaries", async () => {
    const seen = new Set<string>();
    for (const locale of ["en", "pt", "es"] as const) {
      setLocalePreference(locale);
      const { el } = await renderPopover({ kinds: ["claude", "devin"] });
      seen.add(`${autonomySwitch(el).closest("label")!.textContent}|${flagsOf(el)[1]}`);
      expect(flagsOf(el)[1]).toBe(t("center.newAgent.noAutonomy"));
      if (locale === "en") expect(untranslated(el.querySelector("[data-new-agent-popover]")!)).toEqual([]);
    }
    expect(seen.size, "the three dictionaries answer with their own words").toBe(3);
  });
});

describe("AC-076-03 the start carries the user's choice", () => {
  it("starts the agent autonomous with the switch on, and plainly with it off", async () => {
    const on = await renderPopover({ kinds: ["claude", "codex"] });
    rowsOf(on.el).find((row) => row.dataset.agentKind === "claude")!.click();
    await vi.waitFor(() => expect(startsOf(on.bridge)).toHaveLength(1));
    expect(startsOf(on.bridge)[0]!.args.kind).toBe("claude");
    expect(startsOf(on.bridge)[0]!.args.autonomous, "the switch was on").toBe(true);

    const off = await renderPopover({ kinds: ["claude", "codex"], autonomy: "off" });
    rowsOf(off.el).find((row) => row.dataset.agentKind === "claude")!.click();
    await vi.waitFor(() => expect(startsOf(off.bridge)).toHaveLength(1));
    expect(startsOf(off.bridge)[0]!.args.autonomous, "the switch was off").toBe(false);
  });

  it("leaves Shell alone: the engine tab is created with no agent start at all", async () => {
    const { el, bridge } = await renderPopover({ kinds: ["claude", "codex"] });
    rowsOf(el).find((row) => row.dataset.agentKind === SHELL_KIND)!.click();
    await vi.waitFor(() => expect(bridge.calls.filter((c) => c.command === "tab_create")).toHaveLength(1));
    expect(startsOf(bridge), "Shell is not an agent: nothing carries autonomy").toHaveLength(0);
  });
});
