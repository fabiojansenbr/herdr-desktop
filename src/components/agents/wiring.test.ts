// Spec 014 — static source probes of the agents panel. They guard wiring a pure unit test
// cannot see (which component the empty panel opens, which action a card calls); the real proof
// is the `visual-agents` phase of the native flow, which this file does not replace.
import { describe, expect, it } from "vitest";

const sources = import.meta.glob(["./*.svelte", "../AgentPanel.svelte"], {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

const panel = sources["./AgentPanel.svelte"]!;
const region = sources["./AgentsRegion.svelte"]!;
const legacy = sources["../AgentPanel.svelte"]!;
const markup = (source: string) => source.slice(source.indexOf("</script>"));

describe("AC-014-03 the empty panel opens the reusable form", () => {
  // Would catch: an empty panel without the button, or a second form built inside the panel
  // instead of the component the workspace header (013) and the legacy panel use.
  it("renders the Novo agente button and NewAgentForm, the one the other panels import", () => {
    expect(markup(panel)).toMatch(/data-agents-empty/);
    expect(markup(panel)).toMatch(/data-new-agent\b/);
    expect(markup(panel)).toMatch(/<NewAgentForm\b/);
    expect(panel).toMatch(/import NewAgentForm from "\.\/NewAgentForm\.svelte"/);
    expect(legacy).toMatch(/import NewAgentForm from "\.\/agents\/NewAgentForm\.svelte"/);
    expect(markup(legacy)).toMatch(/<NewAgentForm\b/);
    // The extracted form is the only place the panels build the start fields.
    for (const [file, source] of Object.entries(sources)) {
      if (file.endsWith("NewAgentForm.svelte")) continue;
      expect([file, /aria-label=\{t\("agents\.form\.label"\)\}/.test(source)]).toEqual([file, false]);
    }
    expect(sources["./NewAgentForm.svelte"]!).toMatch(/aria-label=\{t\("agents\.form\.label"\)\}/);
  });

  // Would catch: the region ignoring the designed panel (spec 010 left the slot empty).
  it("the agents region renders the designed panel from the frame context", () => {
    expect(markup(region)).toMatch(/<AgentPanel\s+\{ctx\}\s*\/>/);
    expect(region).toMatch(/import AgentPanel from "\.\/AgentPanel\.svelte"/);
    expect(markup(region)).not.toMatch(/Controles da sessão/);
    expect(markup(region)).not.toMatch(/Enviar prompt/);
    expect(markup(region)).not.toMatch(/Tabs e panes/);
  });
});

describe("AC-014-02 the panel only takes the user to the pane", () => {
  // Would catch: a card answering the agent (prompt, input, keys) instead of focusing its pane.
  it("calls openAttention and never prompts, types or sends keys", () => {
    for (const file of ["./AgentPanel.svelte", "./AttentionCard.svelte", "./RunningRow.svelte", "./Counters.svelte"]) {
      const source = sources[file]!;
      for (const forbidden of ["sendPrompt", "pane_input", "sendKeys", "send_keys", "editPrompt", "controller.input"]) {
        expect([file, source.includes(forbidden)]).toEqual([file, false]);
      }
    }
    expect(panel).toMatch(/controllers\.agents\.openAttention\(/);
  });

  // Would catch: an approval control (Permitir/Negar) drawn by the desktop, which belongs to the
  // agent's own terminal UI (out of scope of this spec).
  it("draws no approval control", () => {
    for (const [file, source] of Object.entries(sources)) {
      for (const forbidden of ["Permitir", "Negar", "Sempre nesta sessão"]) {
        expect([file, source.includes(forbidden)]).toEqual([file, false]);
      }
    }
  });
});
