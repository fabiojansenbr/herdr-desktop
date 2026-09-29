// Core area (spec 067): what more than one region shares — the agent states, the connection
// phases and relative time. The agent states used to be written out in four modules
// (`agents/status.ts`, `components/center/model.ts`, `components/frame/palette.ts`,
// `components/sidebar/tab-rows.ts`); this is now their only table.
//
// The `demo.*` and `error.demo_code` keys exist for `src/i18n/i18n.test.ts` alone: `t()`,
// interpolation, plurals and `errorText` need a key that is stable while the areas of 068–071 are
// still being written. No product module reads them.

import { area } from "../index.svelte";

export default area({
  en: {
    "agent.status.working": "Working",
    "agent.status.blocked": "Waiting for you",
    "agent.status.idle": "Idle",
    "agent.status.done": "Done",
    "agent.status.unknown": "Unknown",

    "phase.offline": "Offline",
    "phase.connecting": "Connecting",
    "phase.online": "Online",
    "phase.reconnecting": "Reconnecting",
    "phase.attention": "Needs attention",

    "time.now": "now",
    "time.minutes": { one: "{count} min", other: "{count} min" },
    "time.hours": { one: "{count} h", other: "{count} h" },
    "time.days": { one: "{count} d", other: "{count} d" },

    "demo.hello": "Hello {name}",
    "demo.items": { one: "{count} item", other: "{count} items" },
    "error.demo_code": "Demo error",
  },
  pt: {
    "agent.status.working": "Trabalhando",
    "agent.status.blocked": "Aguardando você",
    "agent.status.idle": "Ocioso",
    "agent.status.done": "Concluído",
    "agent.status.unknown": "Desconhecido",

    "phase.offline": "Offline",
    "phase.connecting": "Conectando",
    "phase.online": "Online",
    "phase.reconnecting": "Reconectando",
    "phase.attention": "Precisa de atenção",

    "time.now": "agora",
    "time.minutes": { one: "{count} min", other: "{count} min" },
    "time.hours": { one: "{count} h", other: "{count} h" },
    "time.days": { one: "{count} d", other: "{count} d" },

    "demo.hello": "Olá {name}",
    "demo.items": { one: "{count} item", other: "{count} itens" },
    "error.demo_code": "Erro de demonstração",
  },
  es: {
    "agent.status.working": "Trabajando",
    "agent.status.blocked": "Esperándote",
    "agent.status.idle": "Inactivo",
    "agent.status.done": "Terminado",
    "agent.status.unknown": "Desconocido",

    "phase.offline": "Sin conexión",
    "phase.connecting": "Conectando",
    "phase.online": "En línea",
    "phase.reconnecting": "Reconectando",
    "phase.attention": "Necesita atención",

    "time.now": "ahora",
    "time.minutes": { one: "{count} min", other: "{count} min" },
    "time.hours": { one: "{count} h", other: "{count} h" },
    "time.days": { one: "{count} d", other: "{count} d" },

    "demo.hello": "Hola {name}",
    "demo.items": { one: "{count} elemento", other: "{count} elementos" },
    "error.demo_code": "Error de demostración",
  },
});
