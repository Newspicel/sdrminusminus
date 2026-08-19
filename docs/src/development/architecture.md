# Architecture

sdr-- has one real-time receiver engine and several thin delivery shells. The headless binary and
desktop app both construct the same server library; the browser and desktop window run the same
React client.

```text
                         control and state
┌──────────────┐       REST / WebSocket / MCP       ┌──────────────┐
│ React client │ ◀────────────────────────────────▶ │ Rust server  │
└──────────────┘                                    └──────┬───────┘
                                                           │ commands
                                                           ▼
┌──────────────┐   IQ blocks   ┌──────────────┐   ┌────────────────┐
│ SDR / file / │ ────────────▶ │ DSP engine   │ ─▶│ audio, events, │
│ network      │               │ and channels │   │ spectrum, IQ   │
└──────────────┘               └──────────────┘   └────────────────┘
```

## Crate boundaries

| Crate | Responsibility |
|---|---|
| `sdrmm-dsp` | Allocation-free signal-processing primitives; no I/O or internal project dependencies |
| `sdrmm-modem` | Reusable modem building blocks and measurement harness |
| `sdrmm-wire` | Shared settings, DTOs, events, patch graph, and OpenAPI schemas |
| `sdrmm-device` | Hardware-independent device traits, capabilities, settings, and registry |
| `sdrmm-device-virtual` | Signal generators and SigMF playback |
| `sdrmm-device-soapy` | Local hardware through SoapySDR |
| `sdrmm-device-sdrplay` | SDRplay RSP receivers through the vendor API, loaded at runtime |
| `sdrmm-device-net` | Direct `rtl_tcp` and SpyServer clients |
| `sdrmm-device-cr8` | Dragon Labs CR-8 through the vendor SDK, loaded at runtime |
| `sdrmm-device-array` | Separate radios framed as one multi-lane radio; counting only, no DSP |
| `sdrmm-channels` | Analog demodulators, protocol decoders, and their descriptors |
| `sdrmm-recorder` | SigMF writing, reading, scanning, and export |
| `sdrmm-engine` | Device supervision, channelization, scanning, streams, recording, and state snapshots |
| `sdrmm-server` | REST, WebSocket, MCP, persistence, band plans, auth, and embedded assets |

`apps/sdrmm` handles CLI configuration and process lifetime. `apps/desktop` binds the server to an
ephemeral loopback port and points a Tauri WebView at it. Both call
`sdrmm_device_soapy::enable_isolated_probes` before anything else, which re-executes them as a
short-lived probe helper whenever the engine looks for radios: vendor SoapySDR modules open USB
devices while searching, and a faulty one must cost a probe rather than the application.

## One source of truth for wire types

REST bodies, WebSocket messages, settings, and patch types are declared in `crates/wire`.
`utoipa` derives the OpenAPI schemas, and `cargo xtask codegen` generates the TypeScript client
types. A new field should not be re-declared independently in Rust, OpenAPI, and TypeScript.

The frontend asks the server for device capabilities, channel descriptors, and the node palette.
This keeps UI controls and connection rules aligned with the running build.

## Data plane and control plane

The real-time DSP path does not perform HTTP, database I/O, asynchronous work, or UI formatting.
It receives settings through command queues and publishes bounded snapshots and output buffers.
Hot processing avoids locks and allocation so timing is predictable under a continuous sample
stream.

The control plane can block or allocate where appropriate. It owns Axum handlers, SQLite,
workspace reconciliation, recording indexes, client subscriptions, and protocol serialization.

High-rate data uses binary WebSocket frames. Durable state stays behind REST, and WebSocket state
events tell clients which query scope to refetch. Decoder events are typed JSON because their rate
and structure suit it; audio is Opus-compressed before crossing to the browser.

## Coherent processing

An ordinary channel reads one lane. A coherent processor reads every lane of one radio at the same
moment, which needs a path of its own beside the per-lane one.

Every capture callback carries the index of its block's first sample, and a driver advances that
index by what the hardware reports dropped, so a device-side gap reaches the engine as a jump
rather than a silently shortened lane. When a coherent node is running, each lane also writes into
a tap ring with an explicit record of the samples it could not keep. The aggregator takes the
largest range every lane agrees on, discards up to the next common index when one of them lost
samples, applies the per-lane delay and weight the calibration solved, and hands the aligned
slices to the processors. It is subject to the same rules as `dsp_loop`: no locks, no allocation,
no async.

A processor may hand back a set of per-lane weights. The aggregator sums the lanes with them and
writes the result into an ordinary capture ring one past the radio's own lanes, so a channel,
recorder or spectrum subscription works on a beam without knowing it is one.

A patch **Array** node is framing rather than processing: the radios wired into it are opened as
one composite device by `sdrmm-device-array`, which numbers their lanes and fans settings back
out. Nothing above the device layer can tell a bank of receivers from a radio that came with
lanes of its own.

## Workspaces and live engine state

A workspace graph is desired state. Applying it binds durable Device nodes to currently discovered
devices, opens or closes engine objects, restores device settings, and creates the channels implied
by IQ connections.

The graph never stores engine IDs because those are allocated anew on each run. Device references
use backend, serial, key, and variant identity; channel binding follows graph order and source
ports. An unplugged receiver therefore leaves a meaningful disconnected graph instead of corrupting
the saved workspace.

## Failure and backpressure

The project favors bounded queues and explicit loss reporting over unbounded memory growth. Device
overruns, dropped decoder frames, recording faults, truncated exports, WebSocket lag, and reconnect
state surface to clients. A busy consumer should not be able to stall the capture thread or grow
the process indefinitely.

## Testing layers

- DSP primitives use analytic and golden-vector tests.
- Decoders use synthesized or recorded IQ fixtures with expected typed output.
- Engine tests run end-to-end through virtual devices.
- Server tests exercise handlers, persistence, WebSocket behavior, auth, and OpenAPI shape.
- Frontend tests cover pure view logic and stores; Playwright runs a complete browser flow.
- CI builds the Soapy release shape without enumerating host modules or requiring hardware.

Keep tests at the narrowest layer that proves a behavior, then add an end-to-end fixture when a
decoder or cross-layer workflow needs it.

## Standard tables and their provenance

Some decoders carry constants that a specification dictates rather than a derivation produces:
the DAB puncturing vectors and protection profiles (`crates/channels/src/dab/protection.rs`),
the DAB phase-reference table (`crates/channels/src/dab/ofdm.rs`), the DVB-S puncturing
patterns and Reed-Solomon parameters (`crates/channels/src/datv/dvbs.rs`), and the DVB-S2 LDPC
accumulator addresses (`crates/channels/src/datv/dvbs2/tables/`).

Those values come from the published standards — ETSI EN 300 401 for DAB, ETSI TS 102 563 for
DAB+, ETSI EN 300 421 for DVB-S, ETSI EN 302 307-1 for DVB-S2, ETSI ES 201 980 for DRM — and
were cross-checked against [welle.io](https://github.com/AlbrechtL/welle.io) (GPL-2.0-or-later)
and GNU Radio's [gr-dtv](https://github.com/gnuradio/gnuradio) (GPL-3.0-or-later), whose
transcriptions of the same tables are the widely deployed references. sdr-- is
GPL-3.0-or-later, so that lineage is compatible; no code was copied, only the standards'
constants were confirmed against them. The DVB-S2 accumulator tables are the one case where
the numbers were transformed mechanically rather than read, because there are 5 124 of them.

Each such table is covered by a test that checks it against a property the standard states
independently of the table itself — monotonic puncturing density, a generator polynomial that
vanishes at every root, a published CRC check value, a parity-check matrix that annihilates
every word its own encoder produces — so a transcription slip fails the suite rather than the
air.
