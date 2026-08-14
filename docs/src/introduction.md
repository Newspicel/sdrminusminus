<img src="icon.svg" alt="" width="96" height="96">

# sdr--

A modular, client–server software-defined radio receiver.

A Rust server owns the hardware and does all the DSP — channelization, demodulation, decoding,
spectrum, recording. A React client renders what the server describes and never touches a
sample of IQ. The same frontend ships two ways: as a Tauri desktop app that embeds the server
in-process, and as static assets served by the server itself, so a Raspberry Pi on the roof and
a browser on the couch run the identical UI.

## Where things are

The server is split so that each crate has one job and the hot signal path has no I/O in it:

| Crate | Owns |
|---|---|
| `dsp` | Signal primitives. No I/O, no internal dependencies. |
| `wire` | Every DTO, WebSocket message and settings type, once. The TypeScript client is generated from it. |
| `device-*` | Hardware backends, each behind its own feature flag. `device-virtual` is the one CI uses. |
| `channels` | One module per demodulator/decoder. |
| `engine` | The DSP plane: devices in, channels and spectrum out. |
| `server` | HTTP, WebSocket, OpenAPI and the embedded UI. A library, not a binary. |

`apps/sdrmm` is the headless binary and `apps/desktop` is the Tauri shell; both are thin
wrappers over `server`.

## Reference

- **API**: `/api/docs` on any running server — Swagger UI over the generated OpenAPI document.
- **Architecture and design principles**: [``](https://github.com/Newspicel/sdrminusminus/blob/main/)
  in the repository.
- **Feature list**: [`implemented behavior`](https://github.com/Newspicel/sdrminusminus/blob/main/implemented behavior).
