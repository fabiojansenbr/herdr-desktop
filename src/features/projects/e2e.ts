// Native window scenario of spec 002 (driven by src-tauri/tests/projects.rs through
// scripts/feature-harness/window.rs). Acts on ProjectNavigator like a user — typing,
// choosing and clicking — over the real IPC bridge and backend, and reports what the
// window shows. Phases: create → restart (new GUI process) → reload (new GUI process).

import { byText, choose, press, type, waitFor } from "../../harness/dom";
import type { Scenario } from "../../harness/native";

interface RenderedRow {
  id: string;
  label: string;
  endpoint: string;
  status: string;
  workspace: string;
  outcome: string;
  error: string | null;
}

function section(root: ParentNode, name: string): HTMLElement | null {
  return root.querySelector<HTMLElement>(`section[aria-label="Coleção ${name}"]`);
}

function rows(scope: ParentNode): RenderedRow[] {
  return Array.from(scope.querySelectorAll<HTMLElement>("li[data-project]")).map((li) => {
    const status = li.querySelector<HTMLElement>(".status");
    return {
      id: li.dataset.project ?? "",
      label: li.querySelector(".label")?.textContent?.trim() ?? "",
      endpoint: li.querySelector(".badge")?.textContent?.trim() ?? "",
      status: status?.dataset.status ?? "",
      workspace: status?.dataset.workspace ?? "",
      outcome: status?.dataset.outcome ?? "",
      error: li.querySelector(".error")?.textContent?.trim() ?? null,
    };
  });
}

function rendered(root: ParentNode) {
  return Array.from(root.querySelectorAll<HTMLElement>("section[data-collection]")).map((s) => ({
    name: s.querySelector("h3")?.firstChild?.textContent?.trim() ?? "",
    projects: rows(s),
  }));
}

function row(root: ParentNode, collection: string, label: string): HTMLElement | null {
  const scope = section(root, collection);
  if (!scope) return null;
  return (
    Array.from(scope.querySelectorAll<HTMLElement>("li[data-project]")).find(
      (li) => li.querySelector(".label")?.textContent?.trim() === label,
    ) ?? null
  );
}

function rowButton(li: HTMLElement, text: string): HTMLButtonElement {
  const button = Array.from(li.querySelectorAll<HTMLButtonElement>("button")).find((b) => b.textContent?.trim() === text);
  if (!button) throw new Error(`button "${text}" not found in row`);
  return button;
}

async function waitLoaded(root: HTMLElement) {
  await waitFor("navigator loaded", () => root.querySelector<HTMLButtonElement>("[data-new-group]"));
}

async function openNewGroup(root: HTMLElement) {
  const neu = root.querySelector<HTMLButtonElement>("[data-new-group]");
  if (neu) neu.click();
  return waitFor("new collection input", () => root.querySelector<HTMLInputElement>('input[placeholder="Nova coleção"]'));
}

async function createCollection(root: HTMLElement, name: string) {
  type(await openNewGroup(root), name);
  press(await waitFor("enabled create collection", () => {
    const b = byText<HTMLButtonElement>(root, "button", "Criar coleção");
    return b && !b.disabled ? b : null;
  }));
  await waitFor(`collection ${name}`, () => section(root, name));
}

async function createProject(root: HTMLElement, collection: string, label: string, session: string, projectRoot: string) {
  const group = section(root, collection)!;
  group.querySelector<HTMLButtonElement>("[data-group-menu]")?.click();
  press(byText<HTMLButtonElement>(group, "button", "Novo projeto aqui")!);
  const form = await waitFor("project form", () => root.querySelector<HTMLFormElement>('form[aria-label="Novo projeto"]'));
  const field = (name: string) => {
    const label = Array.from(form.querySelectorAll("label")).find((l) => l.querySelector("span")?.textContent === name);
    const input = label?.querySelector<HTMLInputElement>("input");
    if (!input) throw new Error(`form field ${name} not found`);
    return input;
  };
  type(field("Nome"), label);
  type(field("Endpoint"), "local");
  type(field("Sessão Herdr"), session);
  type(field("Raiz do projeto"), projectRoot);
  press(byText<HTMLButtonElement>(form, "button", "Salvar projeto")!);
  await waitFor(`project ${label} in ${collection}`, () => {
    const error = form.isConnected ? form.querySelector(".error")?.textContent : null;
    if (error) throw new Error(`project form error: ${error}`);
    return row(root, collection, label);
  });
}

/** Clicks "Abrir" (or "Tentar novamente") and waits until the row shows an outcome other than `previous`. */
async function open(root: HTMLElement, collection: string, label: string, previous: string) {
  const li = row(root, collection, label)!;
  const button = Array.from(li.querySelectorAll<HTMLButtonElement>("button")).find((b) =>
    ["Abrir", "Tentar novamente"].includes(b.textContent?.trim() ?? ""),
  );
  if (!button) throw new Error(`open button not found for ${label}`);
  press(button);
  return waitFor(
    `open of ${label}`,
    () => {
      const current = row(root, collection, label);
      const error = current?.querySelector(".error")?.textContent?.trim();
      if (error) throw new Error(`open of ${label} failed in the window: ${error}`);
      const status = current?.querySelector<HTMLElement>(".status");
      if (!status || status.dataset.status !== "open" || !status.dataset.outcome) return null;
      if (status.dataset.outcome === previous) return null;
      return { workspace: status.dataset.workspace ?? "", outcome: status.dataset.outcome, text: status.textContent?.trim() ?? "" };
    },
    30000,
  );
}

export const run: Scenario = async ({ phase, params, root, progress }) => {
  const session = String(params.session ?? "");
  await waitLoaded(root);
  const loaded = rendered(root);

  if (phase === "create") {
    const onboarding = root.querySelector(".onboarding")?.textContent?.trim() ?? null;
    if (!onboarding) throw new Error("premise: empty store must show onboarding");
    await createCollection(root, "Produto");
    await createCollection(root, "Pessoal");
    await createProject(root, "Produto", "Projeto A", session, String(params.root_a));
    await createProject(root, "Produto", "Projeto B", session, String(params.root_b));
    const pessoal = section(root, "Pessoal")!;
    const select = pessoal.querySelector<HTMLSelectElement>("select")!;
    const optionA = Array.from(select.options).find((o) => o.textContent === "Projeto A");
    if (!optionA) throw new Error("Projeto A not offered for Pessoal");
    choose(select, optionA.value);
    press(await waitFor("enabled add", () => {
      const b = byText<HTMLButtonElement>(pessoal, "button", "Adicionar");
      return b && !b.disabled ? b : null;
    }));
    await waitFor("Projeto A in Pessoal", () => row(root, "Pessoal", "Projeto A"));
    const beforeOpen = rendered(root);
    await progress({ step: "created", collections: beforeOpen });

    const first = await open(root, "Produto", "Projeto A", "");
    const second = await open(root, "Produto", "Projeto A", first.outcome);
    return {
      onboarding,
      loaded,
      collections_before_open: beforeOpen,
      first_open: first,
      second_open: second,
      collections: rendered(root),
    };
  }

  if (phase === "restart") {
    const first = await open(root, "Pessoal", "Projeto A", "");
    const inProduto = row(root, "Produto", "Projeto A");
    if (!inProduto) throw new Error("Projeto A missing from Produto before removal");
    press(rowButton(inProduto, "Remover da coleção"));
    await waitFor("Projeto A removed from Produto", () => (row(root, "Produto", "Projeto A") ? null : true));
    return { loaded, open: first, collections: rendered(root) };
  }

  if (phase === "reload") {
    return { loaded };
  }

  throw new Error(`unknown phase ${phase}`);
};
