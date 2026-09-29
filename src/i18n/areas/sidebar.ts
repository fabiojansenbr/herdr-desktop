// Sidebar area (spec 068): the new sidebar of the 041–063 chain — the head, `Novo agente`, the
// "Precisa de você" box, the WORKSPACES section with its collections, rows, nested tabs and
// menus, the "Novo workspace" modal, the collapsed rail and the host switcher.
//
// English is the source of the keys (spec 067); `pt` keeps, word for word, the wording the area
// already had, so the tests written before the i18n base keep judging the same text.
//
// The connection phases and the agent states are not here: they are the single table of
// `areas/core.ts` (`phaseText`, `statusLabel`), shared with the centre and the palette.

import { area } from "../index.svelte";

export default area({
  en: {
    "sidebar.newAgent": "New agent",
    "sidebar.collapse": "Collapse sidebar",

    "sidebar.inbox.title": "Needs you",

    "sidebar.workspaces": "Workspaces",
    "sidebar.workspaces.new": "New workspace",
    "sidebar.workspaces.empty": "No workspaces",
    "sidebar.waitingHosts": "Connecting hosts",
    "sidebar.loadingWorkspaces": "Loading workspaces…",
    "sidebar.ungrouped": "No collection",
    "sidebar.hidden.title": "Hidden ({count})",
    "sidebar.hidden.list": "Hidden workspaces",
    "sidebar.hidden.unhide": "Show again",

    "sidebar.status.working": "{count} working",
    "sidebar.status.waiting": "{count} waiting",
    "sidebar.status.done": { one: "{count} done", other: "{count} done" },

    "sidebar.host.reconnecting": "reconnecting…",
    "sidebar.host.attention": "attention",
    "sidebar.offline": "offline",

    "sidebar.row.rename": "Rename {name}",
    "sidebar.row.pinned": "Pinned to top",
    "sidebar.row.branch": "Branch: {branch}",
    "sidebar.row.newAgentIn": "New agent in {name}",
    "sidebar.row.actions": "Actions for {name}",

    "sidebar.tabs.list": { one: "{count} tab in {name}", other: "{count} tabs in {name}" },

    "sidebar.collection.header": {
      one: "Collection {name}, {count} workspace",
      other: "Collection {name}, {count} workspaces",
    },
    "sidebar.collection.actions": "Actions for collection {name}",
    "sidebar.collection.color": "Collection colour",
    "sidebar.collection.delete": "Delete collection",
    "sidebar.collection.deleteItem": "Delete collection…",
    "sidebar.collection.deleteConfirm": "Delete {name}? Its workspaces stay in No collection.",

    "sidebar.rename": "Rename",
    "sidebar.color": "Colour",
    "sidebar.colorSwatch": "Colour {color}",
    "sidebar.delete": "Delete",
    "sidebar.cancel": "Cancel",
    "sidebar.close": "Close",
    "sidebar.remove": "Remove",

    "sidebar.menu.pin": "Pin to top",
    "sidebar.menu.unpin": "Unpin",
    "sidebar.menu.move": "Move to collection",
    "sidebar.menu.newCollectionName": "Name of the new collection",
    "sidebar.menu.collectionName": "Collection name",
    "sidebar.menu.newCollection": "New collection…",
    "sidebar.menu.copyPath": "Copy path",
    "sidebar.menu.hide": "Hide from sidebar",
    "sidebar.menu.close": "Close workspace",
    "sidebar.menu.closeItem": "Close workspace…",
    "sidebar.menu.closeConfirm": "Close {name}?",

    "sidebar.host.thisComputer": "This computer",
    "sidebar.host.label": "Host: {name}. {summary}",
    "sidebar.host.sshCount": { one: "{count} SSH host", other: "{count} SSH hosts" },
    "sidebar.host.select": "Select host",
    "sidebar.host.selected": "Selected host",
    "sidebar.host.actions": "Host actions",
    "sidebar.host.newConnection": "New connection",

    "sidebar.rail.label": "Collapsed sidebar",
    "sidebar.rail.expand": "Expand sidebar",
    "sidebar.rail.brand": "herdr — expand sidebar",
    "sidebar.rail.newAgent": "New agent ({hint})",
    "sidebar.rail.inboxCount": "{title}: {count}",
    "sidebar.rail.inboxEmpty": "{title}: nothing right now",

    "sidebar.newWorkspace.subtitle": "A workspace is a folder on a host. Agents run in it, in tabs.",
    "sidebar.newWorkspace.host": "Host",
    "sidebar.newWorkspace.disconnected": "disconnected",
    "sidebar.newWorkspace.hostOffline": "{name} · disconnected",
    "sidebar.newWorkspace.folder": "Folder",
    "sidebar.newWorkspace.localPath": "/path/to/folder",
    "sidebar.newWorkspace.remotePath": "~/path/on/host",
    "sidebar.newWorkspace.browse": "Choose…",
    "sidebar.newWorkspace.recents": "Recents on this host",
    "sidebar.newWorkspace.collection": "Collection",
    "sidebar.newWorkspace.startWith": "Start with",
    "sidebar.newWorkspace.shell": "Shell",
    "sidebar.newWorkspace.note": "Created in the engine of the chosen host",
    "sidebar.newWorkspace.submit": "Create workspace",
    "sidebar.newWorkspace.noAgentStart": "this host's server does not offer starting an agent",

    "sidebar.drag.crossHost": "Workspaces do not change host",
  },
  pt: {
    "sidebar.newAgent": "Novo agente",
    "sidebar.collapse": "Recolher lateral",

    "sidebar.inbox.title": "Precisa de você",

    "sidebar.workspaces": "Workspaces",
    "sidebar.workspaces.new": "Novo workspace",
    "sidebar.workspaces.empty": "Nenhum workspace",
    "sidebar.waitingHosts": "Hosts conectando",
    "sidebar.loadingWorkspaces": "Carregando workspaces…",
    "sidebar.ungrouped": "Sem coleção",
    "sidebar.hidden.title": "Ocultos ({count})",
    "sidebar.hidden.list": "Workspaces ocultos",
    "sidebar.hidden.unhide": "Reexibir",

    "sidebar.status.working": "{count} trabalhando",
    "sidebar.status.waiting": "{count} aguardando",
    "sidebar.status.done": { one: "{count} concluído", other: "{count} concluídos" },

    "sidebar.host.reconnecting": "reconectando…",
    "sidebar.host.attention": "atenção",
    "sidebar.offline": "offline",

    "sidebar.row.rename": "Renomear {name}",
    "sidebar.row.pinned": "Fixado no topo",
    "sidebar.row.branch": "Branch: {branch}",
    "sidebar.row.newAgentIn": "Novo agente em {name}",
    "sidebar.row.actions": "Ações de {name}",

    "sidebar.tabs.list": { one: "{count} aba em {name}", other: "{count} abas em {name}" },

    "sidebar.collection.header": {
      one: "Coleção {name}, {count} workspace",
      other: "Coleção {name}, {count} workspaces",
    },
    "sidebar.collection.actions": "Ações da coleção {name}",
    "sidebar.collection.color": "Cor da coleção",
    "sidebar.collection.delete": "Excluir coleção",
    "sidebar.collection.deleteItem": "Excluir coleção…",
    "sidebar.collection.deleteConfirm": "Excluir {name}? Os workspaces ficam em Sem coleção.",

    "sidebar.rename": "Renomear",
    "sidebar.color": "Cor",
    "sidebar.colorSwatch": "Cor {color}",
    "sidebar.delete": "Excluir",
    "sidebar.cancel": "Cancelar",
    "sidebar.close": "Fechar",
    "sidebar.remove": "Remover",

    "sidebar.menu.pin": "Fixar no topo",
    "sidebar.menu.unpin": "Desafixar",
    "sidebar.menu.move": "Mover para coleção",
    "sidebar.menu.newCollectionName": "Nome da nova coleção",
    "sidebar.menu.collectionName": "Nome da coleção",
    "sidebar.menu.newCollection": "Nova coleção…",
    "sidebar.menu.copyPath": "Copiar caminho",
    "sidebar.menu.hide": "Ocultar da lateral",
    "sidebar.menu.close": "Fechar workspace",
    "sidebar.menu.closeItem": "Fechar workspace…",
    "sidebar.menu.closeConfirm": "Fechar {name}?",

    "sidebar.host.thisComputer": "Este computador",
    "sidebar.host.label": "Host: {name}. {summary}",
    "sidebar.host.sshCount": { one: "{count} host SSH", other: "{count} hosts SSH" },
    "sidebar.host.select": "Selecionar host",
    "sidebar.host.selected": "Host selecionado",
    "sidebar.host.actions": "Ações do host",
    "sidebar.host.newConnection": "Nova conexão",

    "sidebar.rail.label": "Lateral recolhida",
    "sidebar.rail.expand": "Expandir lateral",
    "sidebar.rail.brand": "herdr — expandir lateral",
    "sidebar.rail.newAgent": "Novo agente ({hint})",
    "sidebar.rail.inboxCount": "{title}: {count}",
    "sidebar.rail.inboxEmpty": "{title}: nada por agora",

    "sidebar.newWorkspace.subtitle": "Um workspace é uma pasta num host. Os agentes rodam nele em abas.",
    "sidebar.newWorkspace.host": "Host",
    "sidebar.newWorkspace.disconnected": "desconectado",
    "sidebar.newWorkspace.hostOffline": "{name} · desconectado",
    "sidebar.newWorkspace.folder": "Pasta",
    "sidebar.newWorkspace.localPath": "/caminho/da/pasta",
    "sidebar.newWorkspace.remotePath": "~/caminho/no/host",
    "sidebar.newWorkspace.browse": "Escolher…",
    "sidebar.newWorkspace.recents": "Recentes neste host",
    "sidebar.newWorkspace.collection": "Coleção",
    "sidebar.newWorkspace.startWith": "Iniciar com",
    "sidebar.newWorkspace.shell": "Shell",
    "sidebar.newWorkspace.note": "Criado na engine do host escolhido",
    "sidebar.newWorkspace.submit": "Criar workspace",
    "sidebar.newWorkspace.noAgentStart": "o servidor deste host não oferece iniciar agente",

    "sidebar.drag.crossHost": "Workspaces não mudam de host",
  },
  es: {
    "sidebar.newAgent": "Nuevo agente",
    "sidebar.collapse": "Contraer barra lateral",

    "sidebar.inbox.title": "Te necesita",

    "sidebar.workspaces": "Espacios de trabajo",
    "sidebar.workspaces.new": "Nuevo espacio de trabajo",
    "sidebar.workspaces.empty": "Ningún espacio de trabajo",
    "sidebar.waitingHosts": "Hosts conectándose",
    "sidebar.loadingWorkspaces": "Cargando espacios de trabajo…",
    "sidebar.ungrouped": "Sin colección",
    "sidebar.hidden.title": "Ocultos ({count})",
    "sidebar.hidden.list": "Espacios de trabajo ocultos",
    "sidebar.hidden.unhide": "Mostrar de nuevo",

    "sidebar.status.working": "{count} trabajando",
    "sidebar.status.waiting": "{count} esperando",
    "sidebar.status.done": { one: "{count} terminado", other: "{count} terminados" },

    "sidebar.host.reconnecting": "reconectando…",
    "sidebar.host.attention": "atención",
    "sidebar.offline": "sin conexión",

    "sidebar.row.rename": "Renombrar {name}",
    "sidebar.row.pinned": "Fijado arriba",
    "sidebar.row.branch": "Rama: {branch}",
    "sidebar.row.newAgentIn": "Nuevo agente en {name}",
    "sidebar.row.actions": "Acciones de {name}",

    "sidebar.tabs.list": { one: "{count} pestaña en {name}", other: "{count} pestañas en {name}" },

    "sidebar.collection.header": {
      one: "Colección {name}, {count} espacio de trabajo",
      other: "Colección {name}, {count} espacios de trabajo",
    },
    "sidebar.collection.actions": "Acciones de la colección {name}",
    "sidebar.collection.color": "Color de la colección",
    "sidebar.collection.delete": "Eliminar colección",
    "sidebar.collection.deleteItem": "Eliminar colección…",
    "sidebar.collection.deleteConfirm": "¿Eliminar {name}? Sus espacios de trabajo quedan en Sin colección.",

    "sidebar.rename": "Renombrar",
    "sidebar.color": "Color",
    "sidebar.colorSwatch": "Color {color}",
    "sidebar.delete": "Eliminar",
    "sidebar.cancel": "Cancelar",
    "sidebar.close": "Cerrar",
    "sidebar.remove": "Quitar",

    "sidebar.menu.pin": "Fijar arriba",
    "sidebar.menu.unpin": "Dejar de fijar",
    "sidebar.menu.move": "Mover a colección",
    "sidebar.menu.newCollectionName": "Nombre de la nueva colección",
    "sidebar.menu.collectionName": "Nombre de la colección",
    "sidebar.menu.newCollection": "Nueva colección…",
    "sidebar.menu.copyPath": "Copiar ruta",
    "sidebar.menu.hide": "Ocultar de la barra lateral",
    "sidebar.menu.close": "Cerrar espacio de trabajo",
    "sidebar.menu.closeItem": "Cerrar espacio de trabajo…",
    "sidebar.menu.closeConfirm": "¿Cerrar {name}?",

    "sidebar.host.thisComputer": "Este ordenador",
    "sidebar.host.label": "Host: {name}. {summary}",
    "sidebar.host.sshCount": { one: "{count} host SSH", other: "{count} hosts SSH" },
    "sidebar.host.select": "Seleccionar host",
    "sidebar.host.selected": "Host seleccionado",
    "sidebar.host.actions": "Acciones del host",
    "sidebar.host.newConnection": "Nueva conexión",

    "sidebar.rail.label": "Barra lateral contraída",
    "sidebar.rail.expand": "Expandir barra lateral",
    "sidebar.rail.brand": "herdr — expandir barra lateral",
    "sidebar.rail.newAgent": "Nuevo agente ({hint})",
    "sidebar.rail.inboxCount": "{title}: {count}",
    "sidebar.rail.inboxEmpty": "{title}: nada por ahora",

    "sidebar.newWorkspace.subtitle": "Un espacio de trabajo es una carpeta en un host. Los agentes se ejecutan en él, en pestañas.",
    "sidebar.newWorkspace.host": "Host",
    "sidebar.newWorkspace.disconnected": "desconectado",
    "sidebar.newWorkspace.hostOffline": "{name} · desconectado",
    "sidebar.newWorkspace.folder": "Carpeta",
    "sidebar.newWorkspace.localPath": "/ruta/de/la/carpeta",
    "sidebar.newWorkspace.remotePath": "~/ruta/en/el/host",
    "sidebar.newWorkspace.browse": "Elegir…",
    "sidebar.newWorkspace.recents": "Recientes en este host",
    "sidebar.newWorkspace.collection": "Colección",
    "sidebar.newWorkspace.startWith": "Iniciar con",
    "sidebar.newWorkspace.shell": "Shell",
    "sidebar.newWorkspace.note": "Creado en el motor del host elegido",
    "sidebar.newWorkspace.submit": "Crear espacio de trabajo",
    "sidebar.newWorkspace.noAgentStart": "el servidor de este host no ofrece iniciar un agente",

    "sidebar.drag.crossHost": "Los espacios de trabajo no cambian de host",
  },
});
