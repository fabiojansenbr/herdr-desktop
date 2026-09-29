// Spec 011 (AC-011-03) — progress lines of the connection dialog: pending before the dialog asks
// to connect, then every step from the real connection only (measured latency, negotiated server
// version, workspace list of that host); a failure marks the step that did not arrive.
import { describe, expect, it } from "vitest";
import { connectionProgress, sshUser } from "./progress";
import type { ConnectionsState, DialogState } from "./controller";
import type { HostDto } from "./types";

function dialog(over: Partial<DialogState> = {}): DialogState {
  return {
    open: true,
    draft: { id: null, label: "dev-box-rc", target: "user@dev-box", port: "", session: "trabalho", auth: "key" },
    errors: {},
    submitting: false,
    submitError: null,
    connecting: null,
    ...over,
  };
}

function host(over: Partial<HostDto> = {}): HostDto {
  return {
    endpoint: "abcdef",
    label: "dev-box-rc",
    kind: "ssh",
    session: "trabalho",
    target: "user@dev-box",
    visible: true,
    phase: "offline",
    phase_label: "Offline",
    attempt: 0,
    retry_in_ms: null,
    cancelled: false,
    attention: null,
    guidance: null,
    connection_error: null,
    action_error: null,
    generation: null,
    boot_id: null,
    cached: false,
    surface: null,
    screen: [],
    panes: [],
    actions: [],
    ...over,
  };
}

function state(over: Partial<ConnectionsState> = {}): ConnectionsState {
  return {
    view: { hub: { revision: 1, hosts: [host()] }, profiles: [], store_error: null },
    loading: false,
    globalError: null,
    hostErrors: {},
    inputs: {},
    sending: {},
    dialog: dialog(),
    importReport: null,
    workspaces: {},
    ...over,
  };
}

const statuses = (s: ConnectionsState) => connectionProgress(s).map((step) => step.status);

describe("connection dialog progress (AC-011-03)", () => {
  it("keeps the four steps pending before the dialog asks to connect", () => {
    const before = state({ view: { hub: { revision: 1, hosts: [host({ phase: "online", latency_ms: 999, server_version: "9.9.9", generation: 4 })] }, profiles: [], store_error: null } });
    expect(statuses(before)).toEqual(["pending", "pending", "pending", "pending"]);
    const steps = connectionProgress(before);
    expect(steps.map((s) => s.step)).toEqual([1, 2, 3, 4]);
    expect(steps[0]!.label).toBe("Host alcançável");
    expect(steps[3]!.label).toContain("Lendo workspaces");
  });

  it("advances each step with the measured latency, the negotiated version and the workspace list", () => {
    const connecting = state({
      dialog: dialog({ connecting: "abcdef" }),
      view: { hub: { revision: 2, hosts: [host({ phase: "connecting", phase_label: "Conectando" })] }, profiles: [], store_error: null },
    });
    expect(statuses(connecting)).toEqual(["pending", "pending", "pending", "pending"]);

    const online = state({
      dialog: dialog({ connecting: "abcdef" }),
      view: {
        hub: {
          revision: 3,
          hosts: [host({ phase: "online", phase_label: "Online", latency_ms: 251, server_version: "0.9.0-rc.7", generation: 3, boot_id: "boot-9" })],
        },
        profiles: [],
        store_error: null,
      },
    });
    const handshake = connectionProgress(online);
    expect(handshake.map((s) => s.status)).toEqual(["ok", "ok", "ok", "pending"]);
    expect(handshake[0]!.label).toBe("Host alcançável · 251 ms");
    expect(handshake[0]!.meta).toBe("251 ms");
    expect(handshake[1]!.label).toBe("Autenticado como user");
    expect(handshake[2]!.label).toBe("herdr 0.9.0-rc.7 encontrado · endpoint geração 1");
    expect(handshake[2]!.meta).toBe("compatível");

    const read = state({
      dialog: dialog({ connecting: "abcdef" }),
      view: {
        hub: {
          revision: 4,
          hosts: [host({ phase: "online", phase_label: "Online", latency_ms: 251, server_version: "0.9.0-rc.7", generation: 3, boot_id: "boot-9" })],
        },
        profiles: [],
        store_error: null,
      },
      workspaces: { abcdef: [{ workspace_id: "w7", label: "trabalho" }] },
    });
    expect(statuses(read)).toEqual(["ok", "ok", "ok", "ok"]);
  });

  it("marks the first missing step in error when the connection fails", () => {
    const refused = state({
      dialog: dialog({ connecting: "abcdef" }),
      view: {
        hub: {
          revision: 5,
          hosts: [
            host({
              phase: "attention",
              phase_label: "Precisa de atenção",
              attention: "authentication_required",
              connection_error: { code: "ssh_authentication_required", message: "chave recusada", retryable: false, endpoint: "abcdef" },
            }),
          ],
        },
        profiles: [],
        store_error: null,
      },
    });
    expect(statuses(refused)).toEqual(["error", "blocked", "blocked", "blocked"]);

    const readFailed = state({
      dialog: dialog({ connecting: "abcdef" }),
      view: {
        hub: {
          revision: 6,
          hosts: [host({ phase: "online", phase_label: "Online", latency_ms: 503, server_version: "0.9.0", generation: 2, boot_id: "boot-8" })],
        },
        profiles: [],
        store_error: null,
      },
      hostErrors: { abcdef: { code: "api_missing", message: "servidor sem API JSON", retryable: false, endpoint: "abcdef" } },
    });
    expect(statuses(readFailed)).toEqual(["ok", "ok", "ok", "error"]);
  });

  // Spec 029 (AC-029-02): a missing remote Herdr marks the binary step in error with the reason,
  // never as connected/found.
  it("marks the herdr step error with the reason when the remote binary is missing", () => {
    const missing = state({
      dialog: dialog({ connecting: "abcdef" }),
      view: {
        hub: {
          revision: 7,
          hosts: [
            host({
              phase: "attention",
              phase_label: "Precisa de atenção",
              attention: "herdr_missing",
              connection_error: { code: "remote_herdr_missing", message: "o Herdr não foi encontrado no host remoto", retryable: false, endpoint: "abcdef" },
            }),
          ],
        },
        profiles: [],
        store_error: null,
      },
    });
    expect(statuses(missing)).toEqual(["ok", "ok", "error", "blocked"]);
    const step = connectionProgress(missing)[2]!;
    expect(step.label).toBe("o Herdr não foi encontrado no host remoto");
    expect(step.status).not.toBe("ok");
    // The binary step never claims the binary was found for this failure.
    expect(connectionProgress(missing)[2]!.label).not.toMatch(/herdr .*encontrado/);

    const outdated = state({
      dialog: dialog({ connecting: "abcdef" }),
      view: {
        hub: {
          revision: 8,
          hosts: [
            host({
              phase: "attention",
              phase_label: "Precisa de atenção",
              attention: "herdr_outdated",
              connection_error: { code: "remote_herdr_outdated", message: "o Herdr do host está desatualizado (versão 0.8.0)", retryable: false, endpoint: "abcdef" },
            }),
          ],
        },
        profiles: [],
        store_error: null,
      },
    });
    // Spec 072 (AC-072-03): the step says the text of the code (`error.remote_herdr_outdated`),
    // not the host's English message — which is why the measured version left this line.
    expect(connectionProgress(outdated)[2]!.label).toBe("o Herdr do host está desatualizado e não atende o endpoint geração 1");
  });

  it("marks a workspace timeout on step four and blocks no-longer-running steps", () => {
    const timedOut = state({
      dialog: dialog({ connecting: "abcdef" }),
      view: {
        hub: {
          revision: 9,
          hosts: [host({ phase: "online", phase_label: "Online", latency_ms: 15000, server_version: "0.9.0", generation: 1 })],
        },
        profiles: [],
        store_error: null,
      },
      hostErrors: { abcdef: { code: "timeout", message: "sem resposta ao ler workspaces", retryable: true, endpoint: "abcdef" } },
    });
    const steps = connectionProgress(timedOut);
    expect(statuses(timedOut)).toEqual(["ok", "ok", "ok", "error"]);
    // Spec 072 (AC-072-03): the text of `error.timeout` in pt, not the host's own message.
    expect(steps[3]!.label).toBe("sem resposta a tempo");
  });

  // Spec 031 (AC-031-02): the refused auth choice is an authentication failure with its own
  // literal, never a fabricated "Host alcançável" or a stuck spinner.
  it("marks the authentication step with the ssh-agent refusal", () => {
    const refused = state({
      dialog: dialog({ connecting: "abcdef", submitError: { code: "ssh_agent_unavailable", message: "ssh-agent indisponível nesta sessão", retryable: false } }),
      view: { hub: { revision: 2, hosts: [host({ phase: "connecting", phase_label: "Conectando" })] }, profiles: [], store_error: null },
    });
    const steps = connectionProgress(refused);
    expect(statuses(refused)).toEqual(["ok", "error", "blocked", "blocked"]);
    expect(steps[1]!.label).toBe("ssh-agent indisponível nesta sessão");
  });

  it("reads the SSH user from the form for user@host and ssh://user@host", () => {
    expect(sshUser("user@dev-box")).toBe("user");
    expect(sshUser("ssh://ana@dev-box.tailnet")).toBe("ana");
    expect(sshUser("")).toBe("");
    expect(sshUser("alias-do-ssh-config")).toBe("");
  });
});
