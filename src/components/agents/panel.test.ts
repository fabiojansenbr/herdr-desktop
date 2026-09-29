// Spec 014 — selectors of the agents panel (AC-014-01/02/03). Pure functions over the engine
// data the reducer keeps plus the transitions this client observed; nothing here decides an
// agent's state, invents a time or approves anything.

import { describe, expect, it } from "vitest";
import {
  ATTENTION_LIMIT,
  attentionQueue,
  counters,
  createActivityLedger,
  panelView,
  projectTabLabel,
  relativeTime,
  runningRows,
} from "./panel";
import { initialState, reduce, type AgentsState } from "../../agents/reducer";
import type { AgentDto, AgentStatus, Overview, TabDto } from "../../agents/types";
import type { ProjectsSnapshot } from "../../projects/types";

const identity = { endpoint: "local", session: "hd014", connection_generation: 1, boot_id: "boot-a" };

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

function overview(agents: AgentDto[], tabs: TabDto[] = []): Overview {
  return {
    state: "connected",
    session: "hd014",
    identity,
    server_version: "0.9.1",
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
    tabs,
    topology: null,
    error: null,
  };
}

function connected(agents: AgentDto[], tabs: TabDto[] = []): AgentsState {
  return reduce(reduce(initialState(), { type: "connect_started" }), { type: "connected", overview: overview(agents, tabs) });
}

describe("AC-014-01 counters", () => {
  it("counts working, waiting and idle+done from the engine states", () => {
    const state = connected([
      agent("w1:p1", "working"),
      agent("w1:p2", "working"),
      agent("w1:p3", "blocked"),
      agent("w1:p4", "idle"),
      agent("w1:p5", "done"),
    ]);
    // Would catch: done merged into waiting, or idle and done shown as separate counters.
    expect(counters(state.agents)).toEqual({ active: 2, waiting: 1, idle: 2, unknown: 0 });
  });

  it("keeps unknown in its own counter and never counts it as done or idle", () => {
    const agents = [agent("w1:p1", "unknown"), agent("w1:p2", "unknown"), agent("w1:p3", "done")];
    // Would catch: unknown folded into the idle counter (which includes done).
    expect(counters(agents)).toEqual({ active: 0, waiting: 0, idle: 1, unknown: 2 });
    expect(counters([agent("w1:p1", "idle")]).unknown).toBe(0);
  });

  it("normalizes a status the engine never published as unknown", () => {
    const state = connected([agent("w1:p1", "concluido" as AgentStatus)]);
    // Would catch: an unrecognised wire status leaking into the done/idle counter.
    expect(counters(state.agents)).toEqual({ active: 0, waiting: 0, idle: 0, unknown: 1 });
  });
});

describe("AC-014-02/03 project › tab, time and summary", () => {
  const snapshot: ProjectsSnapshot = {
    version: 1,
    projects: [
      {
        id: "p-erp",
        label: "erp-api",
        endpoint_profile_id: "local",
        session_name: "hd014",
        root: "/work/erp-api",
        binding: { project_id: "p-erp", connection_generation: 1, boot_id: "boot-a", workspace_id: "w1" },
      },
      {
        id: "p-portal",
        label: "portal-web",
        endpoint_profile_id: "local",
        session_name: "hd014",
        root: "/work/portal-web",
        binding: { project_id: "p-portal", connection_generation: 1, boot_id: "boot-a", workspace_id: "w2" },
      },
    ],
    collections: [],
  };
  const tabs: TabDto[] = [
    { tab_id: "w1:t1", workspace_id: "w1", label: "api", focused: true },
    { tab_id: "w2:t1", workspace_id: "w2", label: "ui", focused: false },
  ];

  it("names the project of the agent's workspace and the engine label of its tab", () => {
    // Would catch: the tab of another workspace, or the workspace id shown as a project name.
    expect(projectTabLabel(agent("w2:p1", "working", { tab_id: "w2:t1" }), snapshot, tabs)).toEqual({
      project: "portal-web",
      tab: "ui",
      label: "portal-web › ui",
    });
    expect(projectTabLabel(agent("w1:p1", "working", { tab_id: "w1:t1" }), snapshot, tabs).label).toBe("erp-api › api");
  });

  it("falls back to the engine ids when no project is bound and no tab label exists", () => {
    const unbound = agent("w9:p1", "working", { tab_id: "w9:t3" });
    // Would catch: an empty label, or a project name guessed for a workspace nobody opened.
    expect(projectTabLabel(unbound, snapshot, tabs)).toEqual({ project: "w9", tab: "w9:t3", label: "w9 › w9:t3" });
  });

  it("shows the relative time of the observed transition and a dash when unknown", () => {
    const now = 1_700_000_000_000;
    expect(relativeTime(null, now)).toBe("—");
    expect(relativeTime(now - 2_000, now)).toBe("agora");
    expect(relativeTime(now - 65_000, now)).toBe("1 min");
    expect(relativeTime(now - 12 * 60_000, now)).toBe("12 min");
    expect(relativeTime(now - 2 * 3_600_000, now)).toBe("2 h");
    // Would catch: a future timestamp rendered as a negative age.
    expect(relativeTime(now + 5_000, now)).toBe("agora");
  });
});

describe("AC-014-02/03 observed transitions", () => {
  it("timestamps the first sight of each state change and keeps it while the state holds", () => {
    const ledger = createActivityLedger();
    const first = [agent("w1:p1", "working", { state_change_seq: 3 })];
    ledger.observe(first, 1000);
    expect(ledger.since("w1:p1")).toBe(1000);
    ledger.observe(first, 5000);
    // Would catch: the time being refreshed on every list the engine sends (never aging).
    expect(ledger.since("w1:p1")).toBe(1000);
    ledger.observe([agent("w1:p1", "blocked", { state_change_seq: 4 })], 9000);
    expect(ledger.since("w1:p1")).toBe(9000);
  });

  it("has no time for an agent whose transition it never saw and forgets agents that left", () => {
    const ledger = createActivityLedger();
    // Would catch: a time invented for an agent listed right after connecting.
    expect(ledger.since("w1:p1")).toBeNull();
    ledger.observe([agent("w1:p1", "working")], 1000);
    ledger.observe([agent("w1:p2", "working")], 2000);
    expect(ledger.since("w1:p1")).toBeNull();
    expect(ledger.since("w1:p2")).toBe(2000);
  });

  it("treats a state change with no sequence from the engine as a new transition", () => {
    const ledger = createActivityLedger();
    ledger.observe([agent("w1:p1", "working", { state_change_seq: undefined })], 1000);
    ledger.observe([agent("w1:p1", "blocked", { state_change_seq: undefined })], 4000);
    // Would catch: a server without state_change_seq freezing every card's time.
    expect(ledger.since("w1:p1")).toBe(4000);
  });
});

describe("AC-014-02 attention queue", () => {
  const cards = (count: number) =>
    Array.from({ length: count }, (_, i) => agent(`w1:p${i + 1}`, "blocked", { detection_last_line: `linha ${i + 1}` }));

  it("lists only agents waiting for the user, newest transition first", () => {
    const ledger = createActivityLedger();
    const agents = [agent("w1:p1", "blocked"), agent("w1:p2", "working"), agent("w1:p3", "blocked")];
    ledger.observe([agents[0]!], 1000);
    ledger.observe(agents, 5000);
    const queue = attentionQueue(agents, { ledger, now: 6000, snapshot: null, tabs: [] });
    // Would catch: a working agent in the attention list, or the oldest transition on top.
    expect(queue.cards.map((c) => c.paneId)).toEqual(["w1:p3", "w1:p1"]);
    expect(queue.overflow).toBe(0);
  });

  it("shows the engine's detection line without interpreting it", () => {
    const line = "  Codex quer executar: sqlx migrate run --database-url $DATABASE_URL  ";
    const agents = [agent("w1:p1", "blocked", { detection_last_line: line })];
    const queue = attentionQueue(agents, { ledger: createActivityLedger(), now: 0, snapshot: null, tabs: [] });
    // Would catch: the desktop parsing, truncating or rewriting the engine's snapshot text.
    expect(queue.cards[0]!.lastLine).toBe(line.trim());
    expect(attentionQueue([agent("w1:p2", "blocked")], { ledger: createActivityLedger(), now: 0, snapshot: null, tabs: [] }).cards[0]!.lastLine).toBeNull();
  });

  it("caps the cards at five and reports how many are left", () => {
    const queue = attentionQueue(cards(8), { ledger: createActivityLedger(), now: 0, snapshot: null, tabs: [] });
    // Would catch: an unbounded list of attention cards in a 256 px column.
    expect(ATTENTION_LIMIT).toBe(5);
    expect(queue.cards).toHaveLength(5);
    expect(queue.overflow).toBe(3);
  });
});

describe("AC-014-03 running rows and empty panel", () => {
  it("lists the working agents ordered by the last observed transition", () => {
    const ledger = createActivityLedger();
    const a = agent("w1:p1", "working", { name: "claude", terminal_title: "Refatorando faturamento" });
    const b = agent("w1:p2", "working", { name: "opencode", terminal_title: "Gerando testes do login" });
    ledger.observe([a], 1000);
    ledger.observe([a, b], 120_000);
    const rows = runningRows([a, b], { ledger, now: 130_000, snapshot: null, tabs: [] });
    // Would catch: rows in engine list order instead of activity order.
    expect(rows.map((r) => r.name)).toEqual(["opencode", "claude"]);
    expect(rows[0]!.summary).toBe("Gerando testes do login");
    expect(rows[0]!.time).toBe("agora");
    expect(rows[1]!.time).toBe("2 min");
  });

  it("keeps every engine state in the section with its own text and avatar", () => {
    const rows = runningRows(
      [agent("w1:p1", "working"), agent("w1:p2", "done"), agent("w1:p3", "unknown", { kind: "gemini" })],
      { ledger: createActivityLedger(), now: 0, snapshot: null, tabs: [] },
    );
    // Would catch: unknown shown as done, or an avatar that is not the engine's kind.
    expect(rows.map((r) => [r.status, r.label])).toEqual([
      ["working", "Trabalhando"],
      ["done", "Concluído"],
      ["unknown", "Desconhecido"],
    ]);
    expect(rows.map((r) => r.avatar)).toEqual(["c", "c", "g"]);
  });

  it("reports the empty panel instead of an empty list", () => {
    const view = panelView(connected([]), { ledger: createActivityLedger(), now: 0, snapshot: null, connected: true });
    // Would catch: the empty state hidden behind a list with no rows.
    expect(view.empty).toBe(true);
    expect(view.emptyText).toBe("Nenhum agente nesta sessão");
    expect(view.counters).toEqual({ active: 0, waiting: 0, idle: 0, unknown: 0 });
    expect(panelView(connected([agent("w1:p1", "idle")]), { ledger: createActivityLedger(), now: 0, snapshot: null, connected: true }).empty).toBe(false);
  });

  it("marks the cards as cache and refuses focus while the host is not live", () => {
    const state = connected([agent("w1:p1", "blocked"), agent("w1:p2", "working")]);
    const live = panelView(state, { ledger: createActivityLedger(), now: 0, snapshot: null, connected: true });
    const cached = panelView(state, { ledger: createActivityLedger(), now: 0, snapshot: null, connected: false });
    // Would catch: a disconnected host still offering focus, or cached data shown as live.
    expect(live.cache).toBe(false);
    expect(live.attention.cards[0]!.canFocus).toBe(true);
    expect(cached.cache).toBe(true);
    expect(cached.cacheText).toBe("cache");
    expect(cached.attention.cards[0]!.canFocus).toBe(false);
    expect(cached.running[0]!.canFocus).toBe(false);
  });

  it("refuses focus when the engine does not offer the focus action", () => {
    const state = reduce(reduce(initialState(), { type: "connect_started" }), {
      type: "connected",
      overview: { ...overview([agent("w1:p1", "blocked")]), capabilities: { ...overview([]).capabilities!, focus: false } },
    });
    const view = panelView(state, { ledger: createActivityLedger(), now: 0, snapshot: null, connected: true });
    // Would catch: a focus request sent to a server that never announced pane.focus.
    expect(view.attention.cards[0]!.canFocus).toBe(false);
  });
});
