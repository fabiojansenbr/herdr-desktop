// @vitest-environment happy-dom
// Spec 072 (PRD i18n) — the two leftovers this area still showed in the engine's own words: the
// Local host's name, which the host now sends as the English `This computer` (071), and the
// progress line of the step that failed, which printed `failure.message` instead of the
// translation `errorText` already had for the code (067).
//
// Would catch: the connections panel rendering `host.label` again for the Local host (in its
// text or in the accessible names that quote it), an SSH host losing its own label, or the
// failed step going back to the raw message of the host.
import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it } from "vitest";
import { errorText, setLocalePreference, t, type Locale } from "../i18n/index.svelte";
import { untranslated } from "../i18n/testing";
import ConnectionsPanel from "./ConnectionsPanel.svelte";
import type { ConnectionsBridge } from "./bridge";
import type { ConnectionsState } from "./controller";
import { hostFixture } from "./fake-bridge";
import { connectionProgress } from "./progress";
import type { ConnectionsView, HostDto, RuntimeError } from "./types";

const mounted: { el: HTMLElement; app: Record<string, unknown> }[] = [];

afterEach(() => {
  for (const host of mounted.splice(0)) {
    unmount(host.app as never);
    host.el.remove();
  }
  setLocalePreference("pt");
});

// --- AC-072-01: the Local host is named by the front ------------------------------------------

/** The Local host exactly as 071 publishes it: the label is the host's English text. */
const LOCAL = hostFixture({
  endpoint: "local",
  label: "This computer",
  kind: "local",
  target: null,
  session: "hd072-local",
  phase: "online",
  phase_label: "Online",
  screen: ["$ herdr"],
  panes: [{ pane_id: "w1:p1", workspace_id: "w1", focused: true, input_enabled: true, input_block: null, target: null }],
});

const SSH = hostFixture({
  endpoint: "0123456789abcdef0123456789abcdef",
  label: "dev-box",
  kind: "ssh",
  target: "user@dev-box",
  session: "hd072-remote",
  phase: "online",
  phase_label: "Online",
  screen: ["$ herdr"],
  panes: [{ pane_id: "w2:p1", workspace_id: "w2", focused: true, input_enabled: true, input_block: null, target: null }],
});

/** A bridge that answers `list` once and never resolves its watch: the panel loads and stays. */
function fixedBridge(hosts: readonly HostDto[]): ConnectionsBridge {
  const view: ConnectionsView = { hub: { revision: 1, hosts: [...hosts] }, profiles: [], store_error: null };
  const same = async () => view;
  return {
    list: same,
    watch: () => new Promise<ConnectionsView>(() => {}),
    saveProfile: same,
    importProfiles: async () => ({ report: { imported: [], skipped: [], already_present: 0 }, view }),
    connect: same,
    cancel: same,
    disconnect: same,
    reconnect: same,
    removeProfile: same,
    sendText: async () => {},
    workspaces: async () => [],
    setConnectOnOpen: same,
  };
}

/** The panel mounted in `language`, after its first `list` landed. */
async function panel(language: Locale, hosts: readonly HostDto[]): Promise<HTMLElement> {
  setLocalePreference(language);
  const el = document.createElement("div");
  document.body.append(el);
  const app = mount(ConnectionsPanel, { target: el, props: { bridge: fixedBridge(hosts) } });
  mounted.push({ el, app });
  await new Promise((resolve) => setTimeout(resolve, 0));
  flushSync();
  return el;
}

const article = (root: HTMLElement, endpoint: string) => root.querySelector<HTMLElement>(`article[data-host="${endpoint}"]`)!;

describe("AC-072-01 host local nomeado pelo front", () => {
  const expected: [Locale, string][] = [
    ["pt", "Este computador"],
    ["en", "This computer"],
    ["es", "Este ordenador"],
  ];

  // Would catch: the card's title, its accessible name, the screen's or the pane input's
  // accessible name still quoting the `This computer` the host sent.
  it.each(expected)("names the Local host in %s and keeps the SSH label", async (language, name) => {
    const root = await panel(language, [LOCAL, SSH]);
    const local = article(root, "local");

    expect(local.querySelector(".host-label")!.textContent).toBe(name);
    expect(local.getAttribute("aria-label")).toBe(`${name}, ${t("phase.online")}`);
    expect(local.querySelector("pre.screen")!.getAttribute("aria-label")).toBe(t("connections.panel.screen", { host: name }));
    expect(local.querySelector(".pane input")!.getAttribute("aria-label")).toBe(
      t("connections.panel.paneInput", { pane: "w1:p1", host: name }),
    );

    // An SSH host is named by the user, so its own label survives in every language.
    const ssh = article(root, SSH.endpoint);
    expect(ssh.querySelector(".host-label")!.textContent).toBe("dev-box");
    expect(ssh.getAttribute("aria-label")).toBe(`dev-box, ${t("phase.online")}`);
    expect(ssh.querySelector("pre.screen")!.getAttribute("aria-label")).toBe(t("connections.panel.screen", { host: "dev-box" }));
  });

  /** Everything a user reads under `root`: the visible texts and the accessible names. */
  function readable(root: Element): string {
    const names = Array.from(root.querySelectorAll("*")).flatMap((element) =>
      ["aria-label", "title", "placeholder"].map((attribute) => element.getAttribute(attribute) ?? ""),
    );
    return [root.textContent ?? "", ...names].join("\n");
  }

  // Would catch: `This computer` (the host's own label) leaking into the Portuguese or Spanish
  // panel, or a language falling back to another language's name for the Local host.
  it("shows only the current language's name for the Local host", async () => {
    for (const [language, name] of expected) {
      const read = readable(await panel(language, [LOCAL, SSH]));
      expect([language, read.includes(name)]).toEqual([language, true]);
      for (const [, other] of expected.filter(([tag]) => tag !== language)) {
        expect([language, other, read.includes(other)]).toEqual([language, other, false]);
      }
    }
  });

  // `untranslated` reads the accents Portuguese and Spanish share, so it can only judge the
  // English render; Spanish is judged by the name comparison above.
  it("leaves no Portuguese in the English panel", async () => {
    expect(untranslated(await panel("en", [LOCAL, SSH]))).toEqual([]);
  });
});

// --- AC-072-03: the failed step speaks the user's language ------------------------------------

function progressState(failure: RuntimeError): ConnectionsState {
  return {
    view: {
      hub: {
        revision: 3,
        hosts: [
          hostFixture({
            endpoint: "abcdef",
            label: "dev-box-rc",
            phase: "online",
            phase_label: "Online",
            latency_ms: 251,
            server_version: "0.9.0",
            generation: 1,
            boot_id: "boot-1",
          }),
        ],
      },
      profiles: [],
      store_error: null,
    },
    loading: false,
    globalError: null,
    hostErrors: { abcdef: failure },
    inputs: {},
    sending: {},
    dialog: {
      open: true,
      draft: { id: null, label: "dev-box-rc", target: "user@dev-box", port: "", session: "trabalho", auth: "key" },
      errors: {},
      submitting: false,
      submitError: null,
      connecting: "abcdef",
    },
    importReport: null,
    workspaces: {},
  } as ConnectionsState;
}

describe("AC-072-03 passo de progresso traduzido", () => {
  // `connection_refused` is the shape of the failure this AC fixes, and it is deliberately a code
  // with no key: `errorText` then shows the host's own message, which is the second half of the
  // criterion. The first half is proven right below with `server_unavailable`, the code this
  // product really sends with that message (`contracts.rs`: `ConnectionRefused` →
  // `("server_unavailable", "connection refused")`), so the translation is actually read.
  it("labels the failed step with errorText, falling back to the message without a key", () => {
    setLocalePreference("pt");
    const failure: RuntimeError = { code: "connection_refused", message: "connection refused", retryable: true };
    const step = connectionProgress(progressState(failure))[3]!;
    expect(step.status).toBe("error");
    expect(step.label).toBe(errorText(failure));
    expect(step.label).toBe("connection refused");
  });

  it("shows the Portuguese text of the code on the step that failed", () => {
    setLocalePreference("pt");
    const failure: RuntimeError = { code: "server_unavailable", message: "connection refused", retryable: true };
    const step = connectionProgress(progressState(failure))[3]!;
    expect(step.status).toBe("error");
    expect(step.label).toBe(errorText(failure));
    // Would catch: the English message of the host reaching the Portuguese dialog again.
    expect(step.label).toBe("a sessão Herdr local não está em execução; use Iniciar sessão");
    expect(step.label).not.toBe(failure.message);
  });

  it("translates the same failure into English and Spanish", () => {
    const failure: RuntimeError = { code: "server_unavailable", message: "connection refused", retryable: true };
    setLocalePreference("en");
    expect(connectionProgress(progressState(failure))[3]!.label).toBe("the local Herdr session is not running; use Start session");
    setLocalePreference("es");
    expect(connectionProgress(progressState(failure))[3]!.label).toBe("la sesión Herdr local no está en ejecución; usa Iniciar sesión");
  });
});
