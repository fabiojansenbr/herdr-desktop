// Spec 012 — the home IPC bridge: command names and arguments of the home screen, and that a
// refusal is surfaced instead of falling back to another host or another command.

import { describe, expect, it } from "vitest";
import { tauriHomeBridge, type HomeIpc } from "./bridge";

function fakeIpc() {
  const calls: { command: string; args?: Record<string, unknown> }[] = [];
  const ipc: HomeIpc = {
    invoke: async <T>(command: string, args?: Record<string, unknown>): Promise<T> => {
      calls.push({ command, args });
      return { user: "Fábio", pane_id: "w1:p1", text: "linha\n", revision: 2, truncated: false } as T;
    },
  };
  return { ipc, calls };
}

describe("AC-012-01/02 home bridge", () => {
  it("reads the system user and one pane snapshot with the qualified target", async () => {
    const { ipc, calls } = fakeIpc();
    const bridge = tauriHomeBridge(ipc);
    const user = await bridge.systemUser();
    const snapshot = await bridge.paneRead(
      { endpoint: "local", session: "hd012", connection_generation: 1, boot_id: "boot-a", pane_id: "w1:p1" },
      "w1:p1",
      8,
    );
    // Would catch: another command name, a read without the target or a wrong line bound.
    expect(user.user).toBe("Fábio");
    expect(snapshot.text).toBe("linha\n");
    expect(calls).toEqual([
      { command: "system_user", args: undefined },
      {
        command: "pane_read",
        args: {
          target: { endpoint: "local", session: "hd012", connection_generation: 1, boot_id: "boot-a", pane_id: "w1:p1" },
          paneId: "w1:p1",
          lines: 8,
        },
      },
    ]);
  });

  it("surfaces a refusal instead of retrying or falling back", async () => {
    const ipc: HomeIpc = {
      invoke: async () => {
        throw { code: "target_stale", message: "a conexão do alvo mudou antes do envio; ação não enviada" };
      },
    };
    const bridge = tauriHomeBridge(ipc);
    await expect(
      bridge.paneRead(
        { endpoint: "local", session: "hd012", connection_generation: 1, boot_id: "boot-a", pane_id: "w1:p1" },
        "w1:p1",
        8,
      ),
    ).rejects.toMatchObject({ code: "target_stale" });
  });
});
