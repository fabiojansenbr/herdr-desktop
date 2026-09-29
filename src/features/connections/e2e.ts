// Native window scenario of spec 003 (driven by src-tauri/tests/connections.rs through
// scripts/feature-harness/window.rs). Acts on ConnectionsPanel/ConnectionDialog like a user over
// the real IPC bridge and backend and reports what the window shows. The parent test watches the
// `step` of each progress report to break and restore the SSH host at the right moment.
//
// Single phase "flow": Local + SSH online → input to SSH only → SSH lost (Reconectando, input off,
// Local still accepts input) → reconnection to a restarted server (input only after
// reconciliation) → import TUI profiles → unknown host key needs attention.

import { byText, press, type, waitFor } from "../../harness/dom";
import type { Scenario } from "../../harness/native";

interface PaneView {
  pane: string;
  enabled: boolean;
  block: string;
  input_disabled: boolean;
  send_disabled: boolean;
  block_text: string;
}

interface HostView {
  endpoint: string;
  label: string;
  kind: string;
  phase: string;
  status: string;
  detail: string;
  guidance: string;
  generation: string;
  boot: string;
  attempt: string;
  actions: string[];
  screen: string;
  panes: PaneView[];
}

function hostEl(root: ParentNode, label: string): HTMLElement | null {
  return (
    Array.from(root.querySelectorAll<HTMLElement>("article[data-host]")).find(
      (a) => a.querySelector(".host-label")?.textContent?.trim() === label,
    ) ?? null
  );
}

function read(article: HTMLElement): HostView {
  return {
    endpoint: article.dataset.host ?? "",
    label: article.querySelector(".host-label")?.textContent?.trim() ?? "",
    kind: article.dataset.kind ?? "",
    phase: article.dataset.phase ?? "",
    status: article.querySelector(".status")?.textContent?.trim() ?? "",
    detail: article.querySelector(".detail")?.textContent?.trim() ?? "",
    guidance: article.querySelector(".guidance")?.textContent?.trim() ?? "",
    generation: article.dataset.generation ?? "",
    boot: article.dataset.boot ?? "",
    attempt: article.dataset.attempt ?? "",
    actions: Array.from(article.querySelectorAll<HTMLButtonElement>(".actions button")).map((b) => b.textContent?.trim() ?? ""),
    screen: article.querySelector(".screen")?.textContent ?? "",
    panes: Array.from(article.querySelectorAll<HTMLFormElement>("form[data-pane]")).map((form) => ({
      pane: form.dataset.pane ?? "",
      enabled: form.dataset.inputEnabled === "true",
      block: form.dataset.block ?? "",
      input_disabled: form.querySelector("input")!.disabled,
      send_disabled: form.querySelector<HTMLButtonElement>("button[type=submit]")!.disabled,
      block_text: form.querySelector(".block")?.textContent?.trim() ?? "",
    })),
  };
}

function hosts(root: ParentNode): HostView[] {
  return Array.from(root.querySelectorAll<HTMLElement>("article[data-host]")).map(read);
}

function view(root: ParentNode, label: string): HostView {
  const el = hostEl(root, label);
  if (!el) throw new Error(`host ${label} not rendered`);
  return read(el);
}

function waitHost(root: ParentNode, label: string, what: string, ok: (h: HostView) => boolean, timeoutMs = 30000) {
  return waitFor(
    `${label}: ${what}`,
    () => {
      const el = hostEl(root, label);
      if (!el) return null;
      const h = read(el);
      return ok(h) ? h : null;
    },
    timeoutMs,
  );
}

const paneEnabled = (h: HostView, pane: string) => h.panes.some((p) => p.pane === pane && p.enabled && !p.input_disabled);

function hostButton(root: ParentNode, label: string, text: string): HTMLButtonElement {
  const el = hostEl(root, label);
  const button = el && byText<HTMLButtonElement>(el, "button", text);
  if (!button) throw new Error(`button "${text}" not found for ${label}`);
  return button;
}

async function sendInput(root: ParentNode, label: string, pane: string, text: string) {
  const el = hostEl(root, label)!;
  const form = el.querySelector<HTMLFormElement>(`form[data-pane="${pane}"]`);
  if (!form) throw new Error(`pane ${pane} not rendered for ${label}`);
  const input = form.querySelector<HTMLInputElement>("input")!;
  if (input.disabled) throw new Error(`input of ${pane} in ${label} is disabled`);
  type(input, text);
  press(form.querySelector<HTMLButtonElement>("button[type=submit]")!);
}

async function addSsh(root: HTMLElement, label: string, target: string, port: string, session: string) {
  press(byText<HTMLButtonElement>(root, "button", "Adicionar conexão SSH")!);
  const form = await waitFor("connection dialog", () => root.querySelector<HTMLFormElement>('form[aria-label="Conectar a um servidor herdr"]'));
  const field = (name: string) => {
    const l = Array.from(form.querySelectorAll("label")).find((x) => x.querySelector("span")?.textContent === name);
    const input = l?.querySelector<HTMLInputElement>("input");
    if (!input) throw new Error(`dialog field ${name} not found`);
    return input;
  };
  type(field("Host"), target);
  type(field("Porta"), port);
  type(field("Nome de exibição"), label);
  type(field("Sessão"), session);
  press(form.querySelector<HTMLButtonElement>('button[value="connect"]')!);
  await waitFor(`dialog closed for ${label}`, () => {
    const error = form.isConnected ? form.querySelector(".error")?.textContent : null;
    if (error) throw new Error(`dialog error: ${error}`);
    return !form.isConnected && hostEl(root, label);
  });
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

export const run: Scenario = async ({ phase, params, root, progress }) => {
  if (phase !== "flow") throw new Error(`unknown phase ${phase}`);
  const p = params as Record<string, string>;
  const LOCAL = "Este computador";
  const SSH = "Servidor SSH";
  const BAD = "Chave desconhecida";

  await waitFor("connections loaded", () => hostEl(root, LOCAL));
  const initial = hosts(root);
  const onboarding = root.querySelector(".onboarding")?.textContent?.trim() ?? null;

  // 1. Local and SSH online, explicitly.
  press(hostButton(root, LOCAL, "Conectar"));
  const localOnline = await waitHost(root, LOCAL, "online with input", (h) => h.phase === "online" && paneEnabled(h, "w1:p1"));
  await addSsh(root, SSH, p.ssh_target!, p.ssh_port!, p.ssh_session!);
  const sshOnline = await waitHost(root, SSH, "online with input", (h) => h.phase === "online" && paneEnabled(h, "w1:p1"), 60000);
  await progress({ step: "both_online", hosts: hosts(root) });

  // JSON API lane of each host (independent of the visual lane). An action error stays on the
  // action; the connection remains online.
  press(hostButton(root, LOCAL, "Listar workspaces"));
  const localWorkspaces = await waitFor("local workspaces", () => hostEl(root, LOCAL)?.querySelector<HTMLElement>("[data-workspaces]")?.textContent?.trim(), 30000);
  press(hostButton(root, SSH, "Listar workspaces"));
  const workspaces = await waitFor(
    "ssh workspaces or action error",
    () => {
      const el = hostEl(root, SSH);
      const listed = el?.querySelector<HTMLElement>("[data-workspaces]")?.textContent?.trim();
      const actionError = el?.querySelector<HTMLElement>(".action-error")?.textContent?.trim();
      return listed || actionError ? { listed: listed ?? null, action_error: actionError ?? null, host: read(el!) } : null;
    },
    30000,
  );

  // 2. Input to SSH reaches only SSH (same pane id on both hosts).
  await sendInput(root, SSH, "w1:p1", `echo ${p.marker_ssh}`);
  const sshEcho = await waitHost(root, SSH, "marker on screen", (h) => h.screen.includes(p.marker_ssh!), 20000);
  await sleep(500);
  const localAfterSsh = view(root, LOCAL);
  await progress({ step: "ssh_input", ssh: sshEcho, local: localAfterSsh });

  // 3. The parent test breaks SSH: Reconectando, input off, cache kept; Local keeps input.
  const lost = await waitHost(root, SSH, "reconnecting", (h) => h.phase === "reconnecting", 40000);
  const localWhileLost = view(root, LOCAL);
  if (!paneEnabled(localWhileLost, "w1:p1")) throw new Error("local input disabled while SSH reconnects");
  await sendInput(root, LOCAL, "w1:p1", `echo ${p.marker_local}`);
  const localEcho = await waitHost(root, LOCAL, "local marker", (h) => h.screen.includes(p.marker_local!), 20000);
  const sshStillLost = view(root, SSH);
  await progress({ step: "ssh_lost", lost, ssh_after_local_input: sshStillLost, local: localEcho });

  // 4. The parent restarts the remote server (new boot) and sshd; record every state seen.
  const timeline: { t: number; phase: string; boot: string; generation: string; enabled: boolean; block: string; attempt: string }[] = [];
  const started = Date.now();
  const reconnected = await waitFor(
    "ssh reconciled on the new boot",
    () => {
      const h = view(root, SSH);
      const pane = h.panes.find((x) => x.pane === "w1:p1");
      const sample = { t: Date.now() - started, phase: h.phase, boot: h.boot, generation: h.generation, enabled: !!pane?.enabled && !pane.input_disabled, block: pane?.block ?? "", attempt: h.attempt };
      const last = timeline[timeline.length - 1];
      if (!last || last.phase !== sample.phase || last.boot !== sample.boot || last.enabled !== sample.enabled || last.block !== sample.block || last.generation !== sample.generation) {
        timeline.push(sample);
      }
      return h.phase === "online" && sample.enabled && h.boot !== sshOnline.boot && h.boot !== "" ? h : null;
    },
    120000,
  );
  await sendInput(root, SSH, "w1:p1", `echo ${p.marker_ssh2}`);
  const sshEcho2 = await waitHost(root, SSH, "second marker", (h) => h.screen.includes(p.marker_ssh2!), 20000);
  await progress({ step: "reconnected", timeline });

  // 5. Import TUI profiles (read-only) and an unknown host key.
  press(byText<HTMLButtonElement>(root, "button", "Importar perfis do Herdr")!);
  const importText = await waitFor("import report", () => root.querySelector("[data-import]")?.textContent?.trim());
  await addSsh(root, BAD, p.bad_target!, p.ssh_port!, p.ssh_session!);
  const attention = await waitHost(root, BAD, "needs attention", (h) => h.phase === "attention", 40000);
  await sleep(4000);
  const attentionLater = view(root, BAD);

  return {
    initial,
    onboarding,
    local_online: localOnline,
    ssh_online: sshOnline,
    local_workspaces: localWorkspaces,
    workspaces,
    ssh_echo: sshEcho,
    local_after_ssh: localAfterSsh,
    lost,
    local_while_lost: localWhileLost,
    ssh_after_local_input: sshStillLost,
    local_echo: localEcho,
    timeline,
    reconnected,
    ssh_echo2: sshEcho2,
    import_text: importText,
    attention,
    attention_later: attentionLater,
    hosts: hosts(root),
  };
};
