// SSH phases of the spec 007 native flow (AC-007-04). Called by e2e.ts runPhase for the five SSH
// phases (.local/orchestration/native-ssh-contract.md). Acts only through the
// composed App controls (connection dialog, navigator, agents panel, files) and reports raw DOM
// observations; engine facts come from the parent's own snapshots (tests/fidelity-native/ssh_flow.rs)
// taken at each `harness_await` step. Nothing here talks to an engine or fabricates a result:
// every missing control or parameter throws.

import { byText, choose, press, type, waitFor } from "../../harness/dom";

export const SSH_PHASES = ["hosts-identity", "ssh-agent-actions", "ssh-files-readonly", "legacy-server", "host-switch-dirty"] as const;
export type SshPhase = (typeof SSH_PHASES)[number];

// Must equal ssh_flow::STEPS (asserted by ssh-flow.test.ts reading the Rust source).
export const SSH_STEPS = [
  "ssh-identity",
  "ssh-agent-before",
  "ssh-agent-after",
  "ssh-files-before",
  "ssh-files-after",
  "ssh-legacy-before",
  "ssh-legacy-after",
  "ssh-dirty-before",
  "ssh-dirty-after",
] as const;

export type AwaitParent = (step: string, detail: Record<string, unknown>) => Promise<Record<string, unknown>>;
type Profile = { label: string; target: string; port: number; session: string };
type ProjectInput = { label: string; session: string; root: string };
export type SshFlowParams = {
  ssh_profile: Profile;
  legacy_profile: Profile;
  local: ProjectInput;
  ssh: ProjectInput;
  legacy: ProjectInput;
  shared_pane: string;
  agent_kind: string;
  agent_name: string;
  prompt_nonce: string;
  edit_nonce: string;
  dirty_nonce: string;
  dirty_file: string;
};

const text = (v: unknown, what: string): string => {
  if (typeof v !== "string" || v.length === 0) throw new Error(`ssh_flow params without ${what}`);
  return v;
};

/** Validates `params.ssh_flow` set by the parent (Expectations::page_params); throws on any gap. */
export function sshFlowParams(params: Record<string, unknown>): SshFlowParams {
  const raw = params.ssh_flow as Record<string, any> | undefined;
  if (!raw || typeof raw !== "object") throw new Error("params.ssh_flow missing (parent must link ssh_flow)");
  const profile = (p: any, w: string): Profile => {
    if (!p || !Number.isInteger(p.port)) throw new Error(`ssh_flow params without ${w}.port`);
    return { label: text(p.label, `${w}.label`), target: text(p.target, `${w}.target`), port: p.port, session: text(p.session, `${w}.session`) };
  };
  const project = (p: any, w: string): ProjectInput => ({ label: text(p?.label, `${w}.label`), session: text(p?.session, `${w}.session`), root: text(p?.root, `${w}.root`) });
  return {
    ssh_profile: profile(raw.ssh_profile, "ssh_profile"),
    legacy_profile: profile(raw.legacy_profile, "legacy_profile"),
    local: project(raw.local, "local"),
    ssh: project(raw.ssh, "ssh"),
    legacy: project(raw.legacy, "legacy"),
    shared_pane: text(raw.shared_pane, "shared_pane"),
    agent_kind: text(raw.agent_kind, "agent_kind"),
    agent_name: text(raw.agent_name, "agent_name"),
    prompt_nonce: text(raw.prompt_nonce, "prompt_nonce"),
    edit_nonce: text(raw.edit_nonce, "edit_nonce"),
    dirty_nonce: text(raw.dirty_nonce, "dirty_nonce"),
    dirty_file: text(raw.dirty_file, "dirty_file"),
  };
}

export type ShownIdentity = { pane_id: string; generation: string; boot_prefix: string };
/** Status bar diagnostics `pane P · geração G · boot B`. */
export function parseIdentity(title: string): ShownIdentity | null {
  const m = /^pane (\S+) · geração (\S+) · boot (\S+)/.exec(title);
  return m ? { pane_id: m[1]!, generation: m[2]!, boot_prefix: m[3]! } : null;
}

/** `data-cells="x,y,w,h"` of the agents surface. */
export function parseCells(raw: string | undefined | null): [number, number, number, number] | null {
  const parts = (raw ?? "").split(",");
  if (parts.length !== 4 || parts.some((p) => !/^\d+$/.test(p))) return null;
  return parts.map(Number) as [number, number, number, number];
}

/** Host kind of the status bar selection item `LABEL · KIND` (App.svelte), "" when absent. */
export function hostKind(item: string | undefined): string {
  const m = /^.+ · (Local|SSH)$/.exec((item ?? "").trim());
  return m ? m[1]! : "";
}

/** Navigator row meta `sessão S · ROOT`. */
export function parseProjectMeta(meta: string): { session: string; root: string } | null {
  const m = /^sessão (\S+) · (.+)$/.exec(meta.trim());
  return m ? { session: m[1]!, root: m[2]! } : null;
}

/** Tab of the original workspace, the one holding `w1:p1` in both fixture hosts. */
export const ORIGINAL_TAB = "w1:t1";

/** Current-side text of a rendered diff (unchanged/added rows); removed rows are the base side. */
export function currentSideLines(rows: { op: string; text: string }[]): string[] {
  return rows.filter((row) => row.op !== "removed").map((row) => row.text);
}

/** The one focused pane, or null when none or several are shown focused. */
export function uniqueFocus(panes: { pane_id: string; focused: boolean }[]): string | null {
  const focused = panes.filter((pane) => pane.focused);
  return focused.length === 1 ? focused[0]!.pane_id : null;
}

/** Composed pane list button: confirmed geometry `data-cells` and confirmed focus `aria-pressed`. */
export function paneRow(raw: { pane: string; cells: string | undefined | null; pressed: string | undefined | null }) {
  return { pane_id: raw.pane, cells: parseCells(raw.cells), focused: raw.pressed === "true" };
}

/**
 * Path of a shown file under the shown root, only when strictly inside it (exact `/` boundary, no
 * empty/`.`/`..` segment). Null otherwise: the caller then reports the raw shown path, so a wrong
 * root or an escape still fails the evaluator instead of being normalized away.
 */
export function relativeToRoot(shown: string, root: string): string | null {
  if (!root.startsWith("/") || !shown.startsWith("/")) return null;
  const base = root.replace(/\/+$/, "");
  if (!shown.startsWith(`${base}/`)) return null;
  const rest = shown.slice(base.length + 1);
  if (!rest || rest.split("/").some((segment) => segment === "" || segment === "." || segment === "..")) return null;
  return rest;
}

/** Observable state of the composed tab buttons, recorded when a tab wait times out. */
export function tabAttributes(buttons: ArrayLike<{ dataset: { tab?: string }; getAttribute(name: string): string | null; disabled: boolean; textContent: string | null }>) {
  return Array.from(buttons, (b) => ({ tab: b.dataset.tab ?? "", pressed: b.getAttribute("aria-pressed"), disabled: b.disabled, label: (b.textContent ?? "").trim() }));
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

function activity(root: HTMLElement, label: string): HTMLButtonElement {
  const button = root.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`);
  if (!button) throw new Error(`activity ${label} not found`);
  if (button.getAttribute("aria-pressed") !== "true") button.click();
  return button;
}

/** The Files layer is a toggle independent of the pressed activity: observe the layer itself. */
const filesLayerShown = (root: HTMLElement) => {
  const layer = root.querySelector<HTMLElement>('section.layer[aria-label="Arquivos"]');
  return layer !== null && !layer.classList.contains("hidden");
};
async function setFilesShown(root: HTMLElement, shown: boolean): Promise<void> {
  if (filesLayerShown(root) === shown) return;
  press(await waitFor("files toggle", () => root.querySelector<HTMLButtonElement>('button[aria-label="Arquivos e diff"]'), 30000));
  await waitFor(shown ? "files layer shown" : "files layer hidden", () => filesLayerShown(root) === shown, 30000);
}

function labeled(scope: HTMLElement, label: string): HTMLInputElement {
  const input = Array.from(scope.querySelectorAll("label")).find((l) => l.querySelector("span")?.textContent?.trim() === label)?.querySelector("input");
  if (!input) throw new Error(`field ${label} not found`);
  return input;
}

const statusItems = (root: HTMLElement) => Array.from(root.querySelectorAll<HTMLElement>(".status-bar .status-item")).map((i) => i.innerText.trim());
const shownIdentity = (root: HTMLElement) => parseIdentity(root.querySelector<HTMLElement>(".status-bar [data-phase=live]")?.title ?? "");

/** Live identity whose boot differs from `previous` (a host switch was confirmed). */
async function liveIdentity(root: HTMLElement, previousBoot?: string): Promise<ShownIdentity & { status_phase: string; host_kind: string }> {
  const id = await waitFor(
    "confirmed live identity",
    () => {
      const parsed = shownIdentity(root);
      return parsed && parsed.boot_prefix !== previousBoot ? parsed : null;
    },
    60000,
  );
  return { ...id, status_phase: "live", host_kind: hostKind(statusItems(root)[1]) };
}

const hostItems = (root: HTMLElement) => Array.from(root.querySelectorAll<HTMLElement>("li.host[data-host]"));
const localHost = (li: HTMLElement) => li.querySelector(".kind")?.textContent?.trim() === "Local";

/** SSH host through the connection dialog; returns the endpoint the product assigned. */
async function ensureConnection(root: HTMLElement, profile: Profile): Promise<string> {
  activity(root, "Conexões remotas");
  const find = () => hostItems(root).find((li) => li.querySelector("strong")?.textContent?.trim() === profile.label);
  if (!find()) {
    press(await waitFor("new ssh connection", () => byText<HTMLButtonElement>(root, "button", "Nova conexão SSH")));
    const dialog = await waitFor("connection dialog", () => document.querySelector<HTMLElement>('form[aria-label="Conectar a um servidor herdr"]'));
    type(labeled(dialog, "Host"), profile.target);
    type(labeled(dialog, "Porta"), String(profile.port));
    type(labeled(dialog, "Nome de exibição"), profile.label);
    type(labeled(dialog, "Sessão"), profile.session);
    press(await waitFor("connect", () => byText<HTMLButtonElement>(dialog, "button", "Conectar")));
  }
  const host = await waitFor(`host ${profile.label}`, find, 60000);
  return host.dataset.host!;
}

/** Selects a host with the HostsPanel "Usar este host" action (no project open, no API call). */
async function useHost(root: HTMLElement, match: (li: HTMLElement) => boolean, what: string): Promise<void> {
  activity(root, "Conexões remotas");
  const li = await waitFor(what, () => hostItems(root).find(match), 60000);
  if (!li.classList.contains("selected")) {
    press(await waitFor(`${what} selectable`, () => { const b = byText<HTMLButtonElement>(li, "button", "Usar este host"); return b && !b.disabled ? b : null; }, 60000));
  }
  await waitFor(`${what} selected`, () => hostItems(root).find(match)?.classList.contains("selected"), 60000);
}

/** Native input is measured on Local: select it through HostsPanel whatever the previous phase left. */
export async function returnToLocal(root: HTMLElement): Promise<void> {
  const onLocal = () => shownIdentity(root) !== null && hostKind(statusItems(root)[1]) === "Local";
  if (onLocal()) return;
  await useHost(root, localHost, "Local host");
  await waitFor("Local live identity", onLocal, 60000);
  activity(root, "Projetos e coleções");
}

type ProjectRow = { root: string; session: string; badge: string; status: string; workspace: string; outcome: string; error_code: string };
/** Creates (once) and opens a project through the navigator, waiting for that open to finish. */
async function openProject(root: HTMLElement, project: ProjectInput, endpoint: string): Promise<ProjectRow> {
  activity(root, "Projetos e coleções");
  const rowOf = () => root.querySelector<HTMLElement>(`[aria-label^="${project.label},"]`)?.closest<HTMLElement>("li") ?? null;
  if (!rowOf()) {
    if (!byText(root, "button", "Novo projeto aqui") && !root.querySelector("[data-group-menu]")) {
      root.querySelector<HTMLButtonElement>("[data-new-group]")?.click();
      type(await waitFor("new collection", () => root.querySelector<HTMLInputElement>('input[placeholder="Nova coleção"]')), "Fidelidade SSH");
      press(await waitFor("create collection", () => { const b = byText<HTMLButtonElement>(root, "button", "Criar coleção"); return b && !b.disabled ? b : null; }));
    }
    root.querySelector<HTMLButtonElement>("[data-group-menu]")?.click();
    press(await waitFor("new project", () => byText<HTMLButtonElement>(root, "button", "Novo projeto aqui")));
    const form = await waitFor("project form", () => root.querySelector<HTMLElement>('form[aria-label="Novo projeto"]'));
    type(labeled(form, "Nome"), project.label);
    type(labeled(form, "Endpoint"), endpoint);
    type(labeled(form, "Sessão Herdr"), project.session);
    type(labeled(form, "Raiz do projeto"), project.root);
    press(await waitFor("save project", () => byText<HTMLButtonElement>(form, "button", "Salvar projeto")));
  }
  const row = await waitFor(`project ${project.label}`, rowOf, 20000);
  const status = () => row.querySelector<HTMLElement>(".status");
  let opening = false;
  const observer = new MutationObserver(() => {
    if (status()?.dataset.status === "opening") opening = true;
  });
  observer.observe(row, { attributes: true, subtree: true, childList: true });
  try {
    press(await waitFor("open project", () => { const b = byText<HTMLButtonElement>(row, "button", "Abrir"); return b && !b.disabled ? b : null; }));
    await waitFor(`project ${project.label} open finished`, () => (opening || status()?.dataset.status === "opening") && (opening = true) && status()?.dataset.status !== "opening", 90000);
  } finally {
    observer.disconnect();
  }
  const meta = parseProjectMeta(row.querySelector<HTMLElement>(".meta")?.innerText ?? "");
  if (!meta) throw new Error(`project ${project.label} without session/root meta`);
  const s = status();
  return {
    ...meta,
    badge: row.querySelector<HTMLElement>(".badge")?.innerText.trim() ?? "",
    status: s?.dataset.status ?? "",
    workspace: s?.dataset.workspace ?? "",
    outcome: s?.dataset.outcome ?? "",
    error_code: row.querySelector<HTMLElement>('.error[role="alert"] b')?.innerText.trim() ?? "",
  };
}

const agents = (root: HTMLElement) => root.querySelector<HTMLElement>(".agents");
function startForm(root: HTMLElement): HTMLFormElement {
  const form = root.querySelector<HTMLFormElement>('form[aria-label="Iniciar agente"]');
  if (!form) throw new Error("agents start form not mounted");
  return form;
}
const paneOptions = (root: HTMLElement) => Array.from(startForm(root).querySelectorAll("select")[0]?.options ?? []).map((o) => o.value);
function agentsHost(root: HTMLElement): string {
  const identity = agents(root)?.querySelector<HTMLElement>(".identity")?.innerText ?? "";
  return identity.split(" · ")[0]?.trim() ?? "";
}
const paneButtons = (root: HTMLElement) => Array.from(agents(root)?.querySelectorAll<HTMLButtonElement>('ul[aria-label="Panes confirmados pelo servidor"] button[data-pane]') ?? []);
function surfacePanes(root: HTMLElement) {
  return paneButtons(root).map((b) => paneRow({ pane: b.dataset.pane!, cells: b.dataset.cells, pressed: b.getAttribute("aria-pressed") }));
}
const button = (scope: HTMLElement, label: string) => waitFor(label, () => { const b = byText<HTMLButtonElement>(scope, "button", label); return b && !b.disabled ? b : null; }, 30000);

/**
 * Opening a project creates its own workspace; `w1:p1` lives in the original tab. Select that tab
 * with its real button, then wait for it pressed, `pane` as the only confirmed focus and the
 * terminal surface confirming the same pane (on `boot` when given).
 */
async function selectOriginalTab(root: HTMLElement, pane: string, boot?: string): Promise<ShownIdentity> {
  const panel = await waitFor("agents connected", () => { const a = agents(root); return a?.dataset.phase === "connected" ? a : null; }, 60000);
  const tab = () => panel.querySelector<HTMLButtonElement>(`.tabs button[data-tab="${ORIGINAL_TAB}"]`);
  await waitFor(`tab ${ORIGINAL_TAB}`, tab, 30000);
  if (tab()!.getAttribute("aria-pressed") !== "true") {
    press(await waitFor(`tab ${ORIGINAL_TAB} enabled`, () => { const b = tab(); return b && !b.disabled ? b : null; }, 30000));
  }
  try {
    await waitFor(`${ORIGINAL_TAB} pressed with ${pane} as the unique focus`, () => tab()?.getAttribute("aria-pressed") === "true" && uniqueFocus(surfacePanes(root)) === pane, 30000);
  } catch (error) {
    const tabs = tabAttributes(panel.querySelectorAll<HTMLButtonElement>(".tabs button[data-tab]"));
    throw new Error(`${error instanceof Error ? error.message : String(error)}; tabs=${JSON.stringify(tabs)} panes=${JSON.stringify(surfacePanes(root))}`);
  }
  return waitFor(`terminal surface on ${pane}`, () => { const id = shownIdentity(root); return id && id.pane_id === pane && (boot === undefined || id.boot_prefix === boot) ? id : null; }, 60000);
}

async function hostsIdentity(root: HTMLElement, p: SshFlowParams, awaitParent: AwaitParent, ctx: Ctx) {
  const localRow = await openProject(root, p.local, "local");
  const localOpened = await liveIdentity(root);
  const localShown = await selectOriginalTab(root, p.shared_pane, localOpened.boot_prefix);
  const local = { ...localShown, status_phase: "live", host_kind: hostKind(statusItems(root)[1]), pane_options: paneOptions(root) };
  ctx.ssh = await ensureConnection(root, p.ssh_profile);
  const sshRow = await openProject(root, p.ssh, ctx.ssh);
  const sshOpened = await liveIdentity(root, local.boot_prefix);
  await waitFor("ssh agents connected", () => agents(root)?.dataset.phase === "connected" && agentsHost(root) !== "Local", 60000);
  const sshShown = await selectOriginalTab(root, p.shared_pane, sshOpened.boot_prefix);
  const ssh = { ...sshShown, status_phase: "live", host_kind: hostKind(statusItems(root)[1]), pane_options: paneOptions(root) };
  const agentsIdentityHost = agentsHost(root);
  // The selected host is read with HostsPanel mounted, then the Projects activity is restored.
  activity(root, "Conexões remotas");
  const selectedHost = await waitFor("selected host in HostsPanel", () => root.querySelector<HTMLElement>("li.host.selected"), 30000);
  const selected = { endpoint: selectedHost.dataset.host ?? "", label: selectedHost.querySelector("strong")?.textContent?.trim() ?? "" };
  activity(root, "Projetos e coleções");
  await awaitParent("ssh-identity", { endpoint: ctx.ssh });
  return {
    local_identity: local,
    ssh_identity: ssh,
    projects: { local: localRow, ssh: sshRow },
    selected: {
      endpoint: ctx.ssh,
      hosts_panel_endpoint: selected.endpoint,
      host_kind: ssh.host_kind,
      host_label: selected.endpoint === ctx.ssh ? selected.label : "",
      project_badge: sshRow.badge,
      agents_identity_host: agentsIdentityHost,
    },
  };
}

async function agentActions(root: HTMLElement, p: SshFlowParams, awaitParent: AwaitParent) {
  await selectOriginalTab(root, p.shared_pane);
  const panel = await waitFor("agents panel", () => agents(root));
  const form = startForm(root);
  const [paneSelect, kindSelect] = Array.from(form.querySelectorAll("select"));
  if (!paneSelect || !kindSelect) throw new Error("start form selects missing");
  if (!Array.from(paneSelect.options).some((o) => o.value === p.shared_pane)) throw new Error(`UI does not offer ${p.shared_pane} on SSH`);
  await awaitParent("ssh-agent-before", {});
  choose(paneSelect, p.shared_pane);
  choose(kindSelect, p.agent_kind);
  type(form.querySelector<HTMLInputElement>('input[placeholder="nome do agente"]')!, p.agent_name);
  press(await button(form, "Iniciar agente"));
  const agentRow = () => panel.querySelector<HTMLElement>(`li[data-agent-pane="${p.shared_pane}"]`);
  await waitFor("agent row", agentRow, 60000);
  const prompt = await waitFor("prompt input", () => agentRow()?.querySelector<HTMLTextAreaElement>(`textarea[aria-label="Prompt para ${p.agent_name}"]`), 30000);
  type(prompt as unknown as HTMLInputElement, p.prompt_nonce);
  press(await waitFor("send prompt", () => { const b = byText<HTMLButtonElement>(agentRow()!, "button", "Enviar prompt"); return b && !b.disabled ? b : null; }, 30000));
  const outcome = await waitFor("prompt outcome", () => agentRow()?.querySelector<HTMLElement>(".outcome[data-outcome]")?.dataset.outcome || agentRow()?.querySelector<HTMLElement>(".error")?.innerText, 30000);
  const before = surfacePanes(root).length;
  press(await button(panel, "Dividir à direita"));
  await waitFor("split confirmed", () => surfacePanes(root).length === before + 1 && surfacePanes(root).every((x) => x.cells), 30000);
  const target = () => paneButtons(root).find((b) => b.dataset.pane === p.shared_pane);
  await waitFor("shared pane listed", target, 30000);
  if (target()!.getAttribute("aria-pressed") !== "true") {
    press(await waitFor("shared pane focusable", () => { const b = target(); return b && !b.disabled ? b : null; }, 30000));
  }
  await waitFor("focus confirmed", () => uniqueFocus(surfacePanes(root)) === p.shared_pane, 30000);
  const identity = await waitFor("status on focused pane", () => { const id = shownIdentity(root); return id?.pane_id === p.shared_pane ? id : null; }, 30000);
  await awaitParent("ssh-agent-after", {});
  return {
    start_pane: p.shared_pane,
    agent_status_pane: agentRow()?.dataset.agentPane ?? "",
    prompt_outcome: outcome,
    focus_target: p.shared_pane,
    ssh_identity_after: identity,
    panes_after: surfacePanes(root),
  };
}

async function filesReadonly(root: HTMLElement, p: SshFlowParams, awaitParent: AwaitParent, ctx: Ctx) {
  await awaitParent("ssh-files-before", {});
  await setFilesShown(root, true);
  const workspace = await waitFor("remote files workspace", () => root.querySelector<HTMLElement>('section.layer[aria-label="Arquivos"] [data-remote-files-workspace]'), 30000);
  const header = await waitFor("remote header", () => workspace.querySelector<HTMLElement>("[data-remote-header]"), 30000);
  const tree = () => workspace.querySelector<HTMLElement>('[aria-label="Arquivos do projeto remoto"]');
  const entry = (name: string) => tree()?.querySelector<HTMLButtonElement>(`:scope > .node > button[data-name="${name}"]`) ?? null;
  try {
    await waitFor("remote tree", () => entry("notas.txt"), 5000);
  } catch {
    // The explorer lists on host selection; roots authorized later need the explicit refresh.
    press(await waitFor("Atualizar enabled", () => { const b = workspace.querySelector<HTMLButtonElement>("[data-remote-refresh]"); return b && !b.disabled ? b : null; }, 30000));
    await waitFor("remote tree after Atualizar", () => entry("notas.txt"), 30000);
  }
  const names = () => Array.from(tree()?.querySelectorAll<HTMLElement>(":scope > .node > button[data-name]") ?? []).map((e) => e.dataset.name!);
  const documentOf = (name: string) => {
    const d = workspace.querySelector<HTMLElement>("[data-remote-document]");
    return d && (d.querySelector<HTMLElement>("[data-file-path]")?.innerText ?? "").endsWith(name) ? d : null;
  };
  entry("notas.txt")!.click();
  const doc = await waitFor("remote editor", () => { const c = documentOf("notas.txt")?.querySelector<HTMLElement>(".cm-content"); return c && c.innerText.trim() ? c : null; }, 30000);
  const shownRoot = header.querySelector<HTMLElement>(".root")?.getAttribute("title") ?? "";
  const shownPath = (d: HTMLElement | null) => {
    const shown = d?.querySelector<HTMLElement>("[data-file-path]")?.innerText.trim() ?? "";
    return relativeToRoot(shown, shownRoot) ?? shown;
  };
  const opened = { path: shownPath(documentOf("notas.txt")), text: doc.innerText };
  const badge = header.querySelector<HTMLElement>("[data-read-only]");
  const before = doc.innerText;
  doc.focus();
  document.execCommand("insertText", false, p.edit_nonce);
  await sleep(300);
  const edit = { nonce: p.edit_nonce, contenteditable: doc.getAttribute("contenteditable") ?? "", text_before: before, text_after: doc.innerText };
  entry("diff-target.txt")!.click();
  const diffDoc = await waitFor("diff-target document", () => documentOf("diff-target.txt"), 30000);
  press(await waitFor("reload", () => { const b = documentOf("diff-target.txt")?.querySelector<HTMLButtonElement>("[data-reload]"); return b && !b.disabled ? b : null; }, 30000));
  press(await waitFor("compare", () => { const b = documentOf("diff-target.txt")?.querySelector<HTMLButtonElement>("[data-compare]"); return b && !b.disabled ? b : null; }, 30000));
  const diff = await waitFor("remote diff", () => documentOf("diff-target.txt")?.querySelector<HTMLElement>("[data-remote-diff]"), 30000);
  const rows = Array.from(diff.querySelectorAll<HTMLElement>(".diff-lines li")).map((li) => ({ op: li.dataset.op ?? "", text: li.querySelector("code")?.textContent ?? "" }));
  const report = {
    selected_endpoint: ctx.ssh,
    explorer_host: header.dataset.endpoint ?? "",
    explorer_host_label: header.querySelector<HTMLElement>("[data-remote-badge]")?.dataset.host ?? "",
    explorer_names: names(),
    shown_root: shownRoot,
    opened,
    diff: {
      host: header.dataset.endpoint ?? "",
      path: shownPath(diffDoc),
      lines: currentSideLines(rows),
      sources: diff.querySelector<HTMLElement>("[data-diff-sources]")?.innerText ?? "",
    },
    read_only_badge: badge?.innerText.trim() === "Somente leitura",
    save_control: Boolean(byText(workspace, "[data-remote-document] button", "Salvar")),
    edit_attempt: edit,
  };
  await awaitParent("ssh-files-after", {});
  await setFilesShown(root, false);
  return report;
}

async function legacyServer(root: HTMLElement, p: SshFlowParams, awaitParent: AwaitParent, ctx: Ctx) {
  await awaitParent("ssh-legacy-before", {});
  const before = shownIdentity(root);
  if (!before) throw new Error("legacy-server: no confirmed identity before switching to the legacy host");
  const endpoint = await ensureConnection(root, p.legacy_profile);
  // The legacy server has no JSON API: it is selected in HostsPanel (no project open through the API).
  await useHost(root, (li) => li.dataset.host === endpoint, `legacy host ${p.legacy_profile.label}`);
  const identity = await liveIdentity(root, before.boot_prefix);
  const panel = await waitFor("agents connected on legacy", () => { const a = agents(root); return a?.dataset.phase === "connected" && agentsHost(root) === p.legacy_profile.label ? a : null; }, 60000);
  const alert = await waitFor("capability refusal", () => Array.from(panel.querySelectorAll<HTMLElement>("[data-error-code]")).find((e) => e.dataset.errorCode), 60000);
  const legacy = {
    agents_error_code: alert.dataset.errorCode ?? "",
    agents_error_message: alert.innerText,
    start_enabled: !(startForm(root).querySelector<HTMLButtonElement>('button[type="submit"]')?.disabled ?? true),
    split_enabled: !(byText<HTMLButtonElement>(panel, "button", "Dividir à direita")?.disabled ?? true),
    status_phase: identity.status_phase,
    identity,
  };
  await useHost(root, (li) => li.dataset.host === ctx.ssh, `ssh host ${p.ssh_profile.label}`);
  const sshAfter = await liveIdentity(root, identity.boot_prefix);
  await useHost(root, localHost, "Local host");
  const localAfter = await liveIdentity(root, sshAfter.boot_prefix);
  activity(root, "Projetos e coleções");
  await awaitParent("ssh-legacy-after", {});
  return { legacy, ssh_after: { status_phase: sshAfter.status_phase, identity: sshAfter }, local_after: { status_phase: localAfter.status_phase, identity: localAfter } };
}

async function dirtySwitch(root: HTMLElement, p: SshFlowParams, awaitParent: AwaitParent, ctx: Ctx) {
  await awaitParent("ssh-dirty-before", {});
  await openProject(root, p.local, "local");
  const local = await waitFor("Local live identity", () => { const id = shownIdentity(root); return id && hostKind(statusItems(root)[1]) === "Local" ? id : null; }, 60000);
  await setFilesShown(root, true);
  (await waitFor("local file", () => root.querySelector<HTMLButtonElement>(`button.entry[data-name="${p.dirty_file}"]`), 30000)).click();
  const content = await waitFor("local editor", () => root.querySelector<HTMLElement>('[aria-label="Editor de arquivos"] .cm-content'), 30000);
  content.focus();
  document.execCommand("insertText", false, p.dirty_nonce);
  const tab = () => root.querySelector<HTMLElement>(`[aria-label="Arquivos abertos"] [data-file$="${p.dirty_file}"]`);
  await waitFor("dirty tab", () => tab()?.dataset.dirty === "true", 10000);
  const typed = content.innerText;
  await openProject(root, p.ssh, ctx.ssh!);
  const via = await liveIdentity(root, local.boot_prefix);
  await openProject(root, p.local, "local");
  const back = await liveIdentity(root, via.boot_prefix);
  await setFilesShown(root, true);
  const returned = await waitFor("local editor again", () => root.querySelector<HTMLElement>('[aria-label="Editor de arquivos"] .cm-content'), 30000);
  const report = {
    file: p.dirty_file,
    dirty_before_switch: true,
    dirty_after_return: tab()?.dataset.dirty === "true",
    text_before_switch: typed,
    text_after_return: returned.innerText,
    ssh_during_switch: via,
    local_after_return: back,
  };
  await awaitParent("ssh-dirty-after", {});
  await setFilesShown(root, false);
  return report;
}

/** Endpoint the product assigned to the SSH host, shared by later phases of the same window. */
type Ctx = { ssh?: string };
const ctx: Ctx = {};

export async function runSshPhase(phase: string, root: HTMLElement, params: Record<string, unknown>, awaitParent: AwaitParent): Promise<Record<string, unknown>> {
  if (!(SSH_PHASES as readonly string[]).includes(phase)) throw new Error(`${phase} is not an SSH phase`);
  const p = sshFlowParams(params);
  if (phase !== "hosts-identity" && !ctx.ssh) throw new Error(`${phase}: hosts-identity did not establish the SSH host`);
  switch (phase as SshPhase) {
    case "hosts-identity":
      return hostsIdentity(root, p, awaitParent, ctx);
    case "ssh-agent-actions":
      return agentActions(root, p, awaitParent);
    case "ssh-files-readonly":
      return filesReadonly(root, p, awaitParent, ctx);
    case "legacy-server":
      return legacyServer(root, p, awaitParent, ctx);
    case "host-switch-dirty":
      return dirtySwitch(root, p, awaitParent, ctx);
  }
}
