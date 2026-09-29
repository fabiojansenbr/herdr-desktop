// Spec 007 — call order of the Tauri connections bridge. The backend runs every connections
// command off the GUI thread with no shared lane, in the order its tasks first run; two
// conflicting invokes sent together could race for that first run. The bridge therefore sends
// conflicting calls one at a time, in JS call order: connect/cancel of one host (and a save of
// that host's profile), profile save/import, and text to one host (after lifecycle calls of that
// host already made). Reads (list/watch/workspaces) never wait, independent hosts never wait for
// each other, a pending text never delays cancel, and nothing is re-sent.
import { describe, expect, it } from "vitest";
import { tauriConnectionsBridge, type ConnectionsIpc } from "./bridge";
import type { ConnectionsView, ImportView, QualifiedTarget, SshProfileDraft } from "./types";

interface Deferred<T> {
  promise: Promise<T>;
  resolve(value: T): void;
  reject(error: unknown): void;
}
function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}
const settle = async () => {
  for (let i = 0; i < 5; i++) await new Promise((resolve) => setTimeout(resolve, 0));
};

interface Call {
  command: string;
  args: Record<string, unknown> | undefined;
  reply: Deferred<unknown>;
}

/** Fake IPC: every invoke is recorded (args deep-copied) when sent and answers only when told. */
function fakeIpc() {
  const calls: Call[] = [];
  const ipc: ConnectionsIpc = {
    invoke: <T>(command: string, args?: Record<string, unknown>) => {
      const reply = deferred<unknown>();
      calls.push({ command, args: args === undefined ? undefined : structuredClone(args), reply });
      return reply.promise as Promise<T>;
    },
  };
  const sent = () => calls.map((call) => `${call.command}${label(call.args)}`);
  const call = (index: number): Call => {
    const found = calls[index];
    if (!found) throw new Error(`invoke #${index} was never sent: ${sent().join(", ")}`);
    return found;
  };
  return { ipc, calls, sent, call };
}

function label(args: Record<string, unknown> | undefined): string {
  if (!args) return "";
  if (typeof args.endpoint === "string") return `:${args.endpoint}`;
  const target = args.target as QualifiedTarget | undefined;
  if (target) return `:${target.endpoint}:${String(args.text)}`;
  const draft = args.draft as SshProfileDraft | undefined;
  if (draft) return `:${draft.id ?? "novo"}`;
  return "";
}

function view(revision: number): ConnectionsView {
  return { hub: { revision, hosts: [] }, profiles: [], store_error: null };
}

const HOST_A = "ssh-hd007j-a";
const HOST_B = "ssh-hd007j-b";
const HOST_C = "ssh-hd007j-c";

function target(endpoint: string, boot: string): QualifiedTarget {
  return {
    endpoint,
    session: `sess-${endpoint}`,
    connection_generation: 3,
    boot_id: boot,
    workspace_id: "w1",
    pane_id: "w1:p1",
  };
}

function draft(id: string | null, label: string): SshProfileDraft {
  return { id, label, target: "tester@10.0.0.4", port: null, session: "hd007j-sess" };
}

describe("tauriConnectionsBridge call order", () => {
  // Would catch: connect/cancel of one host invoked together (a later cancel overtaking a connect
  // in the backend's first poll), an error that never releases the next call, results delivered
  // to the wrong caller, or any lifecycle call sent twice.
  it("sends connect, cancel and connect of one host one at a time; errors release the next", async () => {
    const fake = fakeIpc();
    const bridge = tauriConnectionsBridge(fake.ipc);
    const outcomes: string[] = [];
    const connect1 = bridge.connect(HOST_A);
    const cancel = bridge.cancel(HOST_A);
    const connect2 = bridge.connect(HOST_A);
    connect1.catch(() => outcomes.push("connect1-error"));
    cancel.then(() => outcomes.push("cancel"));
    connect2.then(() => outcomes.push("connect2"));

    await settle();
    expect(fake.sent()).toEqual([`connection_connect:${HOST_A}`]);

    const offline = { code: "endpoint_unknown", message: "sem host", retryable: false, endpoint: HOST_A };
    fake.call(0).reply.reject(offline);
    await expect(connect1).rejects.toBe(offline);
    await settle();
    expect(fake.sent()).toEqual([`connection_connect:${HOST_A}`, `connection_cancel:${HOST_A}`]);

    fake.call(1).reply.resolve(view(11));
    await expect(cancel).resolves.toEqual(view(11));
    await settle();
    expect(fake.sent()).toEqual([
      `connection_connect:${HOST_A}`,
      `connection_cancel:${HOST_A}`,
      `connection_connect:${HOST_A}`,
    ]);
    fake.call(2).reply.resolve(view(12));
    await expect(connect2).resolves.toEqual(view(12));
    await settle();
    expect(outcomes).toEqual(["connect1-error", "cancel", "connect2"]);
    expect(fake.calls).toHaveLength(3);
    expect(fake.call(1).args).toEqual({ endpoint: HOST_A });
  });

  // Would catch: one global queue (host B's connect or host C's text waiting for host A's attempt), reads
  // put behind a pending mutation, or a cancel held behind a long watch.
  it("reads, other hosts and a later cancel are sent immediately", async () => {
    const fake = fakeIpc();
    const bridge = tauriConnectionsBridge(fake.ipc);
    const watch = bridge.watch(40);
    const connectA = bridge.connect(HOST_A);
    const connectB = bridge.connect(HOST_B);
    const textC = bridge.sendText(target(HOST_C, "boot-hd007j-c2"), "gama-hd007j", true);
    const list = bridge.list();
    const workspaces = bridge.workspaces(HOST_A);
    const save = bridge.saveProfile(draft(null, "novo-hd007j"), false);
    await settle();
    expect(fake.sent()).toEqual([
      "connections_watch",
      `connection_connect:${HOST_A}`,
      `connection_connect:${HOST_B}`,
      `connection_send_text:${HOST_C}:gama-hd007j`,
      "connections_list",
      `connection_workspaces:${HOST_A}`,
      "connection_profile_save:novo",
    ]);
    expect(fake.call(0).args).toEqual({ revision: 40 });
    expect(fake.call(3).args).toEqual({ target: target(HOST_C, "boot-hd007j-c2"), text: "gama-hd007j", submit: true });
    expect(fake.call(6).args).toEqual({ draft: draft(null, "novo-hd007j"), connect: false });

    // A cancel of B after its connect settled does not wait for the pending watch or host A.
    fake.call(2).reply.resolve(view(41));
    await expect(connectB).resolves.toEqual(view(41));
    const cancelB = bridge.cancel(HOST_B);
    await settle();
    expect(fake.sent()).toContain(`connection_cancel:${HOST_B}`);
    fake.call(7).reply.resolve(view(42));
    await expect(cancelB).resolves.toEqual(view(42));

    for (const [index, value] of [
      [0, view(42)],
      [1, view(43)],
      [3, undefined],
      [4, view(43)],
      [5, [{ workspace_id: "w1", label: "um" }]],
      [6, view(44)],
    ] as const) {
      fake.call(index).reply.resolve(value);
    }
    await expect(watch).resolves.toEqual(view(42));
    await expect(connectA).resolves.toEqual(view(43));
    await expect(textC).resolves.toBeUndefined();
    await expect(list).resolves.toEqual(view(43));
    await expect(workspaces).resolves.toEqual([{ workspace_id: "w1", label: "um" }]);
    await expect(save).resolves.toEqual(view(44));
    expect(fake.calls).toHaveLength(8);
  });

  // Would catch: save and import mutating the profile store in either order, a save of a host's
  // profile (which may connect it) racing that host's cancel, or import held behind a connect.
  it("orders profile save/import, and a host's save with its connect/cancel", async () => {
    const fake = fakeIpc();
    const bridge = tauriConnectionsBridge(fake.ipc);
    const cancelA = bridge.cancel(HOST_A);
    const saveA = bridge.saveProfile(draft(HOST_A, "editado-hd007j"), true);
    const imported = bridge.importProfiles();
    const connectB = bridge.connect(HOST_B);
    const connectA = bridge.connect(HOST_A);
    await settle();
    expect(fake.sent()).toEqual([`connection_cancel:${HOST_A}`, `connection_connect:${HOST_B}`]);

    fake.call(0).reply.resolve(view(50));
    await expect(cancelA).resolves.toEqual(view(50));
    await settle();
    expect(fake.sent().slice(2)).toEqual([`connection_profile_save:${HOST_A}`]);

    const inUse = { code: "profile_in_use", message: "cancele antes", retryable: false, endpoint: HOST_A };
    fake.call(2).reply.reject(inUse);
    await expect(saveA).rejects.toBe(inUse);
    await settle();
    expect(fake.sent().slice(3).sort()).toEqual(["connection_profiles_import", `connection_connect:${HOST_A}`].sort());

    const report: ImportView = { report: { imported: ["tui-hd007j"], skipped: [], already_present: 0 }, view: view(51) };
    const importCall = fake.calls.findIndex((call) => call.command === "connection_profiles_import");
    fake.call(importCall).reply.resolve(report);
    await expect(imported).resolves.toEqual(report);
    const connectCall = fake.calls.findIndex((call, index) => index > 2 && call.command === "connection_connect");
    fake.call(connectCall).reply.resolve(view(52));
    await expect(connectA).resolves.toEqual(view(52));
    fake.call(1).reply.resolve(view(53));
    await expect(connectB).resolves.toEqual(view(53));
    await settle();
    expect(fake.calls).toHaveLength(5);
  });

  // Would catch: text to one host sent concurrently (two writes racing in the backend), text
  // overtaking an earlier cancel of its host, a cancel held behind a stuck text, text of another
  // host waiting, a failed text retried, or a queued text reading a target the caller mutated.
  it("sends text per host in order after earlier lifecycle calls, with the identity of the call", async () => {
    const fake = fakeIpc();
    const bridge = tauriConnectionsBridge(fake.ipc);
    const targetA = target(HOST_A, "boot-hd007j-a1");
    const text1 = bridge.sendText(targetA, "um-hd007j", false);
    const text2 = bridge.sendText(targetA, "dois-hd007j", true);
    const cancelA = bridge.cancel(HOST_A);
    const text3 = bridge.sendText(targetA, "tres-hd007j", false);
    const textB = bridge.sendText(target(HOST_B, "boot-hd007j-b1"), "beta-hd007j", false);
    // The caller reuses its object for the next connection; queued calls keep the old identity.
    targetA.boot_id = "boot-hd007j-a2";
    targetA.connection_generation = 4;

    await settle();
    expect(fake.sent()).toEqual([
      `connection_send_text:${HOST_A}:um-hd007j`,
      `connection_cancel:${HOST_A}`,
      `connection_send_text:${HOST_B}:beta-hd007j`,
    ]);

    const lost = { code: "result_unknown", message: "sem resposta", retryable: false, endpoint: HOST_A };
    fake.call(0).reply.reject(lost);
    await expect(text1).rejects.toBe(lost);
    await settle();
    expect(fake.sent()[3]).toBe(`connection_send_text:${HOST_A}:dois-hd007j`);
    expect(fake.call(3).args).toEqual({ target: target(HOST_A, "boot-hd007j-a1"), text: "dois-hd007j", submit: true });
    expect(fake.calls).toHaveLength(4);

    const offline = { code: "host_offline", message: "desconectado", retryable: true, endpoint: HOST_A };
    fake.call(3).reply.reject(offline);
    await expect(text2).rejects.toBe(offline);
    await settle();
    expect(fake.calls).toHaveLength(4);

    fake.call(1).reply.resolve(view(60));
    await expect(cancelA).resolves.toEqual(view(60));
    await settle();
    expect(fake.sent()[4]).toBe(`connection_send_text:${HOST_A}:tres-hd007j`);
    expect(fake.call(4).args).toEqual({ target: target(HOST_A, "boot-hd007j-a1"), text: "tres-hd007j", submit: false });
    fake.call(4).reply.reject(offline);
    await expect(text3).rejects.toBe(offline);
    fake.call(2).reply.resolve(undefined);
    await expect(textB).resolves.toBeUndefined();
    await settle();
    expect(fake.calls).toHaveLength(5);
    expect(fake.calls.filter((call) => call.args?.text === "um-hd007j")).toHaveLength(1);
  });

  // Would catch: a queued save reading a draft the form kept editing.
  it("a queued save sends the draft as it was when called", async () => {
    const fake = fakeIpc();
    const bridge = tauriConnectionsBridge(fake.ipc);
    const first = bridge.saveProfile(draft(null, "primeiro-hd007j"), false);
    const form = draft(null, "segundo-hd007j");
    const second = bridge.saveProfile(form, true);
    form.label = "digitado-depois";
    form.target = "outro@10.0.0.99";
    await settle();
    expect(fake.calls).toHaveLength(1);
    fake.call(0).reply.resolve(view(70));
    await expect(first).resolves.toEqual(view(70));
    await settle();
    expect(fake.call(1).args).toEqual({ draft: draft(null, "segundo-hd007j"), connect: true });
    fake.call(1).reply.resolve(view(71));
    await expect(second).resolves.toEqual(view(71));
  });
});
