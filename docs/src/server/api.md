# API

REST, WebSocket, and MCP drive the same live receiver as the interface. Changes reach every
connected client.

| Endpoint | Serves |
|---|---|
| `/api/docs` | Swagger UI |
| `/api/openapi.json` | OpenAPI schema |
| `/api/ws` | WebSocket |
| `/mcp` | MCP over streamable HTTP |

The [OpenAPI schema](https://github.com/Newspicel/sdrminusminus/blob/main/openapi.json) is also in
the repository, for generating clients without a running server.

With a [token](configuration.md#token) set, send it on every request:

```sh
curl -H "Authorization: Bearer $SDRMM_TOKEN" http://receiver.local:8080/api/state
```

## REST

| Area | Routes |
|---|---|
| State and discovery | `/api/state`, `/api/devices`, `/api/channeltypes`, `/api/clients` |
| Live receiver | `/api/devicesets`: settings, channels, scanning, recording, playback |
| Workspaces | `/api/workspaces`: activate, apply, undo, redo, export, import |
| Saved setups | `/api/templates`, `/api/presets`, `/api/bookmarks` |
| Data | `/api/decoderlog`, `/api/recordings`, `/api/images`, downloads |
| Reference | `/api/bandplan/regions`, `/api/about`, `/api/doctor` |

Errors are JSON with `error` and an optional `detail`.

## WebSocket

The WebSocket carries commands, decoder events, scanner progress, and binary spectrum, audio, and
video. When it says some state changed, fetch that state again through REST. Stream IDs belong to
one connection. The web client in `web/src` is the reference implementation.

## MCP

Point an MCP client at `http://<server>:8080/mcp`, with the bearer header if a token is set. Its
tools open and tune radios, add and remove channels, scan, record, read decoded history, grab
spectrum snapshots, and run the [tools](../user-guide/tools.md) such as the NanoVNA.
