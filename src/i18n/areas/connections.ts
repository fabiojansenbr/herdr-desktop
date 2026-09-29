// Connections area (spec 070): the "Connect to a herdr server" dialog, the connections panel and
// the host list, plus the words the presentation module gives a host state — why a host is not
// connected, why input is off on a pane, and what a draft profile is missing.
//
// The five link phases are not here: they are `phase.*` of the core area (spec 067), read through
// `phaseText`. The `error.*` keys of the engine's codes belong to spec 071; a coded error without
// a key still shows the engine's own message (`errorText`).
//
// `connections.host.thisComputer` (spec 072) is the panel's own name for the Local host: since
// 071 the host sends it as the English `This computer`, and the label of a host the user did not
// name is product text, like the sidebar's `sidebar.host.thisComputer`.

import { area } from "../index.svelte";

export default area({
  en: {
    "connections.dialog.title": "Connect to a herdr server",
    "connections.dialog.subtitle": "Agents keep running on the server even if you close the app.",
    "connections.dialog.close": "Close",
    "connections.dialog.kindGroup": "Connection type",
    "connections.dialog.progress": "Connection progress",
    "connections.dialog.footerNote": "Uses your ~/.ssh/config and Tailscale",
    "connections.dialog.save": "Save",
    "connections.dialog.connecting": "Connecting…",

    "connections.kind.local": "Local",
    "connections.kind.localHint": "This computer",
    "connections.kind.sshHint": "Server over SSH",

    "connections.field.host": "Host",
    "connections.field.hostPlaceholder": "user@host or ~/.ssh/config alias",
    "connections.field.port": "Port",
    "connections.field.auth": "Authentication",
    "connections.field.label": "Display name",
    "connections.field.group": "Add projects to group",
    "connections.field.session": "Session",

    "connections.auth.keyEd25519": "SSH key · ~/.ssh/id_ed25519",
    "connections.auth.keyRsa": "SSH key · ~/.ssh/id_rsa",

    "connections.group.personal": "Personal",
    "connections.group.none": "(None)",

    "connections.action.connect": "Connect",
    "connections.action.cancel": "Cancel",
    "connections.action.retry": "Try again",

    "connections.detail.cancelled": "cancelled; no further attempt",
    "connections.detail.retry": "attempt {attempt} · next in {seconds} s",

    "connections.failure.herdr_missing": "Herdr not found",
    "connections.failure.herdr_outdated": "Herdr outdated on the host",
    "connections.failure.authentication_required": "authentication refused",
    "connections.failure.host_key_unknown": "host key not confirmed",
    "connections.failure.host_key_changed": "host key changed",
    "connections.failure.ssh_unavailable": "SSH unavailable on this computer",
    "connections.failure.server_not_running": "Herdr server is not running",
    "connections.failure.server_incompatible": "Herdr server incompatible",
    "connections.failure.unknown": "no answer",

    "connections.block.host_offline": "Offline: input disabled",
    "connections.block.host_connecting": "Connecting: input disabled",
    "connections.block.host_reconnecting": "Reconnecting: input disabled",
    "connections.block.host_needs_attention": "Needs attention: input disabled",
    "connections.block.no_snapshot": "Waiting for the server's current state",
    "connections.block.pane_not_in_snapshot": "Pane no longer exists on this server",
    "connections.block.surface_hidden": "Host in the background (metadata only)",
    "connections.block.no_surface": "Waiting for the pane's screen",
    "connections.block.surface_stale": "Screen outdated; waiting for a resync",
    "connections.block.surface_from_previous_boot": "Waiting for the current screen of the restarted server",
    "connections.block.surface_from_previous_connection": "Waiting for the new connection's screen",

    "connections.validation.label": "Enter the display name",
    "connections.validation.target": "Enter the SSH host",
    "connections.validation.targetDash": "The host cannot start with '-'",
    "connections.validation.targetInvalid": "The host has invalid characters",
    "connections.validation.targetPassword": "The host cannot contain a password",
    "connections.validation.port": "Port between 1 and 65535",
    "connections.validation.session": "Enter the session",
    "connections.validation.sessionChars": "A session takes letters, digits, '.', '_' and '-'",

    "connections.error.profileNotFound": "connection profile does not exist",

    "connections.host.thisComputer": "This computer",
    "connections.session": "session {name}",
    "connections.panel.title": "Connections",
    "connections.panel.addSsh": "Add SSH connection",
    "connections.panel.importProfiles": "Import Herdr profiles",
    "connections.panel.importSummary": "Imported: {imported} · already present: {present} · skipped: {skipped}",
    "connections.panel.none": "none",
    "connections.panel.onboarding":
      "No SSH host registered. Add a connection or import Herdr profiles; nothing connects until you ask.",
    "connections.panel.listWorkspaces": "List workspaces",
    "connections.panel.workspaces": "Workspaces: {list}",
    "connections.panel.screen": "Screen of {host}",
    "connections.panel.paneInput": "Input for {pane} on {host}",
    "connections.panel.send": "Send",
    "connections.panel.loading": "Loading connections…",
  },
  pt: {
    "connections.dialog.title": "Conectar a um servidor herdr",
    "connections.dialog.subtitle": "Os agentes continuam rodando no servidor mesmo se você fechar o app.",
    "connections.dialog.close": "Fechar",
    "connections.dialog.kindGroup": "Tipo de conexão",
    "connections.dialog.progress": "Progresso da conexão",
    "connections.dialog.footerNote": "Usa seu ~/.ssh/config e Tailscale",
    "connections.dialog.save": "Salvar",
    "connections.dialog.connecting": "Conectando…",

    "connections.kind.local": "Local",
    "connections.kind.localHint": "Este computador",
    "connections.kind.sshHint": "Servidor via SSH",

    "connections.field.host": "Host",
    "connections.field.hostPlaceholder": "usuario@host ou alias do ~/.ssh/config",
    "connections.field.port": "Porta",
    "connections.field.auth": "Autenticação",
    "connections.field.label": "Nome de exibição",
    "connections.field.group": "Adicionar projetos ao grupo",
    "connections.field.session": "Sessão",

    "connections.auth.keyEd25519": "Chave SSH · ~/.ssh/id_ed25519",
    "connections.auth.keyRsa": "Chave SSH · ~/.ssh/id_rsa",

    "connections.group.personal": "Pessoal",
    "connections.group.none": "(Nenhum)",

    "connections.action.connect": "Conectar",
    "connections.action.cancel": "Cancelar",
    "connections.action.retry": "Tentar novamente",

    "connections.detail.cancelled": "cancelado; nenhuma nova tentativa",
    "connections.detail.retry": "tentativa {attempt} · próxima em {seconds} s",

    "connections.failure.herdr_missing": "Herdr não encontrado",
    "connections.failure.herdr_outdated": "Herdr desatualizado no host",
    "connections.failure.authentication_required": "autenticação recusada",
    "connections.failure.host_key_unknown": "chave do host não confirmada",
    "connections.failure.host_key_changed": "chave do host mudou",
    "connections.failure.ssh_unavailable": "SSH indisponível neste computador",
    "connections.failure.server_not_running": "servidor Herdr não está rodando",
    "connections.failure.server_incompatible": "servidor Herdr incompatível",
    "connections.failure.unknown": "sem resposta",

    "connections.block.host_offline": "Offline: input desabilitado",
    "connections.block.host_connecting": "Conectando: input desabilitado",
    "connections.block.host_reconnecting": "Reconectando: input desabilitado",
    "connections.block.host_needs_attention": "Precisa de atenção: input desabilitado",
    "connections.block.no_snapshot": "Aguardando estado atual do servidor",
    "connections.block.pane_not_in_snapshot": "Painel não existe mais neste servidor",
    "connections.block.surface_hidden": "Host em segundo plano (somente metadados)",
    "connections.block.no_surface": "Aguardando tela do painel",
    "connections.block.surface_stale": "Tela desatualizada; aguardando ressincronização",
    "connections.block.surface_from_previous_boot": "Aguardando tela atual do servidor reiniciado",
    "connections.block.surface_from_previous_connection": "Aguardando tela da nova conexão",

    "connections.validation.label": "Informe o nome de exibição",
    "connections.validation.target": "Informe o host SSH",
    "connections.validation.targetDash": "O host não pode começar com '-'",
    "connections.validation.targetInvalid": "O host contém caracteres inválidos",
    "connections.validation.targetPassword": "O host não pode conter senha",
    "connections.validation.port": "Porta entre 1 e 65535",
    "connections.validation.session": "Informe a sessão",
    "connections.validation.sessionChars": "Sessão aceita letras, números, '.', '_' e '-'",

    "connections.error.profileNotFound": "perfil de conexão inexistente",

    "connections.host.thisComputer": "Este computador",
    "connections.session": "sessão {name}",
    "connections.panel.title": "Conexões",
    "connections.panel.addSsh": "Adicionar conexão SSH",
    "connections.panel.importProfiles": "Importar perfis do Herdr",
    "connections.panel.importSummary": "Importados: {imported} · já presentes: {present} · ignorados: {skipped}",
    "connections.panel.none": "nenhum",
    "connections.panel.onboarding":
      "Nenhum host SSH cadastrado. Adicione uma conexão ou importe perfis do Herdr; nada é conectado até você pedir.",
    "connections.panel.listWorkspaces": "Listar workspaces",
    "connections.panel.workspaces": "Workspaces: {list}",
    "connections.panel.screen": "Tela de {host}",
    "connections.panel.paneInput": "Input para {pane} em {host}",
    "connections.panel.send": "Enviar",
    "connections.panel.loading": "Carregando conexões…",
  },
  es: {
    "connections.dialog.title": "Conectar a un servidor herdr",
    "connections.dialog.subtitle": "Los agentes siguen ejecutándose en el servidor aunque cierres la app.",
    "connections.dialog.close": "Cerrar",
    "connections.dialog.kindGroup": "Tipo de conexión",
    "connections.dialog.progress": "Progreso de la conexión",
    "connections.dialog.footerNote": "Usa tu ~/.ssh/config y Tailscale",
    "connections.dialog.save": "Guardar",
    "connections.dialog.connecting": "Conectando…",

    "connections.kind.local": "Local",
    "connections.kind.localHint": "Este ordenador",
    "connections.kind.sshHint": "Servidor por SSH",

    "connections.field.host": "Host",
    "connections.field.hostPlaceholder": "usuario@host o alias de ~/.ssh/config",
    "connections.field.port": "Puerto",
    "connections.field.auth": "Autenticación",
    "connections.field.label": "Nombre visible",
    "connections.field.group": "Añadir proyectos al grupo",
    "connections.field.session": "Sesión",

    "connections.auth.keyEd25519": "Clave SSH · ~/.ssh/id_ed25519",
    "connections.auth.keyRsa": "Clave SSH · ~/.ssh/id_rsa",

    "connections.group.personal": "Personal",
    "connections.group.none": "(Ninguno)",

    "connections.action.connect": "Conectar",
    "connections.action.cancel": "Cancelar",
    "connections.action.retry": "Reintentar",

    "connections.detail.cancelled": "cancelado; sin nuevos intentos",
    "connections.detail.retry": "intento {attempt} · siguiente en {seconds} s",

    "connections.failure.herdr_missing": "Herdr no encontrado",
    "connections.failure.herdr_outdated": "Herdr desactualizado en el host",
    "connections.failure.authentication_required": "autenticación rechazada",
    "connections.failure.host_key_unknown": "clave del host sin confirmar",
    "connections.failure.host_key_changed": "la clave del host cambió",
    "connections.failure.ssh_unavailable": "SSH no disponible en este equipo",
    "connections.failure.server_not_running": "el servidor Herdr no está en ejecución",
    "connections.failure.server_incompatible": "servidor Herdr incompatible",
    "connections.failure.unknown": "sin respuesta",

    "connections.block.host_offline": "Sin conexión: entrada desactivada",
    "connections.block.host_connecting": "Conectando: entrada desactivada",
    "connections.block.host_reconnecting": "Reconectando: entrada desactivada",
    "connections.block.host_needs_attention": "Necesita atención: entrada desactivada",
    "connections.block.no_snapshot": "Esperando el estado actual del servidor",
    "connections.block.pane_not_in_snapshot": "El panel ya no existe en este servidor",
    "connections.block.surface_hidden": "Host en segundo plano (solo metadatos)",
    "connections.block.no_surface": "Esperando la pantalla del panel",
    "connections.block.surface_stale": "Pantalla desactualizada; esperando resincronización",
    "connections.block.surface_from_previous_boot": "Esperando la pantalla actual del servidor reiniciado",
    "connections.block.surface_from_previous_connection": "Esperando la pantalla de la nueva conexión",

    "connections.validation.label": "Indique el nombre visible",
    "connections.validation.target": "Indique el host SSH",
    "connections.validation.targetDash": "El host no puede empezar por '-'",
    "connections.validation.targetInvalid": "El host contiene caracteres no válidos",
    "connections.validation.targetPassword": "El host no puede contener contraseña",
    "connections.validation.port": "Puerto entre 1 y 65535",
    "connections.validation.session": "Indique la sesión",
    "connections.validation.sessionChars": "La sesión acepta letras, números, '.', '_' y '-'",

    "connections.error.profileNotFound": "el perfil de conexión no existe",

    "connections.host.thisComputer": "Este ordenador",
    "connections.session": "sesión {name}",
    "connections.panel.title": "Conexiones",
    "connections.panel.addSsh": "Añadir conexión SSH",
    "connections.panel.importProfiles": "Importar perfiles de Herdr",
    "connections.panel.importSummary": "Importados: {imported} · ya presentes: {present} · omitidos: {skipped}",
    "connections.panel.none": "ninguno",
    "connections.panel.onboarding":
      "Ningún host SSH registrado. Añade una conexión o importa perfiles de Herdr; nada se conecta hasta que lo pidas.",
    "connections.panel.listWorkspaces": "Listar espacios de trabajo",
    "connections.panel.workspaces": "Espacios de trabajo: {list}",
    "connections.panel.screen": "Pantalla de {host}",
    "connections.panel.paneInput": "Entrada para {pane} en {host}",
    "connections.panel.send": "Enviar",
    "connections.panel.loading": "Cargando conexiones…",
  },
});
