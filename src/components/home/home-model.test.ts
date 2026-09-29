// Spec 012 — selectors of the home screen (AC-012-01/02/03). Pure functions over the 011 tree,
// the 014 agent states and the engine's own fields; the thumbnail schedule is exercised with a
// fake `pane.read` so the read budget and the absence of frames are provable here. The real
// window wiring is guarded by `wiring.test.ts` (this file renders no Svelte component).

import { describe, expect, it, vi } from "vitest";
import {
  GROUP_COLLAPSE_LIMIT,
  THUMBNAIL_LIMIT,
  THUMBNAIL_READS_PER_VISIT,
  buildHomeView,
  buildServerCards,
  collapsedGroupText,
  createChipDispatcher,
  createThumbnailSnapshots,
  emptyText,
  greetingFor,
  homeSummary,
  startsCollapsed,
  toggledCollapse,
  type HomeInput,
} from "./home-model";
import { setLocalePreference, type Locale } from "../../i18n/index.svelte";
import { createActivityLedger } from "../agents/panel";
import { initialState, reduce, type AgentsState } from "../../agents/reducer";
import type { AgentDto, AgentStatus, Overview } from "../../agents/types";
import type { HostDto } from "../../connections/types";
import { initialState as navInitial, reduce as navReduce, type NavigatorState } from "../../projects/reducer";
import type { ProjectsSnapshot } from "../../projects/types";

const identity = { endpoint: "local", session: "hd012", connection_generation: 1, boot_id: "boot-hd012" };

function agent(pane: string, status: AgentStatus, extra: Partial<AgentDto> = {}): AgentDto {
  return {
    pane_id: pane,
    workspace_id: pane.split(":")[0]!,
    tab_id: `${pane.split(":")[0]}:t1`,
    name: `agente-${pane}`,
    kind: "claude",
    status,
    launch_pending: false,
    ready: true,
    focused: false,
    state_change_seq: 1,
    ...extra,
  };
}

function connected(agents: AgentDto[]): AgentsState {
  const overview: Overview = {
    state: "connected",
    session: "hd012",
    identity,
    server_version: "0.9.0",
    capabilities: {
      list_agents: true,
      start_agent: true,
      send_prompt: true,
      open_attention: true,
      split: true,
      focus: true,
      split_ratio: true,
      input: true,
      create_tab: true,
      focus_tab: true,
    },
    kinds: ["claude", "pi"],
    agents,
    tabs: [],
    topology: null,
    error: null,
  };
  return reduce(reduce(initialState(), { type: "connect_started" }), { type: "connected", overview });
}

function navigator(snapshot: ProjectsSnapshot): NavigatorState {
  return navReduce(navInitial(), { type: "loaded", snapshot });
}

function host(endpoint: string, over: Partial<HostDto> = {}): HostDto {
  return {
    endpoint,
    label: endpoint === "local" ? "" : endpoint,
    kind: endpoint === "local" ? "local" : "ssh",
    session: "hd012",
    target: endpoint === "local" ? null : "user@dev-box.tail3a9.ts.net",
    visible: true,
    phase: "online",
    phase_label: "online",
    attempt: 0,
    retry_in_ms: null,
    cancelled: false,
    attention: null,
    guidance: null,
    connection_error: null,
    action_error: null,
    generation: 1,
    boot_id: "boot-hd012",
    cached: false,
    surface: null,
    screen: [],
    panes: [],
    actions: [],
    ...over,
  };
}

const catalog: ProjectsSnapshot = {
  version: 1,
  projects: [
    {
      id: "p-erp",
      label: "erp-api",
      endpoint_profile_id: "local",
      session_name: "hd012",
      root: "/home/user/work/erp-api",
      binding: { project_id: "p-erp", connection_generation: 1, boot_id: "boot-hd012", workspace_id: "w1" },
      branch: "main",
    },
    {
      id: "p-herdr",
      label: "herdr-remote",
      endpoint_profile_id: "dev-box",
      session_name: "hd012-ssh",
      root: "~/Projects/herdr",
      binding: { project_id: "p-herdr", connection_generation: 1, boot_id: "boot-hd012", workspace_id: "w2" },
      branch: null,
    },
  ],
  collections: [
    { id: "c-one", name: "Clientes", project_ids: ["p-erp"] },
    { id: "c-two", name: "Open source", project_ids: ["p-herdr"] },
  ],
};

function input(over: Partial<HomeInput> = {}): HomeInput {
  return {
    hour: 15,
    user: "Fábio",
    navigator: navigator(catalog),
    agents: connected([]),
    hosts: [
      host("local", { server_version: "0.9.0", panes: [{ pane_id: "w1:p1", workspace_id: "w1", focused: true, input_enabled: true, input_block: null, target: null }] }),
      host("dev-box", { latency_ms: 32 }),
    ],
    selectedEndpoint: "local",
    hostLabels: { "dev-box": "dev-box" },
    snapshots: createThumbnailSnapshots({ read: async () => "" }),
    ledger: createActivityLedger(),
    now: 1_700_000_000_000,
    connected: true,
    ...over,
  };
}

describe("AC-012-01 greeting and summary", () => {
  it("greets by the local hour with the system user", () => {
    // Would catch: a fixed greeting or an invented user.
    expect(greetingFor(5, "Fábio")).toBe("Bom dia, Fábio");
    expect(greetingFor(11, "Fábio")).toBe("Bom dia, Fábio");
    expect(greetingFor(12, "Fábio")).toBe("Boa tarde, Fábio");
    expect(greetingFor(17, "Fábio")).toBe("Boa tarde, Fábio");
    expect(greetingFor(18, "Fábio")).toBe("Boa noite, Fábio");
    expect(greetingFor(23, "Fábio")).toBe("Boa noite, Fábio");
    expect(greetingFor(15, "  ")).toBe("Boa tarde, você");
  });

  it("counts projects, groups and waiting agents from the reducers", () => {
    const agents = connected([
      agent("w1:p1", "blocked"),
      agent("w1:p2", "working"),
      agent("w1:p3", "unknown"),
    ]);
    // Would catch: numbers computed from the page's own list instead of the tree/engine states.
    expect(homeSummary(navigator(catalog), agents)).toEqual({
      projects: 2,
      groups: 2,
      waiting: 1,
      text: "2 projetos em 2 grupos · 1 agentes esperando por você",
    });
    expect(homeSummary(navigator(catalog), connected([])).waiting).toBe(0);
    expect(buildHomeView(input({ agents })).summary.waiting).toBe(1);
  });

  it("shows the empty state with only the greeting text and the buttons' data", () => {
    const empty = buildHomeView(
      input({ navigator: navigator({ version: 1, projects: [], collections: [] }), agents: connected([]) }),
    );
    // Would catch: cards, servers or chips fabricated without any collection.
    expect(empty.empty).toBe(true);
    expect(empty.emptyText).toBe(emptyText());
    expect(empty.groups).toEqual([]);
    expect(empty.servers).toEqual([]);
    expect(empty.attention).toEqual([]);
    expect(empty.greeting).toBe("Boa tarde, Fábio");
  });
});

describe("AC-012-02 project cards", () => {
  const agents = connected([
    agent("w1:p1", "working", { name: "claude", kind: "claude" }),
    agent("w1:p2", "blocked", { name: "codex", kind: "codex", detection_last_line: "> aprovar sqlx migrate run?" }),
    agent("w1:p3", "idle", { name: "shell", kind: "shell" }),
    agent("w1:p4", "unknown", { name: "pi", kind: "pi" }),
    agent("w1:p5", "done", { name: "amp", kind: "amp" }),
  ]);

  it("shows name, branch, host-prefixed path and up to three thumbnails", () => {
    const view = buildHomeView(input({ agents }));
    const local = view.groups[0]!.cards[0]!;
    const ssh = view.groups[1]!.cards[0]!;
    // Would catch: an SSH path without the host prefix or a Local path with one.
    expect(local.name).toBe("erp-api");
    expect(local.branch).toBe("main");
    expect(local.path).toBe("/home/user/work/erp-api");
    expect(local.hostBadge).toBeNull();
    expect(local.isSsh).toBe(false);
    expect(ssh.path).toBe("dev-box:~/Projects/herdr");
    expect(ssh.hostBadge).toBe("dev-box");
    expect(ssh.branch).toBe("—");
    expect(view.groups[0]!.hostSummary).toBe("local");
    expect(view.groups[1]!.hostSummary).toBe("SSH");
    expect(local.thumbnails.map((t) => t.name)).toEqual(["claude", "codex", "shell"]);
    expect(local.thumbnails).toHaveLength(THUMBNAIL_LIMIT);
    expect(local.overflow).toBe(2);
  });

  it("highlights the waiting agent in amber and labels every state, unknown included", () => {
    const card = buildHomeView(input({ agents })).groups[0]!.cards[0]!;
    const blocked = card.thumbnails.find((t) => t.name === "codex")!;
    // Would catch: no amber for the waiting agent, or unknown shown as done/idle.
    expect(blocked.dot).toEqual({ status: "blocked", color: "var(--attention, #F4B454)", label: "aguardando", waiting: true });
    expect(card.thumbnails.filter((t) => t.dot.waiting)).toHaveLength(1);
    expect(card.thumbnails.find((t) => t.name === "shell")!.dot.label).toBe("ocioso");
    const unknown = buildHomeView(input({ agents: connected([agent("w1:p9", "unknown")]) })).groups[0]!.cards[0]!;
    expect(unknown.thumbnails[0]!.dot).toEqual({ status: "unknown", color: "var(--idle, #6B7280)", label: "desconhecido", waiting: false });
  });

  it("ages the last activity from the observed transition and the engine title", () => {
    const ledger = createActivityLedger();
    const agents5 = connected([
      agent("w1:p1", "working", { name: "claude", kind: "claude", terminal_title: "Refatorando faturamento", state_change_seq: 1 }),
      agent("w1:p2", "blocked", { name: "codex", kind: "codex", state_change_seq: 4 }),
    ]);
    ledger.observe(agents5.agents, 1_700_000_000_000);
    const card = buildHomeView(input({ agents: agents5, ledger, now: 1_700_000_120_000 })).groups[0]!.cards[0]!;
    // Would catch: a time invented without an observed transition, or a summary not from the engine.
    expect(card.activity.avatars).toEqual(["c", "c"]);
    expect(card.activity.time).toBe("2 min");
    expect(card.activity.summary).toBe("Refatorando faturamento");
    const never = buildHomeView(input({ agents: agents5, ledger: createActivityLedger(), now: 1_700_000_120_000 })).groups[0]!.cards[0]!;
    // The title is the engine's; without an observed transition the time is unknown, never invented.
    expect(never.activity.time).toBe("—");
    expect(never.activity.summary).toBe("Refatorando faturamento");
  });

  it("reads one static snapshot per card and visit, never by frame", async () => {
    const frames: number[] = [];
    // The test runner has no DOM; the stub would count any frame the schedule requested.
    vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => {
      frames.push(1);
      cb(0);
      return frames.length;
    });
    try {
      const content = new Map([["w1:p1", "linha um"], ["w1:p2", "linha dois"], ["w1:p3", "linha três"]]);
      const reads: string[] = [];
      const snapshots = createThumbnailSnapshots({
        read: async (paneId) => {
          reads.push(paneId);
          return content.get(paneId) ?? "";
        },
      });
      const base = input({ agents, snapshots });
      const panes = buildHomeView(base).groups.flatMap((group) =>
        group.cards.flatMap((card) => card.thumbnails.map((thumb) => ({ paneId: thumb.paneId, endpoint: card.endpoint }))),
      );

      snapshots.beginVisit();
      await snapshots.ensure(panes, (endpoint) => endpoint === "local");
      expect(reads).toEqual(["w1:p1", "w1:p2", "w1:p3"]);
      for (const pane of panes) {
        expect(snapshots.readsThisVisit(pane.paneId)).toBe(1);
        expect(snapshots.reads(pane.paneId)).toBeLessThanOrEqual(THUMBNAIL_READS_PER_VISIT);
      }

      // A second ensure inside the same visit never repeats a read.
      await snapshots.ensure(panes, () => true);
      expect(reads).toHaveLength(3);
      // The static text is what the card renders; changing the screen does not change it.
      content.set("w1:p1", "linha nova");
      const first = buildHomeView(base).groups[0]!.cards[0]!;
      expect(first.thumbnails[0]!.text).toBe("linha um");
      // Coming back to the screen is a new visit: one fresh read per card, never more.
      snapshots.beginVisit();
      await snapshots.ensure(panes, () => true);
      expect(reads).toHaveLength(6);
      expect(buildHomeView(base).groups[0]!.cards[0]!.thumbnails[0]!.text).toBe("linha nova");
      for (const pane of panes) expect(snapshots.readsThisVisit(pane.paneId)).toBe(1);
      expect(frames).toHaveLength(0);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("skips reads of a host that is not available and keeps the cached snapshot", async () => {
    const snapshots = createThumbnailSnapshots({ read: async () => "cache" });
    const panes = [{ paneId: "w2:p1", endpoint: "dev-box" }];
    snapshots.beginVisit();
    await snapshots.ensure(panes, (endpoint) => endpoint === "local");
    // Would catch: a silent read on an offline/foreign host.
    expect(snapshots.reads("w2:p1")).toBe(0);
    expect(snapshots.text("w2:p1")).toBeNull();
    const view = buildHomeView(input({ agents: connected([agent("w2:p1", "idle")]), snapshots, connected: false }));
    expect(view.cache).toBe(true);
  });

  it("opens the project and focuses the pane once per chip", async () => {
    const calls: string[] = [];
    const dispatch = createChipDispatcher({
      openProject: async (id) => {
        calls.push(`open:${id}`);
      },
      focusPane: async (paneId) => {
        calls.push(`focus:${paneId}`);
      },
    });
    const chips = buildHomeView(input({ agents })).attention;
    const chip = chips.find((candidate) => candidate.paneId === "w1:p2")!;
    expect(chip.projectName).toBe("erp-api");
    expect(chip.label).toBe("erp-api · > aprovar sqlx migrate run?");
    await Promise.all([dispatch(chip), dispatch(chip)]);
    // Would catch: a chip that focuses twice, skips the project open or sends keys to the agent.
    expect(calls).toEqual(["open:p-erp", "focus:w1:p2"]);
  });
});

describe("AC-012-03 servers and collapsed groups", () => {
  it("lists one card per host with the Local version/panes and the SSH address and state", () => {
    const cards = buildServerCards(
      [
        host("local", {
          server_version: "0.9.0",
          panes: [
            { pane_id: "w1:p1", workspace_id: "w1", focused: true, input_enabled: true, input_block: null, target: null },
            { pane_id: "w1:p2", workspace_id: "w1", focused: false, input_enabled: false, input_block: null, target: null },
          ],
        }),
        host("dev-box", { latency_ms: 32 }),
        host("build-box", { label: "build-box", phase: "offline", phase_label: "offline", target: "user@build-box" }),
      ],
      "dev-box",
    );
    // Would catch: a Local card without herdr version/panes, or SSH without its address.
    expect(cards[0]).toMatchObject({ endpoint: "local", typeBadge: "Local", detail: "herdr 0.9.0 · 2 panes", state: "Online", tone: "ok" });
    expect(cards[0]!.name).toBe("Este computador");
    expect(cards[1]).toMatchObject({ endpoint: "dev-box", typeBadge: "SSH", detail: "user@dev-box.tail3a9.ts.net", state: "32 ms", tone: "ok", active: true });
    expect(cards[2]).toMatchObject({ endpoint: "build-box", state: "Offline", tone: "offline", active: false });
  });

  it("starts a group with more than twelve projects collapsed and summarizes it", () => {
    expect(startsCollapsed(GROUP_COLLAPSE_LIMIT)).toBe(false);
    expect(startsCollapsed(GROUP_COLLAPSE_LIMIT + 1)).toBe(true);
    let collapsed: Record<string, boolean> = {};
    const isCollapsed = (id: string, count: number) => collapsed[id] ?? startsCollapsed(count);
    expect(isCollapsed("big", 13)).toBe(true);
    expect(isCollapsed("small", 2)).toBe(false);
    collapsed = toggledCollapse(collapsed, "big", isCollapsed("big", 13));
    expect(isCollapsed("big", 13)).toBe(false);
    collapsed = toggledCollapse(collapsed, "big", isCollapsed("big", 13));
    expect(isCollapsed("big", 13)).toBe(true);
    expect(collapsedGroupText(3, "sem agentes ativos")).toBe("3 projetos · recolhido · sem agentes ativos");
    expect(collapsedGroupText(1, "1 trabalhando")).toBe("1 projeto · recolhido · 1 trabalhando");
  });

  it("summarizes the group's agents from the tree dots", () => {
    const agents2 = connected([
      agent("w1:p1", "working"),
      agent("w1:p2", "blocked", { workspace_id: "w2" }),
    ]);
    const view = buildHomeView(input({ agents: agents2 }));
    expect(view.groups[0]!.agentSummary).toBe("1 agente: 1 trabalhando");
    expect(view.groups[1]!.agentSummary).toBe("1 agente: 1 aguardando");
  });
});

// --- spec 072: the leftovers the home screen still took from the host --------------------------

describe("AC-072-01 nome do host local nos cartões de servidor", () => {
  const expected: [Locale, string][] = [
    ["pt", "Este computador"],
    ["en", "This computer"],
    ["es", "Este ordenador"],
  ];

  // Since spec 071 the host sends `This computer` as the Local label, and it is never empty — so
  // the old `local.label || t(...)` never reached its fallback. Would catch: the card taking the
  // host's English label again instead of naming the Local host in the user's language.
  it.each(expected)("names the Local card in %s and keeps the SSH label", (language, name) => {
    setLocalePreference(language);
    const cards = buildServerCards([host("local", { label: "This computer" }), host("dev-box", { label: "dev-box" })], "local");
    expect(cards[0]!.name).toBe(name);
    expect(cards[1]!.name).toBe("dev-box");
    setLocalePreference("pt");
  });
});

describe("AC-072-02 usuário sem nome", () => {
  // The host answers `""` when the environment publishes no user (`user_from`), and the greeting
  // is what names them — in their own language. Would catch: the Portuguese `você` coming back
  // from the host, or the fallback being dropped from the greeting.
  it("greets an unnamed user with the translated name", () => {
    for (const [language, greeting] of [
      ["pt", "Bom dia, você"],
      ["en", "Good morning, you"],
      ["es", "Buenos días, tú"],
    ] as [Locale, string][]) {
      setLocalePreference(language);
      expect([language, greetingFor(9, "")]).toEqual([language, greeting]);
      expect([language, greetingFor(9, "   ")]).toEqual([language, greeting]);
    }
    setLocalePreference("pt");
    // A named user is never replaced by the fallback.
    expect(greetingFor(9, "Fábio")).toBe("Bom dia, Fábio");
  });
});
