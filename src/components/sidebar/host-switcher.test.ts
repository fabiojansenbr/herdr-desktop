// @vitest-environment happy-dom
// Spec 048 — host selector at the sidebar foot: the compact line of the approved design
// (`design/v2-01-workspace.png`, AC-048-01) and the popover above it with every host, its
// actions and `+ Nova conexão` (AC-048-02). Bridges are fakes; the switch goes through the
// 037 path (`controllers.surface.select`) and the footer no longer lists the connections.
import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AgentsState } from "../../agents/reducer";
import type { ConnectionsState } from "../../connections/controller";
import { hostFixture } from "../../connections/fake-bridge";
import type { HostDto, SshProfile } from "../../connections/types";
import type { FrameContext } from "../../shell/frame-context";
import type { MenuAction } from "../frame/menus";
import { reactiveHolder, type ReactiveHolder } from "../center/reactive-test-state.svelte";
import HostSwitcher from "./HostSwitcher.svelte";

const sources = import.meta.glob("./*.svelte", { query: "?raw", import: "default", eager: true }) as Record<string, string>;

/** Declarations of the first rule whose selector list contains `selector` exactly (as in 010/041). */
function rule(source: string, selector: string): string {
  const style = source.includes("<style>") ? source.slice(source.indexOf("<style>") + 7, source.indexOf("</style>")) : source;
  for (const m of style.replace(/\/\*[^]*?\*\//g, "").matchAll(/([^{}]+)\{([^}]*)\}/g)) {
    if (m[1]!.split(",").map((s) => s.trim()).includes(selector)) return m[2]!;
  }
  return "";
}

const mounted: { el: HTMLElement; app: Record<string, unknown> }[] = [];

afterEach(() => {
  for (const host of mounted.splice(0)) {
    unmount(host.app as never);
    host.el.remove();
  }
});

function connectionsState(hosts: readonly HostDto[], profiles: readonly SshProfile[]): ConnectionsState {
  return {
    view: { hub: { revision: 1, hosts: [...hosts] }, profiles: [...profiles], store_error: null },
    loading: false,
    globalError: null,
    hostErrors: {},
    inputs: {},
    sending: {},
    dialog: { open: false, draft: { id: null, label: "", target: "", port: "", session: "", auth: "key" }, errors: {}, submitting: false, submitError: null, connecting: null },
    importReport: null,
    workspaces: {},
  } as ConnectionsState;
}

const LOCAL = (over: Partial<HostDto> = {}) =>
  hostFixture({
    endpoint: "local",
    label: "Este computador",
    kind: "local",
    target: null,
    phase: "online",
    phase_label: "Online",
    session: "hd048",
    server_version: "0.9.0",
    ...over,
  });

const MAC = (over: Partial<HostDto> = {}) =>
  hostFixture({ endpoint: "ssh-mac", label: "mac-mini", kind: "ssh", phase: "online", phase_label: "Online", session: "hd048", latency_ms: 42, server_version: "0.9.1", ...over });

const DEV = (over: Partial<HostDto> = {}) =>
  hostFixture({ endpoint: "ssh-dev", label: "dev-box", kind: "ssh", phase: "offline", phase_label: "Offline", session: "hd048", ...over });

/** Saved profile of an SSH host (spec 058: the popover reads `Conectar ao abrir` from it). */
const PROFILE = (endpoint: string, over: Partial<SshProfile> = {}): SshProfile => ({
  id: endpoint,
  label: endpoint,
  target: `user@${endpoint}`,
  port: null,
  session: "hd048",
  connect_on_open: true,
  resume_on_open: false,
  ...over,
});

interface RenderOptions {
  hosts?: readonly HostDto[];
  profiles?: readonly SshProfile[];
  selected?: string | ReactiveHolder<string | null>;
  serverVersion?: string | null;
}

function render(options: RenderOptions = {}) {
  const calls: string[] = [];
  const actions = Object.fromEntries(
    ["showProjects", "openConnections", "paste", "toggleProjects", "toggleAgents", "toggleFiles", "openPalette", "newAgent", "split", "newTab", "reconnect"].map((name) => [
      name,
      () => {
        calls.push(name);
      },
    ]),
  ) as Record<MenuAction, () => void>;
  const holder = typeof options.selected === "string" || options.selected === undefined ? reactiveHolder<string | null>(options.selected ?? "local") : options.selected;
  const select = vi.fn(async () => ({}) as never);
  const connections = {
    openDialog: vi.fn(() => {}),
    connect: vi.fn(async () => {}),
    disconnect: vi.fn(async () => {}),
    reconnect: vi.fn(async () => {}),
    removeProfile: vi.fn(async () => {}),
    editProfile: vi.fn(async () => {}),
    setConnectOnOpen: vi.fn(async () => {}),
  };
  const ctx = {
    surface: null,
    agents: { agents: [], kinds: [], serverVersion: options.serverVersion === undefined ? "0.9.0" : options.serverVersion } as unknown as AgentsState,
    connections: connectionsState(options.hosts ?? [LOCAL(), MAC(), DEV()], options.profiles ?? [PROFILE("ssh-mac"), PROFILE("ssh-dev")]),
    navigator: null,
    get selectedEndpoint() {
      return holder.value;
    },
    activeProject: null,
    hostLabels: {},
    branch: null,
    activity: "projects",
    view: "terminal",
    sidebarOpen: true,
    agentsOpen: true,
    controllers: {
      surface: { select } as unknown as FrameContext["controllers"]["surface"],
      agents: {} as unknown as FrameContext["controllers"]["agents"],
      projects: {} as unknown as FrameContext["controllers"]["projects"],
      connections: connections as unknown as FrameContext["controllers"]["connections"],
    },
    actions,
    unavailable: { split: null, newTab: null, newAgent: null },
  } as unknown as FrameContext;

  const el = document.createElement("div");
  document.body.appendChild(el);
  const app = mount(HostSwitcher, { target: el, props: { ctx } });
  flushSync();
  mounted.push({ el, app: app as never });
  return { el, calls, select, connections, holder };
}

function click(node: Element | null | undefined) {
  (node as HTMLElement).click();
  flushSync();
}

function open(el: HTMLElement) {
  click(el.querySelector("[data-host-trigger]"));
  return el.querySelector<HTMLElement>("[data-host-popover]");
}

const options = (el: HTMLElement) => [...el.querySelectorAll<HTMLElement>("[data-host-option]")];

describe("AC-048-01 compact host line", () => {
  // Would catch: the 041 footer left in place (the whole CONEXÕES list at the foot) instead of
  // the single line of the design.
  it("replaces the connections list with one line: icon, name, dot and summary", () => {
    const { el } = render();
    expect(el.querySelector("footer[aria-label='Conexões']"), "the old list is gone").toBeNull();
    expect(el.textContent).not.toContain("CONEXÕES");
    expect(el.querySelectorAll("[data-host-trigger]")).toHaveLength(1);

    const trigger = el.querySelector<HTMLElement>("[data-host-trigger]")!;
    expect(trigger.querySelector("[data-host-icon]")!.getAttribute("data-host-icon")).toBe("monitor");
    expect(trigger.querySelector("[data-host-name]")!.textContent!.trim()).toBe("Este computador");
    expect(trigger.querySelector("[data-host-dot]")!.getAttribute("data-tone")).toBe("online");
    expect(trigger.querySelector("[data-host-summary]")!.textContent!.trim()).toBe("Local · herdr 0.9.0 · 2 hosts SSH");
  });

  // Would catch: the Local icon/name/type frozen in the line when an SSH host is selected.
  it("shows the SSH label, the server icon and the SSH kind when an SSH host is selected", () => {
    const { el } = render({ selected: "ssh-mac" });
    const trigger = el.querySelector<HTMLElement>("[data-host-trigger]")!;
    expect(trigger.querySelector("[data-host-icon]")!.getAttribute("data-host-icon")).toBe("server");
    expect(trigger.querySelector("[data-host-name]")!.textContent!.trim()).toBe("mac-mini");
    expect(trigger.querySelector("[data-host-summary]")!.textContent!.trim()).toBe("SSH · herdr 0.9.1 · 2 hosts SSH");
  });

  // Would catch: `herdr null`/`herdr undefined` written when no version is known, or the SSH
  // count printed in the plural for a single host.
  it("drops the version when it is unknown and counts one SSH host in the singular", () => {
    const { el } = render({ hosts: [LOCAL({ server_version: null }), DEV()], serverVersion: null });
    expect(el.querySelector("[data-host-summary]")!.textContent!.trim()).toBe("Local · 1 host SSH");
  });

  // Would catch: no SSH host at all still printing `0 hosts SSH`.
  it("omits the SSH count when no SSH host is saved", () => {
    const { el } = render({ hosts: [LOCAL()] });
    expect(el.querySelector("[data-host-summary]")!.textContent!.trim()).toBe("Local · herdr 0.9.0");
  });

  // Would catch: one dot colour for every phase (the design distinguishes the three states).
  it("uses green online, amber connecting and grey offline on the status dot", () => {
    const tone = (host: HostDto) => {
      const { el } = render({ hosts: [host], selected: host.endpoint });
      return el.querySelector("[data-host-dot]")!.getAttribute("data-tone");
    };
    expect(tone(LOCAL())).toBe("online");
    expect(tone(MAC({ phase: "connecting", phase_label: "Conectando" }))).toBe("connecting");
    expect(tone(MAC({ phase: "reconnecting", phase_label: "Reconectando" }))).toBe("connecting");
    expect(tone(DEV())).toBe("offline");
    const css = sources["./HostSwitcher.svelte"]!;
    expect(rule(css, '[data-host-dot][data-tone="online"]')).toMatch(/var\(--working/);
    expect(rule(css, '[data-host-dot][data-tone="connecting"]')).toMatch(/var\(--attention/);
    expect(rule(css, '[data-host-dot][data-tone="offline"]')).toMatch(/var\(--idle/);
  });
});

describe("AC-048-02 host popover", () => {
  // Would catch: the popover opening below the line (clipped by the window foot), the hosts in
  // hub order (SSH before Local), the latency printed for an offline host or no ✓ on the
  // selected one.
  it("opens above the line with every host, Local first, latency only when online and ✓ on the selected", () => {
    const { el } = render();
    expect(el.querySelector("[data-host-popover]")).toBeNull();
    const popover = open(el)!;
    expect(popover).toBeTruthy();
    expect(el.querySelector("[data-host-trigger]")!.getAttribute("aria-expanded")).toBe("true");

    const rows = options(popover);
    expect(rows.map((r) => r.dataset.hostOption)).toEqual(["local", "ssh-mac", "ssh-dev"]);
    expect(rows.map((r) => r.querySelector("[data-host-dot]")!.getAttribute("data-tone"))).toEqual(["online", "online", "offline"]);
    expect(rows[1]!.querySelector("[data-host-latency]")!.textContent).toContain("42 ms");
    expect(rows[2]!.querySelector("[data-host-latency]")).toBeNull();
    expect(rows.map((r) => r.querySelector("[data-host-check]") !== null)).toEqual([true, false, false]);
    expect(rows[0]!.getAttribute("aria-current")).toBe("true");

    expect(rule(sources["./HostPopover.svelte"]!, ".popover")).toMatch(/bottom:\s*100%/);
  });

  // Would catch: the switch rebuilt here (a reconnect instead of the 037 selection), fired twice,
  // or the popover left open over the sidebar after the switch.
  it("switches host through controllers.surface.select once and closes", () => {
    const { el, select, connections } = render();
    const popover = open(el)!;
    click(popover.querySelector('[data-host-option="ssh-mac"]'));
    expect(select).toHaveBeenCalledExactlyOnceWith("ssh-mac");
    expect(connections.connect).not.toHaveBeenCalled();
    expect(connections.reconnect).not.toHaveBeenCalled();
    expect(el.querySelector("[data-host-popover]")).toBeNull();
  });

  // Would catch: the popover losing the per-host actions of the current footer, or offering
  // Desconectar for an offline host (and vice versa).
  it("offers Desconectar for an online SSH host and Reconectar/Editar… for an offline one", () => {
    const { el, connections } = render();
    const popover = open(el)!;
    click(popover.querySelector('[data-host-menu="ssh-mac"]'));
    const menu = popover.querySelector<HTMLElement>('[data-host-menu-for="ssh-mac"]')!;
    expect([...menu.querySelectorAll("[data-action]")].map((b) => b.getAttribute("data-action"))).toEqual(["disconnect", "connect_on_open", "edit", "remove"]);
    expect(menu.textContent).toContain("Desconectar");
    click(menu.querySelector('[data-action="disconnect"]'));
    expect(connections.disconnect).toHaveBeenCalledExactlyOnceWith("ssh-mac");

    click(popover.querySelector('[data-host-menu="ssh-dev"]'));
    const offline = popover.querySelector<HTMLElement>('[data-host-menu-for="ssh-dev"]')!;
    expect([...offline.querySelectorAll("[data-action]")].map((b) => b.getAttribute("data-action"))).toEqual(["reconnect", "connect_on_open", "edit", "remove"]);
    click(offline.querySelector('[data-action="edit"]'));
    expect(connections.editProfile).toHaveBeenCalledExactlyOnceWith("ssh-dev");
  });

  // Would catch (029): a profile removed straight from the menu without the confirmation the
  // footer asks for, or Local offering destructive actions.
  it("asks for confirmation before removing and never opens a menu for Local", () => {
    const { el, connections } = render();
    const popover = open(el)!;
    expect(popover.querySelector('[data-host-menu="local"]')).toBeNull();

    click(popover.querySelector('[data-host-menu="ssh-dev"]'));
    click(popover.querySelector('[data-host-menu-for="ssh-dev"] [data-action="remove"]'));
    expect(connections.removeProfile).not.toHaveBeenCalled();
    const confirm = popover.querySelector<HTMLElement>('[data-host-confirm="ssh-dev"]')!;
    expect(confirm.textContent).toContain("Remover dev-box?");
    click(confirm.querySelector("[data-host-cancel-remove]"));
    expect(connections.removeProfile).not.toHaveBeenCalled();
    expect(popover.querySelector('[data-host-confirm="ssh-dev"]')).toBeNull();

    click(popover.querySelector('[data-host-menu="ssh-dev"]'));
    click(popover.querySelector('[data-host-menu-for="ssh-dev"] [data-action="remove"]'));
    click(popover.querySelector('[data-host-confirm="ssh-dev"] [data-host-confirm-remove]'));
    expect(connections.removeProfile).toHaveBeenCalledExactlyOnceWith("ssh-dev");
  });

  // AC-029-03 kept: disconnecting or removing the selected host falls back to Local, never to
  // another SSH host.
  it("returns the selection to Local when the selected host is disconnected or removed", async () => {
    const { el, select, connections } = render({ selected: "ssh-mac" });
    click(open(el)!.querySelector('[data-host-menu="ssh-mac"]'));
    click(el.querySelector('[data-host-menu-for="ssh-mac"] [data-action="disconnect"]'));
    expect(connections.disconnect).toHaveBeenCalledExactlyOnceWith("ssh-mac");
    await Promise.resolve();
    await Promise.resolve();
    flushSync();
    expect(select).toHaveBeenCalledExactlyOnceWith("local");
  });

  // AC-058-03. Would catch: the preference missing from the `…` menu (the user could never turn
  // the resume off), its ✓ frozen on, a click that only changes the menu instead of writing the
  // profile through the new command, or Local — which has no saved profile — offering it.
  it("offers Conectar ao abrir with ✓ when it is on and toggles it through the connections controller", () => {
    const { el, connections } = render({ profiles: [PROFILE("ssh-mac"), PROFILE("ssh-dev", { connect_on_open: false })] });
    const popover = open(el)!;
    expect(popover.querySelector('[data-host-menu="local"]')).toBeNull();

    click(popover.querySelector('[data-host-menu="ssh-mac"]'));
    const on = popover.querySelector<HTMLElement>('[data-host-menu-for="ssh-mac"] [data-action="connect_on_open"]')!;
    expect(on.textContent).toContain("Conectar ao abrir");
    expect(on.querySelector("[data-action-check]")).toBeTruthy();
    expect(on.getAttribute("aria-checked")).toBe("true");
    click(on);
    expect(connections.setConnectOnOpen).toHaveBeenCalledExactlyOnceWith("ssh-mac", false);

    click(popover.querySelector('[data-host-menu="ssh-dev"]'));
    const off = popover.querySelector<HTMLElement>('[data-host-menu-for="ssh-dev"] [data-action="connect_on_open"]')!;
    expect(off.querySelector("[data-action-check]")).toBeNull();
    expect(off.getAttribute("aria-checked")).toBe("false");
    click(off);
    expect(connections.setConnectOnOpen).toHaveBeenLastCalledWith("ssh-dev", true);
    expect(connections.disconnect).not.toHaveBeenCalled();
    expect(connections.editProfile).not.toHaveBeenCalled();
    expect(connections.removeProfile).not.toHaveBeenCalled();
  });

  // Would catch: the entry wired to the 011 dialog controller instead of the window action the
  // spec names, or fired twice.
  it("calls ctx.actions.openConnections exactly once from + Nova conexão", () => {
    const { el, calls, connections } = render();
    const popover = open(el)!;
    const add = popover.querySelector<HTMLButtonElement>("[data-host-new]")!;
    expect(add.textContent).toContain("Nova conexão");
    click(add);
    expect(calls.filter((c) => c === "openConnections")).toEqual(["openConnections"]);
    expect(connections.openDialog).not.toHaveBeenCalled();
  });

  // Would catch: a popover that only closes by picking a host (trapping the sidebar), or one
  // that closes on its own clicks.
  it("closes on Escape and on a click outside, but not on a click inside", () => {
    const { el } = render();
    open(el);
    el.querySelector<HTMLElement>("[data-host-popover]")!.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true }));
    flushSync();
    expect(el.querySelector("[data-host-popover]")).toBeTruthy();

    document.body.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true }));
    flushSync();
    expect(el.querySelector("[data-host-popover]")).toBeNull();

    open(el);
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    flushSync();
    expect(el.querySelector("[data-host-popover]")).toBeNull();
    expect(el.querySelector("[data-host-trigger]")!.getAttribute("aria-expanded")).toBe("false");
  });
});
