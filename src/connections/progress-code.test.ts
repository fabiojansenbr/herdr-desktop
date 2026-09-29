// Spec 071 (PRD i18n, P3) — the host speaks English and the WebView translates by `code`.
//
// Would catch: the dialog going back to reading the failure's message text (which is English
// since this spec, so a Portuguese substring like "workspaces" no longer matches), an error code
// of connections/files/projects/terminal left without an `error.<code>` key, a key whose `pt`
// stopped being the wording this product showed before the migration, the `phase.*` keys drifting
// away from the old `phase_label`, or the folder picker title losing its translation.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { dictionaries, errorText, LOCALES, phaseText, setLocalePreference, t, type Locale } from "../i18n/index.svelte";
import { connectionProgress, connectingFailureSource } from "./progress";
import type { ConnectionsState, DialogState } from "./controller";
import type { HostDto, LinkPhase, RuntimeError } from "./types";

const root = (path: string) => readFileSync(resolve(path), "utf8");

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

// --- AC-071-03: the dialog decides by code, never by the message -------------------------------

describe("AC-071-03 progresso por código", () => {
  // The host that answered the handshake but not `workspace.list`: since spec 071 the message is
  // "no answer while reading workspaces", so the old `message.includes("workspaces")` would miss
  // it and mark step 1 (no measured latency yet) as the failure.
  it("marks the workspace read on step four with an English message and no measured latency", () => {
    const timedOut = state({
      dialog: dialog({ connecting: "abcdef" }),
      view: {
        hub: {
          revision: 9,
          hosts: [host({ phase: "online", phase_label: "Online", latency_ms: null, server_version: "0.9.0", generation: 1 })],
        },
        profiles: [],
        store_error: null,
      },
      hostErrors: {
        abcdef: { code: "timeout", message: "no answer while reading workspaces", retryable: true, endpoint: "abcdef" },
      },
    });
    expect(connectingFailureSource(timedOut)).toBe("workspaces");
    expect(statuses(timedOut)).toEqual(["ok", "ok", "ok", "error"]);
    // The step that failed is the fourth, whatever the language of the message. Spec 072
    // (AC-072-03) then says it in the user's language: the suite runs on `pt`, so the label is
    // the text of `error.timeout` there, never the English message the host sent.
    expect(connectionProgress(timedOut)[3]!.label).toBe("sem resposta a tempo");
  });

  // `workspace.list` may be refused with any code of the host's lane, not just `timeout`. The old
  // rule only recognized the workspace read by the word "workspaces" in the message, so this
  // refusal landed on step 1; the slot it arrived in places it where it happened.
  it("marks any refusal of the workspace read on step four", () => {
    for (const code of ["endpoint_unavailable", "connection_lost", "result_unknown"]) {
      const refused = state({
        dialog: dialog({ connecting: "abcdef" }),
        view: {
          hub: {
            revision: 4,
            hosts: [host({ phase: "online", phase_label: "Online", latency_ms: null, server_version: "0.9.0", generation: 1 })],
          },
          profiles: [],
          store_error: null,
        },
        hostErrors: { abcdef: { code, message: "endpoint commands are unavailable on this connection", retryable: true } },
      });
      expect(statuses(refused), code).toEqual(["ok", "ok", "ok", "error"]);
    }
  });

  // A handshake timeout carries the same `timeout` code and must stay on the step whose evidence
  // never arrived: the code alone may not send every timeout to step 4.
  it("keeps a link timeout on the step whose evidence is missing", () => {
    const refused = state({
      dialog: dialog({ connecting: "abcdef" }),
      view: {
        hub: {
          revision: 3,
          hosts: [
            host({
              phase: "attention",
              phase_label: "Needs attention",
              connection_error: { code: "timeout", message: "the SSH host did not answer in time", retryable: true },
            }),
          ],
        },
        profiles: [],
        store_error: null,
      },
    });
    expect(connectingFailureSource(refused)).toBe("link");
    expect(statuses(refused)).toEqual(["error", "blocked", "blocked", "blocked"]);
  });

  // The codes that name their own step keep doing so with English messages.
  it("routes the attention and remote-api codes by code alone", () => {
    const byCode: [string, number][] = [
      ["remote_herdr_missing", 2],
      ["remote_herdr_outdated", 2],
      ["remote_server_not_running", 2],
      ["remote_server_incompatible", 2],
      ["ssh_agent_unavailable", 1],
      ["remote_api_unsupported", 3],
    ];
    for (const [code, index] of byCode) {
      const failed = state({
        dialog: dialog({ connecting: "abcdef", submitError: { code, message: "an English message", retryable: false } }),
        view: {
          hub: { revision: 2, hosts: [host({ phase: "connecting", phase_label: "Connecting" })] },
          profiles: [],
          store_error: null,
        },
      });
      expect(statuses(failed)[index], code).toBe("error");
    }
  });

  // Would catch: the decision going back to the message text.
  it("never inspects the failure's message", () => {
    const source = root("src/connections/progress.ts");
    expect(source).not.toContain("failure.message.includes");
    expect(source).toContain("STEP_BY_CODE");
  });
});

// --- AC-071-03: the picker title comes from the front, translated ------------------------------

describe("AC-071-03 título do seletor de pasta", () => {
  it("sends the translated title with the picker command", () => {
    const source = root("src/projects/bridge.ts");
    expect(source).toContain('invoke<string | null>("project_pick_folder", { title: t("dialog.openProject") })');
  });

  it("has the title in the three languages, within the host's 80-character limit", () => {
    const expected: Record<Locale, string> = { en: "Open project", pt: "Abrir projeto", es: "Abrir proyecto" };
    for (const language of LOCALES) {
      setLocalePreference(language);
      expect(t("dialog.openProject")).toBe(expected[language]);
      expect(t("dialog.openProject").length).toBeLessThanOrEqual(80);
    }
    setLocalePreference("pt");
  });
});

// --- AC-071-02: one `error.<code>` per code shown to the user ---------------------------------

/**
 * Codes the host can put in front of the user in the four areas of the AC. Taken from the code
 * literals of `src-tauri/src/connections/**`, `src-tauri/src/files/**`,
 * `src-tauri/src/project_store.rs` and `src-tauri/src/terminal.rs`.
 */
const CODES = {
  connections: [
    "host_offline",
    "host_connecting",
    "host_reconnecting",
    "host_needs_attention",
    "input_blocked",
    "pane_not_focused",
    "pane_not_in_snapshot",
    "tab_not_in_snapshot",
    "workspace_not_in_snapshot",
    "endpoint_unknown",
    "endpoint_duplicate",
    "endpoint_unavailable",
    "endpoint_unsupported",
    "endpoint_guard_unsupported",
    "endpoint_local",
    "endpoint_boot_changed",
    "connection_cancelled",
    "connection_closed",
    "connection_changed",
    "connection_lost",
    "server_shutdown",
    "server_unavailable",
    "surface_interest_invalid",
    "surface_interest_superseded",
    "surface_renegotiating",
    "result_unknown",
    "response_too_large",
    "empty_response",
    "unsupported_method",
    "remote_api_unsupported",
    "timeout",
    "action_failed",
    "watch_failed",
    "command_interrupted",
    "boot_unknown",
    "ssh_agent_unavailable",
    "ssh_host_key_unknown",
    "ssh_host_key_changed",
    "ssh_authentication_required",
    "ssh_unavailable",
    "ssh_unreachable",
    "ssh_failed",
    "ssh_terminated",
    "ssh_unexpected_exit",
    "remote_command_failed",
    "remote_herdr_missing",
    "remote_herdr_outdated",
    "remote_server_not_running",
    "remote_server_incompatible",
    "profile_not_found",
    "profile_in_use",
    "profile_id_invalid",
    "profile_label_invalid",
    "profile_auth_invalid",
    "profile_limit",
    "tui_profile_disabled",
    "tui_catalog_too_large",
    "tui_catalog_invalid",
    "connections_store_corrupt",
    "connections_store_unsupported",
    "prefs_inside_engine_dir",
    "serialization_error",
    "ssh_target_empty",
    "ssh_target_option_like",
    "ssh_target_invalid",
    "ssh_target_password",
    "ssh_port_invalid",
  ],
  files: [
    "file_uri_invalid",
    "file_host_unsupported",
    "file_host_mismatch",
    "file_host_unknown",
    "host_unavailable",
    "remote_path_unsupported",
    "root_not_authorized",
    "path_outside_root",
    "file_not_found",
    "permission_denied",
    "file_io_error",
    "remote_io_error",
    "file_too_large",
    "binary_unsupported",
    "not_a_file",
    "not_a_directory",
    "snapshot_unknown",
    "invalid_cursor",
    "cursor_stale",
    "state_poisoned",
    "operation_id_invalid",
    "operation_duplicate",
    "operation_cancelled",
    "connection_renewed",
    "connection_lost",
    "sftp_queue_full",
    "sftp_unavailable",
    "sftp_frame_rejected",
    "sftp_protocol_error",
    "sftp_operation_unsupported",
    "sftp_runtime_failed",
    "remote_name_unsupported",
    "timeout",
  ],
  projects: [
    "store_unversioned",
    "store_version_unsupported",
    "store_corrupt",
    "store_read_failed",
    "store_write_failed",
    "prefs_dir_forbidden",
    "unknown_endpoint",
    "invalid_root",
    "invalid_color",
    "invalid_collection_name",
    "invalid_project_label",
    "invalid_project_root",
    "invalid_endpoint_profile",
    "association_exists",
    "association_not_found",
    "collection_not_found",
    "project_not_found",
    "picker_unavailable",
    "protocol_error",
    "state_poisoned",
    "command_interrupted",
    "boot_unknown",
    "endpoint_unavailable",
    "target_boot_stale",
    "target_generation_stale",
    "target_endpoint_mismatch",
    "target_session_mismatch",
  ],
  terminal: [
    "no_session",
    "no_target",
    "not_connected",
    "pane_not_found",
    "surface_stale",
    "stale_content",
    "stale_link",
    "unsafe_link",
    "invalid_link",
    "invalid_selection",
    "invalid_input",
    "mouse_not_reporting",
    "scroll_unavailable",
    "server_shutdown",
  ],
} as const;

describe("AC-071-02 tradução de erro por código", () => {
  for (const [group, codes] of Object.entries(CODES)) {
    it(`has error.<code> in en/pt/es for every ${group} code`, () => {
      const dicts = dictionaries();
      const missing: string[] = [];
      for (const code of codes) {
        for (const language of LOCALES) {
          const message = dicts[language][`error.${code}`];
          if (typeof message !== "string" || message.trim() === "") missing.push(`${language}:error.${code}`);
        }
      }
      expect(missing).toEqual([]);
    });
  }

  // Would catch: a `pt` rewritten into something the user never saw, which is what AC-071-02
  // forbids ("com pt, os textos de erro mostrados ao usuário são os de hoje").
  it("keeps the Portuguese wording this product showed before the migration", () => {
    setLocalePreference("pt");
    const asBefore: Record<string, string> = {
      host_offline: "host desconectado; input indisponível",
      host_needs_attention: "o host precisa de atenção; input indisponível",
      ssh_agent_unavailable: "ssh-agent indisponível nesta sessão",
      ssh_unreachable: "host inalcançável (timeout)",
      remote_herdr_missing: "o Herdr não foi encontrado no host remoto",
      remote_herdr_outdated: "o Herdr do host está desatualizado e não atende o endpoint geração 1",
      remote_server_not_running: "o servidor Herdr desta sessão não está rodando no host remoto",
      remote_server_incompatible: "o servidor Herdr remoto não é compatível com o endpoint geração 1",
      ssh_host_key_unknown: "a chave deste host não é conhecida",
      ssh_host_key_changed: "a chave deste host mudou desde a última conexão",
      ssh_authentication_required: "o host exige autenticação interativa (senha/MFA) ou recusou a chave",
      ssh_unavailable: "o cliente OpenSSH não foi encontrado neste computador",
      connection_cancelled: "conexão cancelada; o servidor e suas sessões seguem em execução",
      connection_closed: "desconectado deste host; o servidor e suas sessões seguem em execução",
      server_shutdown: "o servidor Herdr foi encerrado",
      profile_in_use: "cancele a conexão antes de alterar o perfil",
      profile_not_found: "perfil de conexão inexistente",
      ssh_port_invalid: "a porta SSH deve estar entre 1 e 65535",
      path_outside_root: "o arquivo está fora das raízes autorizadas deste projeto",
      file_too_large: "o arquivo passa de 2 MiB; abra fora do editor",
      binary_unsupported: "o arquivo não é texto UTF-8 (ou contém NUL); o editor não foi carregado",
      not_a_file: "o caminho não é um arquivo",
      host_unavailable: "o host SSH não está conectado; o conteúdo em cache não é estado vivo",
      sftp_unavailable:
        "o SSH deste host funciona, mas o subsistema SFTP não está disponível; os terminais Herdr continuam utilizáveis. Habilite o subsistema sftp no sshd do host para ver arquivos",
      project_not_found: "projeto não encontrado",
      collection_not_found: "coleção não encontrada",
      association_exists: "o projeto já está nesta coleção",
      invalid_color: "esta cor não pertence à paleta do desktop",
      picker_unavailable: "seletor de pasta nativo indisponível",
      target_boot_stale: "o servidor reiniciou; selecione o alvo novamente",
      target_generation_stale: "a conexão foi renovada; selecione o alvo novamente",
      not_connected: "desconectado do servidor Herdr",
      no_session: "nenhuma sessão configurada",
      no_target: "nenhum pane alvo qualificado",
      scroll_unavailable: "o pane não possui histórico rolável",
      unsafe_link: "esquema de link não permitido",
      stale_content: "o conteúdo do pane mudou",
    };
    for (const [code, text] of Object.entries(asBefore)) {
      expect(errorText({ code, message: "an English message from the host" }), code).toBe(text);
    }
  });

  // Would catch: a code whose translation is the same string in the three languages (a copy of
  // `en` left in `pt`/`es`), and a code without a key silently showing nothing.
  it("translates a sample of codes into three distinct languages and falls back to the message", () => {
    const sample = ["ssh_unavailable", "project_not_found", "file_too_large", "no_session"];
    for (const code of sample) {
      const texts = LOCALES.map((language) => {
        setLocalePreference(language);
        return errorText({ code, message: "host message" });
      });
      expect(new Set(texts).size, code).toBe(3);
    }
    setLocalePreference("pt");
    expect(errorText({ code: "endpoint_error", message: "whatever the engine said" })).toBe("whatever the engine said");
  });
});

// --- AC-071-02: the phase words come from the 067 keys, not from `phase_label` -----------------

describe("AC-071-02 fase traduzida", () => {
  // The five labels `LinkPhase::label` used to send in Portuguese; `phaseText` must keep saying
  // exactly them under `pt`, so a component that stops rendering `phase_label` shows no change.
  it("says under pt what phase_label used to send", () => {
    setLocalePreference("pt");
    const asBefore: Record<LinkPhase, string> = {
      offline: "Offline",
      connecting: "Conectando",
      online: "Online",
      reconnecting: "Reconectando",
      attention: "Precisa de atenção",
    };
    for (const [phase, text] of Object.entries(asBefore)) {
      expect(phaseText(phase as LinkPhase), phase).toBe(text);
    }
  });

  it("says in en exactly what the host now sends as phase_label", () => {
    setLocalePreference("en");
    expect(["offline", "connecting", "online", "reconnecting", "attention"].map((p) => phaseText(p as LinkPhase))).toEqual(
      ["Offline", "Connecting", "Online", "Reconnecting", "Needs attention"],
    );
    const state = root("src-tauri/src/connections/state.rs");
    for (const label of ["Offline", "Connecting", "Online", "Reconnecting", "Needs attention"]) {
      expect(state, label).toContain(`"${label}"`);
    }
    setLocalePreference("pt");
  });

  it("has a Spanish word for every phase", () => {
    setLocalePreference("es");
    const spanish = ["offline", "connecting", "online", "reconnecting", "attention"].map((p) => phaseText(p as LinkPhase));
    expect(spanish).toEqual(["Sin conexión", "Conectando", "En línea", "Reconectando", "Necesita atención"]);
    setLocalePreference("pt");
  });
});

// --- AC-071-02: the dialog's own four lines, in the three languages ---------------------------

/** The dialog after a complete handshake: every step has arrived, so every line is its own text. */
function settled(): ConnectionsState {
  return state({
    dialog: dialog({
      connecting: "abcdef",
      draft: { id: null, label: "dev-box-rc", target: "ana@dev-box", port: "", session: "trabalho", auth: "key" },
    }),
    view: {
      hub: {
        revision: 7,
        hosts: [
          host({ phase: "online", phase_label: "Online", latency_ms: 251, server_version: "0.9.0-rc.7", generation: 1 }),
        ],
      },
      profiles: [],
      store_error: null,
    },
    workspaces: { abcdef: [{ workspace_id: "w1", label: "trabalho" }] },
  });
}

/** The dialog that only measured the latency: steps 3 and 4 still show their pending text. */
function reached(): ConnectionsState {
  const base = settled();
  return {
    ...base,
    view: {
      ...base.view!,
      hub: { revision: 8, hosts: [host({ phase: "connecting", phase_label: "Connecting", latency_ms: 45 })] },
    },
    workspaces: {},
  };
}

describe("AC-071-02 linhas do diálogo traduzidas", () => {
  // Would catch: a line of the dialog written literally in the file again (the suite is fixed on
  // `pt`, so only en/es can tell a `t()` call from a Portuguese literal).
  it("says the four lines in English", () => {
    setLocalePreference("en");
    const steps = connectionProgress(settled());
    expect(steps.map((step) => step.label)).toEqual([
      "Host reachable · 251 ms",
      "Authenticated as ana",
      "herdr 0.9.0-rc.7 found · endpoint generation 1",
      "Reading remote workspaces...",
    ]);
    expect(steps[2]!.meta).toBe("compatible");
    // Without a measured latency or a version, the lines lose the fact and keep the sentence.
    const pending = connectionProgress(reached());
    expect(pending[0]!.label).toBe("Host reachable · 45 ms");
    expect(pending[2]!.label).toBe("herdr found · endpoint generation 1");
    setLocalePreference("pt");
  });

  it("says the four lines in Spanish", () => {
    setLocalePreference("es");
    const steps = connectionProgress(settled());
    expect(steps.map((step) => step.label)).toEqual([
      "Host alcanzable · 251 ms",
      "Autenticado como ana",
      "herdr 0.9.0-rc.7 encontrado · endpoint generación 1",
      "Leyendo workspaces remotos...",
    ]);
    expect(steps[2]!.meta).toBe("compatible");
    expect(connectionProgress(reached())[2]!.label).toBe("herdr encontrado · endpoint generación 1");
    setLocalePreference("pt");
  });

  // Would catch: a `pt` reworded, which the existing assertions of progress.test.ts,
  // ConnectionDialog.test.ts and visual-projects.test.ts all depend on.
  it("keeps under pt exactly the wording the dialog showed before", () => {
    setLocalePreference("pt");
    const steps = connectionProgress(settled());
    expect(steps.map((step) => step.label)).toEqual([
      "Host alcançável · 251 ms",
      "Autenticado como ana",
      "herdr 0.9.0-rc.7 encontrado · endpoint geração 1",
      "Lendo workspaces remotos...",
    ]);
    expect(steps[2]!.meta).toBe("compatível");
    expect(connectionProgress(reached())[2]!.label).toBe("herdr encontrado · endpoint geração 1");
  });

  // Would catch: one of the eight keys left out of a language, and a visible text still literal.
  it("has the eight progress keys in en/pt/es and no Portuguese literal left in the module", () => {
    const dicts = dictionaries();
    const keys = Object.keys(dicts.en).filter((key) => key.startsWith("progress."));
    expect(keys).toHaveLength(8);
    for (const key of keys) {
      for (const language of LOCALES) expect(typeof dicts[language][key], `${language}:${key}`).toBe("string");
    }
    const source = root("src/connections/progress.ts");
    expect(source).not.toMatch(/[ãõçáéíóúâêô]/i);
    expect(source).toContain('t("progress.readingWorkspaces")');
  });
});
