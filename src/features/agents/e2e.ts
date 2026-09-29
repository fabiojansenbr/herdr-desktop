// Native window scenario of spec 004 (driven by src-tauri/tests/agents.rs through
// scripts/feature-harness/window.rs). Acts on AgentPanel like a user — choosing, typing,
// clicking and pressing keys on the pane surface — over the real IPC bridge and backend, and
// reports what the window shows. The agent is the deterministic fake installed by the test.

import { byText, choose, press, type, waitFor } from "../../harness/dom";
import type { Scenario } from "../../harness/native";

interface KeySpec {
  key: string;
  ctrlKey?: boolean;
  altKey?: boolean;
  shiftKey?: boolean;
}

function panel(root: ParentNode): HTMLElement | null {
  return root.querySelector<HTMLElement>(".agents");
}

function surface(root: ParentNode): HTMLElement {
  const el = root.querySelector<HTMLElement>(".surface");
  if (!el) throw new Error("pane surface not rendered");
  return el;
}

function panes(root: ParentNode) {
  return Array.from(root.querySelectorAll<HTMLElement>(".surface [data-pane]")).map((el) => ({
    pane: el.dataset.pane ?? "",
    cells: (el.dataset.cells ?? "").split(",").map(Number),
    focused: el.dataset.focused === "true",
    pending: el.dataset.pending === "true",
    style: el.getAttribute("style") ?? "",
  }));
}

function agentRow(root: ParentNode, pane: string): HTMLElement | null {
  return root.querySelector<HTMLElement>(`li[data-agent-pane="${pane}"]`);
}

function badge(row: HTMLElement | null) {
  const el = row?.querySelector<HTMLElement>(".status");
  return el ? { status: el.dataset.status ?? "", text: el.textContent?.replace(/\s+/g, " ").trim() ?? "" } : null;
}

function button(scope: ParentNode, text: string): HTMLButtonElement {
  const b = byText<HTMLButtonElement>(scope, "button", text);
  if (!b) throw new Error(`button "${text}" not found`);
  return b;
}

async function enabled(scope: ParentNode, text: string, timeoutMs = 20000): Promise<HTMLButtonElement> {
  return waitFor(`enabled "${text}"`, () => {
    const b = byText<HTMLButtonElement>(scope, "button", text);
    return b && !b.disabled ? b : null;
  }, timeoutMs);
}

function pressKey(target: HTMLElement, spec: KeySpec): boolean {
  const event = new KeyboardEvent("keydown", {
    key: spec.key,
    ctrlKey: spec.ctrlKey ?? false,
    altKey: spec.altKey ?? false,
    shiftKey: spec.shiftKey ?? false,
    bubbles: true,
    cancelable: true,
  });
  target.dispatchEvent(event);
  return event.defaultPrevented;
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

function splitRatio(root: ParentNode): number | null {
  const raw = surface(root).dataset.splitRatio;
  return raw ? Number(raw) : null;
}

export const run: Scenario = async ({ phase, params, root, progress }) => {
  if (phase !== "flow") throw new Error(`unknown phase ${phase}`);
  const firstPane = String(params.pane_id);
  const kind = String(params.kind);
  const agentName = String(params.agent_name);
  const promptText = String(params.prompt);
  const echoMarker = String(params.echo_marker);

  // Every status the agent row showed, with the exact text + icon rendered for it.
  const seen: { status: string; text: string; at: number }[] = [];
  const sampler = setInterval(() => {
    const b = badge(agentRow(root, firstPane));
    if (b && seen[seen.length - 1]?.status !== b.status) seen.push({ ...b, at: Date.now() });
  }, 25);

  try {
    await waitFor("panel connected with a topology", () => panel(root)?.dataset.phase === "connected" && panes(root).length > 0, 30000);
    const startForm = root.querySelector<HTMLFormElement>('form[aria-label="Iniciar agente"]')!;
    const [paneSelect, kindSelect] = Array.from(startForm.querySelectorAll<HTMLSelectElement>("select"));
    const loaded = {
      identity: root.querySelector<HTMLElement>(".identity")?.textContent?.trim() ?? "",
      boot: root.querySelector<HTMLElement>(".identity")?.dataset.boot ?? "",
      onboarding: root.querySelector(".onboarding")?.textContent?.trim() ?? null,
      kinds: Array.from(kindSelect!.options).map((o) => o.value),
      panes: panes(root),
      start_enabled_without_name: !button(startForm, "Iniciar agente").disabled,
    };
    await progress({ step: "loaded", loaded });

    // --- AC-004-01: start the fake agent on the ready pane -----------------------------------
    choose(paneSelect!, firstPane);
    choose(kindSelect!, kind);
    type(startForm.querySelector<HTMLInputElement>("input")!, agentName);
    press(await enabled(startForm, "Iniciar agente"));
    const row = await waitFor("agent row", () => agentRow(root, firstPane), 20000);
    await waitFor("agent ready", () => {
      const error = startForm.querySelector(".error")?.textContent;
      if (error) throw new Error(`start failed in the window: ${error}`);
      return agentRow(root, firstPane)?.querySelector(".readiness")?.textContent?.trim() === "pronto";
    }, 40000);
    const started = { name: row.querySelector(".name")?.textContent?.trim(), kind: row.querySelector(".kind")?.textContent?.trim(), badge: badge(agentRow(root, firstPane)) };
    await progress({ step: "started", loaded, started });

    // --- AC-004-01/02: explicit prompt; the agent works, then blocks for approval -------------
    const textarea = agentRow(root, firstPane)!.querySelector<HTMLTextAreaElement>("textarea")!;
    textarea.value = promptText;
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    press(await enabled(agentRow(root, firstPane)!, "Enviar prompt"));
    const outcome = await waitFor("prompt outcome", () => {
      const current = agentRow(root, firstPane);
      const error = current?.querySelector(".error")?.textContent;
      if (error) throw new Error(`prompt failed in the window: ${error}`);
      return current?.querySelector<HTMLElement>(".outcome")?.dataset.outcome || null;
    }, 20000);
    await waitFor("blocked badge", () => badge(agentRow(root, firstPane))?.status === "blocked", 30000);
    const blockedAt = Date.now();
    const attention = await waitFor("attention item", () => root.querySelector<HTMLElement>(`[data-attention="${firstPane}"]`));
    // The GUI must not answer: stay blocked while the user does nothing.
    await sleep(2000);
    const stillBlocked = badge(agentRow(root, firstPane))?.status === "blocked";
    const promptButtonWhileBlocked = byText<HTMLButtonElement>(agentRow(root, firstPane)!, "button", "Enviar prompt")?.disabled ?? null;
    await progress({ step: "blocked", loaded, started, outcome, seen });

    // --- AC-004-03: split, ratio, focus and input to the confirmed pane -----------------------
    press(await enabled(root, "Dividir à direita"));
    await waitFor("two panes", () => panes(root).length === 2, 20000);
    const afterSplit = { panes: panes(root), ratio: splitRatio(root) };
    const secondPane = afterSplit.panes.find((p) => p.pane !== firstPane)!.pane;

    const ratioForm = root.querySelector<HTMLFormElement>('form[aria-label="Razão do split"]')!;
    type(ratioForm.querySelector<HTMLInputElement>("input")!, "0.3");
    press(await enabled(ratioForm, "Aplicar razão"));
    await waitFor("ratio 0.3 from the server", () => Math.abs((splitRatio(root) ?? 0) - 0.3) < 0.02, 20000);
    const afterRatio = { panes: panes(root), ratio: splitRatio(root) };

    const secondBox = root.querySelector<HTMLElement>(`.surface [data-pane="${secondPane}"]`)!;
    secondBox.click();
    const pendingSeen = panes(root).find((p) => p.pane === secondPane)?.pending ?? false;
    await waitFor("second pane confirmed", () => panes(root).find((p) => p.pane === secondPane)?.focused, 20000);
    const typedKeys = [...`echo ${echoMarker}`].map((ch) => ({ key: ch })).concat([{ key: "Enter" }]);
    const echoPrevented = typedKeys.map((k) => pressKey(surface(root), k));
    await sleep(800);
    const afterFocus = { panes: panes(root), second_pane: secondPane, echo_prevented: echoPrevented.every(Boolean), pending_seen: pendingSeen };
    await progress({ step: "layout", afterSplit, afterRatio, afterFocus });

    // --- AC-004-02: the user opens the blocked agent's pane -----------------------------------
    press(button(attention, "Abrir pane"));
    await waitFor("agent pane confirmed", () => panes(root).find((p) => p.pane === firstPane)?.focused, 20000);

    // --- AC-004-03: GUI shortcut (no leak) then terminal shortcut and the user's answer -------
    const keysStartedAt = Date.now();
    const ratioBefore = splitRatio(root);
    const guiPrevented = pressKey(surface(root), { key: "ArrowLeft", altKey: true, shiftKey: true });
    await waitFor("ratio changed by the GUI shortcut", () => {
      const r = splitRatio(root);
      return r !== null && ratioBefore !== null && Math.abs(r - ratioBefore) > 0.05;
    }, 20000);
    const ratioAfterShortcut = splitRatio(root);
    await sleep(500);
    const ctrlEPrevented = pressKey(surface(root), { key: "e", ctrlKey: true });
    await sleep(500);
    const yPrevented = pressKey(surface(root), { key: "y" });
    const final = await waitFor("agent idle or done after the answer", () => {
      const b = badge(agentRow(root, firstPane));
      return b && (b.status === "idle" || b.status === "done") ? b : null;
    }, 30000);
    await sleep(300);

    return {
      loaded,
      started,
      outcome,
      blocked: {
        at: blockedAt,
        still_blocked_after_wait: stillBlocked,
        prompt_disabled: promptButtonWhileBlocked,
        attention_text: attention.textContent?.replace(/\s+/g, " ").trim(),
      },
      layout: { afterSplit, afterRatio, afterFocus },
      shortcuts: {
        keys_started_at: keysStartedAt,
        gui_prevented: guiPrevented,
        ratio_before: ratioBefore,
        ratio_after: ratioAfterShortcut,
        ctrl_e_prevented: ctrlEPrevented,
        y_prevented: yPrevented,
      },
      final,
      seen,
      notice: root.querySelector(".notice")?.textContent?.trim() ?? null,
      panes_final: panes(root),
    };
  } finally {
    clearInterval(sampler);
  }
};
