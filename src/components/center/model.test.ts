// Spec 013 — pure model of the center: pane frame geometry from `PaneMeta.inner_rect` and the cell
// metrics, per-pane state from the agents reducer data, workspace tabs from `tab.list` +
// `focused_tab_id`, and the project header. The native `visual-center` phase measures the real
// window; these tests fix the arithmetic and the mapping the component may not invent.
import { describe, expect, it } from "vitest";
import type { AgentDto, TabDto } from "../../agents/types";
import type { ProjectDto } from "../../projects/types";
import type { PaneMeta, RectDto } from "../../terminal/types";
import {
  HEADER_ROWS,
  actionAvailability,
  agentName,
  applySurfaceMetadata,
  confirmedTarget,
  bandRect,
  frameRect,
  headerHeightPx,
  metricsFromSurface,
  paneFrames,
  projectHeader,
  workspaceTabs,
  focusedWorkspaceId,
  workspaceCounts,
  AGENT_DISPLAY_TITLES,
  cwdBasename,
  isUnnamedTab,
  tabStatusIcon,
  tabTitle,
  tabTooltip,
  type TabPane,
} from "./model";

const rect = (x: number, y: number, width: number, height: number): RectDto => ({ x, y, width, height });

function pane(id: string, inner: RectDto, focused = false): PaneMeta {
  return {
    pane_id: id,
    content_revision: 1,
    rect: rect(inner.x - 1, inner.y - 1, inner.width + 2, inner.height + 2),
    inner_rect: inner,
    scroll: null,
    focused,
    mouse_reporting: false,
    sgr_pixel_mouse: false,
    alternate_screen_active: false,
    pixel_width: 0,
    pixel_height: 0,
  };
}

function agent(pane_id: string, status: AgentDto["status"], extra: Partial<AgentDto> = {}): AgentDto {
  return {
    pane_id,
    workspace_id: "w1",
    tab_id: "w1:t1",
    name: "hd013-agent",
    kind: "claude",
    status,
    launch_pending: false,
    ready: true,
    focused: false,
    ...extra,
  };
}

const tab = (tab_id: string, label: string, pane_count: number, agent_status: TabDto["agent_status"] = "idle"): TabDto => ({
  tab_id,
  workspace_id: tab_id.split(":")[0]!,
  label,
  focused: false,
  pane_count,
  agent_status,
});

describe("pane frame geometry (AC-013-02)", () => {
  const origin = { left: 10, top: 6 };

  // Would catch: frames drawn over `rect` (the engine border box) instead of `inner_rect`, or the
  // canvas origin ignored (frames anchored at the region instead of the surface).
  it("a frame is inner_rect × cell translated by the canvas origin", () => {
    const m = { cellWidth: 9, cellHeight: 19 };
    expect(frameRect(rect(1, 1, 40, 20), m, origin)).toEqual({ left: 19, top: 25, width: 360, height: 380 });
    expect(frameRect(rect(42, 1, 40, 20), m, origin)).toEqual({ left: 388, top: 25, width: 360, height: 380 });
  });

  // Would catch: metrics hard-coded at 9×18 instead of measured, so a DPR 2 window (bigger cells)
  // would place every frame wrong.
  it("cell metrics come from the measured canvas box and the surface size, at any DPR", () => {
    expect(metricsFromSurface({ width: 1080, height: 760 }, { width: 120, height: 40 })).toEqual({ cellWidth: 9, cellHeight: 19 });
    expect(metricsFromSurface({ width: 1200, height: 800 }, { width: 120, height: 40 })).toEqual({ cellWidth: 10, cellHeight: 20 });
    expect(metricsFromSurface({ width: 1080, height: 760 }, { width: 0, height: 0 })).toBeNull();
  });

  // Would catch: a header band that eats content rows (the 010 debt: a bar over row 0) instead of
  // sitting in the row above the pane's inner rect.
  it("the header band is one cell tall, right above the inner rect, never inside it", () => {
    const m = { cellWidth: 9, cellHeight: 19 };
    expect(HEADER_ROWS).toBe(1);
    expect(headerHeightPx(m.cellHeight)).toBe(19);
    const inner = rect(1, 1, 40, 20);
    const band = bandRect(inner, m, origin);
    const frame = frameRect(inner, m, origin);
    expect(band).toEqual({ left: 19, top: 6, width: 360, height: 19 });
    expect(band.top + band.height).toBe(frame.top);
  });

  // Would catch: a top pane (no engine border row) whose band is pushed into row 0 of the surface.
  it("a pane at row 0 keeps its band above the surface, never over row 0", () => {
    const m = { cellWidth: 9, cellHeight: 19 };
    const band = bandRect(rect(0, 0, 120, 40), m, origin);
    expect(band).toEqual({ left: 10, top: -13, width: 1080, height: 19 });
    expect(band.top + band.height).toBe(6);
  });

  // Would catch: frames for panes of another tab, or a frame missing when the metadata has N panes.
  it("there is one frame per pane of the metadata, keyed by pane id", () => {
    const m = { cellWidth: 9, cellHeight: 19 };
    const frames = paneFrames({
      panes: [pane("w1:p1", rect(1, 1, 40, 20), true), pane("w1:p2", rect(43, 1, 40, 20))],
      agents: [],
      panePaths: {},
      metrics: m,
      origin,
    });
    expect(frames.map((f) => f.pane_id)).toEqual(["w1:p1", "w1:p2"]);
    expect(frames[0]!.rect).toEqual(frameRect(rect(1, 1, 40, 20), m, origin));
    expect(frames[1]!.band).toEqual(bandRect(rect(43, 1, 40, 20), m, origin));
  });
});

describe("pane frame content (AC-013-02)", () => {
  const m = { cellWidth: 9, cellHeight: 19 };
  const origin = { left: 0, top: 0 };
  const build = (agents: AgentDto[], panePaths: Record<string, string> = {}) =>
    paneFrames({
      panes: [pane("w1:p1", rect(1, 1, 40, 20), true), pane("w1:p2", rect(43, 1, 40, 20))],
      agents,
      panePaths,
      metrics: m,
      origin,
    });

  // Would catch: the agent's internal name shown instead of the agent, or a pane without an agent
  // labelled with the previous pane's agent.
  it("names the pane's agent, or shell when the engine detected none", () => {
    const frames = build([agent("w1:p1", "working")]);
    expect(frames.map((f) => f.name)).toEqual(["claude", "shell"]);
    expect(build([agent("w1:p1", "working", { kind: null, name: "revisor" })])[0]!.name).toBe("revisor");
  });

  // Would catch: state shown by colour only, "unknown" counted as done, or a state text invented
  // for a pane the engine did not report.
  it("each state has its own text and tone, and unknown is never done", () => {
    const states: AgentDto["status"][] = ["working", "blocked", "idle", "done", "unknown"];
    const labels = states.map((s) => build([agent("w1:p1", s)])[0]!.status);
    expect(labels.map((s) => s.label)).toEqual(["Trabalhando", "Aguardando você", "Ocioso", "Concluído", "Desconhecido"]);
    expect(labels.map((s) => s.tone)).toEqual(["working", "attention", "idle", "done", "unknown"]);
    expect(labels.map((s) => s.tone)).not.toContain("done-unknown");
    expect(build([agent("w1:p1", "unknown")])[0]!.status.label).not.toBe("Concluído");
    // A pane with no agent is the shell the engine shows as idle; never "Concluído".
    expect(build([])[0]!.status).toEqual({ label: "Ocioso", tone: "idle", icon: "○" });
  });

  // Would catch: the project root shown as if it were the pane's cwd, or a missing cwd faked.
  it("shows the pane path the engine reported for that pane", () => {
    const frames = build([], { "w1:p1": "/work/erp-api" });
    expect(frames[0]!.path).toBe("/work/erp-api");
    expect(frames[1]!.path).toBeNull();
  });

  // Would catch: a wrap border around a single pane (AC-019-02: no frame), or the focused pane
  // of a split missing the accent divider.
  it("a single pane has no wrap edge; split panes use a 1 px divider, accent on the focused side", () => {
    const one = paneFrames({
      panes: [pane("w1:p1", rect(1, 1, 80, 20), true)],
      agents: [agent("w1:p1", "blocked")],
      panePaths: {},
      metrics: m,
      origin,
    });
    expect(one).toHaveLength(1);
    expect(one[0]!.rect).toEqual(frameRect(rect(1, 1, 80, 20), m, origin));
    expect(one[0]!.edge).toBe("none");
    const split = build([agent("w1:p1", "idle"), agent("w1:p2", "idle")]);
    expect(split.map((f) => f.edge)).toEqual(["accent", "border"]);
    const waiting = build([agent("w1:p1", "working"), agent("w1:p2", "blocked")]);
    expect(waiting.map((f) => f.edge)).toEqual(["accent", "border"]);
    expect(waiting[0]!.rect).toEqual(frameRect(rect(1, 1, 40, 20), m, origin));
    expect(waiting[1]!.rect).toEqual(frameRect(rect(43, 1, 40, 20), m, origin));
  });
});

describe("workspace tabs (AC-013-01)", () => {
  const tabs = [tab("w1:t1", "api", 3, "working"), tab("w1:t2", "migrations", 1, "blocked"), tab("w1:t3", "testes e2e", 2)];

  // Would catch: tabs invented or filtered by the client, or the engine's pane count replaced by
  // the number of panes the client can see (only the active tab has metadata).
  it("lists exactly the engine tabs with their pane counts", () => {
    const model = workspaceTabs({ tabs, agents: [], focusedTabId: "w1:t1" });
    expect(model.map((t) => [t.tab_id, t.label, t.panes])).toEqual([
      ["w1:t1", "api", 3],
      ["w1:t2", "migrations", 1],
      ["w1:t3", "testes e2e", 2],
    ]);
  });

  // Would catch: the active tab taken from `tab.list[].focused` (global in the engine) instead of
  // the focus this connection confirmed.
  it("the active tab follows the connection's focused_tab_id, not the global flag", () => {
    const global = tabs.map((t, i) => ({ ...t, focused: i === 0 }));
    expect(workspaceTabs({ tabs: global, agents: [], focusedTabId: "w1:t2" }).map((t) => t.active)).toEqual([false, true, false]);
    expect(workspaceTabs({ tabs: global, agents: [], focusedTabId: null }).map((t) => t.active)).toEqual([false, false, false]);
  });

  // Would catch: the dot collapsed into one colour, or attention lost behind a working agent.
  it("the dot is attention when an agent waits, working when one works, else none", () => {
    const model = workspaceTabs({ tabs, agents: [], focusedTabId: "w1:t1" });
    expect(model.map((t) => t.dot)).toEqual(["working", "attention", null]);
    // Without the engine's per-tab status, the agents of each tab answer the same question.
    const plain = tabs.map((t) => ({ ...t, agent_status: "unknown" as const }));
    const agents = [agent("w1:p1", "working", { tab_id: "w1:t1" }), agent("w1:p9", "blocked", { tab_id: "w1:t3" })];
    expect(workspaceTabs({ tabs: plain, agents, focusedTabId: "w1:t1" }).map((t) => t.dot)).toEqual(["working", null, "attention"]);
  });

  // Would catch: overflow handled by hiding tabs instead of scrolling them.
  it("more than eight tabs are marked as overflowing (scrolled, never dropped)", () => {
    const many = Array.from({ length: 9 }, (_, i) => tab(`w1:t${i + 1}`, `tab ${i + 1}`, 1));
    const model = workspaceTabs({ tabs: many, agents: [], focusedTabId: "w1:t1" });
    expect(model).toHaveLength(9);
    expect(model.every((t) => t.title.startsWith("tab ") || t.label.startsWith("tab "))).toBe(true);
  });
});

describe("orca tab titles and icons (AC-019-01, AC-023-01)", () => {
  // Would catch: the agent type ("Claude Code") replacing TabDto.label, or the glyph not
  // following the focused pane when two agents share a tab.
  it("keeps the engine label as the title and puts the focused agent in the tooltip", () => {
    expect(AGENT_DISPLAY_TITLES).toEqual({
      claude: "Claude Code",
      codex: "Codex",
      opencode: "OpenCode",
      grok: "Grok",
      pi: "Pi",
      agy: "Antigravity",
    });
    const tabs = [tab("w1:t1", "api", 2)];
    const two = [
      agent("w1:p1", "working", { tab_id: "w1:t1", kind: "claude", focused: true }),
      agent("w1:p2", "idle", { tab_id: "w1:t1", kind: "codex", focused: false }),
    ];
    const claude = workspaceTabs({ tabs, agents: two, focusedTabId: "w1:t1", focusedPaneId: "w1:p1" })[0]!;
    expect(claude.label).toBe("api");
    expect(claude.title).toBe("api · claude · Trabalhando · codex · Ocioso");
    expect(claude.glyph).toBe("c");
    const codex = workspaceTabs({ tabs, agents: two, focusedTabId: "w1:t1", focusedPaneId: "w1:p2" })[0]!;
    expect(codex.label).toBe("api");
    expect(codex.title).toBe("api · claude · Trabalhando · codex · Ocioso");
    expect(codex.glyph).toBe("c");
  });

  // Would catch: a cwd basename or a missing label inventing a title the engine did not publish.
  it("uses the engine tab label even when a cwd is known, and empty labels fall back to the tab number", () => {
    const tabs = [tab("w1:t1", "shell-7", 1)];
    expect(cwdBasename("/home/user/Projects/herdr")).toBe("herdr");
    expect(workspaceTabs({ tabs, agents: [], focusedTabId: "w1:t1" })[0]!.label).toBe("shell-7");
    expect(
      workspaceTabs({
        tabs,
        agents: [],
        focusedTabId: "w1:t1",
        focusedPaneId: "w1:p1",
        panePaths: { "w1:p1": "/home/user/Projects/herdr" },
      })[0]!.label,
    ).toBe("shell-7");
    expect(workspaceTabs({ tabs, agents: [], focusedTabId: "w1:t1" })[0]!.title).toBe("shell-7 · shell · Ocioso");
    const numbered = [{ ...tab("w1:t4", "", 1), number: 4 }];
    expect(workspaceTabs({ tabs: numbered, agents: [], focusedTabId: "w1:t4" })[0]!.label).toBe("4");
  });

  it("status icons: check when idle/done, pulse when working, attention when blocked, terminal when no agent", () => {
    expect(tabStatusIcon({ agents: [agent("w1:p1", "idle")], hasAgent: true })).toBe("idle");
    expect(tabStatusIcon({ agents: [agent("w1:p1", "done")], hasAgent: true })).toBe("idle");
    expect(tabStatusIcon({ agents: [agent("w1:p1", "working")], hasAgent: true })).toBe("working");
    expect(tabStatusIcon({ agents: [agent("w1:p1", "blocked")], hasAgent: true })).toBe("attention");
    expect(tabStatusIcon({ agents: [], hasAgent: false })).toBe("shell");
    const tabs = [tab("w1:t1", "api", 1, "idle")];
    expect(workspaceTabs({ tabs, agents: [agent("w1:p1", "idle", { tab_id: "w1:t1" })], focusedTabId: "w1:t1" })[0]!.icon).toBe("idle");
    expect(workspaceTabs({ tabs, agents: [], focusedTabId: "w1:t1" })[0]!.icon).toBe("shell");
  });
});

describe("tab titles from the open app (AC-026-01, AC-026-02)", () => {
  // Would catch: a title built from the tab number or from metadata the engine did not publish,
  // the agent display name ("Claude Code") where the TUI shows the kind, a terminal title with
  // escapes or over 24 characters, or a focused pane moved to the front of the layout order.
  const pane = (over: Partial<TabPane> = {}): TabPane => ({
    pane_id: "w1:p1",
    agent: null,
    title: null,
    cwd: null,
    focused: false,
    status: { label: "Ocioso", tone: "idle", icon: "○" },
    ...over,
  });
  const unnamed = (number = 1): TabDto => ({ ...tab(`w1:t${number}`, String(number), 1), number });
  const named = (label: string): TabDto => ({ ...tab("w1:t1", label, 1), number: 1 });

  it("an unnamed tab takes the agent name, lowercased like the TUI, over title and cwd", () => {
    expect(tabTitle(unnamed(), [pane({ agent: "claude", title: "zsh", cwd: "/tmp" })])).toBe("claude");
    expect(tabTitle(unnamed(), [pane({ agent: "OpenCode" })])).toBe("opencode");
  });

  it("without an agent it uses the printable terminal title, clipped at 24 characters", () => {
    expect(tabTitle(unnamed(), [pane({ title: "zsh" })])).toBe("zsh");
    expect(tabTitle(unnamed(), [pane({ title: " \u001b[31mvim\u001b[0m\u0007 " })])).toBe("vim");
    expect(tabTitle(unnamed(), [pane({ title: "a".repeat(30) })])).toBe(`${"a".repeat(24)}…`);
    expect(tabTitle(unnamed(), [pane({ title: "   " })])).toBe("shell");
  });

  it("without a title it uses the cwd basename and falls back to shell; no panes is vazia", () => {
    expect(tabTitle(unnamed(), [pane({ cwd: "/home/user/Projects/herdr/" })])).toBe("herdr");
    expect(tabTitle(unnamed(), [pane({ cwd: "C:\\work\\erp" })])).toBe("erp");
    expect(tabTitle(unnamed(), [pane()])).toBe("shell");
    expect(tabTitle(unnamed(), [])).toBe("vazia");
  });

  it("lists N panes in layout order: two names then +N, and focus never reorders", () => {
    const four = [
      pane({ pane_id: "w1:p1", agent: "claude" }),
      pane({ pane_id: "w1:p2", agent: "zsh" }),
      pane({ pane_id: "w1:p3", agent: "opencode" }),
      pane({ pane_id: "w1:p4", agent: "codex" }),
    ];
    expect(tabTitle(unnamed(), four.slice(0, 2))).toBe("claude · zsh");
    expect(tabTitle(unnamed(), four)).toBe("claude · zsh +2");
    const refocused = four.map((p, i) => ({ ...p, focused: i === 3 }));
    expect(tabTitle(unnamed(), refocused)).toBe("claude · zsh +2");
    expect(tabTitle(unnamed(), refocused.slice(0, 3))).toBe("claude · zsh +1");
  });

  it("a named tab keeps the engine label (023); every pane lives in the tooltip with its state", () => {
    const panes = [
      pane({ agent: "claude", status: { label: "Trabalhando", tone: "working", icon: "⟳" } }),
      pane({ pane_id: "w1:p2", title: "zsh" }),
    ];
    expect(tabTitle(named("workers"), panes)).toBe("workers");
    expect(tabTooltip(named("workers"), panes)).toBe("workers · claude · Trabalhando · zsh · Ocioso");
    expect(tabTooltip(unnamed(), panes)).toBe("claude · Trabalhando · zsh · Ocioso");
  });

  // r4b: the engine labels an unnamed tab with its position and sends the monotonic id as
  // `number` ("2" vs 21), so a label of digits alone is the position, never a name.
  it("treats a label of digits as the tab position, not a name", () => {
    const positional: TabDto = { ...tab("w1:t21", "2", 1), number: 21 };
    expect(isUnnamedTab(positional)).toBe(true);
    expect(tabTitle(positional, [pane({ agent: "grok" })])).toBe("grok");
    expect(tabTooltip(positional, [pane({ agent: "grok" })])).toBe("grok · Ocioso");
    expect(isUnnamedTab(unnamed())).toBe(true);
    expect(isUnnamedTab(named("workers"))).toBe(false);
    expect(tabTitle(named("workers"), [pane({ agent: "grok" })])).toBe("workers");
    expect(isUnnamedTab(named("3xxx"))).toBe(false);
    expect(tabTitle(named("3xxx"), [pane({ agent: "grok" })])).toBe("3xxx");
  });
});

describe("workspace tabs read the open app (AC-026-01, AC-026-02)", () => {
  // Would catch: the visible title still taken from the engine label on an unnamed tab, the
  // engine label lost for the rename draft, the layout order ignored, or a pane the client only
  // knows by count dropped from the title/tooltip instead of shown as shell.
  it("an unnamed tab shows the agent of its pane and keeps the engine label for rename", () => {
    const tabs = [{ ...tab("w1:t1", "1", 1), number: 1 }];
    const model = workspaceTabs({
      tabs,
      agents: [agent("w1:p1", "idle", { tab_id: "w1:t1", kind: "claude", terminal_title: "Claude" })],
      focusedTabId: "w1:t1",
      focusedPaneId: "w1:p1",
    })[0]!;
    expect(model.label).toBe("1");
    expect(model.display).toBe("claude");
    expect(model.more).toBe(0);
    expect(model.panes).toBe(1);
    expect(model.title).toBe("claude · Ocioso");
  });

  // r4b: `tab.list` sends the position as label and the monotonic id as number; the title must
  // still come from the pane's app.
  it("a tab labelled with its position uses the pane app even when number differs", () => {
    const tabs = [{ ...tab("w1:t21", "2", 1), number: 21 }];
    const model = workspaceTabs({
      tabs,
      agents: [agent("w1:p1", "idle", { tab_id: "w1:t21", kind: "grok" })],
      focusedTabId: "w1:t21",
      focusedPaneId: "w1:p1",
    })[0]!;
    expect(model.named).toBe(false);
    expect(model.label).toBe("2");
    expect(model.display).toBe("grok");
    expect(model.title).toBe("grok · Ocioso");
  });

  it("the confirmed layout names the panes in its own order; an unnamed pane is shell", () => {
    const tabs = [{ ...tab("w1:t1", "1", 2), number: 1 }];
    const model = workspaceTabs({
      tabs,
      agents: [agent("w1:p2", "working", { tab_id: "w1:t1", kind: "codex" })],
      focusedTabId: "w1:t1",
      focusedPaneId: "w1:p2",
      layoutPanes: [
        { pane_id: "w1:p1", x: 0, y: 0, width: 60, height: 40, focused: false, cwd: "/srv/api" },
        { pane_id: "w1:p2", x: 61, y: 0, width: 59, height: 40, focused: true, cwd: "/srv/web" },
      ],
    })[0]!;
    expect(model.display).toBe("api · codex");
    expect(model.more).toBe(0);
    expect(model.title).toBe("api · Ocioso · codex · Trabalhando");
    expect(model.glyph).toBe("c");
  });

  it("panes the client knows only by count keep the title honest: shell and +N", () => {
    const tabs = [{ ...tab("w1:t2", "2", 4), number: 2 }];
    const model = workspaceTabs({
      tabs,
      agents: [
        agent("w1:p1", "idle", { tab_id: "w1:t2", kind: "claude" }),
        agent("w1:p2", "idle", { tab_id: "w1:t2", kind: "codex" }),
      ],
      focusedTabId: "w1:t1",
    })[0]!;
    expect(model.display).toBe("claude · codex");
    expect(model.more).toBe(2);
    expect(tabTitle(tabs[0]!, [])).toBe("vazia");
  });

  it("a named tab keeps the engine label as the visible title", () => {
    const tabs = [tab("w1:t1", "workers", 2)];
    const model = workspaceTabs({ tabs, agents: [], focusedTabId: "w1:t1" })[0]!;
    expect(model.label).toBe("workers");
    expect(model.display).toBe("workers");
    expect(model.more).toBe(0);
  });
});

describe("project header (AC-013-01)", () => {
  const project: ProjectDto = { id: "p1", label: "erp-api", root: "/work/acme/erp-api", endpoint_profile_id: "local", session_name: "hd013", binding: null };

  // Would catch: the breadcrumb showing only the project (the collection is the group of the design)
  // or a branch invented when the engine reported none.
  it("shows <grupo> › <projeto>, the branch and the path", () => {
    const model = projectHeader({ project, collection: "Acme · Clientes", branch: "main", tabs: 3 });
    expect(model.crumbs).toEqual(["Acme · Clientes", "erp-api"]);
    expect(model.branch).toBe("main");
    expect(model.path).toBe("/work/acme/erp-api");
    expect(model.empty).toBe(false);
    expect(projectHeader({ project, collection: null, branch: null, tabs: 1 })).toMatchObject({ crumbs: ["erp-api"], branch: null });
  });

  // Would catch: an empty session offering split/agent actions that have no workspace to act on.
  it("without tabs the header is the empty state; split stays off until a pane exists", () => {
    const model = projectHeader({ project: null, collection: null, branch: null, tabs: 0 });
    expect(model.empty).toBe(true);
    expect(model.emptyText).toBe("Sem workspace");
    expect(model.actions).toEqual(["newTab", "newAgent"]);
    expect(projectHeader({ project, collection: null, branch: null, tabs: 2 }).actions).toEqual(["split", "newTab", "newAgent"]);
  });

  it("without a bound project shows sessão › workspace and the focused pane cwd, not an orphan branch", () => {
    const model = projectHeader({
      project: null,
      collection: null,
      branch: "master",
      tabs: 2,
      session: "hd017",
      workspace: "w1",
      cwd: "/tmp/work",
    });
    expect(model.crumbs).toEqual(["sessão hd017", "w1"]);
    expect(model.path).toBe("/tmp/work");
    expect(model.branch).toBeNull();
  });
});

describe("surface metadata the overlay follows (AC-013-02)", () => {
  const full = (revision: number, panes: PaneMeta[]) => ({ type: "metadata" as const, revision, panes, hyperlinks: [] as string[] });
  const partial = (revision: number, panes: PaneMeta[]) => ({ type: "metadata" as const, revision, panes, hyperlinks: null });
  const frame = (revision: number) => ({ type: "full" as const, revision, width: 100, height: 40, cells: [], cursor: null, panes: [] });

  // Would catch (the native run of r1/r2 did): a patch's metadata, which carries ONLY the updated
  // panes, replacing the layout, so a split's second frame disappears from the overlay.
  it("a partial metadata updates its panes and keeps the others", () => {
    const a = pane("w1:p1", rect(1, 1, 40, 20), true);
    const b = pane("w1:p2", rect(43, 1, 40, 20));
    const after = applySurfaceMetadata(applySurfaceMetadata({ revision: 0, panes: [] }, frame(7)), full(7, [a, b]));
    expect(after.panes.map((p) => p.pane_id)).toEqual(["w1:p1", "w1:p2"]);
    const moved = { ...b, focused: true };
    const merged = applySurfaceMetadata(after, partial(7, [moved]));
    expect(merged.panes.map((p) => p.pane_id)).toEqual(["w1:p1", "w1:p2"]);
    expect(merged.panes.map((p) => p.focused)).toEqual([true, true]);
  });

  // Would catch: metadata of another revision merged into the committed layout (frames of a layout
  // the surface no longer shows).
  it("metadata of a revision other than the committed full frame is ignored", () => {
    const a = pane("w1:p1", rect(1, 1, 40, 20), true);
    const committed = applySurfaceMetadata({ revision: 0, panes: [] }, frame(7));
    expect(applySurfaceMetadata(committed, full(6, [a])).panes).toEqual([]);
    expect(applySurfaceMetadata(committed, full(7, [a])).panes).toHaveLength(1);
  });

  // Would catch: a full metadata that does not replace the layout (panes of a closed split kept).
  it("a full metadata (with the link table) replaces the layout", () => {
    const a = pane("w1:p1", rect(1, 1, 40, 20), true);
    const b = pane("w1:p2", rect(43, 1, 40, 20));
    const two = applySurfaceMetadata(applySurfaceMetadata({ revision: 0, panes: [] }, frame(9)), full(9, [a, b]));
    const one = applySurfaceMetadata(applySurfaceMetadata(two, frame(10)), full(10, [a]));
    expect(one.panes.map((p) => p.pane_id)).toEqual(["w1:p1"]);
  });
});

describe("confirmed target of the header actions (AC-013-01)", () => {
  const identity = { endpoint: "local", session: "hd013", connection_generation: 3, boot_id: "boot-013", pane_id: "w3:p1" };
  const agentsIdentity = { endpoint: "local", session: "hd013", connection_generation: 3, boot_id: "boot-013" };
  const topology = (focused: string | null, panes: string[] = ["w3:p1"]) => ({
    revision: 1,
    width: 80,
    height: 24,
    focused_pane_id: focused,
    panes: panes.map((pane_id) => ({ pane_id, x: 0, y: 0, width: 80, height: 24, focused: pane_id === focused, cwd: null })),
    splits: [],
  });

  // Would catch: the actions addressing the pane of the agents topology (which can be stale after a
  // host switch), sending tab.create/pane.split to a pane the engine no longer lists.
  it("the target is the pane the surface confirmed, and only when the topology agrees", () => {
    expect(confirmedTarget({ identity, agents: { identity: agentsIdentity, topology: topology("w3:p1") } })).toEqual({ pane_id: "w3:p1" });
    // The gate of the run of r3: topology focused on a pane the surface never confirmed.
    expect(confirmedTarget({ identity, agents: { identity: agentsIdentity, topology: topology("w3:p2", ["w3:p2"]) } })).toBeNull();
    expect(confirmedTarget({ identity, agents: { identity: agentsIdentity, topology: topology(null) } })).toBeNull();
    expect(confirmedTarget({ identity: null, agents: { identity: agentsIdentity, topology: topology("w3:p1") } })).toBeNull();
    expect(confirmedTarget({ identity, agents: null })).toBeNull();
  });

  // Would catch: a target accepted across a reconnect/reboot/host of another connection.
  it("refuses a topology of another connection (endpoint, session, generation or boot)", () => {
    const other = (patch: Partial<typeof agentsIdentity>) =>
      confirmedTarget({ identity, agents: { identity: { ...agentsIdentity, ...patch }, topology: topology("w3:p1") } });
    expect(other({ endpoint: "dev-box" })).toBeNull();
    expect(other({ session: "outra" })).toBeNull();
    expect(other({ connection_generation: 4 })).toBeNull();
    expect(other({ boot_id: "boot-outro" })).toBeNull();
    expect(other({})).toEqual({ pane_id: "w3:p1" });
  });

  // Would catch: a pane confirmed by the surface but absent from the topology's pane list (the
  // engine's pane.list would refuse the action).
  it("refuses a pane that is not in the topology's pane list", () => {
    expect(confirmedTarget({ identity, agents: { identity: agentsIdentity, topology: topology("w3:p1", ["w3:p9"]) } })).toBeNull();
  });

  // Would catch: buttons enabled while the action would be silently refused (the r3 failure), or a
  // disabled button without a reason for the user.
  it("without a confirmed target the actions are disabled with a reason", () => {
    const enabled = actionAvailability({ target: { pane_id: "w3:p1" }, unavailable: { split: null, newTab: null, newAgent: null } });
    expect(enabled).toEqual({ split: null, newTab: null, newAgent: null });
    const waiting = actionAvailability({ target: null, unavailable: { split: null, newTab: null, newAgent: null } });
    expect(Object.values(waiting).every((reason) => typeof reason === "string" && reason.length > 0)).toBe(true);
    expect(waiting.split).toBe("Aguardando o servidor confirmar o pane em foco");
    // The host reasons the window already computes keep precedence.
    expect(actionAvailability({ target: null, unavailable: { split: "Indisponível: host desconectado", newTab: null, newAgent: null } }).split).toBe(
      "Indisponível: host desconectado",
    );
  });
});

describe("name of the agent started from the header (AC-013-01)", () => {
  // Would catch (the gate r5 of spec 013 found it): a name with a space or a colon, which the
  // engine refuses ("agent name must start with a lowercase letter and contain only lowercase
  // letters, digits, '-' or '_' (1-32 characters)"), so no agent is ever started.
  it("is accepted by the engine's rule and identifies the pane", () => {
    const rule = /^[a-z][a-z0-9_-]{0,31}$/;
    expect(agentName("pi", "w3:p4")).toBe("pi-w3-p4");
    expect(rule.test(agentName("pi", "w3:p4"))).toBe(true);
    expect(rule.test(agentName("Claude Code", "w12:p3"))).toBe(true);
    expect(rule.test(agentName("", "w1:p1"))).toBe(true);
    expect(rule.test(agentName("2fast", "w1:p1"))).toBe(true);
    expect(agentName("pi", "w1:p1").length).toBeLessThanOrEqual(32);
    expect(rule.test(agentName("x".repeat(40), "w1:p1"))).toBe(true);
    // Two panes never get the same name (the engine refuses a repeated agent name).
    expect(agentName("pi", "w1:p1")).not.toBe(agentName("pi", "w1:p2"));
  });
});

// ---------------------------------------------------------------------------------------
// Spec 025 AC-025-04 — the center belongs to ONE engine workspace: the tab bar, the counts and
// the + are scoped to the focused workspace and the whole bar is replaced when the focus moves.
// Would catch: the session-wide `tab.list` rendered as-is (5 tabs of 4 workspaces on screen).
// ---------------------------------------------------------------------------------------

describe("workspace-scoped tabs and counts (AC-025-04)", () => {
  const four = [
    tab("w1:t1", "api", 2),
    tab("w1:t2", "migrations", 1),
    tab("w2:t1", "site", 3),
    tab("w2:t2", "deploy", 1),
  ];

  // Would catch: filtering by the tab id instead of its workspace, or inventing a tab.
  it("lists only the tabs of the given workspace", () => {
    expect(workspaceTabs({ tabs: four, agents: [], focusedTabId: "w2:t1", workspaceId: "w1" }).map((t) => t.tab_id)).toEqual([
      "w1:t1",
      "w1:t2",
    ]);
    expect(workspaceTabs({ tabs: four, agents: [], focusedTabId: "w1:t1", workspaceId: "w2" }).map((t) => t.tab_id)).toEqual([
      "w2:t1",
      "w2:t2",
    ]);
    // Without a known focused workspace the engine list stays whole (legacy callers).
    expect(workspaceTabs({ tabs: four, agents: [], focusedTabId: null }).map((t) => t.tab_id)).toHaveLength(4);
  });

  // Would catch: the fallback reading `tab.list[].focused` (global) instead of the connection's
  // confirmed focus, or another host's snapshot scoping the selected host's bar.
  it("focusedWorkspaceId prefers the selected host's snapshot and falls back to the confirmed tab", () => {
    const hosts = [
      {
        endpoint: "local",
        workspaces: [
          { workspace_id: "w2", number: 2, label: "site", focused: true, tab_count: 2, pane_count: 1, active_tab_id: "w2:t1", agent_status: "idle" as const, cwd: "/srv/site", branch: null },
          { workspace_id: "w1", number: 1, label: "api", focused: false, tab_count: 2, pane_count: 1, active_tab_id: "w1:t1", agent_status: "idle" as const, cwd: "/srv/api", branch: null },
        ],
      },
      {
        endpoint: "ssh-dev",
        workspaces: [
          { workspace_id: "w9", number: 1, label: "remote", focused: true, tab_count: 1, pane_count: 1, active_tab_id: "w9:t1", agent_status: "idle" as const, cwd: null, branch: null },
        ],
      },
    ];
    expect(focusedWorkspaceId({ hosts, endpoint: "local", tabs: four, focusedTabId: "w1:t1" })).toBe("w2");
    // The snapshot of another host never scopes the selected host.
    expect(focusedWorkspaceId({ hosts, endpoint: "ssh-dev", tabs: four, focusedTabId: null })).toBe("w9");
    // No host snapshot: the tab the connection confirmed answers.
    expect(focusedWorkspaceId({ hosts: null, endpoint: "local", tabs: four, focusedTabId: "w1:t2" })).toBe("w1");
    expect(focusedWorkspaceId({ hosts: [], endpoint: null, tabs: four, focusedTabId: null })).toBeNull();
  });

  // Would catch: status bar counting the session (all workspaces) instead of the focused one.
  it("counts panes and tabs of the focused workspace only", () => {
    const panes = [{ pane_id: "w1:p1" }, { pane_id: "w1:p2" }, { pane_id: "w2:p1" }];
    expect(workspaceCounts({ tabs: four, panes, workspaceId: "w1" })).toEqual({ tabs: 2, panes: 2 });
    expect(workspaceCounts({ tabs: four, panes, workspaceId: "w2" })).toEqual({ tabs: 2, panes: 1 });
    expect(workspaceCounts({ tabs: four, panes, workspaceId: null })).toEqual({ tabs: 4, panes: 3 });
  });
});
