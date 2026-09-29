// Spec 013 — the expand button of a pane frame: one qualified `pane.zoom` per click, refused when
// the server does not announce it. The backend contract is src-tauri/tests/center_runtime.rs.
import { describe, expect, it } from "vitest";
import type { AgentsBridge } from "./bridge";
import { createAgentsController } from "./controller";
import type { Capabilities, Overview, QualifiedTarget, ZoomMode } from "./types";

const identity = { endpoint: "local", session: "hd013", connection_generation: 3, boot_id: "boot-013" };

function caps(zoom: boolean): Capabilities {
  return {
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
    zoom,
  };
}

function overview(zoom: boolean): Overview {
  return {
    state: "live",
    session: "hd013",
    identity,
    server_version: "0.9.0",
    capabilities: caps(zoom),
    kinds: ["pi"],
    agents: [],
    tabs: [],
    topology: {
      revision: 1,
      width: 80,
      height: 22,
      focused_pane_id: "w1:p1",
      panes: [
        { pane_id: "w1:p1", x: 0, y: 0, width: 40, height: 22, focused: true, cwd: "/work" },
        { pane_id: "w1:p2", x: 40, y: 0, width: 40, height: 22, focused: false, cwd: null },
      ],
      splits: [],
    },
    error: null,
  };
}

function controllerWith(zoom: boolean) {
  const sent: { target: QualifiedTarget; mode: ZoomMode }[] = [];
  const bridge = {
    connect: async () => overview(zoom),
    overview: async () => overview(zoom),
    detach: async () => overview(zoom),
    startAgent: async () => {
      throw new Error("not used");
    },
    prompt: async () => {
      throw new Error("not used");
    },
    openAttention: async () => {},
    split: async () => {},
    focusPane: async () => {},
    setSplitRatio: async () => {},
    input: async () => {},
    createTab: async () => {},
    focusTab: async () => {},
    zoomPane: async (target: QualifiedTarget, mode: ZoomMode) => {
      sent.push({ target, mode });
    },
    closeTab: async () => {},
    renameTab: async () => {},
    closePane: async () => {},
  } satisfies AgentsBridge;
  const controller = createAgentsController(bridge);
  return { controller, sent };
}

describe("pane zoom (AC-013-02)", () => {
  // Would catch: an expand button that sends nothing, or one that sends an unqualified target the
  // backend would have to trust (host/boot/generation are the identity of the action).
  it("sends one qualified toggle per click", async () => {
    const { controller, sent } = controllerWith(true);
    await controller.connect({ cols: 80, rows: 22, cell_width_px: 9, cell_height_px: 19 });
    await controller.zoomPane("w1:p2");
    expect(sent).toHaveLength(1);
    expect(sent[0]!.mode).toBe("toggle");
    expect(sent[0]!.target).toMatchObject({ ...identity, pane_id: "w1:p2" });
    await controller.zoomPane("w1:p2", "off");
    expect(sent.map((s) => s.mode)).toEqual(["toggle", "off"]);
  });

  // Would catch: a button offered (and a request sent) on a server that never announced pane.zoom.
  it("sends nothing when the server does not offer zoom", async () => {
    const { controller, sent } = controllerWith(false);
    await controller.connect({ cols: 80, rows: 22, cell_width_px: 9, cell_height_px: 19 });
    await controller.zoomPane("w1:p1");
    expect(sent).toEqual([]);
  });

  // Would catch: a zoom of a pane that is not in the topology this connection confirmed.
  it("sends nothing for a pane outside the confirmed topology", async () => {
    const { controller, sent } = controllerWith(true);
    await controller.connect({ cols: 80, rows: 22, cell_width_px: 9, cell_height_px: 19 });
    await controller.zoomPane("w9:p9");
    expect(sent).toEqual([]);
  });
});

describe("tab list reconciled by the confirmed focus (AC-013-01)", () => {
  function controllerWithTabs() {
    const tabs = [{ tab_id: "w1:t1", workspace_id: "w1", label: "1", focused: true, pane_count: 1, agent_status: "idle" as const }];
    const engineTabs = [...tabs, { tab_id: "w1:t2", workspace_id: "w1", label: "2", focused: true, pane_count: 1, agent_status: "idle" as const }];
    let overviews = 0;
    let emit: ((event: import("./types").AgentsEvent) => void) | null = null;
    const base = overview(true);
    const bridge = {
      connect: async (_g: unknown, onEvent: (event: import("./types").AgentsEvent) => void) => {
        emit = onEvent;
        return { ...base, tabs };
      },
      overview: async () => {
        overviews += 1;
        return { ...base, tabs: engineTabs };
      },
      detach: async () => base,
      startAgent: async () => {
        throw new Error("not used");
      },
      prompt: async () => {
        throw new Error("not used");
      },
      openAttention: async () => {},
      split: async () => {},
      focusPane: async () => {},
      setSplitRatio: async () => {},
      input: async () => {},
      createTab: async () => {},
      focusTab: async () => {},
      zoomPane: async () => {},
      closeTab: async () => {},
    renameTab: async () => {},
    closePane: async () => {},
    } satisfies AgentsBridge;
    const controller = createAgentsController(bridge);
    return { controller, count: () => overviews, fire: (tab_id: string) => emit!({ type: "tab_focus", focus: { ...identity, revision: 2, tab_id } }) };
  }

  // Would catch (the gate r4 of spec 013 found it): the window focused on a tab it does not list,
  // because the engine event that announces the new tab never arrived.
  it("asks for the list once when the confirmed tab is unknown", async () => {
    const { controller, count, fire } = controllerWithTabs();
    await controller.connect({ cols: 80, rows: 22, cell_width_px: 9, cell_height_px: 19 });
    expect(controller.state.tabs.map((t) => t.tab_id)).toEqual(["w1:t1"]);
    fire("w1:t2");
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(count()).toBe(1);
    expect(controller.state.tabs.map((t) => t.tab_id)).toEqual(["w1:t1", "w1:t2"]);
    // A confirmation of a tab already listed asks for nothing.
    fire("w1:t1");
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(count()).toBe(1);
  });
});
