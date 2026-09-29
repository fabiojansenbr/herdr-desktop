// Spec 011 — page side of the native visual-projects phase with a fake page and parent.
// The page only observes (project tree, connections footer, connection dialog DOM);
// screenshot capture and window keys come from the parent, and the parent computes every check.
import { describe, expect, it } from "vitest";
import {
  runVisualProjects,
  VISUAL_PROJECTS_STEPS,
  type VisualProjectsPage,
} from "./visual-projects";

const identity = {
  pane_id: "w3:p1",
  generation: "5",
  boot_prefix: "2040265-",
  endpoint: "Este computador · Local",
};

function fakePage(over: Partial<VisualProjectsPage> = {}) {
  const log: string[] = [];
  let dialogOpen = false;

  const page: VisualProjectsPage = {
    identity: () => identity,
    settle: async () => {
      log.push("settle");
      return ["Projetos e coleções: click"];
    },
    sidebar: () => ({
      groups: [
        { name: "Frontend", color: "rgb(244, 180, 84)", count: 2, collapsed: false },
        { name: "Backend", color: "rgb(91, 214, 138)", count: 1, collapsed: false },
      ],
      projects: [
        {
          name: "herdr-desktop",
          branch: "main",
          host_badge: null,
          active: true,
          bg_color: "rgb(30, 34, 43)",
          accent_bar_width: 2,
          accent_bar_color: "rgb(143, 168, 255)",
          agent_dots: [{ status: "working", label: "Agente trabalhando" }],
          has_menu: true,
        },
        {
          name: "remote-api",
          branch: "feat-auth",
          host_badge: "dev-box",
          active: false,
          bg_color: "rgba(0, 0, 0, 0)",
          accent_bar_width: 0,
          accent_bar_color: "",
          agent_dots: [],
          has_menu: true,
        },
      ],
      connections: [
        {
          label: "Este computador · Local",
          type_badge: "Local",
          latency_text: "12 ms",
          status_text: "Online",
          tone: "ok",
        },
        {
          label: "dev-box",
          type_badge: "SSH",
          latency_text: "45 ms",
          status_text: "online",
          tone: "ok",
        },
      ],
      small_texts: [
        {
          scope: "projects-sidebar",
          text: "Frontend",
          color: "rgb(231, 233, 238)",
          background: "rgba(17, 19, 24, 1)",
          opacity: 1,
          font_size: 13,
          font_weight: 500,
          visibility: "visible",
        },
      ],
    }),
    openDialog: async () => {
      log.push("open-dialog");
      dialogOpen = true;
      return true;
    },
    dialog: () =>
      dialogOpen
        ? {
            open: true,
            width: 620,
            height: 520,
            border_radius: 12,
            cards: [
              { kind: "local", role: "radio", tabindex: -1, selected: false },
              { kind: "ssh", role: "radio", tabindex: 0, selected: true },
            ],
            fields: [
              "Host",
              "Porta",
              "Autenticação",
              "Nome de exibição",
              "Adicionar projetos ao grupo",
              "Sessão",
            ],
            progress_items: [
              { text: "Host alcançável · 45 ms", step: 1, status: "ok" },
              { text: "Autenticado como dev", step: 2, status: "ok" },
              {
                text: "herdr 0.9.0 encontrado · endpoint geração 1",
                step: 3,
                status: "ok",
              },
              { text: "Lendo workspaces remotos", step: 4, status: "ok" },
            ],
            small_texts: [
              {
                scope: "connection-dialog",
                text: "Conectar a um servidor herdr",
                color: "rgb(231, 233, 238)",
                background: "rgba(23, 26, 33, 1)",
                opacity: 1,
                font_size: 16,
                font_weight: 600,
                visibility: "visible",
              },
            ],
          }
        : null,
    activeId: () => (dialogOpen ? "add-connection-btn" : "add-connection-btn"),
    ...over,
  };

  const answers: Record<string, Record<string, unknown>> = {};
  const details: Record<string, Record<string, unknown>> = {};
  const parent = async (step: string, detail: Record<string, unknown>) => {
    details[step] = detail;
    log.push(step);
    if (step === "visual-projects-close") {
      dialogOpen = false;
    }
    answers[step] = { step };
    return answers[step]!;
  };

  return { page, parent, log, details, answers };
}

describe("visual-projects page scenario", () => {
  it("runs the parent steps in order with confirmed identity", async () => {
    const { page, parent, log, details } = fakePage();
    const report = await runVisualProjects(page, parent);

    expect(log).toEqual([
      "settle",
      "visual-projects-sidebar",
      "open-dialog",
      "visual-projects-dialog",
      "visual-projects-close",
    ]);
    expect(log.filter((l) => l.startsWith("visual-projects-"))).toEqual([
      ...VISUAL_PROJECTS_STEPS,
    ]);
    for (const step of VISUAL_PROJECTS_STEPS) {
      expect(details[step]).toMatchObject(identity);
    }
    expect(report.settled).toEqual(["Projetos e coleções: click"]);
    expect(report.sidebar).toBeDefined();
    expect(report.dialog).toBeDefined();
    expect(report.dialog_closed).toBe(true);
  });

  it("waits for the agent in the active project before measuring the sidebar", async () => {
    const { page, parent, log, details } = fakePage();
    const read = page.sidebar;
    page.sidebar = () => {
      log.push("sidebar-read");
      return read();
    };
    page.waitForAgent = async () => {
      log.push("wait-agent");
    };

    await runVisualProjects(page, parent);

    expect(log).toEqual([
      "settle",
      "visual-projects-sidebar",
      "wait-agent",
      "sidebar-read",
      "open-dialog",
      "visual-projects-dialog",
      "visual-projects-close",
    ]);
    expect(details["visual-projects-sidebar"]).toMatchObject({ pane_id: identity.pane_id });
  });

  it("reports raw observations and the parent answers only", async () => {
    const { page, parent, answers } = fakePage();
    const report = await runVisualProjects(page, parent);

    expect(report.parent).toEqual(answers);
    expect(report.sidebar).toMatchObject({
      groups: expect.arrayContaining([expect.objectContaining({ name: "Frontend" })]),
      projects: expect.arrayContaining([expect.objectContaining({ name: "herdr-desktop" })]),
    });
    expect(report.dialog).toMatchObject({
      width: 620,
      border_radius: 12,
    });
    expect(JSON.stringify(report)).not.toMatch(/"(ok|pass|passed)":/);
  });

  it("refuses without the Local identity", async () => {
    await expect(
      runVisualProjects(fakePage({ identity: () => null }).page, fakePage().parent)
    ).rejects.toThrow(/confirmed Local identity/);

    await expect(
      runVisualProjects(
        fakePage({ identity: () => ({ ...identity, endpoint: "dev-box · SSH" }) }).page,
        fakePage().parent
      )
    ).rejects.toThrow(/confirmed Local identity/);
  });
});
