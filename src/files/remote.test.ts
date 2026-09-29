// Spec 006 — remote explorer state and controller over a scripted bridge. The backend (SFTP
// provider) stays the authority on identity, limits and errors; these tests prove what the
// WebView does with its answers: host badge and read-only tabs, stale cache after a lost
// connection, reload only on the new connection, diff of two identified snapshots, cancel and
// errors kept on the affected resource.
import { describe, expect, it } from "vitest";
import {
  REMOTE_PROVIDER,
  createRemoteFilesController,
  remoteErrorHint,
  remoteViewModel,
  type RemoteFileTarget,
  type RemoteFilesBridge,
  type RemoteHostDto,
  type RemoteHostsView,
  type RemoteTextSnapshotDto,
} from "./remote";
import type { FileEntryDto, FilePageDto, FileUriDto, RuntimeError } from "./types";

const A = "0123456789abcdef0123456789abcdef";
const B = "fedcba9876543210fedcba9876543210";
const ROOT = "/srv/projeto";

function host(endpoint: string, label: string, over: Partial<RemoteHostDto> = {}): RemoteHostDto {
  return {
    endpoint,
    label,
    session: endpoint === A ? "hd006-remote-a" : "hd006-remote-b",
    phase: "online",
    phase_label: "Online",
    online: true,
    connection_generation: 1,
    boot_id: "boot-a-1",
    roots: [ROOT],
    provider: REMOTE_PROVIDER,
    capabilities: { list: true, read: true, stat: true, write: false },
    ...over,
  };
}

function uri(path: string, endpoint = A): FileUriDto {
  return { provider: REMOTE_PROVIDER, host: endpoint, path };
}

function entry(name: string, kind: FileEntryDto["kind"] = "File", endpoint = A): FileEntryDto {
  return { uri: uri(`${ROOT}/${name}`, endpoint), name, kind };
}

interface Call {
  method: string;
  opId?: string;
  target?: RemoteFileTarget;
  uri?: FileUriDto;
  cursor?: string | null;
}

interface Pending<T> {
  resolve(value: T): void;
  reject(error: RuntimeError): void;
}

/** Bridge whose answers are released by the test, recording every call. */
function scripted() {
  const calls: Call[] = [];
  const pending: Pending<unknown>[] = [];
  const cancelled: string[] = [];
  let view: RemoteHostsView = { revision: 1, hosts: [] };
  const wait = <T>(call: Call) =>
    new Promise<T>((resolve, reject) => {
      calls.push(call);
      pending.push({ resolve: resolve as (v: unknown) => void, reject });
    });
  const bridge: RemoteFilesBridge = {
    hosts: async () => view,
    watch: async () => view,
    list: (opId, target, u, cursor) => wait<FilePageDto & { connection: RemoteTextSnapshotDto["connection"] }>({ method: "list", opId, target, uri: u, cursor }),
    read: (opId, target, u) => wait<RemoteTextSnapshotDto>({ method: "read", opId, target, uri: u }),
    stat: (opId, target, u) => wait({ method: "stat", opId, target, uri: u }),
    cancel: async (opId) => {
      cancelled.push(opId);
      return true;
    },
  };
  return {
    bridge,
    calls,
    cancelled,
    setHosts(hosts: RemoteHostDto[]) {
      view = { revision: view.revision + 1, hosts };
      return view;
    },
    /** Resolves the oldest unanswered call. */
    answer(value: unknown) {
      const next = pending.shift();
      if (!next) throw new Error("no pending call");
      next.resolve(value);
    },
    /** Resolves the unanswered call at `index` (0 = oldest), out of order. */
    answerAt(index: number, value: unknown) {
      const [next] = pending.splice(index, 1);
      if (!next) throw new Error(`no pending call at ${index}`);
      next.resolve(value);
    },
    fail(error: RuntimeError) {
      const next = pending.shift();
      if (!next) throw new Error("no pending call");
      next.reject(error);
    },
    get unanswered() {
      return pending.length;
    },
  };
}

function stamp(h: RemoteHostDto, channel = 1) {
  return {
    endpoint: h.endpoint,
    session: h.session,
    connection_generation: h.connection_generation!,
    boot_id: h.boot_id!,
    channel,
  };
}

function page(h: RemoteHostDto, entries: FileEntryDto[], next: string | null = null) {
  return { uri: uri(ROOT, h.endpoint), entries, next_cursor: next, connection: stamp(h) };
}

let snapshotCounter = 0;
function snapshot(h: RemoteHostDto, name: string, content: string, channel = 1): RemoteTextSnapshotDto {
  snapshotCounter += 1;
  return {
    id: `snap${snapshotCounter}-0000-4000-8000-000000000000`,
    uri: uri(`${ROOT}/${name}`, h.endpoint),
    content,
    bom: false,
    eol: "lf",
    size: content.length,
    modified_unix_ms: null,
    connection: stamp(h, channel),
  };
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

function ids() {
  let n = 0;
  return () => `op-${++n}`;
}

async function withRoot(h: RemoteHostDto[], names: FileEntryDto[]) {
  const s = scripted();
  const controller = createRemoteFilesController(s.bridge, () => {}, ids());
  await controller.applyHosts(s.setHosts(h));
  const selecting = controller.selectHost(h[0]!.endpoint);
  await flush();
  s.answer(page(h[0]!, names));
  await selecting;
  return { s, controller };
}

describe("remote files — AC-006-01 host isolation and read-only tabs", () => {
  // Would catch: an operation addressed without the SSH host identity (or with a local uri),
  // a tab without the host badge / read-only flag, or a save path offered for remote files.
  it("lists and reads only through the selected SSH host and marks tabs as read-only", async () => {
    const a = host(A, "Servidor SSH");
    const { s, controller } = await withRoot([a, host(B, "Outro host", { boot_id: "boot-b-9" })], [entry("notas.txt"), entry("sub", "Directory")]);
    expect(s.calls[0]).toEqual({
      method: "list",
      opId: "op-1",
      target: { endpoint: A, session: "hd006-remote-a", connection_generation: 1, boot_id: "boot-a-1" },
      uri: { provider: "sftp", host: A, path: ROOT },
      cursor: null,
    });

    const opening = controller.openFile(entry("notas.txt"));
    await flush();
    s.answer(snapshot(a, "notas.txt", "conteudo REMOTO\n"));
    await opening;
    expect(s.calls[1]!.method).toBe("read");
    expect(s.calls[1]!.uri).toEqual(uri(`${ROOT}/notas.txt`));
    expect(s.calls[1]!.target!.endpoint).toBe(A);

    const view = remoteViewModel(controller.state);
    expect(view.host?.label).toBe("Servidor SSH");
    expect(view.readOnly).toBe(true);
    const tab = view.active!;
    expect(tab.badge).toEqual({ host: "Servidor SSH", readOnly: true, stale: false });
    expect(tab.content).toBe("conteudo REMOTO\n");
    expect(tab.stale).toBe(false);
    expect("canSave" in tab).toBe(false);
    expect(Object.keys(s.bridge)).not.toContain("save");
    expect(view.nodes.map((n) => n.name)).toEqual(["notas.txt", "sub"]);
  });

  // Would catch: listing (or spawning anything) while no SSH host is online, i.e. work per
  // render instead of an explicit empty state.
  it("keeps the explorer disabled with onboarding until a host is online", async () => {
    const s = scripted();
    const controller = createRemoteFilesController(s.bridge, () => {}, ids());
    expect(remoteViewModel(controller.state).onboarding).toBe("no_host");
    await controller.applyHosts(s.setHosts([host(A, "Servidor SSH", { phase: "offline", phase_label: "Offline", online: false, connection_generation: null, boot_id: null })]));
    await controller.selectHost(A);
    await controller.loadRoot();
    const view = remoteViewModel(controller.state);
    expect(view.canList).toBe(false);
    expect(view.onboarding).toBe("host_offline");
    expect(s.calls).toEqual([]);
  });
});

describe("remote files — AC-006-02 stale cache and reconnection", () => {
  // Would catch: cached content shown as live after the connection drops, reload attempted on
  // the lost connection, reload addressed to the old generation, or the tab left stale after a
  // successful reload on the new connection.
  it("shows the cached read as Desatualizado and reloads only on the new connection", async () => {
    const a1 = host(A, "Servidor SSH");
    const { s, controller } = await withRoot([a1], [entry("notas.txt")]);
    const opening = controller.openFile(entry("notas.txt"));
    await flush();
    const first = snapshot(a1, "notas.txt", "linha um\nlinha dois\n");
    s.answer(first);
    await opening;

    await controller.applyHosts(s.setHosts([host(A, "Servidor SSH", { phase: "reconnecting", phase_label: "Reconectando", online: false })]));
    let tab = remoteViewModel(controller.state).active!;
    expect(tab.stale).toBe(true);
    expect(tab.badge.stale).toBe(true);
    expect(tab.content).toBe("linha um\nlinha dois\n");
    expect(tab.canReload).toBe(false);
    await controller.reload(tab.id);
    expect(s.calls.filter((c) => c.method === "read")).toHaveLength(1);

    const a2 = host(A, "Servidor SSH", { connection_generation: 2 });
    await controller.applyHosts(s.setHosts([a2]));
    const view = remoteViewModel(controller.state);
    tab = view.active!;
    expect(tab.stale).toBe(true);
    expect(tab.staleReason).toBe("connection_renewed");
    expect(view.treeStale).toBe(true);
    expect(tab.canReload).toBe(true);

    const reloading = controller.reload(tab.id);
    await flush();
    const read = s.calls.at(-1)!;
    expect(read.method).toBe("read");
    expect(read.target).toEqual({ endpoint: A, session: "hd006-remote-a", connection_generation: 2, boot_id: "boot-a-1" });
    const second = snapshot(a2, "notas.txt", "linha um\nlinha 2 nova\nlinha tres\n", 2);
    s.answer(second);
    await reloading;
    tab = remoteViewModel(controller.state).active!;
    expect(tab.stale).toBe(false);
    expect(tab.content).toBe("linha um\nlinha 2 nova\nlinha tres\n");
    expect(tab.canCompare).toBe(true);

    // AC-006-03: diff between the two identified snapshots.
    controller.toggleCompare(tab.id);
    const diff = remoteViewModel(controller.state).active!.diff!;
    expect(diff.baseLabel).toBe(`leitura ${first.id.slice(0, 8)} · geração 1`);
    expect(diff.currentLabel).toBe(`leitura ${second.id.slice(0, 8)} · geração 2`);
    expect(diff.lines.filter((l) => l.op === "removed").map((l) => l.text)).toEqual(["linha dois"]);
    expect(diff.lines.filter((l) => l.op === "added").map((l) => l.text)).toEqual(["linha 2 nova", "linha tres"]);
    expect([diff.added, diff.removed, diff.identical]).toEqual([2, 1, false]);
  });

  // Would catch: a new boot not treated as a different connection, or the explorer keeping
  // pages (and cursors) of the previous connection as if they were live.
  it("treats a new boot as a renewed connection and reloads the tree explicitly", async () => {
    const a1 = host(A, "Servidor SSH");
    const { s, controller } = await withRoot([a1], [entry("velho.txt")]);
    await controller.applyHosts(s.setHosts([host(A, "Servidor SSH", { boot_id: "boot-a-2" })]));
    let view = remoteViewModel(controller.state);
    expect(view.treeStale).toBe(true);
    expect(view.rootHasMore).toBe(false);
    const loading = controller.loadRoot();
    await flush();
    expect(s.calls.at(-1)!.target!.boot_id).toBe("boot-a-2");
    expect(s.calls.at(-1)!.cursor).toBeNull();
    s.answer({ ...page(a1, [entry("novo.txt")]), connection: { ...stamp(a1), boot_id: "boot-a-2" } });
    await loading;
    view = remoteViewModel(controller.state);
    expect(view.treeStale).toBe(false);
    expect(view.nodes.map((n) => n.name)).toEqual(["novo.txt"]);
  });
});

describe("remote files — AC-006-03 errors, cancel and late answers", () => {
  // Would catch: cancelling without telling the backend which operation, or a cancelled reload
  // discarding the cached snapshot (buffers must survive cancellation).
  it("cancels the tab's own operation and keeps the cached snapshot", async () => {
    const a = host(A, "Servidor SSH");
    const { s, controller } = await withRoot([a], [entry("notas.txt"), entry("outro.txt")]);
    const opening = controller.openFile(entry("notas.txt"));
    await flush();
    s.answer(snapshot(a, "notas.txt", "em cache\n"));
    await opening;
    const other = controller.openFile(entry("outro.txt"));
    await flush();
    s.answer(snapshot(a, "outro.txt", "outro\n"));
    await other;
    controller.focusTab(`sftp|${A}|${ROOT}/notas.txt`);

    const reloading = controller.reload(`sftp|${A}|${ROOT}/notas.txt`);
    await flush();
    const opId = s.calls.at(-1)!.opId!;
    expect(remoteViewModel(controller.state).active!.canCancel).toBe(true);
    await controller.cancel(`sftp|${A}|${ROOT}/notas.txt`);
    expect(s.cancelled).toEqual([opId]);
    s.fail({ code: "operation_cancelled", message: "operação cancelada", retryable: false });
    await reloading;
    const view = remoteViewModel(controller.state);
    expect(view.active!.content).toBe("em cache\n");
    expect(view.active!.error?.code).toBe("operation_cancelled");
    expect(view.active!.canCancel).toBe(false);
    expect(view.tabs.find((t) => t.name === "outro.txt")!.error).toBeNull();
  });

  // Would catch: a file error affecting other tabs or the tree, a missing SFTP subsystem shown
  // as a generic connection failure, or the explanation that SSH ≠ SFTP missing (TASK-006-05).
  it("keeps binary, permission and missing-SFTP errors on the affected resource", async () => {
    const a = host(A, "Servidor SSH");
    const b = host(B, "Sem SFTP", { boot_id: "boot-b-9" });
    const { s, controller } = await withRoot([a, b], [entry("notas.txt"), entry("bin.dat"), entry("segredo.txt")]);
    const good = controller.openFile(entry("notas.txt"));
    await flush();
    s.answer(snapshot(a, "notas.txt", "texto\n"));
    await good;
    for (const [name, code] of [["bin.dat", "binary_unsupported"], ["segredo.txt", "permission_denied"]] as const) {
      const opening = controller.openFile(entry(name));
      await flush();
      s.fail({ code, message: `erro ${code}`, retryable: false, endpoint: A });
      await opening;
      const view = remoteViewModel(controller.state);
      expect(view.active!.status).toBe("failed");
      expect(view.active!.error?.code).toBe(code);
      expect(view.active!.content).toBeNull();
      expect(view.rootError).toBeNull();
    }
    const view = remoteViewModel(controller.state);
    expect(view.tabs.find((t) => t.name === "notas.txt")!.content).toBe("texto\n");

    const selecting = controller.selectHost(B);
    await flush();
    expect(s.calls.at(-1)!.target!.endpoint).toBe(B);
    s.fail({ code: "sftp_unavailable", message: "o SSH deste host funciona, mas o subsistema SFTP não está disponível", retryable: false, endpoint: B });
    await selecting;
    const onB = remoteViewModel(controller.state);
    expect(onB.rootError?.code).toBe("sftp_unavailable");
    expect(onB.rootHint).toBe(remoteErrorHint("sftp_unavailable"));
    expect(onB.rootHint).toMatch(/SSH/);
    expect(onB.rootHint).toMatch(/SFTP/);
    expect(onB.rootHint).toMatch(/terminais/);
    expect(onB.tabs.find((t) => t.name === "notas.txt")!.content).toBe("texto\n");
  });

  // Would catch: an older answer (e.g. a reload started before another one) overwriting the
  // newer snapshot shown in the tab.
  it("drops answers of superseded operations", async () => {
    const a = host(A, "Servidor SSH");
    const { s, controller } = await withRoot([a], [entry("notas.txt")]);
    const opening = controller.openFile(entry("notas.txt"));
    await flush();
    s.answer(snapshot(a, "notas.txt", "v1\n"));
    await opening;
    const id = remoteViewModel(controller.state).active!.id;
    const first = controller.reload(id);
    await flush();
    const second = controller.reload(id);
    await flush();
    expect(s.unanswered).toBe(2);
    // The newer reload answers first; the older answer arrives afterwards and must be dropped.
    s.answerAt(1, snapshot(a, "notas.txt", "nova\n"));
    await second;
    s.answer(snapshot(a, "notas.txt", "antiga\n"));
    await first;
    const tab = remoteViewModel(controller.state).active!;
    expect(tab.content).toBe("nova\n");
    expect(tab.loading).toBe(false);
  });

  // Would catch: the remote workspace offering save, importing the editor statically, or
  // mounting it without the read-only option.
  it("the remote workspace has no save action and loads the editor lazily read-only", () => {
    const sources = import.meta.glob("../components/RemoteFilesWorkspace.svelte", {
      query: "?raw",
      import: "default",
      eager: true,
    }) as Record<string, string>;
    const component = Object.values(sources)[0]!;
    expect(component).toBeTruthy();
    expect(component).not.toMatch(/Salvar/);
    expect(component).not.toMatch(/\.save\(/);
    expect(component).toMatch(/import\(\s*["']\.\.\/editor\/editor["']\s*\)/);
    expect(component).not.toMatch(/^\s*import\s+(?!type\s)[^;]*from\s+["'][^"']*editor\/editor["']/m);
    expect(component).toMatch(/RemoteFileBadge/);
  });
});
