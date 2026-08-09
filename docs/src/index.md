# sdr--

sdr-- is a modular, client–server software-defined radio receiver.

A Rust server owns the hardware and does all of the signal processing: channelization,
demodulation, decoding, spectrum analysis, recording. A React client renders what the server
describes and never touches a sample of IQ. The same frontend ships two ways — as a Tauri
desktop app that embeds the server in-process, and as static assets served by the server
itself — so a Raspberry Pi in the attic and a browser on the couch run the identical UI.

## The shape of it

```
device → capture ring → DSP thread ──┬─ spectrum tap ──────────→ binary WS frames
                                     ├─ recorder tap ──────────→ SigMF on disk
                                     └─ per channel: DDC → demod/decoder
                                                              ├→ Opus audio (WS)
                                                              └→ typed decoder events (WS + SQLite)
```

Three properties follow from that picture, and most of the design falls out of them:

- **The server is authoritative.** All clients — the desktop window, three browsers, a Python
  script, an LLM agent over MCP — see the same state and converge through one WebSocket event
  stream. Nothing polls.
- **Control plane and data plane are separate.** REST mutates state; the WebSocket pushes
  events and binary streams. Spectrum and audio are throttled per connection and drop oldest,
  so a slow phone never stalls the DSP. Recording and decoder paths are lossless: a drop is a
  reported error, never silence.
- **Wire types exist once.** Every DTO, WebSocket message and settings struct lives in
  `crates/wire` with `serde` + `utoipa` derives; the TypeScript client is generated from the
  resulting OpenAPI document, and CI fails on drift. There are no hand-written frontend
  types mirroring Rust structs.

## What it is not

Scope is deliberately narrow (`PLAN.md` §1):

- **Receive only.** The device trait carries a TX half from day one, unimplemented. Transmit
  and RF-security tooling are a later phase behind an explicit controlled-environment gate.
- **No Windows.** Linux (x86_64 and aarch64) and macOS (arm64). Raspberry Pi 4 is the
  performance floor every DSP budget is measured against.
- **No browser-side DSP**, no WebUSB driving hardware from the client. The server does the
  work — that is the point of the split.
- **No multi-user accounts.** LAN trust plus an optional shared token
  ([Remote access and security](operating/security.md)).

## Status

Milestones M0–M4 are complete: walking skeleton, real hardware over SoapySDR, the listening
chain (NFM/AM/SSB/WFM with Opus audio), SigMF record and replay, and the first wave of
decoders (RDS, POCSAG, ADS-B, AIS, APRS/AX.25, RTTY, Morse) with a queryable decoder log.

M5 — frequency scanner, token auth, MCP server, template gallery, native RTL-SDR and HackRF
backends, `--doctor`, packaging, and this documentation — is in progress. Anything not yet
built, or built with limits worth knowing, is called out on the page that describes it.

`PROGRESS.md` in the repository is the authoritative record of what is built and tested;
`PLAN.md` is the source of truth for architecture and scope.

## Where to go next

- [Install and run](install.md) — build it, or run the headless binary
- [First run](first-run.md) — open a device, add a channel, listen (no hardware required)
- [Decoders](features/decoders.md) — what the wave-1 decoders do and how they are configured
- [API and automation](operating/api.md) — REST, the WebSocket protocol, MCP
- [Architecture](dev/architecture.md) — the crate map and why the boundaries sit where they do
