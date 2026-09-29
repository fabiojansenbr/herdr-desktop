// Spec 005 — files state (pure). Fixtures use distinct hosts/contents/ids so a value that
// leaks between tabs, targets or snapshots changes the observed result.
import { describe, expect, it } from "vitest";
import { initialState, reduce, targetUri, viewModel, type FilesState } from "./reducer";
import { localUri, uriKey, type FileEntryDto, type FileKind, type FileTarget, type FileUriDto, type TextSnapshotDto } from "./types";

const target: FileTarget = { provider: "local", host: null, root: "/projeto" };
const otherTarget: FileTarget = { provider: "local", host: "boot-beta", root: "/projeto" };
const rootUri: FileUriDto = targetUri(target);
const subUri = localUri("/projeto/sub");
const notas = localUri("/projeto/notas.txt");
const outro = localUri("/projeto/outro.txt");

function snapshot(id: string, uri: FileUriDto, content: string): TextSnapshotDto {
  return { id, uri, content, bom: false, eol: "lf", size: content.length, modified_unix_ms: 1 };
}

function entry(name: string, kind: FileKind = "File", dir = "/projeto"): FileEntryDto {
  return { uri: localUri(`${dir}/${name}`), name, kind };
}

function loaded(state: FilesState, id: string, content: string, token = 1): FilesState {
  const tab = state.tabs[id]!;
  return reduce(state, { type: "tab_loaded", id, token, snapshot: snapshot(`snap-${id}-${content.length}`, tab.uri, content) });
}

describe("explorer pagination", () => {
  // Would catch: append replacing the first page, duplicated entries on overlap, a cursor kept
  // after the last page, or an empty page reported as loading.
  it("appends pages by cursor and never refetches the whole tree", () => {
    let state = reduce(initialState(), { type: "target_set", target });
    state = reduce(state, { type: "dir_load_started", uri: rootUri });
    state = reduce(state, {
      type: "dir_loaded",
      uri: rootUri,
      entries: [entry("notas.txt"), entry("outro.txt")],
      nextCursor: "opaque-1",
      append: false,
    });
    const first = state.dirs[uriKey(rootUri)]!;
    expect([first.entries.map((e) => e.name), first.nextCursor, first.loading]).toEqual([
      ["notas.txt", "outro.txt"],
      "opaque-1",
      false,
    ]);
    expect(viewModel(state).rootHasMore).toBe(true);

    state = reduce(state, {
      type: "dir_loaded",
      uri: rootUri,
      entries: [entry("z.txt")],
      nextCursor: null,
      append: true,
    });
    expect(state.dirs[uriKey(rootUri)]!.entries.map((e) => e.name)).toEqual(["notas.txt", "outro.txt", "z.txt"]);
    expect(state.dirs[uriKey(rootUri)]!.nextCursor).toBeNull();
    expect(viewModel(state).rootHasMore).toBe(false);

    state = reduce(state, {
      type: "dir_loaded",
      uri: rootUri,
      entries: [entry("z.txt"), entry("z2.txt")],
      nextCursor: null,
      append: true,
    });
    expect(state.dirs[uriKey(rootUri)]!.entries.map((e) => e.name)).toEqual([
      "notas.txt",
      "outro.txt",
      "z.txt",
      "z2.txt",
    ]);
  });

  // Would catch: a recursive tree walk on render (children shown before the directory page is
  // loaded), or collapse discarding the loaded page and refetching it later.
  it("renders children only for expanded directories, keeping loaded pages", () => {
    let state = reduce(initialState(), { type: "target_set", target });
    state = reduce(state, {
      type: "dir_loaded",
      uri: rootUri,
      entries: [entry("sub", "Directory"), entry("notas.txt")],
      nextCursor: null,
      append: false,
    });
    let view = viewModel(state);
    expect(view.nodes.map((node) => [node.name, node.expanded, node.children.length])).toEqual([
      ["sub", false, 0],
      ["notas.txt", false, 0],
    ]);

    state = reduce(state, { type: "dir_expanded", uri: subUri, expanded: true });
    state = reduce(state, { type: "dir_load_started", uri: subUri });
    state = reduce(state, {
      type: "dir_loaded",
      uri: subUri,
      entries: [entry("um.txt", "File", "/projeto/sub")],
      nextCursor: "opaque-sub",
      append: false,
    });
    view = viewModel(state);
    const sub = view.nodes[0]!;
    expect([sub.expanded, sub.loading, sub.hasMore]).toEqual([true, false, true]);
    expect(sub.children.map((node) => [node.name, node.depth, node.uri.path])).toEqual([
      ["um.txt", 1, "/projeto/sub/um.txt"],
    ]);

    state = reduce(state, { type: "dir_expanded", uri: subUri, expanded: false });
    view = viewModel(state);
    expect(view.nodes[0]!.children).toEqual([]);
    expect(state.dirs[uriKey(subUri)]!.entries).toHaveLength(1);
  });

  // Would catch: an empty listing rendered as a blank tree without an explicit empty state, or
  // a loading page treated as empty.
  it("marks an empty loaded root as an explicit empty state", () => {
    let state = reduce(initialState(), { type: "target_set", target });
    expect(viewModel(state).empty).toBe(false);
    state = reduce(state, { type: "dir_load_started", uri: rootUri });
    expect(viewModel(state).empty).toBe(false);
    state = reduce(state, { type: "dir_loaded", uri: rootUri, entries: [], nextCursor: null, append: false });
    expect(viewModel(state).empty).toBe(true);
    state = reduce(state, { type: "dir_failed", uri: rootUri, error: { code: "file_io_error", message: "x", retryable: false } });
    expect(viewModel(state).rootError?.code).toBe("file_io_error");
  });
});

describe("tabs and buffers", () => {
  // Would catch: opening the same file twice creating two tabs, or opening a file losing the
  // active tab of another file.
  it("keeps one tab per file and focuses an already open file", () => {
    let state = reduce(initialState(), { type: "tab_open", id: uriKey(notas), uri: notas, name: "notas.txt" });
    state = loaded(state, uriKey(notas), "linha\n", state.tabs[uriKey(notas)]!.token);
    state = reduce(state, { type: "tab_open", id: uriKey(outro), uri: outro, name: "outro.txt" });
    expect(Object.keys(state.tabs)).toHaveLength(2);
    state = reduce(state, { type: "tab_open", id: uriKey(notas), uri: notas, name: "notas.txt" });
    expect(Object.keys(state.tabs)).toHaveLength(2);
    expect(state.active).toBe(uriKey(notas));
    expect(state.order).toEqual([uriKey(notas), uriKey(outro)]);
  });

  // Would catch: a late read response for a closed tab resurrecting it, a stale token replacing
  // a buffer that was already edited, or a failed reload clearing text.
  it("ignores abandoned, stale and foreign read responses", () => {
    let state = reduce(initialState(), { type: "tab_open", id: uriKey(notas), uri: notas, name: "notas.txt" });
    const notasId = uriKey(notas);
    state = reduce(state, { type: "tab_open", id: uriKey(outro), uri: outro, name: "outro.txt" });
    const outroId = uriKey(outro);

    // Load and edit notas (token 1).
    state = loaded(state, notasId, "base\n", state.tabs[notasId]!.token);
    state = reduce(state, { type: "tab_edited", id: notasId, text: "rascunho\n" });
    // A stale response with an old token must change nothing.
    state = reduce(state, {
      type: "tab_loaded",
      id: notasId,
      token: state.tabs[notasId]!.token + 99,
      snapshot: snapshot("snap-forasteiro", notas, "conteudo de outra aba\n"),
    });
    expect(state.tabs[notasId]!.buffer).toBe("rascunho\n");
    expect(state.tabs[notasId]!.dirty).toBe(true);

    // A response for a closed tab must not resurrect it.
    state = reduce(state, { type: "tab_closed", id: outroId });
    state = reduce(state, {
      type: "tab_loaded",
      id: outroId,
      token: state.tabs[notasId]!.token,
      snapshot: snapshot("snap-outro", outro, "conteudo\n"),
    });
    expect(state.tabs[outroId]).toBeUndefined();
    expect(state.tabs[notasId]!.buffer).toBe("rascunho\n");
  });

  // Would catch: an edit during a save being marked clean, or a save of the wrong snapshot.
  it("tracks dirty against the last saved snapshot", () => {
    let state = reduce(initialState(), { type: "tab_open", id: uriKey(notas), uri: notas, name: "notas.txt" });
    const id = uriKey(notas);
    state = loaded(state, id, "base\n", state.tabs[id]!.token);
    expect(state.tabs[id]!.dirty).toBe(false);
    state = reduce(state, { type: "tab_edited", id, text: "base\neditado\n" });
    expect(state.tabs[id]!.dirty).toBe(true);
    state = reduce(state, { type: "tab_save_started", id });
    state = reduce(state, { type: "tab_edited", id, text: "base\neditado\nmais\n" });
    state = reduce(state, { type: "tab_saved", id, snapshot: snapshot("snap-salvo", notas, "base\neditado\n") });
    expect(state.tabs[id]!.dirty).toBe(true);
    expect(state.tabs[id]!.saving).toBe(false);
    state = reduce(state, { type: "tab_saved", id, snapshot: snapshot("snap-salvo-2", notas, "base\neditado\nmais\n") });
    expect(state.tabs[id]!.dirty).toBe(false);
  });
});

describe("conflict and close", () => {
  // Would catch: a conflict overwriting or clearing the buffer, or the conflict path marking
  // the tab saved/clean.
  it("preserves the buffer on a conflict and requires an explicit choice", () => {
    let state = reduce(initialState(), { type: "tab_open", id: uriKey(notas), uri: notas, name: "notas.txt" });
    const id = uriKey(notas);
    state = loaded(state, id, "linha um\nlinha dois\n", state.tabs[id]!.token);
    state = reduce(state, { type: "tab_edited", id, text: "linha um\nlinha do conflito\n" });
    state = reduce(state, { type: "tab_save_started", id });
    state = reduce(state, {
      type: "tab_conflict",
      id,
      current: snapshot("snap-disco", notas, "versao externa\nagente\n"),
      message: "o arquivo mudou no disco depois da leitura",
    });
    const tab = state.tabs[id]!;
    expect(tab.buffer).toBe("linha um\nlinha do conflito\n");
    expect(tab.dirty).toBe(true);
    expect(tab.base!.content).toBe("linha um\nlinha dois\n");
    expect(tab.saving).toBe(false);
    expect(tab.conflict!.message).toContain("mudou no disco");

    // Compare builds the diff against the disk snapshot without touching the buffer.
    state = reduce(state, { type: "tab_diff", id, base: "disk" });
    const diff = viewModel(state).active!.diff!;
    expect(diff.base).toBe("disk");
    expect(diff.lines.some((line) => line.op === "added" && line.text.includes("linha do conflito"))).toBe(true);
    expect(diff.lines.some((line) => line.op === "removed" && line.text.includes("versao externa"))).toBe(true);
    expect(state.tabs[id]!.buffer).toBe("linha um\nlinha do conflito\n");

    // Save copy preserves the text and reports the copy path.
    state = reduce(state, {
      type: "tab_recovery_saved",
      id,
      copy: { path: "/projeto/notas.txt.herdr-recuperacao-1", uri: localUri("/projeto/notas.txt.herdr-recuperacao-1"), bytes: 30 },
    });
    expect(viewModel(state).active!.recovery!.path).toContain("herdr-recuperacao");
    expect(state.tabs[id]!.buffer).toBe("linha um\nlinha do conflito\n");
  });

  // Would catch: the "original" diff built over the disk version (or the disk diff over the
  // original), a label naming the wrong origin, or added/removed lines taken from the wrong
  // side. Original, disk and buffer are three distinct texts with distinct snapshot ids, so
  // each base yields different added/removed lines.
  it("diffs the buffer against the chosen base: original snapshot or disk version", () => {
    let state = reduce(initialState(), { type: "tab_open", id: uriKey(notas), uri: notas, name: "notas.txt" });
    const id = uriKey(notas);
    const token = state.tabs[id]!.token;
    state = reduce(state, {
      type: "tab_loaded",
      id,
      token,
      snapshot: snapshot("orig1111-base", notas, "linha um\nlinha dois\nfim\n"),
    });
    const buffer = "linha um\nfim\nlinha do buffer\n";
    state = reduce(state, { type: "tab_edited", id, text: buffer });

    // Without a conflict the original diff already works; the disk base has nothing to show.
    state = reduce(state, { type: "tab_diff", id, base: "original" });
    let diff = viewModel(state).active!.diff!;
    expect([diff.base, diff.baseLabel, diff.currentLabel]).toEqual(["original", "conteúdo-base orig1111", "buffer atual"]);
    expect(diff.lines.filter((l) => l.op === "removed").map((l) => [l.text, l.baseLine])).toEqual([["linha dois", 2]]);
    expect(diff.lines.filter((l) => l.op === "added").map((l) => [l.text, l.currentLine])).toEqual([["linha do buffer", 3]]);
    expect([diff.added, diff.removed, diff.identical]).toEqual([1, 1, false]);

    state = reduce(state, {
      type: "tab_conflict",
      id,
      current: snapshot("disk2222-novo", notas, "versao externa\nfim\n"),
      message: "o arquivo mudou no disco depois da leitura",
    });

    // Original base during a conflict: still the snapshot the buffer was read from.
    diff = viewModel(state).active!.diff!;
    expect([diff.base, diff.baseLabel]).toEqual(["original", "conteúdo-base orig1111"]);
    expect(diff.lines.filter((l) => l.op === "removed").map((l) => l.text)).toEqual(["linha dois"]);
    expect(diff.lines.filter((l) => l.op === "added").map((l) => l.text)).toEqual(["linha do buffer"]);

    // Disk base: the fresh version detected by the save.
    state = reduce(state, { type: "tab_diff", id, base: "disk" });
    diff = viewModel(state).active!.diff!;
    expect([diff.base, diff.baseLabel, diff.currentLabel]).toEqual(["disk", "versão no disco disk2222", "buffer atual"]);
    expect(diff.lines.filter((l) => l.op === "removed").map((l) => l.text)).toEqual(["versao externa"]);
    expect(diff.lines.filter((l) => l.op === "added").map((l) => l.text)).toEqual(["linha um", "linha do buffer"]);
    expect([diff.added, diff.removed]).toEqual([2, 1]);

    // Neither diff touches the buffer or the base snapshot.
    expect(state.tabs[id]!.buffer).toBe(buffer);
    expect(state.tabs[id]!.base!.id).toBe("orig1111-base");
  });

  // Would catch: reload happening implicitly (without the user choosing it), or reload keeping
  // the conflict/dirty state.
  it("reload replaces the buffer only through the explicit reload action", () => {
    let state = reduce(initialState(), { type: "tab_open", id: uriKey(notas), uri: notas, name: "notas.txt" });
    const id = uriKey(notas);
    state = loaded(state, id, "base\n", state.tabs[id]!.token);
    state = reduce(state, { type: "tab_edited", id, text: "sujo\n" });
    state = reduce(state, {
      type: "tab_conflict",
      id,
      current: snapshot("snap-disco", notas, "externo\n"),
      message: "mudou",
    });
    state = reduce(state, { type: "tab_diff", id, base: "original" });
    state = reduce(state, { type: "tab_reload_started", id });
    expect(state.tabs[id]!.buffer).toBe("sujo\n");
    expect(state.tabs[id]!.reloading).toBe(true);
    state = reduce(state, {
      type: "tab_reloaded",
      id,
      token: state.tabs[id]!.token,
      snapshot: snapshot("snap-recarregado", notas, "externo\n"),
    });
    const tab = state.tabs[id]!;
    expect([tab.buffer, tab.dirty, tab.conflict, tab.diffBase, tab.reloading]).toEqual([
      "externo\n",
      false,
      null,
      null,
      false,
    ]);
  });

  // Would catch: closing a dirty tab without asking, cancel dropping the text, or closing one
  // tab touching another dirty tab.
  it("asks before closing a dirty tab and cancel preserves the text", () => {
    let state = reduce(initialState(), { type: "tab_open", id: uriKey(notas), uri: notas, name: "notas.txt" });
    state = reduce(state, { type: "tab_open", id: uriKey(outro), uri: outro, name: "outro.txt" });
    const notasId = uriKey(notas);
    const outroId = uriKey(outro);
    state = loaded(state, notasId, "a\n", state.tabs[notasId]!.token);
    state = loaded(state, outroId, "b\n", state.tabs[outroId]!.token);
    state = reduce(state, { type: "tab_edited", id: notasId, text: "a sujo\n" });
    state = reduce(state, { type: "tab_edited", id: outroId, text: "b sujo\n" });

    state = reduce(state, { type: "tab_close_prompt", id: outroId });
    expect(viewModel(state).active!.closePrompt).toBe(true);
    state = reduce(state, { type: "tab_close_cancelled", id: outroId });
    expect(state.tabs[outroId]!.buffer).toBe("b sujo\n");
    expect(state.tabs[outroId]!.dirty).toBe(true);

    state = reduce(state, { type: "tab_closed", id: outroId });
    expect(state.tabs[outroId]).toBeUndefined();
    expect(state.tabs[notasId]!.buffer).toBe("a sujo\n");
    expect(state.order).toEqual([notasId]);
    expect(state.active).toBe(notasId);
  });

  // Would catch: an external-change notice that lies after a save, or a watcher that never
  // clears when the disk matches the base again.
  it("tracks the external flag and clears it on save", () => {
    let state = reduce(initialState(), { type: "tab_open", id: uriKey(notas), uri: notas, name: "notas.txt" });
    const id = uriKey(notas);
    state = loaded(state, id, "base\n", state.tabs[id]!.token);
    state = reduce(state, { type: "tab_external", id, external: true });
    expect(viewModel(state).active!.external).toBe(true);
    state = reduce(state, { type: "tab_saved", id, snapshot: snapshot("snap-novo", notas, "base\n") });
    expect(state.tabs[id]!.external).toBe(false);
    expect(state.tabs[id]!.notice).toBeTruthy();
  });
});

describe("target identity", () => {
  // Would catch: pages from one target (host/boot) rendered for another, or a target switch
  // dropping dirty buffers.
  it("a different target clears pages but never replaces open buffers", () => {
    let state = reduce(initialState(), { type: "target_set", target });
    state = reduce(state, {
      type: "dir_loaded",
      uri: rootUri,
      entries: [entry("notas.txt")],
      nextCursor: null,
      append: false,
    });
    state = reduce(state, { type: "tab_open", id: uriKey(notas), uri: notas, name: "notas.txt" });
    const id = uriKey(notas);
    state = loaded(state, id, "base\n", state.tabs[id]!.token);
    state = reduce(state, { type: "tab_edited", id, text: "rascunho\n" });

    state = reduce(state, { type: "target_set", target: otherTarget });
    expect(Object.keys(state.dirs)).toHaveLength(0);
    expect(state.tabs[id]!.buffer).toBe("rascunho\n");
    expect(viewModel(state).nodes).toEqual([]);
  });
});
