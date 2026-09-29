// Home area (spec 069): the "Good afternoon" screen — greeting, summary, project cards with
// their thumbnails, the waiting strip and the server cards (`src/components/home/`).
//
// The greeting is split into the period and the name so a language may order them its own way
// through `home.greeting`; the three periods follow the same hours in every language (AC-069-01).

import { area } from "../index.svelte";

export default area({
  en: {
    // --- greeting and summary -------------------------------------------------------------------
    "home.greeting": "{period}, {name}",
    "home.greeting.morning": "Good morning",
    "home.greeting.afternoon": "Good afternoon",
    "home.greeting.evening": "Good evening",
    "home.user": "you",
    "home.summary": "{projects} projects in {groups} groups · {waiting} agents waiting for you",
    "home.empty": "No collections yet",

    // --- buttons of the header ---------------------------------------------------------------------
    "home.connectServer": "Connect server",
    "home.newGroup": "New group",
    "home.openProject": "Open project",

    // --- groups ------------------------------------------------------------------------------------
    "home.group.expand": "Expand {name}",
    "home.group.collapse": "Collapse {name}",
    "home.group.projects": { one: "{count} project", other: "{count} projects" },
    "home.group.collapsed": "{projects} · collapsed · {agents}",
    "home.group.detail": "{projects} · {hosts}",
    "home.group.addProject": "Add a project to {name}",
    "home.group.addProjectLabel": "Add project",

    // --- project cards --------------------------------------------------------------------------------
    "home.card.branch": "Branch {name}",
    "home.card.thumbnail": "{name}: {state}",
    "home.card.noPanes": "No panes in this project",
    "home.cache": "cache",
    "home.dot.working": "working",
    "home.dot.blocked": "waiting",
    "home.dot.idle": "idle",
    "home.dot.unknown": "unknown",

    // --- waiting strip -----------------------------------------------------------------------------------
    "home.attention.label": "{count} agents waiting for you",
    "home.attention.count": { one: "{count} agent waiting", other: "{count} agents waiting" },
    "home.attention.chip": "Open the project and go to this agent's pane",
    "home.attention.cta": "Answer →",

    // --- servers -------------------------------------------------------------------------------------------
    "home.servers": "Servers",
    "home.server.select": "Select {name}",
    "home.server.thisComputer": "This computer",
    "home.server.panes": { one: "{count} pane", other: "{count} panes" },
    "home.server.detail": "herdr {version} · {panes}",
    "home.server.detailNoVersion": "herdr · {panes}",

    // --- thumbnail reads ------------------------------------------------------------------------------------
    "home.read.noConnection": "no confirmed connection to read the pane",
  },
  pt: {
    "home.greeting": "{period}, {name}",
    "home.greeting.morning": "Bom dia",
    "home.greeting.afternoon": "Boa tarde",
    "home.greeting.evening": "Boa noite",
    "home.user": "você",
    "home.summary": "{projects} projetos em {groups} grupos · {waiting} agentes esperando por você",
    "home.empty": "Nenhuma coleção ainda",

    "home.connectServer": "Conectar servidor",
    "home.newGroup": "Novo grupo",
    "home.openProject": "Abrir projeto",

    "home.group.expand": "Expandir {name}",
    "home.group.collapse": "Recolher {name}",
    "home.group.projects": { one: "{count} projeto", other: "{count} projetos" },
    "home.group.collapsed": "{projects} · recolhido · {agents}",
    "home.group.detail": "{projects} · {hosts}",
    "home.group.addProject": "Adicionar projeto a {name}",
    "home.group.addProjectLabel": "Adicionar projeto",

    "home.card.branch": "Branch {name}",
    "home.card.thumbnail": "{name}: {state}",
    "home.card.noPanes": "Sem panes neste projeto",
    "home.cache": "cache",
    "home.dot.working": "trabalhando",
    "home.dot.blocked": "aguardando",
    "home.dot.idle": "ocioso",
    "home.dot.unknown": "desconhecido",

    "home.attention.label": "{count} agentes esperando por você",
    "home.attention.count": { one: "{count} agente esperando", other: "{count} agentes esperando" },
    "home.attention.chip": "Abrir o projeto e ir ao pane deste agente",
    "home.attention.cta": "Responder →",

    "home.servers": "Servidores",
    "home.server.select": "Selecionar {name}",
    "home.server.thisComputer": "Este computador",
    "home.server.panes": { one: "{count} pane", other: "{count} panes" },
    "home.server.detail": "herdr {version} · {panes}",
    "home.server.detailNoVersion": "herdr · {panes}",

    "home.read.noConnection": "nenhuma conexão confirmada para ler o pane",
  },
  es: {
    "home.greeting": "{period}, {name}",
    "home.greeting.morning": "Buenos días",
    "home.greeting.afternoon": "Buenas tardes",
    "home.greeting.evening": "Buenas noches",
    "home.user": "tú",
    "home.summary": "{projects} proyectos en {groups} grupos · {waiting} agentes esperando por ti",
    "home.empty": "Ninguna colección todavía",

    "home.connectServer": "Conectar servidor",
    "home.newGroup": "Nuevo grupo",
    "home.openProject": "Abrir proyecto",

    "home.group.expand": "Expandir {name}",
    "home.group.collapse": "Contraer {name}",
    "home.group.projects": { one: "{count} proyecto", other: "{count} proyectos" },
    "home.group.collapsed": "{projects} · contraído · {agents}",
    "home.group.detail": "{projects} · {hosts}",
    "home.group.addProject": "Añadir un proyecto a {name}",
    "home.group.addProjectLabel": "Añadir proyecto",

    "home.card.branch": "Branch {name}",
    "home.card.thumbnail": "{name}: {state}",
    "home.card.noPanes": "Sin paneles en este proyecto",
    "home.cache": "cache",
    "home.dot.working": "trabajando",
    "home.dot.blocked": "esperando",
    "home.dot.idle": "inactivo",
    "home.dot.unknown": "desconocido",

    "home.attention.label": "{count} agentes esperando por ti",
    "home.attention.count": { one: "{count} agente esperando", other: "{count} agentes esperando" },
    "home.attention.chip": "Abrir el proyecto e ir al panel de este agente",
    "home.attention.cta": "Responder →",

    "home.servers": "Servidores",
    "home.server.select": "Seleccionar {name}",
    "home.server.thisComputer": "Este ordenador",
    "home.server.panes": { one: "{count} panel", other: "{count} paneles" },
    "home.server.detail": "herdr {version} · {panes}",
    "home.server.detailNoVersion": "herdr · {panes}",

    "home.read.noConnection": "ninguna conexión confirmada para leer el panel",
  },
});
