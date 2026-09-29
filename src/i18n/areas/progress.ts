// Progress area (spec 071). The four lines of the connection dialog (`src/connections/progress.ts`,
// spec 011 AC-011-03) are the only visible texts that file composes itself: reachability with the
// measured latency, the authenticated SSH user, the herdr found on the host with its version, and
// the remote workspace read. 071 owns the file, so it owns its words too.
//
// Everything else the dialog shows comes from elsewhere: the message of a failed step is the
// host's, translated by `errorText` (the `errors.ts` area), and the attention reason comes from
// `failureReason` in `presentation.ts` (spec 070).
//
// `pt` is the wording the dialog showed before this migration, character for character — that is
// what keeps the existing Portuguese assertions of `progress.test.ts`,
// `ConnectionDialog.test.ts` and `visual-projects.test.ts` true.

import { area } from "../index.svelte";

export default area({
  en: {
    "progress.reachable": "Host reachable",
    "progress.reachableLatency": "Host reachable · {latency} ms",
    "progress.authenticatedAs": "Authenticated as {user}",
    "progress.authenticated": "Authenticated",
    "progress.herdrFound": "herdr found · endpoint generation 1",
    "progress.herdrFoundVersion": "herdr {version} found · endpoint generation 1",
    "progress.compatible": "compatible",
    "progress.readingWorkspaces": "Reading remote workspaces...",
  },
  pt: {
    "progress.reachable": "Host alcançável",
    "progress.reachableLatency": "Host alcançável · {latency} ms",
    "progress.authenticatedAs": "Autenticado como {user}",
    "progress.authenticated": "Autenticado",
    "progress.herdrFound": "herdr encontrado · endpoint geração 1",
    "progress.herdrFoundVersion": "herdr {version} encontrado · endpoint geração 1",
    "progress.compatible": "compatível",
    "progress.readingWorkspaces": "Lendo workspaces remotos...",
  },
  es: {
    "progress.reachable": "Host alcanzable",
    "progress.reachableLatency": "Host alcanzable · {latency} ms",
    "progress.authenticatedAs": "Autenticado como {user}",
    "progress.authenticated": "Autenticado",
    "progress.herdrFound": "herdr encontrado · endpoint generación 1",
    "progress.herdrFoundVersion": "herdr {version} encontrado · endpoint generación 1",
    "progress.compatible": "compatible",
    "progress.readingWorkspaces": "Leyendo workspaces remotos...",
  },
});
