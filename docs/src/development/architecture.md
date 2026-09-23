# Architecture

The desktop app and the headless server run the same Rust server and receiver engine, and serve
the same React interface.

```text
React client ↔ REST / WebSocket / MCP ↔ Server control plane
                                              ↓ commands
Radio / network / recording → DSP engine → audio, events, spectrum, IQ
```

## Crates

| Crate | Responsibility |
|---|---|
| `sdrmm-dsp` | Allocation-free signal-processing primitives; no I/O or internal project dependencies |
| `sdrmm-modem` | Reusable modem algorithms depending only on DSP |
| `sdrmm-modem-test-support` | Modem measurement catalogs, simulations, and baseline tooling; tests and developer tools only |
| `sdrmm-wire` | Shared settings, DTOs, events, patch graph, and OpenAPI schemas |
| `sdrmm-device` | Hardware-independent device traits, capabilities, settings, and registry |
| `sdrmm-device-recording` | SigMF playback behind the Recording node |
| `sdrmm-device-siggen` | Test signals behind the Signal generator node |
| `sdrmm-device-virtual` | Synthetic radios for debug builds and tests |
| `sdrmm-usb-stream` | Bulk USB streaming shared by the native drivers |
| `sdrmm-device-rtlsdr` | Native RTL-SDR driver |
| `sdrmm-device-airspy`, `sdrmm-device-airspyhf` | Native Airspy drivers |
| `sdrmm-device-hackrf` | Native HackRF driver |
| `sdrmm-device-ad936x` | AntSDR, PlutoSDR and other AD936x boards, speaking iiod over Ethernet or USB |
| `sdrmm-device-soapy` | Local hardware through SoapySDR |
| `sdrmm-device-sdrplay` | SDRplay RSP receivers through the vendor API, loaded at runtime |
| `sdrmm-device-rtltcp` | Direct `rtl_tcp` client |
| `sdrmm-device-spyserver` | Direct SpyServer client |
| `sdrmm-device-sdrconnect` | SDRplay SDRconnect over its WebSocket API |
| `sdrmm-device-cr8` | Dragon Labs CR-8 through the vendor SDK, loaded at runtime |
| `sdrmm-device-array` | Already-open streams composed as logical lanes; no hardware opens |
| `sdrmm-channels` | Analog demodulators, protocol decoders, and their descriptors |
| `sdrmm-recorder` | SigMF writing, reading, scanning, and export |
| `sdrmm-orbit` | SGP4, pass prediction, and Doppler |
| `sdrmm-tools` | Antenna calculator and NanoVNA |
| `sdrmm-cps` | Codeplug reading, writing, and conversion |
| `sdrmm-test-support` | Allocation and timing helpers for tests |
| `sdrmm-engine` | Device supervision, channelization, scanning, streams, recording, and state snapshots |
| `sdrmm-server` | REST, WebSocket, MCP, persistence, band plans, auth, and embedded assets |

`apps/sdrmm` is the CLI and owns the process. `apps/desktop` starts the same server on a random
loopback port and opens it in a Tauri window. Both probe SoapySDR in a short-lived child process.

The dependency rules:

- `dsp` does no I/O and depends on no project crate.
- `modem` builds reusable modulation algorithms on `dsp` only.
- `channels` depends on `dsp`, `modem`, and `wire`.
- Measurement tooling lives in test-support crates, outside the application graph.

`cargo xtask check` enforces them.

## One source of truth for wire types

REST bodies, WebSocket messages, settings, and the patch graph are defined once in `crates/wire`.
OpenAPI derives from them, and `cargo xtask codegen` generates the TypeScript types.

The client builds its controls from what the server reports: device capabilities, channel
descriptors, and the node palette. A control never exists in the UI that the running build does
not support.

## Control plane and DSP plane

The DSP path takes settings through command queues and publishes through bounded snapshots and
buffers. It never does I/O, takes a lock, allocates, or awaits.

The control plane owns HTTP, SQLite, workspace reconciliation, subscriptions, and serialization.
It may block and allocate.

Media and recording data leave DSP through preallocated single-producer, single-consumer buffer
pools. Workers turn them into network payloads. A full queue never blocks DSP: lost media is
reported and recordings fail loudly. Some decoders still allocate for variable-size results.

Spectrum, audio, and video travel as binary WebSocket frames; browser audio is Opus. Decoder
events are typed JSON. After a WebSocket invalidation, clients fetch durable state over REST.

`cargo xtask perf` measures DSP throughput, allocation, decoder searches, and publication.

## Coherent processing

Every capture block carries the index of its first sample, so reported hardware gaps are visible.
Coherent processing buffers each lane and works on the sample range all lanes share. After a gap
it skips to the next shared index, then applies the calibrated delays and weights.

A beam is written to an ordinary capture ring, so channels, recorders, and scopes use it like any
single-lane source.

An Array node combines streams that Device nodes already own. `device-array` exposes them as
logical lanes. The engine forwards corrected IQ, coordinates tuning, and recovers members. The
array never opens hardware itself.

## Workspaces and the live engine

The workspace graph is the desired state. Applying it binds saved Device references to found
radios, restores their settings, and reconciles channels and engine objects.

Saved references identify a radio by backend, serial, key, and variant. Engine IDs are temporary
and never saved. A disconnected radio keeps its node and settings until it returns.

## Placing channels on radios

When Devices tune themselves, the control plane searches for tuning windows that cover the most
channels, using branch-and-bound. Each independently tunable stream gets one window. The search
respects wires, bandwidths, tuning ranges, manual settings, and pinned channels.

It stops after 50 ms or 100,000 search nodes and keeps the best answer found. Apply reports
include `placement.heard` and `placement.upper_bound`. When they are equal, coverage is proven
optimal for that snapshot. Ties favour existing placements.

Tests compare the search with an exhaustive oracle. For the larger comparison:

```sh
cargo test -p sdrmm-engine --lib compares_realistic_sizes -- --ignored --nocapture
```

## Failure and backpressure

Every queue is bounded. Drops, recording faults, truncated exports, WebSocket lag, and
reconnects are reported to clients. A slow consumer can never block capture or grow memory
without limit.

## Tests

| Layer | Tested with |
|---|---|
| DSP | Analytic and golden vectors, allocation and throughput gates |
| Decoders | Recorded IQ with expected output, generated vectors |
| Engine | End-to-end runs on virtual devices |
| Server | Handlers, persistence, streams, auth, OpenAPI, codegen drift |
| Client | Unit tests and browser smoke flows |

Test at the narrowest layer that proves the behaviour. Add end-to-end coverage when a change
crosses layers. CI never touches real radios.

## Tables from standards

Some decoder constants are copied from the standards:

| Constants | File |
|---|---|
| DAB puncturing and protection profiles | `crates/channels/src/dab/protection.rs` |
| DAB phase reference | `crates/channels/src/dab/ofdm.rs` |
| DVB-S puncturing and Reed-Solomon parameters | `crates/channels/src/datv/dvbs.rs` |
| DVB-S2 LDPC accumulator addresses | `crates/channels/src/datv/dvbs2/tables/` |
| VL-SNR header sequence | `crates/channels/src/datv/dvbs2/vlsnr.rs` |

Sources: ETSI EN 300 401 (DAB), TS 102 563 (DAB+), EN 300 421 (DVB-S), EN 302 307-1 and -2
(DVB-S2/S2X), TS 102 606 (GSE), and ES 201 980 (DRM).

The values were cross-checked against [welle.io](https://github.com/AlbrechtL/welle.io)
(GPL-2.0-or-later) and GNU Radio's [gr-dtv](https://github.com/gnuradio/gnuradio)
(GPL-3.0-or-later). No decoder code was copied. The 7,378 DVB-S2 accumulator addresses were
converted by script. The VL-SNR seed and Walsh-Hadamard rows were typed from the standard, and the
sixteen patterns they generate match gr-dtv.

Tests catch transcription errors by checking independent properties: puncturing density,
polynomial roots, published CRC values, and parity of encoded words.
