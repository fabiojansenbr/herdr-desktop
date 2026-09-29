// Spec 011 (AC-011-03) — the connection dialog tested by behavior, not by scanning its source:
// the component itself is server-rendered and its DOM read (cards, fields, ordered progress),
// the Local/SSH radio group moves with the keyboard, and the four progress lines follow a real
// controller over the fake bridge from pending to ok/error. The native `visual-projects` phase
// measures the same dialog in the real window (620px width, 12px radius, Esc returns focus).
import { readFileSync } from "node:fs";
import { compile } from "svelte/compiler";
import { render } from "svelte/server";
import { describe, expect, it } from "vitest";
import { createConnectionsController, type ConnectionsController, type ConnectionsState } from "../connections/controller";
import { createFakeConnectionsBridge } from "../connections/fake-bridge";
import type { HostDto, RuntimeError, WorkspaceDto } from "../connections/types";
import ConnectionDialog from "./ConnectionDialog.svelte";
import { isKindActivation, kindCard, kindForKey } from "./connection-kind";

const stub: ConnectionsController = {
  cancelDialog: () => {},
  editDraft: () => {},
  submitDialog: async () => {},
} as unknown as ConnectionsController;

function dialogState(over: Partial<ConnectionsState["dialog"]> = {}): ConnectionsState["dialog"] {
  return {
    open: true,
    draft: { id: null, label: "dev-box-rc", target: "ana@dev-box", port: "", session: "trabalho", auth: "key" },
    errors: {},
    submitting: false,
    submitError: null,
    connecting: null,
    ...over,
  };
}

function host(over: Partial<HostDto> = {}): HostDto {
  return {
    endpoint: "abcdef",
    label: "dev-box-rc",
    kind: "ssh",
    session: "trabalho",
    target: "ana@dev-box",
    visible: true,
    phase: "offline",
    phase_label: "Offline",
    attempt: 0,
    retry_in_ms: null,
    cancelled: false,
    attention: null,
    guidance: null,
    connection_error: null,
    action_error: null,
    generation: null,
    boot_id: null,
    cached: false,
    surface: null,
    screen: [],
    panes: [],
    actions: [],
    ...over,
  };
}

function state(over: Partial<ConnectionsState> = {}): ConnectionsState {
  return {
    view: { hub: { revision: 1, hosts: [host()] }, profiles: [], store_error: null },
    loading: false,
    globalError: null,
    hostErrors: {},
    inputs: {},
    sending: {},
    dialog: dialogState(),
    importReport: null,
    workspaces: {},
    ...over,
  };
}

function dialogBody(current: ConnectionsState, controller: ConnectionsController = stub): string {
  return render(ConnectionDialog, { props: { controller, state: current } }).body;
}

/** Attributes of the rendered opening tag containing `marker` (the DOM the window would get). */
function tagAttrs(html: string, marker: string): Record<string, string> {
  const at = html.indexOf(marker);
  if (at < 0) throw new Error(`rendered dialog has no tag containing ${marker}`);
  const start = html.lastIndexOf("<", at);
  const end = html.indexOf(">", at);
  const attrs: Record<string, string> = {};
  for (const match of html.slice(start, end + 1).matchAll(/([a-zA-Z][a-zA-Z0-9-]*)(?:="([^"]*)")?/g)) {
    if (match[1]) attrs[match[1]] = match[2] ?? "";
  }
  return attrs;
}

const optionTexts = (html: string) =>
  Array.from(html.matchAll(/<option[^>]*>([^<]*)<\/option>/g)).map((m) => m[1] ?? "");

/** The four rendered progress rows: step, data-status and the accessible label text. */
function progressRows(html: string): { step: string; status: string; text: string }[] {
  return html
    .split('<div class="progress-row')
    .slice(1)
    .map((chunk) => ({
      step: /data-step="(\d+)"/.exec(chunk)?.[1] ?? "",
      status: /data-status="(\w+)"/.exec(chunk)?.[1] ?? "",
      text: /<span class="step-text step-label[^"]*">([^<]*)<\/span>/.exec(chunk)?.[1] ?? "",
    }));
}

describe("ConnectionDialog (AC-011-03)", () => {
  it("keeps 620px width and 12px radius in the styles it compiles", () => {
    // The native visual-projects phase measures both in the real window; here the component's own
    // compiled stylesheet is asserted, never a text search of the file.
    const source = readFileSync(new URL("./ConnectionDialog.svelte", import.meta.url), "utf8");
    const { css } = compile(source, { filename: "ConnectionDialog.svelte", css: "external" });
    expect(css!.code).toContain("min(620px");
    expect(css!.code).toContain("border-radius: 12px");
  });

  it("renders the Local/SSH radio group with exactly the chosen card checked and tabbable", () => {
    const body = dialogBody(state());
    const group = tagAttrs(body, 'aria-label="Tipo de conexão"');
    expect(group["role"]).toBe("radiogroup");
    const local = tagAttrs(body, 'data-kind="local"');
    const ssh = tagAttrs(body, 'data-kind="ssh"');
    for (const [kind, card] of [["local", local], ["ssh", ssh]] as const) {
      const expected = kindCard(kind, "ssh");
      expect(card["role"]).toBe("radio");
      expect(card["aria-checked"]).toBe(String(expected.checked));
      expect(card["tabindex"]).toBe(String(expected.tabindex));
    }
    expect(local["aria-checked"]).toBe("false");
    expect(ssh["aria-checked"]).toBe("true");
  });

  it("moves the checked card with the arrow keys and activates with Enter or Space", () => {
    expect(kindForKey("ArrowLeft")).toBe("local");
    expect(kindForKey("ArrowUp")).toBe("local");
    expect(kindForKey("ArrowRight")).toBe("ssh");
    expect(kindForKey("ArrowDown")).toBe("ssh");
    expect(kindForKey("Tab")).toBeNull();
    expect(isKindActivation("Enter")).toBe(true);
    expect(isKindActivation(" ")).toBe(true);
    expect(isKindActivation("Escape")).toBe(false);
    // The rendered aria-checked/tabindex of both cards follow the moved selection.
    const moved = kindForKey("ArrowLeft")!;
    expect(kindCard(moved, moved)).toEqual({ checked: true, tabindex: 0 });
    expect(kindCard("ssh", moved)).toEqual({ checked: false, tabindex: -1 });
  });

  it("renders Porta with 22 and the ~/.ssh key options", () => {
    const body = dialogBody(state());
    expect(tagAttrs(body, 'placeholder="22"')["value"]).toBe("22");
    expect(optionTexts(body)).toContain("Chave SSH · ~/.ssh/id_ed25519");
    expect(optionTexts(body)).toContain("Chave SSH · ~/.ssh/id_rsa");
    for (const label of ["Host", "Porta", "Autenticação", "Nome de exibição", "Sessão"]) {
      expect(body).toContain(`>${label}</span>`);
    }
    // The "Adicionar projetos ao grupo" select was a design placeholder never wired to a
    // collection (fixed demo options); it was removed and must not come back unwired.
    expect(body).not.toContain(">Adicionar projetos ao grupo</span>");
  });

  it("carries the draft's ssh-agent choice in the authentication select (spec 031)", () => {
    const body = dialogBody(
      state({
        dialog: dialogState({ draft: { ...dialogState().draft, auth: "ssh-agent" } }),
      }),
    );
    expect(optionTexts(body)).toContain("ssh-agent");
    expect(body).toMatch(/<option value="ssh-agent"[^>]*selected=""/);
    const key = dialogBody(state());
    expect(key).toMatch(/<option value="key-ed25519"[^>]*selected=""/);
    expect(key).not.toMatch(/<option value="ssh-agent"[^>]*selected=""/);
  });

  it("keeps the four steps in order and pending before the dialog asks to connect", () => {
    const rows = progressRows(dialogBody(state()));
    expect(rows.map((row) => row.step)).toEqual(["1", "2", "3", "4"]);
    expect(rows.map((row) => row.status)).toEqual(["pending", "pending", "pending", "pending"]);
    expect(rows.map((row) => row.text)).toEqual([
      "Host alcançável",
      "Autenticado como ana",
      "herdr encontrado · endpoint geração 1",
      "Lendo workspaces remotos...",
    ]);
  });

  it("advances the steps with the measured handshake and the host's workspaces, then closes", async () => {
    const bridge = createFakeConnectionsBridge();
    const controller = createConnectionsController(bridge);
    await controller.load();
    controller.openDialog();
    controller.editDraft("label", "dev-box-rc");
    controller.editDraft("target", "ana@dev-box");
    controller.editDraft("session", "trabalho");
    await controller.submitDialog(true);
    const endpoint = controller.state.dialog.connecting!;
    expect(endpoint).toBeTruthy();

    // The workspace read stays gated so the online state is observed with step 4 still pending.
    const gate: { release?: (list: WorkspaceDto[]) => void } = {};
    bridge.workspaces = () => new Promise((resolve) => (gate.release = resolve));
    bridge.setOnline(endpoint, "boot-rc", 3, { latency_ms: 251, server_version: "0.9.0-rc.7" });
    await controller.refresh();

    const body = dialogBody(controller.state);
    expect(progressRows(body).map((row) => row.status)).toEqual(["ok", "ok", "ok", "pending"]);
    expect(body).toContain("Host alcançável · 251 ms");
    expect(body).toContain("herdr 0.9.0-rc.7 encontrado · endpoint geração 1");
    expect(progressRows(body)[3]!.text).toContain("Lendo workspaces");

    gate.release!([{ workspace_id: "w7", label: "trabalho" }]);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(controller.state.dialog.open).toBe(false);
    expect(progressRows(dialogBody(controller.state))).toEqual([]);
  });

  it("marks the step that did not arrive in error", () => {
    const refused: RuntimeError = { code: "ssh_authentication_required", message: "chave recusada", retryable: false, endpoint: "abcdef" };
    const attention = state({
      dialog: dialogState({ connecting: "abcdef" }),
      view: {
        hub: { revision: 2, hosts: [host({ phase: "attention", phase_label: "Precisa de atenção", connection_error: refused })] },
        profiles: [],
        store_error: null,
      },
    });
    expect(progressRows(dialogBody(attention)).map((row) => row.status)).toEqual(["error", "blocked", "blocked", "blocked"]);

    const readFailed = state({
      dialog: dialogState({ connecting: "abcdef" }),
      view: {
        hub: {
          revision: 3,
          hosts: [host({ phase: "online", phase_label: "Online", latency_ms: 503, server_version: "0.9.0", generation: 2 })],
        },
        profiles: [],
        store_error: null,
      },
      hostErrors: { abcdef: { code: "api_missing", message: "servidor sem API JSON", retryable: false, endpoint: "abcdef" } },
    });
    const rows = progressRows(dialogBody(readFailed));
    expect(rows.map((row) => row.status)).toEqual(["ok", "ok", "ok", "error"]);
    expect(rows[0]!.text).toBe("Host alcançável · 503 ms");
  });

  it("shows the failure reason, stops later spinners and restores Conectar", () => {
    const current = state({
      dialog: dialogState({ connecting: "abcdef" }),
      view: {
        hub: {
          revision: 4,
          hosts: [host({ phase: "attention", phase_label: "Precisa de atenção", attention: "herdr_missing", connection_error: { code: "remote_herdr_missing", message: "o Herdr não foi encontrado no host remoto", retryable: false, endpoint: "abcdef" } })],
        },
        profiles: [],
        store_error: null,
      },
    });
    const body = dialogBody(current);
    expect(body).toContain("o Herdr não foi encontrado no host remoto");
    expect(body).toContain("step-icon blocked");
    expect(body).toMatch(/>Tentar novamente<\/button>/);
  });
});
