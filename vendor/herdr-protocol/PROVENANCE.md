# herdr-protocol — provenance of the minimal extraction

A read-only copy of Herdr's **pure contracts**: no server logic, no `AppState`, no path
dependency on the engine and no Zig. The upstream engine was not modified.

| Item | Value |
|---|---|
| Source repository | https://github.com/herdrdev/herdr |
| Commit | `03749ae3970a74e077fc16b9327bddfc957771c9` (herdr 0.9.0) |
| Source license | Apache-2.0 — copy in `LICENSES/Apache-2.0-herdr.txt` (sha256 `c71d239df91726fc519c6eb72d318ec65820627232b2f796219e87dcf35d0ab4`) |
| Binary codec | bincode 2.0.1, `bincode::config::standard()`, serde; frame `[u32 LE len][payload]`, max 2 MiB |
| Negotiated generation | endpoint generation **1** (`endpoint.hello.v1` / `endpoint.welcome.v1`) |

## Mirrored source files (sha256 at the commit above)

| Engine file | sha256 | What was mirrored |
|---|---|---|
| `src/protocol/wire.rs` | `525dcf81e3e81d00a848653efedde3c331b873f7c472d7085dc673a8501fc652` | `ClientMessage`, `ServerMessage` and every reachable type; `color_to_u32`/`u32_to_color` (as `WireColor`); underline bits; framing |
| `src/protocol/endpoint.rs` | `348737dc7871c4e2be1b0985f016af8bd0cf6ba0fb11eb4cfec73201352a8689` | constants, `EndpointClientHello`, `EndpointServerWelcome`, `EndpointHandshakeError`, `EndpointAgentViewProjection`, `supports_required_codecs` |
| `src/input/model.rs` | (leaf) | `WindowsKeyRecord` |
| `src/api/schema/common.rs` | (leaf) | `AgentStatus` |
| `src/config/model.rs` | (leaf) | `ToastHerdrPosition` |
| `src/server/client_commands.rs` | `3c4dcd0f069421ddaadf26ec50a60477e4e4e515b5f38b3c95cd8496f67db22a` | reference only: list of methods announced in the welcome; not copied |

Frozen fixtures copied byte for byte into `tests/fixtures/`:

| Fixture | sha256 |
|---|---|
| `endpoint-hello-v1.json` | `349fd82bbf682b6aaa774d3b570a9cc4384a4eba1f66de667b0cffba74000efe` |
| `endpoint-welcome-v1.json` | `ba4da436017145f2ad00516b8bb446d28c76f9dcf27b42d0382186951a6a92d0` |
| `endpoint-snapshot-v1.json` | `a1e4b57f593e4e37d8c2089ea51a3a16ab958201780b714d96ff5d3beeb1163f` |
| `endpoint-method-shapes-v1.json` | `a1691c44ed8fd5a3cee213ea9bf8d2ee777169a945bcbcc5ec77d273fd713f01` |

## What was not copied

- crossterm/ratatui conversions (`from_crossterm`, `to_ratatui_buffer`, `BlitEncoder`).
- `AppState`, agent detection, PTY, persistence, `EndpointRegistry`, `EndpointCatalog`.
- The SSH client/server (`remote/*`).

## Compatibility rules

1. Variant and field order is frozen; `tests/frozen_v1.rs` compares the SHA-256 digests of the
   representative payloads published by the engine and the tag of each variant.
2. New behaviour arrives through `EndpointControl { kind, data }` (tag 20) with JSON tolerant of
   unknown fields; never through a new variant.
3. The JSON snapshot (`shell.snapshot.v1`) accepts unknown statuses and actions as `Unknown`.
4. A wide refactor (moving the core, creating an SDK) needs its own design review before editing.
5. When updating the reference commit: recompute the digests in the tables and run
   `just check-protocol`.
