# Contributing

Thanks for looking at Herdr Desktop. This page covers the development environment, the
commands that have to pass before a change is proposed, the shape of the repository and the
conventions the code follows.

## Environment

Toolchain versions live in [`mise.toml`](mise.toml); [mise](https://mise.jdx.dev) installs
them per project so nothing is pinned globally.

```bash
mise install          # Rust (stable) and just, as pinned in mise.toml
bun install           # frontend dependencies, from bun.lock
```

You also need, outside mise:

- [Bun](https://bun.sh) — the frontend package manager and test runner.
- The system libraries Tauri needs to compile the window. On Linux that is `webkit2gtk-4.1`
  and its GTK 3 dependencies; see [docs/building.md](docs/building.md) for the full list per
  operating system.
- A [Herdr](https://github.com/herdrdev/herdr) engine (0.9.x) on the `PATH`, for running the
  app and the native end-to-end tests.

Commands below are written with `mise exec --`, which is how they run inside this project.

## Validation

These five commands are the gate. All of them must pass on a clean tree:

```bash
mise exec -- bunx vitest run
mise exec -- bun run check
mise exec -- cargo nextest run --workspace
mise exec -- cargo clippy --workspace --all-targets -- -D warnings
mise exec -- cargo fmt --all -- --check
```

- `bunx vitest run` — the frontend unit tests (renderer, models, i18n, styles).
- `bun run check` — `svelte-check`; it must report `0 errors and 0 warnings`.
- `cargo nextest run --workspace` — the Rust suite: protocol contracts, frame store, bridge,
  connections, stores.
- `cargo clippy … -D warnings` and `cargo fmt --all -- --check` — lint and formatting.

The native end-to-end tests are marked `#[ignore]` because they need a real window and a
running engine; `cargo nextest run --workspace` reports them as skipped. Run them deliberately,
against a **disposable** Herdr session — never the session you are working in:

```bash
mise exec -- just e2e-session-start          # prints the ids of a throwaway session
mise exec -- just e2e-session-stop <session>
```

[`justfile`](justfile) has the rest of the recipes (`just lint`, `just build`, `just dev <session>`,
the benchmarks).

## Repository layout

```
src/                  Svelte 5 frontend (the WebView)
  components/         window frame, sidebar, centre panes, files, home
  agents/ connections/ projects/ files/ editor/ terminal/ shell/ theme/
  i18n/               translation runtime; dictionaries in i18n/areas/
src-tauri/            Tauri host (Rust): IPC commands, bridges, stores
crates/herdr-client/  runtime gateway: contracts, frame store, transport, JSON API client
vendor/herdr-protocol/ read-only mirror of the Herdr generation-1 wire contract
tests/                native end-to-end tests (window + engine)
docs/                 building, releasing, architecture
LICENSES/             third-party license texts
```

[docs/architecture.md](docs/architecture.md) explains how these talk to each other.

## Conventions

**Tests first.** Write the test that fails for the change you are making, then make it pass.
Never weaken an existing assertion, delete one or mark it skipped to get a green run. When a
change deliberately alters a value a test pins (a visual token, a label), update the expected
value only, and say so in the pull request.

**Visible text goes through `t()`.** Every string a user can read comes from
`src/i18n/index.svelte.ts` and a dictionary under `src/i18n/areas/<area>.ts`, which carries
the three languages (`en`, `pt`, `es`) together. Never hard-code a user-facing string in a
component. `src/i18n/testing.ts` has a detector that fails a test when untranslated text
reaches the screen.

**The server owns the terminals.** PTYs, agent detection and session lifetime belong to the
Herdr engine. The client owns the visual layout, the project collections and the local
preferences. Runtime actions go through the JSON API; frames and input go through the visual
channel. Do not poll for pane contents.

**Validate before acting.** Endpoint, session, generation, boot id and pane are checked on
every action, and a failed check is an error — never a silent fallback to the local host.

**Keep the WebView narrow.** No secrets and no general shell access are exposed to the
frontend; the IPC surface is an explicit, bounded list of commands.

## Pull requests

- One focused change per pull request, with the validation output in the description.
- Say which platform you ran on. Linux is the only platform the maintainers test.
- New user-facing text needs the three languages in the same commit.
