# API and automation

Everything the UI can do, the API can do — because the UI has no other way in. There is no
private channel between the frontend and the server, so a Python script, a `curl` one-liner
and an LLM agent are first-class clients.

## OpenAPI

| URL | What |
|---|---|
| `/api/docs` | Swagger UI |
| `/api/openapi.json` | The generated OpenAPI document |

The document is not written by hand and not extracted from a running server: `crates/wire`
defines every DTO, WebSocket message and settings struct with `serde` + `utoipa` derives, and
`cargo xtask codegen` calls `ApiDoc::openapi()` directly to emit `openapi.json`, then generates
`web/src/generated/schema.d.ts` from it. CI regenerates and fails on any diff, so the
document, the TypeScript client and the Rust types cannot drift.

Both `/api/docs` and `/api/openapi.json` stay reachable without a token — they describe the
API's shape, never its data.

## REST

The control plane. Resource-oriented, mirroring the state model.

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/api/state` | Full snapshot — the initial load |
| `GET` | `/api/devices` | Discovered hardware, recordings and virtual devices |
| `GET` | `/api/channeltypes` | Channel descriptors, for building an "add channel" UI |
| `POST` | `/api/devicesets` | Open a device → create a device set |
| `DELETE` | `/api/devicesets/{ds}` | Close it |
| `PATCH` | `/api/devicesets/{ds}/device` | Frequency, rate, gains, antenna, bandwidth, PPM, extras |
| `POST` | `/api/devicesets/{ds}/channels` | Add a channel |
| `PATCH` | `/api/devicesets/{ds}/channels/{ch}` | Typed per-channel settings |
| `DELETE` | `/api/devicesets/{ds}/channels/{ch}` | Remove a channel |
| `POST` | `/api/devicesets/{ds}/record` | Start or stop a SigMF recording |
| `GET`/`DELETE` | `/api/recordings`, `/api/recordings/{id}` | The recordings index |
| `GET`/`DELETE` | `/api/decoderlog` | Filtered decoder log |
| `GET` | `/api/decoderlog/export/{csv\|json}` | The same filter as a download |
| `GET`/`POST`/`DELETE` | `/api/presets`, `/api/presets/{id}`, `/api/presets/{id}/apply` | Presets |
| `GET`/`POST`/`DELETE` | `/api/bookmarks`, `/api/bookmarks/{id}` | Bookmarks |
| `POST` | `/api/devicesets/{ds}/scanner` | Start or stop a frequency scan |
| `GET`/`POST` | `/api/templates`, `/api/templates/{id}/apply` | Built-in station templates |
| `GET` | `/api/doctor` | Environment diagnostics, same report as `sdrmm --doctor` |
| `GET` | `/api/auth` | Whether this server requires a token (unauthenticated) |

`/api/docs` on your build is the authority on what is actually mounted.

Failures return a uniform body — `{"error": "...", "detail": "..."}` — including extractor
rejections, so a malformed request looks like every other error rather than like a framework
page.

### A session with curl

```sh
# What is out there? A device is addressed as "driver:key".
curl -s localhost:8080/api/devices | jq -r '.devices[] | "\(.driver):\(.key)\t\(.label)"'

# Open the signal generator
DS=$(curl -s -XPOST localhost:8080/api/devicesets \
      -H 'content-type: application/json' \
      -d '{"device_id":"virtual:siggen"}' | jq -r .id)

# Tune it and add an NFM channel on the built-in test carrier
curl -s -XPATCH localhost:8080/api/devicesets/$DS/device \
  -H 'content-type: application/json' \
  -d '{"center_hz":100000000,"sample_rate":2048000}'

curl -s -XPOST localhost:8080/api/devicesets/$DS/channels \
  -H 'content-type: application/json' \
  -d '{"settings":{"offset_hz":300000,"params":{"type":"nfm","settings":{}}}}'

# What has been decoded lately?
curl -s 'localhost:8080/api/decoderlog?kind=adsb&limit=20' | jq '.entries[].summary'
```

With a token, add `-H "Authorization: Bearer $TOKEN"`.

## WebSocket

One socket per client at `/api/ws`, carrying both push events and binary streams.

### Text frames — JSON

Server → client (`ServerEvent`), adjacently tagged as `{"type": ..., "data": ...}`:

| Event | Meaning |
|---|---|
| `Hello` | First frame after connect; carries the current state revision so a client can detect a gap |
| `StateChanged` | Something changed in `scope`; refetch the matching resource |
| `StreamStarted` / `AudioStreamStarted` | A subscribed binary stream is live, with its stream id |
| `StreamStopped` | A stream ended; carries the `kind`, since spectrum and audio ids come from different spaces |
| `Decoded` | One decoder frame, typed |
| `DecodedLost` | Decoder frames were dropped before reaching you |
| `ScannerUpdate` | Live scanner progress (M5) |
| `Error` | A non-fatal server-side error |

`StateChanged` is the **only** cache-invalidation mechanism. Scopes are `All`, `Devices`,
`DeviceSet(id)`, `Presets`, `Bookmarks`, `Recordings` and `DecoderLog`. Nothing polls; a
client that receives a scope refetches that resource and nothing else.

Decoder output does not use `StateChanged`. It travels on its own broadcast, because ADS-B
alone can emit hundreds of frames a second and a lagging control receiver resyncs with a
full-state refetch — a cost decode traffic must never be able to trigger. `StateChanged {
DecoderLog }` fires only when the *stored* log changes structurally.

Client → server (`ClientCommand`):

```json
{"type":"SubscribeSpectrum","data":{"device_set":0,"fps":20,"bins":2048}}
{"type":"UnsubscribeSpectrum","data":{"device_set":0}}
{"type":"SubscribeAudio","data":{"device_set":0,"channel":1}}
{"type":"UnsubscribeAudio","data":{"device_set":0,"channel":1}}
```

Subscriptions are per connection and the server clamps what it cannot serve, so a phone can
ask for 10 fps and 1024 bins while a desktop takes 30 fps and 4096.

### Binary frames

All little-endian. This is the one wire format written by hand on both sides — the deliberate
exception to codegen, defined once in `crates/wire` with a Rust encoder and a small
TypeScript decoder.

```text
header (16 bytes):
  u8  ver              protocol version, currently 1
  u8  kind             0 = SPECTRUM, 1 = AUDIO_OPUS, 2 = IQ_F32
  u16 stream_id
  u32 seq
  u64 timestamp        sample count since capture start

SPECTRUM payload:
  f64 center_hz
  f32 span_hz
  f32 db_min
  f32 db_max
  u16 n
  u8[n] bins           quantized over [db_min, db_max]

AUDIO_OPUS payload:
  u8  ch_layout        1 = mono
  u8[] opus            one Opus packet (20 ms), to the end of the frame
```

Demux on the pair `(kind, stream_id)`: spectrum stream ids are device-set ids (below
`0x8000`) and audio ids are allocated per connection from `0x8000..=0xFFFF`, so the kind is
what actually disambiguates them.

Audio timestamps count 48 kHz samples since the channel's audio started. A jump in the
timestamp is encoder-lag loss, surfaced so a client can conceal it instead of playing shorter
audio than it was sent.

Sample-count timestamps are in the protocol from day one because scanner accuracy, recording
alignment and any future multi-device coherence all need them, and adding them later would
be a breaking change.

## MCP

The server also speaks the Model Context Protocol over streamable HTTP at `/mcp` on the same
axum app, behind the same token.

| Tool | Does |
|---|---|
| `get_state` | The whole picture: device sets, channels, recordings, running scans. The ids every other tool takes come from here |
| `list_devices` | Discovered hardware, recordings and virtual devices |
| `list_channel_types` | Channel descriptors |
| `open_device` / `close_device_set` | Manage device sets |
| `tune_device` | Centre frequency, rate, gains |
| `add_channel` / `remove_channel` | Manage channels |
| `start_scan` / `stop_scan` | Drive the frequency scanner |
| `record` | Start or stop a SigMF recording |
| `query_decoder_log` | "Which aircraft did you see in the last hour?", pager messages, APRS stations |
| `spectrum_snapshot` | The current spectrum as data |

Every tool calls the same `Engine` and `Store` methods the REST handlers do and returns the
same `wire` types as structured JSON. An agent gets the same contract as every other client —
there is no parallel implementation to drift.

## Writing a client

- Fetch `GET /api/state` once, then keep it current from `StateChanged`. Do not poll.
- Treat the server as authoritative. Optimistic local updates are fine; reconciling against
  the next snapshot is mandatory.
- Generate your types from `/api/openapi.json` rather than transcribing them. Hand-writing a
  type that mirrors a Rust struct is a review-blocking offense in this repository, and it is
  bad advice outside it too.
