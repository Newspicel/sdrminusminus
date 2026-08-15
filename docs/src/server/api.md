# API and automation

The server exposes the same receiver state used by the web interface through REST, WebSocket, and
MCP. The wire types live in one Rust crate and generate both the OpenAPI document and the
TypeScript client, which keeps the three surfaces aligned.

## Interactive reference

On a running server:

- Swagger UI: `/api/docs`
- OpenAPI JSON: `/api/openapi.json`
- WebSocket: `/api/ws`
- MCP streamable HTTP: `/mcp`

The repository also commits the generated
[`openapi.json`](https://github.com/Newspicel/sdrminusminus/blob/main/openapi.json) so clients can
be generated without a running receiver.

When authentication is enabled, Swagger, REST, WebSocket, and MCP require the shared token. See
[Configuration and security](configuration.md#shared-token-authentication).

## REST resources

The API covers:

| Area | Example routes |
|---|---|
| Discovery and state | `/api/devices`, `/api/channeltypes`, `/api/state`, `/api/clients` |
| Live receiver | `/api/devicesets`, device settings, channels, scanner, recording, playback |
| Workspaces | `/api/workspaces`, activate, apply, undo and redo |
| Reuse | `/api/templates`, `/api/presets`, `/api/bookmarks` |
| Data | `/api/decoderlog`, exports, `/api/recordings`, downloads |
| Reference | `/api/bandplan/regions`, `/api/about`, `/api/doctor` |

Use Swagger for exact request bodies, status codes, and schemas. Errors use a consistent JSON body
with `error` and optional `detail` fields instead of framework-specific plain text.

For an authenticated request:

```sh
curl \
  -H "Authorization: Bearer $SDRMM_TOKEN" \
  http://receiver.local:8080/api/state
```

## WebSocket events and streams

The WebSocket carries control commands, state invalidations, decoder events, scanner progress, and
binary spectrum, audio, and video frames. Stream-start events allocate identifiers per connection,
so clients should not assume that another connection uses the same stream ID.

Use the generated schema and existing web client as the protocol reference. REST remains the
authoritative way to fetch current durable state after an invalidation; high-rate samples and
events are streamed rather than stored in that state response.

## MCP

The MCP endpoint exposes receiver tools suitable for an automation client or assistant. Current
tools can:

- get state and discover devices or channel types;
- open, close, and tune devices;
- add or remove channels;
- start and stop scans;
- start or stop recordings;
- query decoded history;
- capture a spectrum snapshot.

Configure an MCP client for streamable HTTP at `http://<server>:8080/mcp` and attach the same
bearer authorization header when the server uses a token. MCP actions affect the live shared
receiver just like changes made in the interface.

## Generated-code workflow

After changing a REST type or route in `crates/wire` or `crates/server`, regenerate the checked-in
contract and TypeScript declarations:

```sh
cargo xtask codegen
```

This updates `openapi.json` and `web/src/generated`. `cargo xtask check` fails when either output
has drifted from the Rust source.
