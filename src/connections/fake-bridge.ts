// In-memory ConnectionsBridge for the isolated preview and the controller tests. Records every
// command with the exact argument object the Tauri bridge would send and simulates host states;
// it is not the backend implementation.

import type { ConnectionsBridge } from "./bridge";
import type { AttentionReason, ConnectionsView, HostDto, PaneDto, RuntimeError, SshProfile } from "./types";

export const SSH_ENDPOINT = "0123456789abcdef0123456789abcdef";

export interface RecordedCall {
  command: string;
  args: Record<string, unknown>;
}

export interface FakeConnectionsBridge extends ConnectionsBridge {
  calls: RecordedCall[];
  /** Brings a host online; `handshake` installs the data the real connect measured (spec 011). */
  setOnline(
    endpoint: string,
    bootId: string,
    generation?: number,
    handshake?: { latency_ms: number; server_version: string; herdr_binary?: { path: string; version: string | null } },
  ): void;
  setReconnecting(endpoint: string): void;
  setAttention(endpoint: string, reason: AttentionReason): void;
  failNextSend(error: RuntimeError): void;
}

export function hostFixture(overrides: Partial<HostDto>): HostDto {
  return {
    endpoint: SSH_ENDPOINT,
    label: "dev-box",
    kind: "ssh",
    session: "hd003-remote-b",
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
    ...overrides,
  };
}

const LABELS = { offline: "Offline", connecting: "Conectando", online: "Online", reconnecting: "Reconectando", attention: "Precisa de atenção" };

export function createFakeConnectionsBridge(): FakeConnectionsBridge {
  const calls: RecordedCall[] = [];
  let revision = 1;
  let sendFailure: RuntimeError | null = null;
  const profiles: SshProfile[] = [
    { id: SSH_ENDPOINT, label: "dev-box", target: "user@dev-box", port: null, session: "hd003-remote-b", connect_on_open: true, resume_on_open: false },
  ];
  const hosts: HostDto[] = [
    hostFixture({ endpoint: "local", label: "Este computador", kind: "local", session: "hd003-local-a", target: null }),
    hostFixture({}),
  ];
  const record = (command: string, args: Record<string, unknown> = {}) => calls.push({ command, args });
  const host = (endpoint: string) => {
    const found = hosts.find((h) => h.endpoint === endpoint);
    if (!found) throw { code: "endpoint_unknown", message: "endpoint desconhecido", retryable: false, endpoint } satisfies RuntimeError;
    return found;
  };
  // Local is not a saved profile: the destructive actions are refused by the backend.
  const sshOnly = (endpoint: string) => {
    if (endpoint === "local") {
      throw { code: "endpoint_local", message: "Este computador não tem perfil salvo", retryable: false, endpoint } satisfies RuntimeError;
    }
  };
  const view = (): ConnectionsView => structuredClone({ hub: { revision, hosts }, profiles, store_error: null });
  const change = (endpoint: string, patch: Partial<HostDto>) => {
    Object.assign(host(endpoint), patch, patch.phase ? { phase_label: LABELS[patch.phase] } : {});
    revision += 1;
  };

  return {
    calls,
    setOnline(endpoint, bootId, generation = 1, handshake) {
      const h = host(endpoint);
      const pane: PaneDto = {
        pane_id: "w1:p1",
        workspace_id: "w1",
        focused: true,
        input_enabled: true,
        input_block: null,
        target: { endpoint, session: h.session, connection_generation: generation, boot_id: bootId, workspace_id: "w1", pane_id: "w1:p1" },
      };
      change(endpoint, {
        phase: "online",
        boot_id: bootId,
        generation,
        panes: [pane],
        screen: [`${h.label}$`],
        cached: false,
        attempt: 0,
        retry_in_ms: null,
        latency_ms: handshake?.latency_ms ?? null,
        server_version: handshake?.server_version ?? null,
        herdr_binary: handshake?.herdr_binary ?? null,
      });
    },
    setReconnecting(endpoint) {
      const h = host(endpoint);
      change(endpoint, {
        phase: "reconnecting",
        attempt: 1,
        retry_in_ms: 1000,
        cached: true,
        panes: h.panes.map((p) => ({ ...p, input_enabled: false, input_block: "host_reconnecting", target: null })),
      });
    },
    setAttention(endpoint, reason) {
      change(endpoint, {
        phase: "attention",
        attention: reason,
        guidance: "Configure o host em um terminal e use Tentar novamente.",
        connection_error: { code: `ssh_${reason}`, message: "a chave deste host não é conhecida", retryable: false, endpoint },
      });
    },
    failNextSend(error) {
      sendFailure = error;
    },
    async list() {
      record("connections_list");
      return view();
    },
    async watch(since) {
      record("connections_watch", { revision: since });
      await new Promise((resolve) => setTimeout(resolve, since === revision ? 500 : 0));
      return view();
    },
    async saveProfile(draft, connect) {
      record("connection_profile_save", { draft, connect });
      if (!draft.id) {
        const id = `${Date.now().toString(16).padStart(32, "0")}`.slice(-32);
        profiles.push({ id, label: draft.label, target: draft.target, port: draft.port, session: draft.session, connect_on_open: true, resume_on_open: connect });
        hosts.push(hostFixture({ endpoint: id, label: draft.label, target: draft.target, session: draft.session, phase: connect ? "connecting" : "offline", phase_label: connect ? "Conectando" : "Offline" }));
        revision += 1;
      }
      return view();
    },
    async importProfiles() {
      record("connection_profiles_import");
      return { report: { imported: ["build-box"], skipped: [], already_present: 0 }, view: view() };
    },
    async connect(endpoint) {
      record("connection_connect", { endpoint });
      const h = host(endpoint);
      change(endpoint, { phase: h.panes.length > 0 ? "reconnecting" : "connecting", attention: null, guidance: null });
      return view();
    },
    async cancel(endpoint) {
      record("connection_cancel", { endpoint });
      change(endpoint, { phase: "offline", cancelled: true, retry_in_ms: null });
      return view();
    },
    async disconnect(endpoint) {
      record("connection_disconnect", { endpoint });
      sshOnly(endpoint);
      change(endpoint, {
        phase: "offline",
        cancelled: true,
        retry_in_ms: null,
        attempt: 0,
        panes: [],
        workspaces: [],
        boot_id: null,
        generation: null,
        latency_ms: null,
        server_version: null,
        herdr_binary: null,
      });
      return view();
    },
    async reconnect(endpoint) {
      record("connection_reconnect", { endpoint });
      sshOnly(endpoint);
      change(endpoint, { phase: host(endpoint).panes.length > 0 ? "reconnecting" : "connecting", cancelled: false, retry_in_ms: null });
      return view();
    },
    async removeProfile(endpoint) {
      record("connection_remove", { endpoint });
      sshOnly(endpoint);
      const profile = profiles.findIndex((p) => p.id === endpoint);
      if (profile >= 0) profiles.splice(profile, 1);
      const saved = hosts.findIndex((h) => h.endpoint === endpoint);
      if (saved >= 0) hosts.splice(saved, 1);
      revision += 1;
      return view();
    },
    async sendText(target, text, submit) {
      record("connection_send_text", { target, text, submit });
      if (sendFailure) {
        const error = sendFailure;
        sendFailure = null;
        throw error;
      }
    },
    async workspaces(endpoint) {
      record("connection_workspaces", { endpoint });
      return [{ workspace_id: "w1", label: "work" }];
    },
    async setConnectOnOpen(endpoint, enabled) {
      record("connections_set_connect_on_open", { endpoint, enabled });
      sshOnly(endpoint);
      const profile = profiles.find((p) => p.id === endpoint);
      if (!profile) throw { code: "profile_not_found", message: "perfil de conexão inexistente", retryable: false, endpoint } satisfies RuntimeError;
      profile.connect_on_open = enabled;
      revision += 1;
      return view();
    },
  };
}
