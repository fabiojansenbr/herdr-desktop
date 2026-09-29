// Pure seams of the SSH phases (no DOM, engine or GUI): step names equal the Rust parent, params
// gaps throw instead of producing observations, and UI text parsers reject near-misses.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { currentSideLines, hostKind, ORIGINAL_TAB, paneRow, parseCells, parseIdentity, parseProjectMeta, relativeToRoot, runSshPhase, SSH_PHASES, SSH_STEPS, sshFlowParams, tabAttributes, uniqueFocus } from "./ssh-flow";
import { PHASES } from "./e2e";

const rust = readFileSync(new URL("../../../tests/fidelity-native/ssh_flow.rs", import.meta.url), "utf8");
const quoted = (block: string) => Array.from(block.matchAll(/"([a-z0-9-]+)"/g), (m) => m[1]);

const good = {
  ssh_profile: { label: "Host SSH (Referencia)", target: "u@127.0.0.1", port: 2222, session: "hd007-remote-1-a-ssh" },
  legacy_profile: { label: "Legado", target: "u@127.0.0.1", port: 2223, session: "hd007-remote-1-a-leg" },
  local: { label: "L", session: "hd007-remote-1-a-loc", root: "/l" },
  ssh: { label: "S", session: "hd007-remote-1-a-ssh", root: "/s" },
  legacy: { label: "G", session: "hd007-remote-1-a-leg", root: "/g" },
  shared_pane: "w1:p1", agent_kind: "pi", agent_name: "hd007-agent-x", prompt_nonce: "hd007-prompt-x",
  edit_nonce: "hd007-remote-edit-x", dirty_nonce: "hd007-dirty-x", dirty_file: "notas.txt",
};

describe("ssh flow seams", () => {
  it("steps equal ssh_flow::STEPS and phases keep the e2e order", () => {
    const block = /pub const STEPS: \[&str; \d+\] = \[([\s\S]*?)\];/.exec(rust)?.[1] ?? "";
    expect(quoted(block)).toEqual([...SSH_STEPS]);
    expect(PHASES.filter((p) => (SSH_PHASES as readonly string[]).includes(p))).toEqual([...SSH_PHASES]);
  });

  it("missing params or unknown phase throw before any parent step (no fabricated report)", async () => {
    const calls: string[] = [];
    const parent = async (step: string) => (calls.push(step), {});
    const root = {} as HTMLElement;
    await expect(runSshPhase("native-keys", root, { ssh_flow: good }, parent)).rejects.toThrow(/not an SSH phase/);
    await expect(runSshPhase("hosts-identity", root, {}, parent)).rejects.toThrow(/params.ssh_flow missing/);
    await expect(runSshPhase("ssh-agent-actions", root, { ssh_flow: good }, parent)).rejects.toThrow(/did not establish the SSH host/);
    expect(() => sshFlowParams({ ssh_flow: { ...good, prompt_nonce: "" } })).toThrow(/prompt_nonce/);
    expect(() => sshFlowParams({ ssh_flow: { ...good, ssh_profile: { ...good.ssh_profile, port: "22" } } })).toThrow(/port/);
    expect(calls).toEqual([]);
  });

  it("parsers accept the product formats and reject near misses", () => {
    expect(parseIdentity("pane w1:p1 · geração 3 · boot 55e4aabb")).toEqual({ pane_id: "w1:p1", generation: "3", boot_prefix: "55e4aabb" });
    expect(parseIdentity("sem identidade confirmada")).toBeNull();
    expect(parseCells("0,0,40,24")).toEqual([0, 0, 40, 24]);
    expect(parseCells("0,0,40")).toBeNull();
    expect(parseCells("0,-1,40,24")).toBeNull();
    expect(parseProjectMeta("sessão hd007-remote-1-a-ssh · /w/fixtures/ssh-project")).toEqual({ session: "hd007-remote-1-a-ssh", root: "/w/fixtures/ssh-project" });
    expect(parseProjectMeta("/w/fixtures/ssh-project")).toBeNull();
  });
});

describe("status bar host kind", () => {
  // Would catch: comparing the whole item with "SSH"/"Local" (the App renders `LABEL · KIND`), which
  // left host_kind empty in run1, or a label merely containing the kind word.
  it("reads the kind suffix of the selection item only", () => {
    expect(hostKind("Host SSH (Referencia) · SSH")).toBe("SSH");
    expect(hostKind("Este computador · Local")).toBe("Local");
    expect(hostKind("SSH")).toBe("");
    expect(hostKind("Local")).toBe("");
    expect(hostKind("Host Local de testes · SSH")).toBe("SSH");
    expect(hostKind("sessão hd007-remote-1-abc123-loc")).toBe("");
    expect(hostKind(undefined)).toBe("");
  });
});

// r2 driver faults confirmed by the root review of run2 (evidencias/007/native-ssh-live/root-review).
// No DOM environment in vitest: pure seams are tested directly; wiring the native window needs is
// probed on the sources, the native run remains the proof.
const sources = import.meta.glob(["../../components/AgentPanel.svelte", "../../components/RemoteFilesWorkspace.svelte", "./ssh-flow.ts", "./e2e.ts"], {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;
const src = (suffix: string) => Object.entries(sources).find(([k]) => k.endsWith(suffix))![1];
const fnBody = (source: string, name: string) => {
  const start = source.indexOf(`async function ${name}(`);
  const next = source.indexOf("\nasync function ", start + 1);
  const exported = source.indexOf("\nexport ", start + 1);
  const ends = [next, exported].filter((i) => i > start);
  return source.slice(start, ends.length ? Math.min(...ends) : undefined);
};

describe("r2 driver seams", () => {
  // Would catch: the page reporting data-current-line (a line number) or removed base lines as the
  // current remote content of the diff.
  it("current side of a diff keeps text of unchanged/added rows only", () => {
    const rows = [
      { op: "unchanged", text: "linha 1: base comum" },
      { op: "removed", text: "linha 2: apenas versao local" },
      { op: "added", text: "linha 2: MODIFICADA no host SSH remoto" },
    ];
    expect(currentSideLines(rows)).toEqual(["linha 1: base comum", "linha 2: MODIFICADA no host SSH remoto"]);
    expect(currentSideLines([])).toEqual([]);
  });

  // Would catch: accepting a list with no or two focused panes as the confirmed focus, or reading a
  // pane without its confirmed geometry.
  it("unique confirmed focus and composed pane rows", () => {
    expect(uniqueFocus([{ pane_id: "w1:p1", focused: true }, { pane_id: "w1:p2", focused: false }])).toBe("w1:p1");
    expect(uniqueFocus([{ pane_id: "w1:p1", focused: true }, { pane_id: "w1:p2", focused: true }])).toBeNull();
    expect(uniqueFocus([{ pane_id: "w1:p1", focused: false }])).toBeNull();
    expect(paneRow({ pane: "w1:p2", cells: "38,0,37,29", pressed: "true" })).toEqual({ pane_id: "w1:p2", cells: [38, 0, 37, 29], focused: true });
    expect(paneRow({ pane: "w1:p2", cells: undefined, pressed: "false" })).toEqual({ pane_id: "w1:p2", cells: null, focused: false });
    expect(ORIGINAL_TAB).toBe("w1:t1");
  });

  // Would catch: composed AgentPanel hiding confirmed position/focus and the error code, and the
  // controlled remote explorer hiding which endpoint it shows (run2 observed none of them).
  it("composed panels expose confirmed geometry, focus, error code and remote endpoint", () => {
    const panel = src("AgentPanel.svelte");
    const list = panel.slice(panel.indexOf('aria-label={t("terminal.panel.confirmedPanes")}'));
    expect(list).toMatch(/data-cells="\{pane\.cells\.x\},\{pane\.cells\.y\},\{pane\.cells\.width\},\{pane\.cells\.height\}"/);
    expect(panel).toMatch(/role="alert" data-error-code=\{panel\.connectionError\?\.code/);
    expect(panel).toMatch(/role="status" data-error-code=\{panel\.streamError\?\.code/);
    expect(src("RemoteFilesWorkspace.svelte")).toMatch(/data-remote-header data-endpoint=\{view\.host\.endpoint\}/);
  });

  // Would catch the run2 driver faults: standalone-only selectors, a submit on a form that does not
  // exist, the legacy project opened through the absent API, the host label read with HostsPanel
  // unmounted, and native input starting without returning to Local.
  it("driver uses the composed controls and returns to Local before native input", () => {
    const flow = src("ssh-flow.ts");
    for (const stale of ["Superfície de panes", "button[data-remote-host]", "requestSubmit", "dataset.currentLine"]) expect([stale, flow.includes(stale)]).toEqual([stale, false]);
    expect(fnBody(flow, "legacyServer")).not.toMatch(/openProject\(/);
    expect(fnBody(flow, "legacyServer")).toMatch(/useHost\(/);
    const hosts = fnBody(flow, "hostsIdentity");
    expect(hosts.indexOf('activity(root, "Conexões remotas")')).toBeGreaterThan(-1);
    expect(hosts.indexOf('activity(root, "Conexões remotas")')).toBeLessThan(hosts.indexOf("li.host.selected"));
    expect(hosts).toMatch(/selectOriginalTab\(/);
    const e2e = src("e2e.ts");
    expect(e2e).toMatch(/case "native-keys":\s*case "native-ime":\s*await returnToLocal\(root\);\s*return nativeInput\(root, phase\);/);
  });

  // Would catch: reporting the absolute shown path (evaluator expects the path under the shown
  // root), stripping any prefix that merely starts like the root (sibling `ssh-project2`), or
  // accepting an escape / empty segment as a file under the root.
  it("reports a shown file path relative to the actual shown root with an exact boundary", () => {
    const root = "/tmp/hd7Sab/ssh-project";
    expect(relativeToRoot("/tmp/hd7Sab/ssh-project/notas.txt", root)).toBe("notas.txt");
    expect(relativeToRoot("/tmp/hd7Sab/ssh-project/sub/diff-target.txt", `${root}/`)).toBe("sub/diff-target.txt");
    expect(relativeToRoot("/notas.txt", "/")).toBe("notas.txt");
    for (const wrong of ["/tmp/hd7Sab/ssh-project2/notas.txt", "/tmp/hd7Sab/local-project/notas.txt", "tmp/hd7Sab/ssh-project/notas.txt", "/tmp/hd7Sab/ssh-project", "/tmp/hd7Sab/ssh-project/", "/tmp/hd7Sab/ssh-project/../local-project/notas.txt", "/tmp/hd7Sab/ssh-project//notas.txt", "/tmp/hd7Sab/ssh-project/./notas.txt"]) {
      expect([wrong, relativeToRoot(wrong, root)]).toEqual([wrong, null]);
    }
    expect(relativeToRoot("/tmp/hd7Sab/ssh-project/notas.txt", "")).toBeNull();
  });

  // Would catch: a tab timeout that loses which tab was pressed/disabled (r2 retained only the
  // innerText "1", where aria-pressed is invisible).
  it("records every tab button's id, pressed and disabled state", () => {
    const tabs = [
      { dataset: { tab: "w1:t1" }, getAttribute: (n: string) => (n === "aria-pressed" ? "false" : null), disabled: true, textContent: " 1 " },
      { dataset: { tab: "w2:t1" }, getAttribute: (n: string) => (n === "aria-pressed" ? "true" : null), disabled: false, textContent: "1" },
    ];
    expect(tabAttributes(tabs)).toEqual([
      { tab: "w1:t1", pressed: "false", disabled: true, label: "1" },
      { tab: "w2:t1", pressed: "true", disabled: false, label: "1" },
    ]);
    expect(fnBody(src("ssh-flow.ts"), "selectOriginalTab")).toMatch(/tabAttributes\(/);
  });
});
