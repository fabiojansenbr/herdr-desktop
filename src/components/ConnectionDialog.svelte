<script lang="ts">
  // "Conectar a um servidor herdr" (design/conectar-ssh.png, spec 011, AC-011-03).
  // 620px width, 12px radius, Local/SSH cards selectable by keyboard,
  // fields: Host, Porta (22), Autenticação (chave de ~/.ssh), Nome de exibição,
  // "Adicionar projetos ao grupo", Sessão, and 4 progress lines in order updated by the real
  // connection (pending before the dialog asks to connect; every value comes from the host).
  import { errorText, t } from "../i18n/index.svelte";
  import type { ConnectionsController, ConnectionsState } from "../connections/controller";
  import { connectingFailure, connectionProgress } from "../connections/progress";
  import type { DraftForm } from "../connections/types";
  import { isKindActivation, kindCard, kindForKey, type Kind } from "./connection-kind";
  import { modalCloser, modalTrace, wrapTarget } from "./modal-focus";

  interface Props {
    controller: ConnectionsController;
    state: ConnectionsState;
    /** Composed window: returns keyboard focus to the WebView after the modal closes (surface_focus_host). */
    focusHost?: () => Promise<void>;
  }

  let { controller, state: connState, focusHost }: Props = $props();
  const dialog = $derived(connState.dialog);
  const titleId = $props.id();

  const TABBABLE =
    "input:not([disabled]), button:not([disabled]), select:not([disabled]), textarea:not([disabled]), a[href], [tabindex]:not([tabindex='-1'])";
  let modalEl: HTMLDialogElement | null = null;
  let lastInside: HTMLElement | null = null;
  let requestClose: () => void = () => controller.cancelDialog();

  let selectedKind = $state<Kind>("ssh");
  // Spec 031 (AC-031-02): a escolha de autenticação vai no draft (o backend recusa `ssh-agent`
  // sem SSH_AUTH_SOCK antes de qualquer processo); as duas opções de chave usam os arquivos padrão.
  const authChoice = $derived(dialog.draft.auth === "ssh-agent" ? "ssh-agent" : "key-ed25519");

  const localCard = $derived(kindCard("local", selectedKind));
  const sshCard = $derived(kindCard("ssh", selectedKind));
  // Progress lines of this dialog's own connection; all four stay pending until it asks to
  // connect, and a failure marks the step that did not arrive (AC-011-03).
  const progress = $derived(connectionProgress(connState));
  const failure = $derived(connectingFailure(connState));
  const connecting = $derived(
    dialog.connecting !== null && progress.some((s) => s.status === "pending") && !progress.some((s) => s.status === "error"),
  );

  const focused = () => (document.activeElement instanceof HTMLElement ? document.activeElement : null);

  function modal(el: HTMLDialogElement) {
    const opener = focused();
    const close = modalCloser({
      isOpen: () => el.open,
      release: () => el.close(),
      opener,
      cancel: () => controller.cancelDialog(),
      focusHost,
      trace: modalTrace,
    });
    const tabbables = () =>
      Array.from(el.querySelectorAll<HTMLElement>(TABBABLE)).filter(
        (c) => c.tabIndex >= 0 && c.getClientRects().length > 0,
      );
    const onKeydown = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.isComposing || e.altKey || e.ctrlKey || e.metaKey) return;
      if (e.key === "Escape") {
        e.preventDefault();
        requestClose();
      } else if (e.key === "Tab") {
        const target = wrapTarget(tabbables(), focused(), e.shiftKey);
        if (target) {
          e.preventDefault();
          target.focus();
        }
      }
    };
    const onFocusin = (e: FocusEvent) => {
      if (e.target instanceof HTMLElement && el.contains(e.target)) lastInside = e.target;
    };
    const onClose = () => void requestClose();
    el.addEventListener("keydown", onKeydown);
    el.addEventListener("focusin", onFocusin);
    el.addEventListener("close", onClose);
    el.showModal();
    modalEl = el;
    requestClose = () => void close();
    el.querySelector<HTMLInputElement>("input:not([disabled])")?.focus();
    return () => {
      el.removeEventListener("keydown", onKeydown);
      el.removeEventListener("focusin", onFocusin);
      el.removeEventListener("close", onClose);
      if (el.open) el.close();
      if (modalEl === el) {
        modalEl = lastInside = null;
        requestClose = () => controller.cancelDialog();
      }
      if (opener?.isConnected && !opener.matches(":disabled")) opener.focus();
    };
  }

  $effect(() => {
    if (dialog.submitting || !dialog.open) return;
    void dialog.submitError;
    const el = modalEl;
    if (!el?.open || el.contains(focused())) return;
    const back =
      lastInside?.isConnected && !lastInside.matches(":disabled")
        ? lastInside
        : el.querySelector<HTMLElement>("input:not([disabled])");
    back?.focus();
  });

  function selectKind(kind: Kind) {
    selectedKind = kind;
  }

  function onKindKeydown(e: KeyboardEvent, kind: Kind) {
    if (isKindActivation(e.key)) {
      e.preventDefault();
      selectKind(kind);
      return;
    }
    const moved = kindForKey(e.key);
    if (moved !== null) {
      e.preventDefault();
      selectKind(moved);
      modalEl?.querySelector<HTMLElement>(`.kind[data-kind='${moved}']`)?.focus();
    }
  }

  function submit(event: SubmitEvent) {
    event.preventDefault();
    const submitter = (event.submitter as HTMLButtonElement | null)?.value;
    void controller.submitDialog(submitter !== "save");
  }
</script>

{#if dialog.open}
  <dialog class="backdrop connection-dialog" aria-labelledby={titleId} {@attach modal}>
    <form class="dialog" aria-label={t("connections.dialog.title")} onsubmit={submit}>
      <header>
        <div class="header-content">
          <div class="header-icon" aria-hidden="true">🌐</div>
          <div>
            <h2 id={titleId}>{t("connections.dialog.title")}</h2>
            <p>{t("connections.dialog.subtitle")}</p>
          </div>
        </div>
        <button
          type="button"
          class="close-x"
          aria-label={t("connections.dialog.close")}
          onclick={() => requestClose()}
        >
          ✕
        </button>
      </header>

      <!-- Local / SSH cards selectable by keyboard -->
      <div class="kinds" role="radiogroup" aria-label={t("connections.dialog.kindGroup")}>
        <div
          class="kind kind-card"
          class:selected={localCard.checked}
          role="radio"
          aria-checked={localCard.checked}
          tabindex={localCard.tabindex}
          data-kind="local"
          onclick={() => selectKind("local")}
          onkeydown={(e) => onKindKeydown(e, "local")}
        >
          <div class="kind-icon" aria-hidden="true">🖥️</div>
          <div class="kind-info">
            <b>{t("connections.kind.local")}</b>
            <span>{t("connections.kind.localHint")}</span>
          </div>
        </div>

        <div
          class="kind kind-card"
          class:selected={sshCard.checked}
          role="radio"
          aria-checked={sshCard.checked}
          tabindex={sshCard.tabindex}
          data-kind="ssh"
          onclick={() => selectKind("ssh")}
          onkeydown={(e) => onKindKeydown(e, "ssh")}
        >
          <div class="kind-icon" aria-hidden="true">📟</div>
          <div class="kind-info">
            <b>SSH</b>
            <span>{t("connections.kind.sshHint")}</span>
          </div>
        </div>
      </div>

      <!-- Fields -->
      <div class="fields-grid">
        <div class="row-1">
          <label class="field host">
            <span>{t("connections.field.host")}</span>
            <input
              class="mono"
              placeholder={t("connections.field.hostPlaceholder")}
              value={dialog.draft.target ?? ""}
              oninput={(e) => controller.editDraft("target", (e.currentTarget as HTMLInputElement).value)}
              aria-invalid={dialog.errors.target ? "true" : undefined}
            />
            {#if dialog.errors.target}<small class="error">{dialog.errors.target}</small>{/if}
          </label>

          <label class="field port">
            <span>{t("connections.field.port")}</span>
            <input
              placeholder="22"
              value={dialog.draft.port || "22"}
              inputmode="numeric"
              oninput={(e) => controller.editDraft("port", (e.currentTarget as HTMLInputElement).value)}
              aria-invalid={dialog.errors.port ? "true" : undefined}
            />
            {#if dialog.errors.port}<small class="error">{dialog.errors.port}</small>{/if}
          </label>
        </div>

        <div class="row-2">
          <label class="field auth">
            <span>{t("connections.field.auth")}</span>
            <select
              value={authChoice}
              onchange={(e) => {
                const value = (e.currentTarget as HTMLSelectElement).value;
                controller.editDraft("auth", value === "ssh-agent" ? "ssh-agent" : "key");
              }}
            >
              <option value="key-ed25519">{t("connections.auth.keyEd25519")}</option>
              <option value="key-rsa">{t("connections.auth.keyRsa")}</option>
              <option value="ssh-agent">ssh-agent</option>
            </select>
          </label>

          <label class="field label-field">
            <span>{t("connections.field.label")}</span>
            <input
              placeholder="dev-box"
              value={dialog.draft.label ?? ""}
              oninput={(e) => controller.editDraft("label", (e.currentTarget as HTMLInputElement).value)}
              aria-invalid={dialog.errors.label ? "true" : undefined}
            />
            {#if dialog.errors.label}<small class="error">{dialog.errors.label}</small>{/if}
          </label>
        </div>

        <div class="row-3">
          <label class="field session">
            <span>{t("connections.field.session")}</span>
            <input
              class="mono"
              placeholder="default"
              value={dialog.draft.session ?? ""}
              oninput={(e) => controller.editDraft("session", (e.currentTarget as HTMLInputElement).value)}
              aria-invalid={dialog.errors.session ? "true" : undefined}
            />
            {#if dialog.errors.session}<small class="error">{dialog.errors.session}</small>{/if}
          </label>
        </div>
      </div>

      <!-- 4 progress lines in order, every value from the connection this dialog asked for -->
      <div class="progress-box" role="list" aria-label={t("connections.dialog.progress")}>
        {#each progress as item (item.step)}
          <div class="progress-row progress-item" role="listitem" data-step={item.step} data-status={item.status}>
            <div class="step-left">
              {#if item.status === "ok"}
                <span class="step-icon ok" aria-hidden="true">✓</span>
              {:else if item.status === "error"}
                <span class="step-icon error" aria-hidden="true">✕</span>
              {:else if item.status === "blocked"}
                <span class="step-icon blocked" aria-hidden="true">—</span>
              {:else}
                <span class="step-icon spinner" aria-hidden="true">⟳</span>
              {/if}
              <span class="step-text step-label">{item.label}</span>
            </div>
            {#if item.meta}
              <span class="step-meta" class:compat={item.compat}>{item.meta}</span>
            {/if}
          </div>
        {/each}
      </div>

      {#if failure}
        <p class="error" role="alert"><b>{failure.code}</b> — {errorText(failure)}</p>
      {/if}

      <footer>
        <div class="footer-note">
          <span class="note-icon" aria-hidden="true">🛡️</span>
          <span>{t("connections.dialog.footerNote")}</span>
        </div>

        <div class="footer-actions">
          <button type="button" onclick={() => requestClose()}>{t("connections.action.cancel")}</button>
          <button type="submit" value="save" class="btn-save" disabled={dialog.submitting || connecting}>{t("connections.dialog.save")}</button>
          <button type="submit" value="connect" class="btn-primary" disabled={dialog.submitting || connecting}>
            {dialog.submitting || connecting
              ? t("connections.dialog.connecting")
              : failure
                ? t("connections.action.retry")
                : t("connections.action.connect")}
          </button>
        </div>
      </footer>
    </form>
  </dialog>
{/if}

<style>
  dialog.connection-dialog {
    position: fixed;
    inset: 0;
    width: min(620px, calc(100vw - 32px));
    box-sizing: border-box;
    margin: auto;
    background: var(--surface, #111318);
    border: 1px solid var(--border, #242833);
    border-radius: 12px;
    padding: 24px;
    color: var(--text, #e7e9ee);
    box-shadow: 0 16px 32px rgba(0, 0, 0, 0.6);
  }
  dialog.connection-dialog::backdrop {
    background: #0b0c10b3;
  }
  .dialog {
    width: 100%;
    background: transparent;
    border: none;
    padding: 0;
    margin: 0;
    color: inherit;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
  }
  .header-content {
    display: flex;
    align-items: flex-start;
    gap: 12px;
  }
  .header-icon {
    font-size: 24px;
    line-height: 1;
    margin-top: 2px;
  }
  h2 {
    margin: 0;
    font-size: 16px;
    font-weight: 600;
    color: var(--text, #e7e9ee);
  }
  header p {
    margin: 4px 0 0;
    color: var(--text-muted, #8c93a3);
    font-size: 13px;
  }
  .close-x {
    background: transparent;
    border: none;
    color: var(--text-muted, #8c93a3);
    font-size: 14px;
    cursor: pointer;
    padding: 4px 8px;
    border-radius: 4px;
  }
  .close-x:hover {
    color: var(--text, #e7e9ee);
    background: var(--surface-2, #171a21);
  }
  .kinds {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 12px;
  }
  .kind {
    border: 1px solid var(--border, #242833);
    border-radius: 8px;
    padding: 12px 14px;
    display: flex;
    align-items: center;
    gap: 10px;
    color: var(--text-muted, #8c93a3);
    cursor: pointer;
    background: var(--surface-2, #171a21);
    transition: border-color 0.15s ease, background-color 0.15s ease;
  }
  .kind:hover {
    border-color: var(--text-muted, #8c93a3);
  }
  .kind.selected {
    border-color: var(--accent, #8fa8ff);
    background: var(--accent-soft, #8fa8ff1f);
    color: var(--text, #e7e9ee);
  }
  .kind:focus-visible {
    outline: 2px solid var(--accent, #8fa8ff);
    outline-offset: 1px;
  }
  .kind-icon {
    font-size: 20px;
    line-height: 1;
  }
  .kind-info {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .kind-info b {
    font-size: 13px;
    font-weight: 600;
    color: var(--text, #e7e9ee);
  }
  .kind-info span {
    font-size: 11px;
  }
  .fields-grid {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .row-1 {
    display: grid;
    grid-template-columns: 3fr 1fr;
    gap: 10px;
  }
  .row-2,
  .row-3 {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
    color: var(--text-muted, #8c93a3);
  }
  input,
  select {
    background: var(--surface-2, #171a21);
    border: 1px solid var(--border, #242833);
    border-radius: 6px;
    color: var(--text, #e7e9ee);
    padding: 8px 10px;
    font: inherit;
    font-size: 13px;
    transition: border-color 0.15s ease;
  }
  input:focus-visible,
  select:focus-visible {
    outline: 2px solid var(--accent, #8fa8ff);
    outline-offset: 1px;
    border-color: var(--accent, #8fa8ff);
  }
  input::placeholder {
    color: var(--text-muted, #8c93a3);
    opacity: 1;
  }
  input[aria-invalid="true"] {
    border-color: var(--error, #f2777a);
  }
  .mono {
    font-family: var(--font-mono, "JetBrains Mono", monospace);
  }
  .error {
    color: var(--error, #f2777a);
    font-size: 11px;
  }
  .progress-box {
    background: var(--surface-2, #171a21);
    border: 1px solid var(--border, #242833);
    border-radius: 8px;
    padding: 12px 14px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .progress-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    font-size: 12px;
  }
  .step-left {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .step-icon {
    font-size: 12px;
    font-weight: 700;
  }
  .step-icon.ok {
    color: var(--working, #5bd68a);
  }
  .step-icon.error {
    color: var(--error, #f2777a);
  }
  .step-icon.spinner {
    color: var(--accent, #8fa8ff);
    animation: spin 1.5s linear infinite;
    display: inline-block;
  }
  @keyframes spin {
    100% {
      transform: rotate(360deg);
    }
  }
  .step-text {
    color: var(--text, #e7e9ee);
  }
  .step-meta {
    font-size: 11px;
    color: var(--text-muted, #8c93a3);
  }
  .step-meta.compat {
    color: var(--working, #5bd68a);
  }
  footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-top: 4px;
  }
  .footer-note {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 11px;
    color: var(--text-muted, #8c93a3);
  }
  .footer-actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  button {
    background: var(--surface-3, #1e222b);
    color: var(--text, #e7e9ee);
    border: 1px solid var(--border, #242833);
    border-radius: 6px;
    padding: 7px 14px;
    font: inherit;
    font-size: 12px;
    cursor: pointer;
    transition: border-color 0.15s ease, background-color 0.15s ease;
  }
  button:hover:not(:disabled) {
    border-color: var(--accent, #8fa8ff);
    background: #242a36;
  }
  button:focus-visible {
    outline: 2px solid var(--accent, #8fa8ff);
    outline-offset: 1px;
  }
  button:disabled {
    opacity: 0.55;
    cursor: default;
  }
  .btn-primary {
    background: var(--accent, #8fa8ff);
    color: #0b0c10;
    font-weight: 600;
    border-color: var(--accent, #8fa8ff);
  }
  .btn-primary:hover:not(:disabled) {
    background: #a3b8ff;
    border-color: #a3b8ff;
  }
</style>
