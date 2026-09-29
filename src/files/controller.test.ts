// Spec 005 — controller over the in-memory bridge: every user intent is one command, errors
// stay on the affected tab and listing/reading never happen during render.
import { describe, expect, it } from "vitest";
import { createFilesController } from "./controller";
import { createFakeFilesBridge } from "./fake-bridge";
import { targetUri, viewModel } from "./reducer";
import { localUri, type FileEntryDto, type FileTarget } from "./types";

const target: FileTarget = { provider: "local", host: null, root: "/projeto" };
const notasUri = localUri("/projeto/notas.txt");

function entry(name: string, kind: FileEntryDto["kind"] = "File", dir = "/projeto"): FileEntryDto {
  return { uri: localUri(`${dir}/${name}`), name, kind };
}

function harness(): { bridge: ReturnType<typeof createFakeFilesBridge>; controller: ReturnType<typeof createFilesController> } {
  // More files than one page so loadMore has a real cursor to continue from.
  const files: Record<string, string> = {
    "/projeto/notas.txt": "linha um\nlinha dois\n",
    "/projeto/outro.txt": "outro arquivo\n",
    "/projeto/sub/um.txt": "dentro do subdiretorio\n",
  };
  for (let index = 0; index < 130; index++) {
    files[`/projeto/p${String(index).padStart(3, "0")}.txt`] = `pagina ${index}\n`;
  }
  const bridge = createFakeFilesBridge({ files });
  return { bridge, controller: createFilesController(bridge) };
}

function commands(bridge: ReturnType<typeof createFakeFilesBridge>): string[] {
  return bridge.calls.map((call) => call.command);
}

describe("one intent, one command", () => {
  // Would catch: a component fetching inside render, an edit sending a command, or the watcher
  // listing directories instead of stat-ing only the open files.
  it("maps listing, reading, saving and releasing to their commands", async () => {
    const { bridge, controller } = harness();
    await controller.setTarget(target);
    expect(bridge.calls).toEqual([
      { command: "files_list", args: { uri: targetUri(target), cursor: null } },
    ]);

    await controller.loadMore(targetUri(target));
    expect(commands(bridge)).toEqual(["files_list", "files_list"]);
    expect(bridge.calls[1]!.args.cursor).toBeTruthy();

    await controller.openFile(entry("notas.txt"));
    expect(commands(bridge)).toEqual(["files_list", "files_list", "files_read"]);
    expect(bridge.calls[2]!.args).toEqual({ uri: notasUri });

    const id = controller.state.active!;
    controller.edit(id, "linha editada\n");
    expect(commands(bridge), "editing is in-memory only").toHaveLength(3);
    expect(viewModel(controller.state).active!.dirty).toBe(true);

    const baseId = controller.state.tabs[id]!.base!.id;
    await controller.save(id);
    expect(commands(bridge)).toEqual(["files_list", "files_list", "files_read", "files_save"]);
    expect(bridge.calls[3]!.args).toEqual({ snapshotId: baseId, content: "linha editada\n" });
    expect(controller.state.tabs[id]!.dirty).toBe(false);

    const savedId = controller.state.tabs[id]!.base!.id;
    await controller.close(id, "discard");
    expect(commands(bridge)).toEqual([
      "files_list",
      "files_list",
      "files_read",
      "files_save",
      "files_release",
    ]);
    expect(bridge.calls[4]!.args).toEqual({ snapshotIds: [savedId] });
    expect(controller.state.tabs[id]).toBeUndefined();
  });

  // Would catch: listing/reading triggered by a viewModel call, or a watcher that walks the
  // tree (files_list) instead of stat-ing open files only.
  it("keeps render pure and watches only open files", async () => {
    const { bridge, controller } = harness();
    await controller.setTarget(target);
    const listCalls = () => commands(bridge).filter((command) => command === "files_list").length;
    const lists = listCalls();
    viewModel(controller.state);
    viewModel(controller.state);
    expect(listCalls()).toBe(lists);

    await controller.openFile(entry("notas.txt"));
    bridge.writeExternal("/projeto/notas.txt", "versao externa\n");
    await controller.pollOpenFiles();
    expect(commands(bridge).filter((command) => command === "files_stat")).toHaveLength(1);
    expect(listCalls()).toBe(lists);
    expect(viewModel(controller.state).active!.external).toBe(true);

    await controller.pollOpenFiles();
    expect(commands(bridge).filter((command) => command === "files_stat")).toHaveLength(2);
    expect(controller.state.tabs[controller.state.active!]!.external).toBe(true);
  });
});

describe("errors stay on the affected tab", () => {
  // Would catch: a binary/large file opening the editor or failing the whole explorer.
  it("shows the specific message for a file that cannot be edited", async () => {
    const { bridge, controller } = harness();
    await controller.setTarget(target);
    bridge.failRead("/projeto/bin.dat", {
      code: "binary_unsupported",
      message: "o arquivo não é texto UTF-8 (ou contém NUL); o editor não foi carregado",
      retryable: false,
    });
    await controller.openFile(entry("bin.dat"));
    const view = viewModel(controller.state);
    expect(view.active!.status).toBe("failed");
    expect(view.active!.error!.code).toBe("binary_unsupported");
    expect(view.active!.canSave).toBe(false);
    expect(view.rootError).toBeNull();
  });

  // Would catch: a read failure on one file clearing another tab's buffer or the listing.
  it("a failing file never touches another open buffer", async () => {
    const { bridge, controller } = harness();
    await controller.setTarget(target);
    await controller.openFile(entry("notas.txt"));
    const notasId = controller.state.active!;
    controller.edit(notasId, "rascunho\n");
    bridge.failRead("/projeto/outro.txt", { code: "file_not_found", message: "x", retryable: false });
    await controller.openFile(entry("outro.txt"));
    expect(controller.state.tabs[notasId]!.buffer).toBe("rascunho\n");
    expect(controller.state.tabs[notasId]!.error).toBeNull();
  });
});

describe("conflict choices", () => {
  // Would catch: a conflict reporting success, overwriting the disk version, or discarding the
  // dirty buffer before the user picks reload.
  it("never writes on conflict and keeps the buffer dirty", async () => {
    const { bridge, controller } = harness();
    await controller.setTarget(target);
    await controller.openFile(entry("notas.txt"));
    const id = controller.state.active!;
    controller.edit(id, "linha do conflito\n");
    bridge.writeExternal("/projeto/notas.txt", "versao externa\nagente\n");

    await controller.save(id);
    const tab = controller.state.tabs[id]!;
    expect(tab.dirty).toBe(true);
    expect(tab.saving).toBe(false);
    expect(tab.conflict!.current!.content).toBe("versao externa\nagente\n");
    expect(tab.buffer).toBe("linha do conflito\n");
    expect(bridge.files.get("/projeto/notas.txt")).toBe("versao externa\nagente\n");

    controller.compare(id);
    expect(viewModel(controller.state).active!.diff!.base).toBe("disk");

    await controller.saveCopy(id);
    const recovery = controller.state.tabs[id]!.recovery!;
    expect(recovery.path).toContain("herdr-recuperacao");
    expect(bridge.files.get(recovery.path)).toBe("linha do conflito\n");
    expect(bridge.files.get("/projeto/notas.txt")).toBe("versao externa\nagente\n");
    expect(controller.state.tabs[id]!.buffer).toBe("linha do conflito\n");
  });

  // Would catch: reload happening without an explicit action or keeping dirty/conflict state.
  it("reload adopts the disk version only when asked", async () => {
    const { bridge, controller } = harness();
    await controller.setTarget(target);
    await controller.openFile(entry("notas.txt"));
    const id = controller.state.active!;
    controller.edit(id, "rascunho\n");
    bridge.writeExternal("/projeto/notas.txt", "externo\n");
    await controller.save(id);
    expect(controller.state.tabs[id]!.conflict).not.toBeNull();

    await controller.reload(id);
    const tab = controller.state.tabs[id]!;
    expect([tab.buffer, tab.dirty, tab.conflict, tab.reloading]).toEqual(["externo\n", false, null, false]);
  });

  // Would catch: cancel losing the text, or discard keeping the tab.
  it("asks before closing and cancel preserves the buffer", async () => {
    const { bridge, controller } = harness();
    await controller.setTarget(target);
    await controller.openFile(entry("notas.txt"));
    const id = controller.state.active!;
    controller.edit(id, "nao perder\n");
    controller.requestClose(id);
    expect(viewModel(controller.state).active!.closePrompt).toBe(true);
    controller.cancelClose(id);
    expect(controller.state.tabs[id]!.buffer).toBe("nao perder\n");
    expect(controller.state.tabs[id]!.dirty).toBe(true);

    await controller.close(id, "discard");
    expect(controller.state.tabs[id]).toBeUndefined();
    expect(commands(bridge)).toContain("files_release");
  });

  // Would catch: "save and close" closing a tab whose save conflicted.
  it("save and close keeps the tab open when the save conflicts", async () => {
    const { bridge, controller } = harness();
    await controller.setTarget(target);
    await controller.openFile(entry("notas.txt"));
    const id = controller.state.active!;
    controller.edit(id, "rascunho\n");
    bridge.writeExternal("/projeto/notas.txt", "externo\n");
    await controller.close(id, "save");
    expect(controller.state.tabs[id]!.conflict).not.toBeNull();
    expect(controller.state.tabs[id]!.buffer).toBe("rascunho\n");
  });
});
