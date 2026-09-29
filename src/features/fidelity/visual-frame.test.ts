// Spec 010 — page side of the native visual-frame phase with a fake page and parent. The page only
// observes (rects, computed styles, palette DOM, trusted keys); PTY bytes, engine data and output
// commands come from the parent, and the parent test computes every check.
import { describe, expect, it } from "vitest";
import { runVisualFrame, VISUAL_STEPS, type KeySeen, type VisualFramePage } from "./visual-frame";

const identity = { pane_id: "w3:p1", generation: "5", boot_prefix: "2040265-", endpoint: "Este computador · Local" };

function fakePage(over: Partial<VisualFramePage> = {}) {
  const log: string[] = [];
  let paletteOpen = false;
  let viewport = { inner_width: 1280, inner_height: 673, dpr: 1 };
  const seen = (target: string): KeySeen => ({ trusted: true, target, palette_at_first_frame: paletteOpen, frames_waited: 1 });
  const page: VisualFramePage = {
    identity: () => identity,
    viewport: () => viewport,
    waitViewport: async (w, h) => {
      log.push(`wait ${w}x${h}`);
      viewport = { inner_width: w, inner_height: h, dpr: 1 };
      return true;
    },
    settle: async () => (log.push("settle"), []),
    frame: () => ({ regions: { topbar: { left: 0, top: 0, width: 1440, height: 44 } } }),
    menus: async () => [{ label: "Arquivo", items: [] }],
    palette: () => (paletteOpen ? { sections: [{ title: "Comandos", entries: [] }], input_focused: true } : null),
    focusOutside: () => (log.push("focus outside"), "host-selector"),
    focusTerminal: () => (log.push("focus terminal"), true),
    activeId: () => (paletteOpen ? "palette-input" : "host-selector"),
    recordKeys: () => ({ take: () => [seen("host-selector")] }),
    ...over,
  };
  const answers: Record<string, Record<string, unknown>> = {};
  const details: Record<string, Record<string, unknown>> = {};
  const parent = async (step: string, detail: Record<string, unknown>) => {
    details[step] = detail;
    log.push(step);
    if (step === "visual-ctrlk-outside") paletteOpen = true;
    if (step === "visual-escape") paletteOpen = false;
    answers[step] = { step, pty_hex: step === "visual-ctrlk-terminal" ? "0b" : "" };
    return answers[step]!;
  };
  return { page, parent, log, details, answers };
}

describe("visual-frame page scenario", () => {
  // Would catch: steps out of order (Escape before the palette opened, terminal press before the
  // capture), a missing identity in a step, or measuring the frame before the 1440×900 viewport.
  it("runs the parent steps in order with the confirmed identity and measures at 1440×900", async () => {
    const { page, parent, log, details } = fakePage();
    const report = await runVisualFrame(page, parent);
    expect(log).toEqual(["visual-viewport", "wait 1440x900", "settle", "visual-engine", "focus outside", "visual-ctrlk-outside", "visual-escape", "focus terminal", "visual-ctrlk-terminal", "visual-restore"]);
    expect(log.filter((l) => l.startsWith("visual-"))).toEqual([...VISUAL_STEPS]);
    for (const step of VISUAL_STEPS) expect(details[step]).toMatchObject(identity);
    expect(details["visual-viewport"]).toMatchObject({ inner_width: 1280, inner_height: 673, target_width: 1440, target_height: 900 });
    expect(report.viewport).toEqual({ inner_width: 1440, inner_height: 900, dpr: 1 });
    expect(report.settled).toEqual([]);
  });

  // Would catch: the page writing verdict booleans, or losing the raw palette/focus observations.
  it("reports raw observations and the parent answers only", async () => {
    const { page, parent, answers } = fakePage();
    const report = await runVisualFrame(page, parent);
    expect(report.parent).toEqual(answers);
    expect(report.ctrlk_outside).toMatchObject({ opener: "host-selector", palette: { sections: [{ title: "Comandos" }] } });
    expect(report.escape).toEqual({ palette: null, focus_after: "host-selector" });
    expect(report.ctrlk_terminal).toMatchObject({ focused: true, palette: null });
    expect(JSON.stringify(report)).not.toMatch(/"(ok|pass|passed)":/);
  });

  // Would catch: a phase measured on the SSH host or with no confirmed pane, or a viewport that
  // never reached 1440×900 reported as if it had.
  it("refuses without the Local identity or the target viewport", async () => {
    await expect(runVisualFrame(fakePage({ identity: () => null }).page, fakePage().parent)).rejects.toThrow(/confirmed Local identity/);
    await expect(runVisualFrame(fakePage({ identity: () => ({ ...identity, endpoint: "dev-box · SSH" }) }).page, fakePage().parent)).rejects.toThrow(/confirmed Local identity/);
    const stuck = fakePage({ waitViewport: async () => false });
    await expect(runVisualFrame(stuck.page, stuck.parent)).rejects.toThrow(/1440×900/);
    expect(stuck.log).toEqual(["visual-viewport"]);
  });
});
