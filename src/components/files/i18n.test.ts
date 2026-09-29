// @vitest-environment happy-dom
// Spec 070 (PRD i18n) — connections, files, terminal and shell in the three languages. The region
// is proved the way a user reads it: the real components are mounted over fake bridges, and the
// rendered tree (texts plus the accessible names) is what the assertions look at.
//
// Would catch: a label, placeholder, `aria-label` or `title` of this region left in Portuguese
// when the locale is English; a Spanish window still showing Portuguese words; the `(s)` plural
// shortcuts surviving as `linha(s)`/`importado(s)` instead of real plural forms; or one of the
// four `{code} — {message}` error lines bypassing `errorText` from spec 067.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import ConnectionDialog from "../ConnectionDialog.svelte";
import FilesWorkspace from "../FilesWorkspace.svelte";
import ProjectNavigator from "../ProjectNavigator.svelte";
import RemoteFilesWorkspace from "../RemoteFilesWorkspace.svelte";
import { createConnectionsController, type ConnectionsController, type ConnectionsState } from "../../connections/controller";
import { createFakeConnectionsBridge } from "../../connections/fake-bridge";
import { createFilesController } from "../../files/controller";
import { createFakeFilesBridge } from "../../files/fake-bridge";
import { createFakeRemoteFilesBridge, createRemoteFilesController } from "../../files/remote";
import { createProjectsController } from "../../projects/controller";
import { createFakeProjectsBridge } from "../../projects/fake-bridge";
import HostsPanel from "../../shell/HostsPanel.svelte";
import TerminalView from "../../terminal/TerminalView.svelte";
import type { FrameEvent, InputDto, PaneMeta } from "../../terminal/types";
import { errorText, setLocalePreference, type Locale } from "../../i18n/index.svelte";
import { untranslated } from "../../i18n/testing";

const source = (path: string) => readFileSync(resolve(path), "utf8");

// --- mounting ---------------------------------------------------------------------------------

const live: { el: HTMLElement; app: ReturnType<typeof mount> }[] = [];

/** Mounts `component` into a fresh element of `root` and flushes, as the window would. */
function show<P extends Record<string, unknown>>(root: HTMLElement, component: unknown, props: P): HTMLElement {
  const el = document.createElement("div");
  root.appendChild(el);
  const app = mount(component as Parameters<typeof mount>[0], { target: el, props });
  live.push({ el, app });
  flushSync();
  return el;
}

function fresh(): HTMLElement {
  const root = document.createElement("div");
  document.body.appendChild(root);
  live.push({ el: root, app: null as unknown as ReturnType<typeof mount> });
  return root;
}

afterEach(() => {
  for (const item of live.splice(0)) {
    if (item.app) unmount(item.app);
    item.el.remove();
  }
});

beforeEach(() => {
  // happy-dom has no modal dialog: the component only needs `showModal`/`close` to toggle `open`.
  const proto = window.HTMLDialogElement?.prototype as HTMLDialogElement | undefined;
  if (proto && typeof proto.showModal !== "function") {
    proto.showModal = function showModal(this: HTMLDialogElement) {
      this.open = true;
    };
    proto.close = function close(this: HTMLDialogElement) {
      this.open = false;
      this.dispatchEvent(new Event("close"));
    };
  }
});

/** Every visible text and accessible name of `root`, as a screen reader would announce them. */
function announced(root: Element): string[] {
  const found: string[] = [];
  const visit = (node: Node): void => {
    if (node.nodeType === Node.ELEMENT_NODE) {
      const element = node as Element;
      if (element.tagName === "STYLE" || element.tagName === "SCRIPT") return;
      for (const attribute of ["aria-label", "title", "placeholder"]) {
        const value = element.getAttribute(attribute)?.trim() ?? "";
        if (value !== "") found.push(value);
      }
      for (const child of Array.from(element.childNodes)) visit(child);
      return;
    }
    if (node.nodeType === Node.TEXT_NODE) {
      const text = (node.textContent ?? "").trim();
      if (text !== "") found.push(text);
    }
  };
  visit(root);
  return found;
}

/** Words this region used to show in Portuguese; none of them belongs to a Spanish window. */
const PORTUGUESE =
  /(?<![\p{L}])(?:não|nenhum|nenhuma|você|arquivo|arquivos|conexão|conexões|linha|linhas|seleção|fechar|salvar|sessão|ação|ações|vazia|atualizar|aguardando|carregando)(?![\p{L}])/iu;

function portuguese(root: Element): string[] {
  return announced(root).filter((text) => PORTUGUESE.test(text));
}

// --- fixtures ---------------------------------------------------------------------------------

const DEMO_ERROR = { code: "demo_code", message: "raw engine message", retryable: false };
const UNKEYED_ERROR = { code: "no_such_code_070", message: "raw engine message", retryable: false };

/** Connections state with the dialog open on an invalid draft (AC-070-01: validation errors). */
async function connectionsFixture(): Promise<{ controller: ConnectionsController; state: () => ConnectionsState }> {
  let current: ConnectionsState | null = null;
  const controller = createConnectionsController(createFakeConnectionsBridge(), (next) => (current = next));
  await controller.load();
  controller.openDialog();
  controller.editDraft("session", "");
  await controller.submitDialog(false);
  return { controller, state: () => current ?? controller.state };
}

async function projectsFixture() {
  let current = null as ReturnType<typeof createProjectsController>["state"] | null;
  const controller = createProjectsController(createFakeProjectsBridge({ bootId: "boot-070" }), (next) => (current = next));
  await controller.load();
  await controller.createCollection("Work");
  const collection = (current ?? controller.state).snapshot!.collections[0]!;
  controller.openForm(collection.id);
  await controller.submitProject();
  return { controller, state: () => current ?? controller.state };
}

async function filesFixture() {
  const root = "/work";
  let current = null as ReturnType<typeof createFilesController>["state"] | null;
  const controller = createFilesController(
    createFakeFilesBridge({ root, files: { [`${root}/README.md`]: "one\ntwo\n" } }),
    (next) => (current = next),
  );
  await controller.setTarget({ provider: "local", host: null, root });
  return { controller, state: () => current ?? controller.state, target: { provider: "local", host: null, root } };
}

async function remoteFixture() {
  let current = null as ReturnType<typeof createRemoteFilesController>["state"] | null;
  const controller = createRemoteFilesController(createFakeRemoteFilesBridge(), (next) => (current = next));
  await controller.refreshHosts();
  const endpoint = (current ?? controller.state).hosts[0]!.endpoint;
  await controller.selectHost(endpoint);
  await controller.loadRoot();
  return { controller, state: () => current ?? controller.state, endpoint };
}

/** A terminal with three selected rows and a notice, so the toolbar renders both. */
function terminalFixture(root: HTMLElement) {
  let push: (event: FrameEvent) => void = () => {};
  const el = show(root, TerminalView, {
    subscribe: (handler: (event: FrameEvent) => void) => {
      push = handler;
      return () => {};
    },
    onInput: (_events: InputDto[]) => {},
    onResize: () => {},
    fontSize: 16,
  });
  const width = 8;
  const height = 6;
  const cells = [];
  for (let y = 0; y < height; y++) for (let x = 0; x < width; x++) cells.push({ s: "a", fg: 0, bg: 0, m: 0 });
  const pane: PaneMeta = {
    pane_id: "p1",
    content_revision: 1,
    rect: { x: 0, y: 0, width, height },
    inner_rect: { x: 0, y: 0, width, height },
    scroll: { offset_from_bottom: 0, max_offset_from_bottom: 0, viewport_rows: height },
    focused: true,
    mouse_reporting: false,
    sgr_pixel_mouse: false,
    alternate_screen_active: false,
    pixel_width: width * 10,
    pixel_height: height * 20,
  };
  push({
    type: "full",
    revision: 1,
    width,
    height,
    cells,
    cursor: { x: 0, y: 0, visible: false, shape: 2 },
    panes: [{ pane_id: "p1", x: 0, y: 0, width, height, focused: true }],
  });
  push({ type: "metadata", revision: 1, panes: [pane], hyperlinks: [] });
  flushSync();
  return { el, push };
}

/** Drags a selection of `rows` rows over the mounted terminal and returns the toolbar status. */
function selectRows(view: { el: HTMLElement }, rows: number): string {
  const target = view.el.querySelector<HTMLTextAreaElement>("textarea.ime-target")!;
  const cell = { w: 10, h: 20 };
  target.dispatchEvent(new PointerEvent("pointerdown", { button: 0, clientX: 1, clientY: 1, bubbles: true }));
  target.dispatchEvent(
    new PointerEvent("pointermove", { buttons: 1, clientX: 3 * cell.w, clientY: (rows - 1) * cell.h + 1, bubbles: true }),
  );
  flushSync();
  return view.el.querySelector<HTMLElement>(".actions .status")!.textContent!.replace(/\s+/g, " ").trim();
}

/** The whole region of this spec, mounted in one tree. */
async function region(): Promise<HTMLElement> {
  const root = fresh();
  const connections = await connectionsFixture();
  show(root, ConnectionDialog, { controller: connections.controller, get state() { return connections.state(); } });
  show(root, HostsPanel, {
    controller: connections.controller,
    get state() { return connections.state(); },
    selected: null,
    onSelect: () => {},
  });
  const projects = await projectsFixture();
  show(root, ProjectNavigator, { controller: projects.controller, get state() { return projects.state(); } });
  const files = await filesFixture();
  show(root, FilesWorkspace, {
    controller: files.controller,
    get state() { return files.state(); },
    target: files.target,
  });
  const remote = await remoteFixture();
  show(root, RemoteFilesWorkspace, {
    controller: remote.controller,
    get state() { return remote.state(); },
    endpoint: remote.endpoint,
  });
  terminalFixture(root);
  flushSync();
  // The four connection progress lines are spec 071's area (`src/connections/progress.ts`), which
  // runs in parallel with this one; they are not part of what 070 translates.
  for (const box of Array.from(root.querySelectorAll(".progress-box"))) box.remove();
  return root;
}

// --- AC-070-01: English -----------------------------------------------------------------------

describe("AC-070-01 inglês", () => {
  // Would catch: any label, accessible name or placeholder of the connections/files/terminal/shell
  // region still written in Portuguese while the window is in English.
  it("renders the whole region with nothing left in Portuguese", async () => {
    setLocalePreference("en");
    const root = await region();
    expect(untranslated(root)).toEqual([]);
  });

  // Would catch: the connection dialog's own chrome or its validation messages left untranslated.
  it("shows the connection dialog and its validation in English", async () => {
    setLocalePreference("en");
    const root = fresh();
    const connections = await connectionsFixture();
    const el = show(root, ConnectionDialog, {
      controller: connections.controller,
      get state() { return connections.state(); },
    });
    const texts = announced(el);
    expect(texts).toContain("Connect to a herdr server");
    expect(texts).toContain("Display name");
    expect(texts).toContain("Enter the session");
    for (const box of Array.from(el.querySelectorAll(".progress-box"))) box.remove();
    expect(untranslated(el)).toEqual([]);
  });

  // Would catch: `linha(s)` surviving as a fake plural instead of `1 line` / `3 lines`.
  it("pluralises the selected terminal rows", () => {
    setLocalePreference("en");
    const root = fresh();
    const view = terminalFixture(root);
    expect(selectRows(view, 1)).toContain("Selection: 1 line");
    expect(selectRows(view, 3)).toContain("Selection: 3 lines");
  });

  // Would catch: the project form's own validation left in Portuguese in an English window.
  it("shows the project form validation in English", async () => {
    setLocalePreference("en");
    const root = fresh();
    const projects = await projectsFixture();
    const el = show(root, ProjectNavigator, { controller: projects.controller, get state() { return projects.state(); } });
    expect(announced(el)).toContain("Required");
    expect(untranslated(el)).toEqual([]);
  });

  // Would catch: `importado(s)` surviving in the imported-profiles report.
  it("pluralises the imported profiles report", async () => {
    setLocalePreference("en");
    for (const [count, expected] of [[1, "1 imported"], [2, "2 imported"]] as const) {
      const root = fresh();
      const connections = await connectionsFixture();
      const state = connections.state();
      const report = {
        imported: Array.from({ length: count }, (_, index) => ({ label: `host-${index}`, id: `id-${index}` })),
        skipped: [],
        already_present: 0,
      };
      const el = show(root, HostsPanel, {
        controller: connections.controller,
        state: { ...state, importReport: report },
        selected: null,
        onSelect: () => {},
      });
      expect(announced(el).join(" ")).toContain(expected);
    }
  });
});

// --- AC-070-02: Spanish -----------------------------------------------------------------------

describe("AC-070-02 espanhol", () => {
  // Would catch: a Spanish window falling back to the Portuguese text of this region.
  it("renders the whole region with no Portuguese left", async () => {
    setLocalePreference("es");
    const root = await region();
    expect(portuguese(root)).toEqual([]);
  });

  // Would catch: the Spanish plural rule not reaching the terminal selection status.
  it("pluralises the selected terminal rows in Spanish", () => {
    setLocalePreference("es");
    const root = fresh();
    const view = terminalFixture(root);
    expect(selectRows(view, 1)).toContain("1 línea");
    expect(selectRows(view, 3)).toContain("3 líneas");
  });

  // Would catch: form validation left in another language (the AC's own example).
  it("validates the forms in Spanish", async () => {
    setLocalePreference("es");
    const root = fresh();
    const projects = await projectsFixture();
    const navigator = show(root, ProjectNavigator, {
      controller: projects.controller,
      get state() { return projects.state(); },
    });
    expect(announced(navigator)).toContain("Obligatorio");
    const connections = await connectionsFixture();
    const dialog = show(root, ConnectionDialog, {
      controller: connections.controller,
      get state() { return connections.state(); },
    });
    expect(announced(dialog)).toContain("Indique la sesión");
    expect(portuguese(root)).toEqual([]);
  });
});

// --- AC-070-03: engine errors and the Portuguese window ---------------------------------------

describe("AC-070-03 erros e português", () => {
  const places = [
    "src/components/ConnectionDialog.svelte",
    "src/components/ProjectNavigator.svelte",
    "src/components/RemoteFilesWorkspace.svelte",
    "src/App.svelte",
  ];

  // Would catch: one of the four places printing `error.message` directly, so a coded engine error
  // would stay in the engine's language even when the product has words for it.
  it("routes the four {code} — {message} places through errorText", () => {
    for (const path of places) {
      expect(source(path), path).toContain("errorText(");
    }
  });

  // Would catch: `errorText` hiding an engine message that has no key of its own.
  it("shows the key of a coded error and otherwise the engine message", () => {
    for (const [language, keyed] of [["en", "Demo error"], ["pt", "Erro de demonstração"], ["es", "Error de demostración"]] as const) {
      setLocalePreference(language as Locale);
      expect(errorText(DEMO_ERROR)).toBe(keyed);
      expect(errorText(UNKEYED_ERROR)).toBe("raw engine message");
    }
  });

  // Would catch: a coded error rendered as its raw message inside the mounted components.
  it("renders errorText where the components show an engine error", async () => {
    setLocalePreference("en");
    const root = fresh();
    const projects = await projectsFixture();
    const state = projects.state();
    const navigator = show(root, ProjectNavigator, {
      controller: projects.controller,
      state: { ...state, globalError: DEMO_ERROR },
    });
    const texts = announced(navigator).join(" ");
    expect(texts).toContain("Demo error");
    expect(texts).not.toContain("raw engine message");
  });

  // Would catch: a Portuguese window changing text this spec only had to translate (P4 of the PRD:
  // the existing pt assertions stay valid).
  it("keeps the Portuguese window on the words it already had", async () => {
    setLocalePreference("pt");
    const root = fresh();
    const connections = await connectionsFixture();
    const el = show(root, ConnectionDialog, {
      controller: connections.controller,
      get state() { return connections.state(); },
    });
    const texts = announced(el);
    expect(texts).toContain("Conectar a um servidor herdr");
    expect(texts).toContain("Nome de exibição");
    expect(texts).toContain("Informe a sessão");
  });
});
