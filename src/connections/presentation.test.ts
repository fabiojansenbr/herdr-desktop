// Spec 003 — what each connection state shows and which explicit actions it offers.
import { describe, expect, it } from "vitest";
import { failureReason, hostActions, hostStatus, inputBlockText, validateDraft } from "./presentation";
import { hostFixture } from "./fake-bridge";
import type { HostDto } from "./types";

describe("host status text", () => {
  // Would catch: states distinguished only by colour, Reconectando shown as Offline, or the
  // attention guidance (explicit configuration) missing.
  it("gives every phase its own text and detail", () => {
    expect(hostStatus(hostFixture({ phase: "offline" })).text).toBe("Offline");
    expect(hostStatus(hostFixture({ phase: "connecting" })).text).toBe("Conectando");
    expect(hostStatus(hostFixture({ phase: "online" })).text).toBe("Online");
    const reconnecting = hostStatus(hostFixture({ phase: "reconnecting", attempt: 3, retry_in_ms: 3500 }));
    expect(reconnecting.text).toBe("Reconectando");
    expect(reconnecting.detail).toBe("tentativa 3 · próxima em 4 s");
    const attention = hostStatus(
      hostFixture({
        phase: "attention",
        attention: "host_key_unknown",
        guidance: "Confirme a impressão digital…",
        connection_error: { code: "ssh_host_key_unknown", message: "a chave deste host não é conhecida", retryable: false },
      }),
    );
    expect(attention.text).toBe("Precisa de atenção");
    expect(attention.detail).toBe("a chave deste host não é conhecida");
    expect(attention.guidance).toBe("Confirme a impressão digital…");
    expect(hostStatus(hostFixture({ phase: "offline", cancelled: true })).detail).toBe("cancelado; nenhuma nova tentativa");
    expect(new Set(["offline", "connecting", "online", "reconnecting", "attention"].map((p) => hostStatus(hostFixture({ phase: p as never })).tone)).size).toBe(5);
  });

  // Would catch: automatic retry buttons on attention, or no way to cancel a reconnection.
  it("offers only explicit actions per phase", () => {
    expect(hostActions(hostFixture({ phase: "offline" }))).toEqual(["connect"]);
    expect(hostActions(hostFixture({ phase: "connecting" }))).toEqual(["cancel"]);
    expect(hostActions(hostFixture({ phase: "reconnecting" }))).toEqual(["cancel"]);
    expect(hostActions(hostFixture({ phase: "online" }))).toEqual(["cancel"]);
    expect(hostActions(hostFixture({ phase: "attention" }))).toEqual(["retry"]);
  });

  it("explains why input is unavailable", () => {
    expect(inputBlockText("host_reconnecting")).toBe("Reconectando: input desabilitado");
    expect(inputBlockText("surface_from_previous_boot")).toBe("Aguardando tela atual do servidor reiniciado");
    expect(inputBlockText(null)).toBe("");
  });
});

// Spec 029 (AC-029-02): every connection failure has its own words, so a failed host is never
// left showing only "conectado"/"Online".
describe("connection failure reasons (AC-029-02)", () => {
  const failed = (attention: HostDto["attention"], code: string) =>
    hostFixture({
      phase: "attention",
      phase_label: "Precisa de atenção",
      attention,
      connection_error: { code, message: "detalhe bruto do backend", retryable: false, endpoint: "ssh-x" },
    });

  it("names herdr missing, outdated, auth, key and no answer", () => {
    expect(failureReason(failed("herdr_missing", "remote_herdr_missing"))).toBe("Herdr não encontrado");
    expect(failureReason(failed("herdr_outdated", "remote_herdr_outdated"))).toBe("Herdr desatualizado no host");
    expect(failureReason(failed("authentication_required", "ssh_authentication_required"))).toBe("autenticação recusada");
    expect(failureReason(failed("host_key_unknown", "ssh_host_key_unknown"))).toBe("chave do host não confirmada");
    expect(failureReason(failed("host_key_changed", "ssh_host_key_changed"))).toBe("chave do host mudou");
    expect(failureReason(failed("ssh_unavailable", "ssh_unavailable"))).toBe("SSH indisponível neste computador");
    expect(failureReason(failed("server_not_running", "remote_server_not_running"))).toBe("servidor Herdr não está rodando");
    expect(failureReason(failed("server_incompatible", "remote_server_incompatible"))).toBe("servidor Herdr incompatível");
    expect(
      failureReason(
        hostFixture({
          phase: "offline",
          connection_error: { code: "ssh_unreachable", message: "host SSH inacessível", retryable: true, endpoint: "ssh-x" },
        }),
      ),
    ).toBe("sem resposta");
  });

  it("has no reason without a failure and never fabricates one", () => {
    expect(failureReason(hostFixture({ phase: "online" }))).toBeNull();
    expect(failureReason(hostFixture({ phase: "offline" }))).toBeNull();
  });
});

describe("SSH form validation", () => {
  const valid = { label: "dev-box", target: "user@dev-box.tail3a9.ts.net", port: "22", session: "trabalho", auth: "key" };

  // Mirrors the backend rules so the form never submits what the backend would refuse.
  it("accepts a complete draft and refuses ambiguous targets, ports and sessions", () => {
    expect(validateDraft(valid)).toEqual({});
    expect(validateDraft({ ...valid, port: "" })).toEqual({});
    expect(validateDraft({ ...valid, label: "  " })).toEqual({ label: "Informe o nome de exibição" });
    expect(validateDraft({ ...valid, target: "-oProxyCommand=x" })).toEqual({ target: "O host não pode começar com '-'" });
    expect(validateDraft({ ...valid, target: "user:senha@host" })).toEqual({ target: "O host não pode conter senha" });
    expect(validateDraft({ ...valid, target: "dois hosts" })).toEqual({ target: "O host contém caracteres inválidos" });
    expect(validateDraft({ ...valid, target: "" })).toEqual({ target: "Informe o host SSH" });
    expect(validateDraft({ ...valid, port: "0" })).toEqual({ port: "Porta entre 1 e 65535" });
    expect(validateDraft({ ...valid, port: "22a" })).toEqual({ port: "Porta entre 1 e 65535" });
    expect(validateDraft({ ...valid, session: "default" })).toEqual({});
    expect(validateDraft({ ...valid, session: "a/b" })).toEqual({ session: "Sessão aceita letras, números, '.', '_' e '-'" });
  });
});
