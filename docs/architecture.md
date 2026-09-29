# Architecture

Herdr Desktop is a client. The [Herdr](https://github.com/herdrdev/herdr) engine owns the
PTYs, detects the agents and keeps the sessions alive; this app renders what the engine
publishes and sends input back. Nothing in here spawns a terminal.

```
┌──────────────────────────────────────────────────────────┐
│ WebView — Svelte 5 (src/)                                │
│   sidebar · centre panes · files · editor · i18n          │
└───────────────┬──────────────────────────────────────────┘
                │ Tauri IPC: bounded command list + one event channel
┌───────────────┴──────────────────────────────────────────┐
│ Host — Rust (src-tauri/)                                 │
│   connections hub · selection/composition · agents ·      │
│   project store · file providers                          │
└───────────────┬──────────────────────────────────────────┘
                │ crates/herdr-client — runtime gateway
                │ vendor/herdr-protocol — wire contract (generation 1)
┌───────────────┴──────────────────────────────────────────┐
│ Herdr engine — local socket, or an SSH host              │
└──────────────────────────────────────────────────────────┘
```

## The WebView (`src/`)

Svelte 5 with runes, bundled by Vite into `dist/`, which the Rust binary embeds at compile
time. It holds the whole visual model: the workspace tree per host, tabs and panes, the agent
list, the files browser and the supporting editor.

It has no sockets, no credentials, no environment and no shell. Everything it can do is an
explicit IPC command, and everything it learns arrives as a validated DTO.

## The host (`src-tauri/`)

The Tauri binary. `lib.rs` composes one `DesktopServices` that owns:

- **`connections/`** — the hub: endpoint profiles, the connection state machine per host
  (offline, connecting, online, reconnecting, needs attention), SSH options and the remote
  binary check.
- **`bridge/`** — the seam to the WebView: pane composition and the surface the window paints,
  the selection, agent commands, workspace commands and the event channel.
- **`project_store.rs`** — project collections, groups and per-workspace preferences, on disk
  with schema migrations.
- **`files/`** — the local file provider and the SFTP provider for remote hosts (read-only in
  this version).
- **`locale.rs`, `theme.rs`, `agent_kinds.rs`** — the host's answers to questions the WebView
  cannot answer for itself: system language, theme file, which agent binaries are on the
  `PATH`.

Every module publishes a `COMMANDS` list, and a test asserts the assembled handler matches
that registry — a module cannot expose a command silently.

## `crates/herdr-client`

The runtime gateway, with no Tauri, no SSH and no editor in it:

- `contracts` — `QualifiedTarget`, `RuntimeError`, `RuntimeGateway`, `FileProvider`.
- `local` — the transport over the engine's client socket (a filesystem socket on Unix, a
  named pipe on Windows).
- `api` — the newline-delimited JSON API client used for runtime actions that are not part of
  the visual lane (`workspace.create`, `pane.process_info`, …), one request per connection.
- `frame_store` — full frames and patches with revisions, stale rejection and input gating.
- `event_queue` — the bounded queue shared by the local and SSH transports.
- `session`, `bootstrap` — session-name validation, socket paths, and the fatal vs recoverable
  classification when starting a session.

## `vendor/herdr-protocol`

A read-only mirror of the engine's generation-1 wire contract: the endpoint handshake
(`EndpointClientHello` / `EndpointServerWelcome`), `ClientMessage` / `ServerMessage` and the
surface frame types. The source files, their checksums and the commit they were taken from
are recorded in `vendor/herdr-protocol/PROVENANCE.md`. Frozen fixtures and digest tests keep
the encoding from drifting.

The codec is bincode 2.0.1 pinned to the exact version the engine locks, framed as
`[u32 LE length][payload]`. The published codecs and enums are immutable: a change there is a
new generation, never an edit.

## Connecting to an engine

1. The host resolves the endpoint — the local socket, or an OpenSSH channel to a saved host,
   reusing your `~/.ssh/config`, keys and agent.
2. It sends the endpoint hello and requires generation **1** in the welcome. A mismatch, an
   absent engine or an outdated one is a typed failure shown in the interface; there is never
   a silent fallback to the local host.
3. The engine then streams the session snapshot and the surface frames for the visible panes.
   Hidden panes do not cause repaints.
4. Runtime actions (create a workspace, start an agent, resize) go over the JSON API, so the
   visual lane carries only frames and input.

Every action re-validates endpoint, session, generation, boot id and pane before it is sent.
When a boot id changes the client resynchronises instead of replaying input.

## Internationalisation

`src/i18n/index.svelte.ts` is the runtime: `t()`, `formatRelative`, `phaseText`, `errorText`
and the locale preference. Dictionaries live per area in `src/i18n/areas/<area>.ts`, each
carrying `en`, `pt` and `es` side by side, and are discovered by glob.

The host decides the starting language (`locale.rs`: the `HERDR_DESKTOP_LOCALE` override, then
the system locale, then English) and the frontend narrows the tag to one of the three
languages. The user can pin one from the command palette. `src/i18n/testing.ts` has a detector
that fails a test if untranslated text reaches the screen.
