// Native window scenario of spec 006 (driven by src-tauri/tests/files_remote.rs through
// scripts/feature-harness/window.rs). Acts on the real remote files workspace next to the local
// files workspace of 005, over the real IPC bridges and backends, and reports what the window
// shows. Connecting hosts and typing into terminals use the real connections bridge of 003 (its
// panel is composed with this feature only in spec 007). The parent test watches `step` to
// interrupt and restore the SSH host at the right moments.
//
// Single phase "flow": empty state → same path Local vs SSH → read-only remote tab with host badge
// → binary/permission errors and missing SFTP on the resource while terminals keep working → SSH
// lost: cached tab Desatualizado → reconnected: reload on the new connection → diff of two reads.

import { tauriConnectionsBridge } from "../../connections/bridge";
import type { HostDto, QualifiedTarget } from "../../connections/types";
import { waitFor } from "../../harness/dom";
import type { Scenario } from "../../harness/native";

type View = {
  state: { doc: { toString(): string; length: number }; readOnly: boolean };
};

function press(button: HTMLButtonElement | null | undefined, what: string): void {
  if (!button) throw new Error(`button not found: ${what}`);
  if (button.disabled) throw new Error(`button "${what}" is disabled`);
  button.click();
}

async function waitForAsync<T>(what: string, probe: () => Promise<T | null | undefined | false>, timeoutMs = 30000): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await probe();
    if (value) return value;
    if (Date.now() > deadline) {
      throw new Error(`timed out waiting for ${what}; page: ${document.body.innerText.slice(0, 2000)}`);
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
}

async function editorView(scope: ParentNode | null): Promise<View | null> {
  const element = scope?.querySelector<HTMLElement>(".cm-editor");
  if (!element) return null;
  const { EditorView } = await import("@codemirror/view");
  return (EditorView.findFromDOM(element) as unknown as View | null) ?? null;
}

async function editorText(scope: ParentNode | null): Promise<string> {
  return (await editorView(scope))?.state.doc.toString() ?? "";
}

/** Types like a user: focus the editable content and insert through the browser. */
async function typeInto(scope: ParentNode | null, text: string): Promise<void> {
  const content = scope?.querySelector<HTMLElement>(".cm-content");
  if (!content) throw new Error("editor content not found");
  content.focus();
  document.execCommand("insertText", false, text);
  await new Promise((resolve) => setTimeout(resolve, 300));
}

export const run: Scenario = async ({ phase, params, root, progress }) => {
  if (phase !== "flow") throw new Error(`unknown phase ${phase}`);
  const endpointA = String(params.endpoint_a);
  const endpointB = String(params.endpoint_b);
  const markers = params.markers as Record<string, string>;
  const connections = tauriConnectionsBridge();

  const remote = await waitFor("remote workspace", () => root.querySelector<HTMLElement>("[data-remote-files-workspace]"));
  const local = await waitFor("local workspace", () => root.querySelector<HTMLElement>("[data-files-workspace]"));
  const hostButton = (endpoint: string) => remote.querySelector<HTMLButtonElement>(`button[data-remote-host="${endpoint}"]`);
  const remoteEntry = (name: string) =>
    Array.from(remote.querySelectorAll<HTMLButtonElement>("button.remote-entry")).find((b) => b.dataset.name === name) ?? null;
  const localEntry = (name: string) =>
    Array.from(local.querySelectorAll<HTMLButtonElement>("button.entry")).find((b) => b.dataset.name === name) ?? null;
  const remoteTab = (name: string) =>
    Array.from(remote.querySelectorAll<HTMLElement>(".remote-tab")).find((t) => t.querySelector(".tab-name")?.textContent?.trim() === name) ??
    null;
  const remoteDoc = () => remote.querySelector<HTMLElement>("[data-remote-document]");
  const buttonsWithText = (scope: ParentNode, text: string) =>
    Array.from(scope.querySelectorAll<HTMLButtonElement>("button")).filter((b) => b.textContent?.trim() === text).length;
  const tabBadge = (name: string) => remoteTab(name)?.querySelector("[data-remote-badge]")?.textContent?.replace(/\s+/g, " ").trim() ?? "";

  async function paneTarget(endpoint: string, generationAbove = 0): Promise<QualifiedTarget> {
    return waitForAsync(
      `input-enabled pane of ${endpoint}`,
      async () => {
        const view = await connections.list();
        const host: HostDto | undefined = view.hub.hosts.find((h) => h.endpoint === endpoint);
        const pane = host?.panes.find((p) => p.pane_id === "w1:p1" && p.input_enabled && p.target);
        return pane?.target && pane.target.connection_generation > generationAbove ? pane.target : null;
      },
      120000,
    );
  }

  // --- empty state: selected host offline, nothing listed --------------------------------
  await waitFor("hosts listed", () => hostButton(endpointA) && hostButton(endpointB));
  press(hostButton(endpointA), "host A");
  await waitFor("offline onboarding", () => remote.querySelector('[data-remote-onboarding="host_offline"]'));
  const empty = {
    onboarding: remote.querySelector<HTMLElement>("[data-remote-onboarding]")?.dataset.remoteOnboarding ?? null,
    refresh_disabled: remote.querySelector<HTMLButtonElement>("[data-remote-refresh]")?.disabled ?? null,
    entries: remote.querySelectorAll("button.remote-entry").length,
    text: remote.querySelector("[data-remote-onboarding]")?.textContent?.trim() ?? "",
  };

  // --- connect Local, SSH A (with SFTP) and SSH B (without SFTP) --------------------------
  await connections.connect("local");
  await connections.connect(endpointA);
  await connections.connect(endpointB);
  await waitFor("SSH hosts online", () => hostButton(endpointA)?.dataset.online === "true" && hostButton(endpointB)?.dataset.online === "true", 90000);

  // --- AC-006-01: SSH tree and read-only tab vs the local tree at the same path -----------
  press(remote.querySelector<HTMLButtonElement>("[data-remote-refresh]"), "Atualizar");
  await waitFor("remote tree", () => remoteEntry("notas.txt"));
  const remote_tree = {
    names: Array.from(remote.querySelectorAll<HTMLButtonElement>("button.remote-entry")).map((b) => b.dataset.name ?? ""),
    header_badge: remote.querySelector("[data-remote-header] [data-remote-badge]")?.textContent?.replace(/\s+/g, " ").trim() ?? "",
  };
  await waitFor("local tree", () => localEntry("notas.txt"));
  const local_tree = {
    names: Array.from(local.querySelectorAll<HTMLButtonElement>("button.entry")).map((b) => b.dataset.name ?? ""),
  };

  press(remoteEntry("notas.txt"), "remote notas.txt");
  await waitFor("remote notas tab", () => remoteTab("notas.txt"));
  const remoteText = await waitForAsync("remote notas content", async () => {
    const text = await editorText(remoteDoc());
    return text.includes("linha um remota") ? text : null;
  });
  const remoteView = await editorView(remoteDoc());
  await typeInto(remoteDoc(), "DIGITADO-REMOTO ");
  const remote_tab = {
    path: remoteDoc()?.querySelector("[data-file-path]")?.textContent?.trim() ?? "",
    text: remoteText,
    badge: tabBadge("notas.txt"),
    label: remoteDoc()?.querySelector("[data-snapshot-label]")?.textContent?.trim() ?? "",
    generation: Number(hostButton(endpointA)?.dataset.generation ?? "0"),
    save_buttons: buttonsWithText(remote, "Salvar"),
    editor_read_only: remoteView?.state.readOnly ?? null,
    editable: remoteDoc()?.querySelector(".cm-content")?.getAttribute("contenteditable") ?? null,
    typed_ignored: (await editorText(remoteDoc())) === remoteText,
  };

  press(localEntry("notas.txt"), "local notas.txt");
  const localText = await waitForAsync("local notas content", async () => {
    const text = await editorText(local);
    return text.includes("conteudo LOCAL") ? text : null;
  });
  const localPath = local.querySelector("[data-file-path]")?.textContent?.trim() ?? "";
  await typeInto(local, "DIGITADO-LOCAL ");
  const local_tab = {
    path: localPath,
    text: localText,
    save_buttons: buttonsWithText(local, "Salvar"),
    // Same typing probe on the editable local editor: proves the probe can change a document.
    typed_changed: (await editorText(local)) !== localText,
  };
  if (!local_tab.typed_changed) throw new Error("typing probe did not change the editable local editor");

  // --- AC-006-03: errors on the resource; terminals keep working ---------------------------
  press(remoteEntry("bin.dat"), "remote bin.dat");
  await waitFor("binary error", () => remote.querySelector('[data-remote-file-error="binary_unsupported"]'));
  const binary = {
    code: "binary_unsupported",
    message: remote.querySelector('[data-remote-file-error="binary_unsupported"]')?.textContent?.trim() ?? "",
  };
  press(remoteEntry("segredo.txt"), "remote segredo.txt");
  await waitFor("permission error", () => remote.querySelector('[data-remote-file-error="permission_denied"]'));
  const permission = {
    code: "permission_denied",
    message: remote.querySelector('[data-remote-file-error="permission_denied"]')?.textContent?.trim() ?? "",
  };
  const errors = {
    binary,
    permission,
    host_a_online: hostButton(endpointA)?.dataset.online === "true",
    tree_entries: remote.querySelectorAll("button.remote-entry").length,
  };
  const targetA = await paneTarget(endpointA);
  await connections.sendText(targetA, `echo ${markers.a_errors}`, true);
  await connections.sendText(await paneTarget("local"), `echo ${markers.local}`, true);

  press(hostButton(endpointB), "host B");
  await waitFor("missing SFTP error", () => remote.querySelector("[data-remote-root-error]"), 30000);
  const nosftp = {
    code: remote.querySelector<HTMLElement>("[data-remote-root-error]")?.dataset.remoteRootError ?? "",
    message: remote.querySelector("[data-remote-root-error]")?.textContent?.trim() ?? "",
    hint: remote.querySelector("[data-remote-root-hint]")?.textContent?.trim() ?? "",
    host_online: hostButton(endpointB)?.dataset.online === "true",
    tabs_kept: remote.querySelectorAll(".remote-tab").length,
  };
  await connections.sendText(await paneTarget(endpointB), `echo ${markers.b_nosftp}`, true);

  // --- AC-006-02: read, lose the connection, cached tab, reconnect, reload, diff -----------
  press(hostButton(endpointA), "host A again");
  await waitFor("remote tree again", () => remoteEntry("notas.txt"));
  press(remoteTab("notas.txt")?.querySelector<HTMLButtonElement>(".tab-name"), "notas tab");
  await waitForAsync("notas tab shown", async () => (await editorText(remoteDoc())) === remoteText);
  await progress({ step: "read" });

  await waitFor("SSH A lost", () => hostButton(endpointA)?.dataset.online === "false", 60000);
  press(remoteTab("notas.txt")?.querySelector<HTMLButtonElement>(".tab-name"), "notas tab while lost");
  await waitFor("stale notice", () => remote.querySelector('[data-stale-notice="host_offline"]'));
  const stale = {
    host_online: hostButton(endpointA)?.dataset.online === "true",
    reason: remote.querySelector<HTMLElement>("[data-stale-notice]")?.dataset.staleNotice ?? "",
    notice: remote.querySelector("[data-stale-notice]")?.textContent?.replace(/\s+/g, " ").trim() ?? "",
    text: await editorText(remoteDoc()),
    badge: tabBadge("notas.txt"),
    reload_disabled: remote.querySelector<HTMLButtonElement>("[data-reload]")?.disabled ?? null,
  };
  await progress({ step: "stale" });

  await waitFor(
    "SSH A back with a new generation",
    () => hostButton(endpointA)?.dataset.online === "true" && Number(hostButton(endpointA)?.dataset.generation ?? 0) > remote_tab.generation,
    120000,
  );
  await waitFor("renewed notice", () => remote.querySelector('[data-stale-notice="connection_renewed"]'));
  const renewed = {
    reason: remote.querySelector<HTMLElement>("[data-stale-notice]")?.dataset.staleNotice ?? "",
    generation: Number(hostButton(endpointA)?.dataset.generation ?? "0"),
    text: await editorText(remoteDoc()),
    tree_stale: remote.querySelector("[data-tree-stale]") !== null,
    badge: tabBadge("notas.txt"),
  };

  press(remote.querySelector<HTMLButtonElement>("[data-reload]"), "Recarregar");
  const reloadedText = await waitForAsync("reloaded content", async () => {
    const text = await editorText(remoteDoc());
    return text.includes("linha dois ALTERADA") ? text : null;
  });
  await waitFor("stale cleared", () => remote.querySelector("[data-stale-notice]") === null);
  const reloaded = {
    text: reloadedText,
    stale: remote.querySelector("[data-stale-notice]") !== null,
    label: remoteDoc()?.querySelector("[data-snapshot-label]")?.textContent?.trim() ?? "",
    badge: tabBadge("notas.txt"),
  };

  press(remote.querySelector<HTMLButtonElement>("[data-compare]"), "Comparar com leitura anterior");
  await waitFor("diff", () => remote.querySelector("[data-remote-diff]"));
  const rows = (op: string) =>
    Array.from(remote.querySelectorAll(`[data-remote-diff] [data-op="${op}"]`)).map((el) =>
      (el.textContent ?? "").replace(/^\s*[+−]\s*/, "").trim(),
    );
  const diff = {
    sources: (remote.querySelector("[data-diff-sources]")?.textContent ?? "").split("→").map((s) => s.trim()),
    removed: rows("removed"),
    added: rows("added"),
    summary: remote.querySelector("[data-diff-summary]")?.textContent?.trim() ?? "",
  };

  press(remote.querySelector<HTMLButtonElement>("[data-remote-refresh]"), "Atualizar after reconnection");
  await waitFor("tree refreshed", () => remote.querySelector("[data-tree-stale]") === null && remoteEntry("notas.txt"));
  const tree_after_refresh = { stale: remote.querySelector("[data-tree-stale]") !== null };

  await connections.sendText(await paneTarget(endpointA, targetA.connection_generation), `echo ${markers.a_reconnected}`, true);

  return {
    empty,
    remote_tree,
    local_tree,
    remote_tab,
    local_tab,
    errors,
    nosftp,
    stale,
    renewed,
    reloaded,
    diff,
    tree_after_refresh,
  };
};
