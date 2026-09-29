// Spec 010 (AC-010-02) — status bar text from the live session: server version and state, branch,
// panes/tabs and release channel. Text always carries the state (never color only).
import { describe, expect, it } from "vitest";
import { statusBarModel, type StatusInput } from "./status";

const live: StatusInput = { phase: "live", hasEndpoint: true, serverVersion: "0.9.0", branch: "main", panes: 3, tabs: 2 };

describe("status bar model", () => {
  // Would catch: a hardcoded version, a state other than the surface phase, counts not from the
  // session, or a channel not derived from the engine version.
  it("shows the connected server, branch, counts and channel from the session", () => {
    expect(statusBarModel(live)).toEqual({ server: "herdr server 0.9.0 · conectado", branch: "main", counts: "3 panes · 2 abas", channel: "canal stable", agents: null, cwd: null });
    expect(statusBarModel({ ...live, serverVersion: "0.9.1-preview.42", panes: 1, tabs: 1, agents: 4, cwd: "/work/erp-api" })).toEqual({
      server: "herdr server 0.9.1-preview.42 · conectado",
      branch: "main",
      counts: "1 pane · 1 aba",
      channel: "canal preview",
      agents: "4 agentes ativos",
      cwd: "/work/erp-api",
    });
  });

  // Would catch: a fabricated branch when the engine reports none.
  it("shows an em dash when the engine reports no branch", () => {
    expect(statusBarModel({ ...live, branch: null }).branch).toBe("—");
    expect(statusBarModel({ ...live, branch: undefined }).branch).toBe("—");
  });

  // Would catch: "conectado" (or stale counts/channel) while there is no session; state by color only.
  it("without a session says desconectado in text", () => {
    expect(statusBarModel({ phase: "empty", hasEndpoint: false, serverVersion: null, branch: null, panes: null, tabs: null })).toEqual({
      server: "desconectado",
      branch: "—",
      counts: null,
      channel: null,
      agents: null,
      cwd: null,
    });
    expect(statusBarModel({ ...live, phase: "disconnected" }).server).toBe("herdr server 0.9.0 · desconectado");
    expect(statusBarModel({ ...live, phase: "connecting", serverVersion: null }).server).toBe("herdr server · conectando");
    expect(statusBarModel({ ...live, phase: "stale" }).server).toBe("herdr server 0.9.0 · ressincronizando");
    expect(statusBarModel({ ...live, phase: "switching" }).server).toBe("herdr server 0.9.0 · trocando de host");
  });
});
