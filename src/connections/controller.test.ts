// Spec 003 — connections controller over a fake bridge that records every command.
import { describe, expect, it } from "vitest";
import { createConnectionsController } from "./controller";
import { createFakeConnectionsBridge, SSH_ENDPOINT } from "./fake-bridge";

async function loaded() {
  const bridge = createFakeConnectionsBridge();
  const controller = createConnectionsController(bridge);
  await controller.load();
  bridge.calls.length = 0;
  return { bridge, controller };
}

describe("connections controller", () => {
  // Empty state: rendering/loading never connects a host.
  it("loads with a single list command and connects nothing", async () => {
    const bridge = createFakeConnectionsBridge();
    const controller = createConnectionsController(bridge);
    await controller.load();
    expect(bridge.calls.map((c) => c.command)).toEqual(["connections_list"]);
    expect(controller.state.view!.hub.hosts.map((h) => h.phase)).toEqual(["offline", "offline"]);
  });

  // AC-003-01 at the WebView seam. Would catch: input addressed by pane id alone, or the
  // Local buffer consumed by an SSH send.
  it("sends SSH input with the SSH target even when Local has the same pane id", async () => {
    const { bridge, controller } = await loaded();
    bridge.setOnline("local", "boot-local-7");
    bridge.setOnline(SSH_ENDPOINT, "boot-remote-1");
    await controller.refresh();
    controller.editInput("local", "w1:p1", "echo local");
    controller.editInput(SSH_ENDPOINT, "w1:p1", "echo remoto");
    await controller.sendInput(SSH_ENDPOINT, "w1:p1");
    expect(bridge.calls.filter((c) => c.command === "connection_send_text")).toEqual([
      {
        command: "connection_send_text",
        args: {
          target: { endpoint: SSH_ENDPOINT, session: "hd003-remote-b", connection_generation: 1, boot_id: "boot-remote-1", workspace_id: "w1", pane_id: "w1:p1" },
          text: "echo remoto",
          submit: true,
        },
      },
    ]);
    expect(controller.state.inputs[`${SSH_ENDPOINT}/w1:p1`]).toBe("");
    expect(controller.state.inputs["local/w1:p1"]).toBe("echo local");
  });

  // AC-003-01 loss + AC-003-02 no replay. Would catch: queued input sent when the host comes
  // back, or Local blocked while SSH reconnects.
  it("does not send or queue input while SSH reconnects and never resends after it returns", async () => {
    const { bridge, controller } = await loaded();
    bridge.setOnline("local", "boot-local-7");
    bridge.setOnline(SSH_ENDPOINT, "boot-remote-1");
    await controller.refresh();
    bridge.setReconnecting(SSH_ENDPOINT);
    await controller.refresh();
    controller.editInput(SSH_ENDPOINT, "w1:p1", "echo perdido");
    await controller.sendInput(SSH_ENDPOINT, "w1:p1");
    controller.editInput("local", "w1:p1", "echo local");
    await controller.sendInput("local", "w1:p1");
    bridge.setOnline(SSH_ENDPOINT, "boot-remote-2", 2);
    await controller.refresh();
    const sends = bridge.calls.filter((c) => c.command === "connection_send_text");
    expect(sends.map((c) => (c.args.target as { endpoint: string }).endpoint)).toEqual(["local"]);
    expect(controller.state.inputs[`${SSH_ENDPOINT}/w1:p1`]).toBe("echo perdido");
    expect(controller.state.hostErrors[SSH_ENDPOINT]).toBeUndefined();
  });

  // Would catch: a failed send retried automatically, its buffer lost, or its error shown on
  // the other host.
  it("keeps the buffer and the error on the host whose send failed", async () => {
    const { bridge, controller } = await loaded();
    bridge.setOnline("local", "boot-local-7");
    bridge.setOnline(SSH_ENDPOINT, "boot-remote-1");
    await controller.refresh();
    bridge.failNextSend({ code: "target_generation_stale", message: "a conexão foi renovada", retryable: false, endpoint: SSH_ENDPOINT });
    controller.editInput(SSH_ENDPOINT, "w1:p1", "echo x");
    await controller.sendInput(SSH_ENDPOINT, "w1:p1");
    await controller.refresh();
    expect(bridge.calls.filter((c) => c.command === "connection_send_text")).toHaveLength(1);
    expect(controller.state.inputs[`${SSH_ENDPOINT}/w1:p1`]).toBe("echo x");
    expect(controller.state.hostErrors[SSH_ENDPOINT]?.code).toBe("target_generation_stale");
    expect(controller.state.hostErrors.local).toBeUndefined();
  });

  // Would catch: invalid drafts reaching the backend or the typed buffer lost on errors.
  it("validates the SSH form before saving and connects explicitly", async () => {
    const { bridge, controller } = await loaded();
    controller.openDialog();
    controller.editDraft("label", "dev-box");
    controller.editDraft("target", "-oProxyCommand=x");
    controller.editDraft("session", "trabalho");
    await controller.submitDialog(true);
    expect(bridge.calls).toEqual([]);
    expect(controller.state.dialog.errors.target).toBe("O host não pode começar com '-'");
    expect(controller.state.dialog.draft.label).toBe("dev-box");
    controller.editDraft("target", "user@dev-box");
    controller.editDraft("port", "2222");
    await controller.submitDialog(true);
    expect(bridge.calls).toEqual([
      {
        command: "connection_profile_save",
        args: { draft: { id: null, label: "dev-box", target: "user@dev-box", port: 2222, session: "trabalho", auth: "key" }, connect: true },
      },
    ]);
    // Spec 011 (AC-011-03): the dialog stays open while the connection it asked for advances,
    // bound to the real host the backend registered and ready for a retry on the same profile.
    const profile = controller.state.view!.profiles.find((p) => p.target === "user@dev-box" && p.session === "trabalho");
    expect(profile).toBeDefined();
    expect(controller.state.dialog.open).toBe(true);
    expect(controller.state.dialog.connecting).toBe(profile!.id);
    expect(controller.state.dialog.draft.id).toBe(profile!.id);
  });

  // Spec 034 (AC-034-01): a brand-new dialog already carries the engine's default session and
  // the form submits it like any other (the old refusal of `default` is revoked).
  it("opens a new dialog with the default session and submits it", async () => {
    const { bridge, controller } = await loaded();
    controller.openDialog();
    expect(controller.state.dialog.draft.session).toBe("default");
    controller.editDraft("label", "mac-mini");
    controller.editDraft("target", "ec2-user@mac-mini");
    await controller.submitDialog(false);
    expect(controller.state.dialog.errors).toEqual({});
    expect(bridge.calls).toEqual([
      {
        command: "connection_profile_save",
        args: { draft: { id: null, label: "mac-mini", target: "ec2-user@mac-mini", port: null, session: "default", auth: "key" }, connect: false },
      },
    ]);
  });

  // Spec 031 (AC-031-02): the dialog's auth choice travels in the draft the backend validates
  // (an ssh-agent choice without a session socket is refused before any connection).
  it("sends the chosen ssh-agent authentication in the draft", async () => {
    const { bridge, controller } = await loaded();
    controller.openDialog();
    controller.editDraft("label", "dev-box");
    controller.editDraft("target", "user@dev-box");
    controller.editDraft("session", "trabalho");
    controller.editDraft("auth", "ssh-agent");
    await controller.submitDialog(true);
    expect(bridge.calls[0]!.args).toEqual({
      draft: { id: null, label: "dev-box", target: "user@dev-box", port: null, session: "trabalho", auth: "ssh-agent" },
      connect: true,
    });
  });

  // Spec 011 (AC-011-03): the steps follow the real connection — measured handshake, negotiated
  // version and the host's own workspace list — and the dialog closes when the read settles.
  it("reads the connecting host's workspaces once online and closes the dialog when done", async () => {
    const { bridge, controller } = await loaded();
    controller.openDialog();
    controller.editDraft("label", "dev-box-rc");
    controller.editDraft("target", "ana@dev-box");
    controller.editDraft("session", "trabalho");
    await controller.submitDialog(true);
    const endpoint = controller.state.dialog.connecting!;
    expect(endpoint).toBeTruthy();
    expect(bridge.calls.map((c) => c.command)).toEqual(["connection_profile_save"]);
    expect(controller.state.workspaces[endpoint]).toBeUndefined();

    expect(controller.state.dialog.open).toBe(true);
    bridge.setOnline(endpoint, "boot-rc", 3, { latency_ms: 251, server_version: "0.9.0-rc.7" });
    await controller.refresh();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(bridge.calls.map((c) => c.command)).toEqual(["connection_profile_save", "connections_list", "connection_workspaces"]);
    expect(controller.state.workspaces[endpoint]).toEqual([{ workspace_id: "w1", label: "work" }]);
    expect(controller.state.dialog.open).toBe(false);
    expect(controller.state.dialog.connecting).toBeNull();
  });

  it("turns a workspace timeout into a visible retry state instead of leaving a spinner", async () => {
    const { bridge, controller } = await loaded();
    controller.openDialog();
    controller.editDraft("label", "dev-box-timeout");
    controller.editDraft("target", "ana@dev-box");
    controller.editDraft("session", "trabalho");
    await controller.submitDialog(true);
    const endpoint = controller.state.dialog.connecting!;
    bridge.workspaces = async () => {
      bridge.calls.push({ command: "connection_workspaces", args: { endpoint } });
      throw { code: "timeout", message: "sem resposta ao ler workspaces", retryable: true, endpoint };
    };
    bridge.setOnline(endpoint, "boot-timeout", 1, { latency_ms: 15000, server_version: "0.9.0" });
    await controller.refresh();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(controller.state.dialog.open).toBe(true);
    expect(controller.state.dialog.connecting).toBe(endpoint);
    expect(controller.state.hostErrors[endpoint]?.message).toBe("sem resposta ao ler workspaces");
    expect(bridge.calls.filter((call) => call.command === "connection_workspaces")).toHaveLength(1);
  });

  it("cancels the backend attempt when the connection dialog is closed", async () => {
    const { bridge, controller } = await loaded();
    controller.openDialog();
    controller.editDraft("label", "dev-box-cancel");
    controller.editDraft("target", "ana@dev-box");
    controller.editDraft("session", "trabalho");
    await controller.submitDialog(true);
    const endpoint = controller.state.dialog.connecting!;

    controller.cancelDialog();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(bridge.calls.filter((call) => call.command === "connection_cancel")).toEqual([
      { command: "connection_cancel", args: { endpoint } },
    ]);
    expect(controller.state.dialog.open).toBe(false);
    expect(controller.state.view!.hub.hosts.find((host) => host.endpoint === endpoint)?.cancelled).toBe(true);
  });

  // Spec 029 (AC-029-03): disconnect/reconnect/remove are one explicit bridge call each; a
  // removed profile leaves the view, a disconnected host goes offline without its workspaces and
  // Local refuses the destructive actions.
  it("maps disconnect, reconnect and remove to one bridge call each", async () => {
    const { bridge, controller } = await loaded();
    bridge.setOnline(SSH_ENDPOINT, "boot-remote-1");
    await controller.refresh();
    bridge.calls.length = 0;

    await controller.disconnect(SSH_ENDPOINT);
    expect(bridge.calls.map((c) => c.command)).toEqual(["connection_disconnect"]);
    const offline = controller.state.view!.hub.hosts.find((h) => h.endpoint === SSH_ENDPOINT)!;
    expect(offline.phase).toBe("offline");
    expect(offline.cancelled).toBe(true);
    expect(offline.panes).toEqual([]);
    expect(offline.workspaces).toEqual([]);

    bridge.calls.length = 0;
    await controller.reconnect(SSH_ENDPOINT);
    expect(bridge.calls.map((c) => c.command)).toEqual(["connection_reconnect"]);
    expect(controller.state.view!.hub.hosts.find((h) => h.endpoint === SSH_ENDPOINT)!.phase).toBe("connecting");

    bridge.calls.length = 0;
    await controller.removeProfile(SSH_ENDPOINT);
    expect(bridge.calls.map((c) => c.command)).toEqual(["connection_remove"]);
    expect(controller.state.view!.hub.hosts.some((h) => h.endpoint === SSH_ENDPOINT)).toBe(false);
    expect(controller.state.view!.profiles.some((p) => p.id === SSH_ENDPOINT)).toBe(false);

    bridge.setOnline("local", "boot-local-1");
    await controller.refresh();
    bridge.calls.length = 0;
    await controller.disconnect("local");
    expect(bridge.calls.map((c) => c.command)).toEqual(["connection_disconnect"]);
    expect(controller.state.hostErrors.local?.code).toBe("endpoint_local");
    expect(controller.state.view!.hub.hosts.find((h) => h.endpoint === "local")!.phase).toBe("online");
  });

  // Spec 029 (AC-029-03): Editar… opens the dialog bound to the saved profile (no bridge call);
  // the submit updates that same profile.
  it("opens the edit dialog from the saved profile without a backend call", async () => {
    const { bridge, controller } = await loaded();
    await controller.editProfile(SSH_ENDPOINT);
    expect(bridge.calls).toEqual([]);
    expect(controller.state.dialog.open).toBe(true);
    expect(controller.state.dialog.draft.id).toBe(SSH_ENDPOINT);
    expect(controller.state.dialog.draft.label).toBe("dev-box");
    expect(controller.state.dialog.draft.target).toBe("user@dev-box");
    expect(controller.state.dialog.draft.session).toBe("hd003-remote-b");
    expect(controller.state.dialog.draft.port).toBe("");
    await controller.submitDialog(false);
    expect(bridge.calls).toEqual([
      {
        command: "connection_profile_save",
        args: {
          draft: { id: SSH_ENDPOINT, label: "dev-box", target: "user@dev-box", port: null, session: "hd003-remote-b", auth: "key" },
          connect: false,
        },
      },
    ]);
  });

  // Attention: retry only by explicit action; import reads profiles only through its command.
  it("retries attention only on request and imports profiles explicitly", async () => {
    const { bridge, controller } = await loaded();
    bridge.setAttention(SSH_ENDPOINT, "host_key_unknown");
    await controller.refresh();
    await controller.refresh();
    expect(bridge.calls.map((c) => c.command)).toEqual(["connections_list", "connections_list"]);
    await controller.connect(SSH_ENDPOINT);
    await controller.importProfiles();
    expect(bridge.calls.map((c) => c.command)).toEqual([
      "connections_list",
      "connections_list",
      "connection_connect",
      "connection_profiles_import",
    ]);
    expect(controller.state.importReport?.imported).toEqual(["build-box"]);
  });
});
