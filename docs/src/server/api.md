# API and automation

REST, WebSocket, and MCP control the same live receiver as the interface. Changes affect every
connected client.

## Interactive reference

| Endpoint | Purpose |
|---|---|
| `/api/docs` | Swagger UI |
| `/api/openapi.json` | OpenAPI schema |
| `/api/ws` | WebSocket |
| `/mcp` | MCP over streamable HTTP |

The checked-in [OpenAPI schema](https://github.com/Newspicel/sdrminusminus/blob/main/openapi.json)
can generate clients without a running server. Swagger lists request bodies, responses, and errors.

When [authentication](configuration.md#shared-token-authentication) is enabled, these endpoints
require the shared token:

```sh
curl \
  -H "Authorization: Bearer $SDRMM_TOKEN" \
  http://receiver.local:8080/api/state
```

## REST resources

| Area | Routes and operations |
|---|---|
| Discovery and state | `/api/devices`, `/api/channeltypes`, `/api/state`, `/api/clients` |
| Live receiver | `/api/devicesets`, settings, channels, scanning, recording, playback |
| Workspaces | `/api/workspaces`, activate, apply, undo, redo, export, import |
| Saved setups | `/api/templates`, `/api/presets`, `/api/bookmarks` |
| Data | `/api/decoderlog`, exports, `/api/recordings`, downloads |
| Reference | `/api/bandplan/regions`, `/api/about`, `/api/doctor` |

Errors use JSON with `error` and optional `detail` fields.

## WebSocket events and streams

The WebSocket carries commands, state invalidations, decoder events, scanner progress, and binary
spectrum, audio, and video. Stream IDs belong to one connection; do not reuse them across clients.

Refetch durable state through REST after an invalidation. High-rate samples and events arrive on
the stream. Use the generated types and existing web client as the protocol reference.

## MCP

Connect an MCP client to `http://<server>:8080/mcp`, adding the bearer header when required.
Tools cover:

- Device discovery, opening, closing, and tuning.
- Channel creation and removal.
- Scanning, recording, decoded history, and spectrum snapshots.
- Measurement tools, antenna dimensions, and NanoVNA discovery, sweeps, and calibration.

MCP operates the shared live receiver with the same permissions as the interface.

## Generated-code workflow

Shared types live in `crates/wire`. After changing API types or server routes, run:

```sh
cargo xtask codegen
```

Commit `openapi.json` and the generated TypeScript declarations under `web/src/generated`.
`cargo xtask check` detects drift from the Rust source.
