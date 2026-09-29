// Drives the connections panel and dialog: one bridge command per user intent, results replace
// the view. Input is sent only with the qualified target of an enabled pane; a refused or failed
// send keeps its buffer and is never re-sent automatically. Errors stay on their host.

import { t } from "../i18n/index.svelte";
import type { ConnectionsBridge } from "./bridge";
import { validateDraft } from "./presentation";
import type { ConnectionsView, DraftForm, HostDto, ImportReport, RuntimeError, SshProfile, WorkspaceDto } from "./types";

export interface DialogState {
  open: boolean;
  draft: DraftForm;
  errors: Partial<Record<keyof DraftForm, string>>;
  submitting: boolean;
  submitError: RuntimeError | null;
  /** Endpoint of the host this dialog asked to connect (spec 011, AC-011-03); null before it. */
  connecting: string | null;
}

export interface ConnectionsState {
  view: ConnectionsView | null;
  loading: boolean;
  globalError: RuntimeError | null;
  hostErrors: Record<string, RuntimeError>;
  inputs: Record<string, string>;
  sending: Record<string, boolean>;
  dialog: DialogState;
  importReport: ImportReport | null;
  workspaces: Record<string, WorkspaceDto[]>;
}

export interface ConnectionsController {
  readonly state: ConnectionsState;
  load(): Promise<void>;
  refresh(): Promise<void>;
  /** Long-poll loop; resolves when stopped. */
  watch(): Promise<void>;
  stop(): void;
  connect(endpoint: string): Promise<void>;
  cancel(endpoint: string): Promise<void>;
  disconnect(endpoint: string): Promise<void>;
  reconnect(endpoint: string): Promise<void>;
  removeProfile(endpoint: string): Promise<void>;
  /** Turns "Conectar ao abrir" of a saved SSH profile on or off (spec 058, AC-058-03). */
  setConnectOnOpen(endpoint: string, enabled: boolean): Promise<void>;
  /** Opens the dialog bound to the saved profile of `endpoint` (spec 029, AC-029-03). */
  editProfile(endpoint: string): Promise<void>;
  openDialog(): void;
  editDraft(field: keyof DraftForm, value: string): void;
  cancelDialog(): void;
  submitDialog(connect: boolean): Promise<void>;
  importProfiles(): Promise<void>;
  editInput(endpoint: string, paneId: string, text: string): void;
  sendInput(endpoint: string, paneId: string): Promise<void>;
  listWorkspaces(endpoint: string): Promise<void>;
}

const emptyDraft = (): DraftForm => ({ id: null, label: "", target: "", port: "", session: "default", auth: "key" });

function asRuntimeError(error: unknown): RuntimeError {
  if (error && typeof error === "object" && "code" in error && "message" in error) return error as RuntimeError;
  return { code: "ipc_error", message: t("shell.error.ipc"), retryable: true };
}

/** Host the save just registered for this draft: by profile id, else by target and session. */
function hostForDraft(view: ConnectionsView, draft: DraftForm): HostDto | null {
  const hosts = view.hub.hosts.filter((h) => h.kind === "ssh");
  if (draft.id !== null) return hosts.find((h) => h.endpoint === draft.id) ?? null;
  const target = draft.target.trim();
  const session = draft.session.trim();
  return hosts.find((h) => (h.target ?? "").trim() === target && h.session === session) ?? null;
}

/** Saved profile of this draft, so a retry connects the same profile instead of a second one. */
function profileForDraft(view: ConnectionsView, draft: DraftForm): SshProfile | null {
  if (draft.id !== null) return view.profiles.find((p) => p.id === draft.id) ?? null;
  const target = draft.target.trim();
  const session = draft.session.trim();
  return view.profiles.find((p) => p.target.trim() === target && p.session === session) ?? null;
}

export const inputKey = (endpoint: string, paneId: string) => `${endpoint}/${paneId}`;

export function createConnectionsController(
  bridge: ConnectionsBridge,
  onChange: (state: ConnectionsState) => void = () => {},
): ConnectionsController {
  let state: ConnectionsState = {
    view: null,
    loading: false,
    globalError: null,
    hostErrors: {},
    inputs: {},
    sending: {},
    dialog: { open: false, draft: emptyDraft(), errors: {}, submitting: false, submitError: null, connecting: null },
    importReport: null,
    workspaces: {},
  };
  let watching = false;
  // Endpoints whose workspace list this dialog already asked for once (never replayed on failure).
  const reads = new Set<string>();

  // A dialog whose connection completed closes only after the workspace read succeeds. A failed
  // link or read stays visible with its reason and an explicit retry; no failure is hidden by
  // closing the modal.
  function closeSettled(current: ConnectionsState): ConnectionsState {
    const endpoint = current.dialog.open ? current.dialog.connecting : null;
    if (endpoint === null || current.view === null) return current;
    const host = current.view.hub.hosts.find((h) => h.endpoint === endpoint);
    if (!host || host.phase !== "online") return current;
    if (current.workspaces[endpoint] === undefined) return current;
    return { ...current, dialog: { open: false, draft: emptyDraft(), errors: {}, submitting: false, submitError: null, connecting: null } };
  }

  // Step 4 of the dialog (AC-011-03): read the connecting host's own workspace list once it is
  // online, through the host's lane, exactly once per request.
  function readWorkspaces() {
    const endpoint = state.dialog.open ? state.dialog.connecting : null;
    if (endpoint === null || reads.has(endpoint)) return;
    if (state.workspaces[endpoint] !== undefined || state.hostErrors[endpoint] !== undefined) return;
    const host = state.view?.hub.hosts.find((h) => h.endpoint === endpoint);
    if (!host || host.phase !== "online") return;
    reads.add(endpoint);
    void bridge.workspaces(endpoint).then(
      (list) => {
        const hostErrors = { ...state.hostErrors };
        delete hostErrors[endpoint];
        set({ workspaces: { ...state.workspaces, [endpoint]: list }, hostErrors });
      },
      (error) => hostError(endpoint, asRuntimeError(error)),
    );
  }

  const set = (patch: Partial<ConnectionsState>) => {
    state = closeSettled({ ...state, ...patch });
    onChange(state);
    readWorkspaces();
  };
  const hostError = (endpoint: string, error: RuntimeError | null) => {
    const hostErrors = { ...state.hostErrors };
    if (error) hostErrors[endpoint] = error;
    else delete hostErrors[endpoint];
    set({ hostErrors });
  };

  async function retryDialogConnection(endpoint: string): Promise<void> {
    const host = state.view?.hub.hosts.find((candidate) => candidate.endpoint === endpoint);
    const online = host?.phase === "online";
    if (online) reads.add(endpoint);
    else reads.delete(endpoint);
    const hostErrors = { ...state.hostErrors };
    delete hostErrors[endpoint];
    const workspaces = { ...state.workspaces };
    delete workspaces[endpoint];
    set({
      hostErrors,
      workspaces,
      dialog: { ...state.dialog, submitting: true, submitError: null },
    });
    try {
      if (online) {
        const list = await bridge.workspaces(endpoint);
        const nextHostErrors = { ...state.hostErrors };
        delete nextHostErrors[endpoint];
        set({ workspaces: { ...state.workspaces, [endpoint]: list }, hostErrors: nextHostErrors });
      } else {
        set({ view: await bridge.connect(endpoint), dialog: { ...state.dialog, submitting: false, submitError: null } });
      }
    } catch (error) {
      set({ dialog: { ...state.dialog, submitting: false, submitError: asRuntimeError(error) } });
    }
  }
  async function hostCommand(endpoint: string, run: () => Promise<ConnectionsView>) {
    try {
      set({ view: await run() });
      hostError(endpoint, null);
    } catch (error) {
      hostError(endpoint, asRuntimeError(error));
    }
  }

  return {
    get state() {
      return state;
    },
    async load() {
      set({ loading: true });
      try {
        set({ view: await bridge.list(), loading: false, globalError: null });
      } catch (error) {
        set({ loading: false, globalError: asRuntimeError(error) });
      }
    },
    async refresh() {
      try {
        set({ view: await bridge.list() });
      } catch (error) {
        set({ globalError: asRuntimeError(error) });
      }
    },
    async watch() {
      if (watching) return;
      watching = true;
      while (watching) {
        try {
          const view = await bridge.watch(state.view?.hub.revision ?? 0);
          if (!watching) break;
          if (view.hub.revision !== state.view?.hub.revision) set({ view, globalError: null });
        } catch (error) {
          set({ globalError: asRuntimeError(error) });
          await new Promise((resolve) => setTimeout(resolve, 1000));
        }
      }
    },
    stop() {
      watching = false;
    },
    connect: (endpoint) => hostCommand(endpoint, () => bridge.connect(endpoint)),
    cancel: (endpoint) => hostCommand(endpoint, () => bridge.cancel(endpoint)),
    disconnect: (endpoint) => hostCommand(endpoint, () => bridge.disconnect(endpoint)),
    reconnect: (endpoint) => hostCommand(endpoint, () => bridge.reconnect(endpoint)),
    removeProfile: (endpoint) => hostCommand(endpoint, () => bridge.removeProfile(endpoint)),
    setConnectOnOpen: (endpoint, enabled) => hostCommand(endpoint, () => bridge.setConnectOnOpen(endpoint, enabled)),
    async editProfile(endpoint) {
      const profile = state.view?.profiles.find((p) => p.id === endpoint) ?? null;
      if (!profile) {
        hostError(endpoint, { code: "profile_not_found", message: t("connections.error.profileNotFound"), retryable: false, endpoint });
        return;
      }
      reads.clear();
      set({
        dialog: {
          open: true,
          draft: {
            id: profile.id,
            label: profile.label,
            target: profile.target,
            port: profile.port === null ? "" : String(profile.port),
            session: profile.session,
            auth: profile.auth === "ssh-agent" ? "ssh-agent" : "key",
          },
          errors: {},
          submitting: false,
          submitError: null,
          connecting: null,
        },
      });
    },
    openDialog() {
      reads.clear();
      set({ dialog: { open: true, draft: emptyDraft(), errors: {}, submitting: false, submitError: null, connecting: null } });
    },
    editDraft(field, value) {
      const errors = { ...state.dialog.errors };
      delete errors[field];
      set({ dialog: { ...state.dialog, draft: { ...state.dialog.draft, [field]: value }, errors } });
    },
    cancelDialog() {
      const endpoint = state.dialog.connecting;
      set({ dialog: { ...state.dialog, open: false, submitting: false, submitError: null, connecting: null } });
      if (endpoint !== null) {
        void bridge.cancel(endpoint).then(
          (view) => set({ view }),
          (error) => hostError(endpoint, asRuntimeError(error)),
        );
      }
    },
    async submitDialog(connect) {
      const draft = state.dialog.draft;
      const errors = validateDraft(draft);
      if (Object.keys(errors).length > 0) {
        set({ dialog: { ...state.dialog, errors } });
        return;
      }
      const connectingEndpoint = state.dialog.connecting;
      const connectingHost = connectingEndpoint === null
        ? null
        : state.view?.hub.hosts.find((candidate) => candidate.endpoint === connectingEndpoint) ?? null;
      if (connect && connectingEndpoint !== null && (state.dialog.submitError !== null || state.hostErrors[connectingEndpoint] !== undefined || connectingHost?.phase === "attention")) {
        await retryDialogConnection(connectingEndpoint);
        return;
      }
      set({ dialog: { ...state.dialog, errors: {}, submitting: true, submitError: null } });
      try {
        const view = await bridge.saveProfile(
          {
            id: draft.id,
            label: draft.label.trim(),
            target: draft.target.trim(),
            port: draft.port.trim() === "" ? null : Number(draft.port.trim()),
            session: draft.session.trim(),
            auth: draft.auth,
          },
          connect,
        );
        const profile = connect ? profileForDraft(view, draft) : null;
        const host = connect ? hostForDraft(view, draft) : null;
        const endpoint = host?.endpoint ?? profile?.id ?? null;
        if (!connect || endpoint === null) {
          set({ view, dialog: { open: false, draft: emptyDraft(), errors: {}, submitting: false, submitError: null, connecting: null } });
          return;
        }
        reads.delete(endpoint);
        // Spec 011 (AC-011-03): the dialog stays open following this connection; the progress
        // steps read only the real host data, and the draft keeps the saved profile id so a retry
        // updates the same profile instead of creating another one.
        set({
          view,
          dialog: {
            open: true,
            draft: { ...draft, id: profile?.id ?? draft.id },
            errors: {},
            submitting: false,
            submitError: null,
            connecting: endpoint,
          },
        });
      } catch (error) {
        set({ dialog: { ...state.dialog, submitting: false, submitError: asRuntimeError(error) } });
      }
    },
    async importProfiles() {
      try {
        const result = await bridge.importProfiles();
        set({ view: result.view, importReport: result.report, globalError: null });
      } catch (error) {
        set({ globalError: asRuntimeError(error) });
      }
    },
    editInput(endpoint, paneId, text) {
      set({ inputs: { ...state.inputs, [inputKey(endpoint, paneId)]: text } });
    },
    async sendInput(endpoint, paneId) {
      const key = inputKey(endpoint, paneId);
      const pane = state.view?.hub.hosts.find((h) => h.endpoint === endpoint)?.panes.find((p) => p.pane_id === paneId);
      const text = state.inputs[key] ?? "";
      if (!pane?.input_enabled || !pane.target || state.sending[key]) return;
      set({ sending: { ...state.sending, [key]: true } });
      try {
        await bridge.sendText(pane.target, text, true);
        set({ inputs: { ...state.inputs, [key]: "" } });
        hostError(endpoint, null);
      } catch (error) {
        hostError(endpoint, asRuntimeError(error));
      } finally {
        set({ sending: { ...state.sending, [key]: false } });
      }
    },
    async listWorkspaces(endpoint) {
      try {
        const list = await bridge.workspaces(endpoint);
        set({ workspaces: { ...state.workspaces, [endpoint]: list } });
        hostError(endpoint, null);
      } catch (error) {
        hostError(endpoint, asRuntimeError(error));
      }
    },
  };
}
