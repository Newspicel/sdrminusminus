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
| `sdrmm-device-siggen` | Signals for the Signal generator node |
| `sdrmm-device-virtual` | Synthetic radios for debug builds and tests |
| `sdrmm-usb-stream` | Bulk USB streaming shared by the native drivers |
| `sdrmm-device-rtlsdr` | Native RTL-SDR driver |
| `sdrmm-device-airspy`, `sdrmm-device-airspyhf` | Native Airspy drivers |
| `sdrmm-device-hackrf` | Native HackRF driver |
| `sdrmm-device-espsdr` | ESP32 running ESP-SDR firmware, over serial |
| `sdrmm-device-ad936x` | AntSDR, PlutoSDR and other AD936x boards, speaking iiod over Ethernet or USB |
| `sdrmm-device-soapy` | Local hardware through SoapySDR |
| `sdrmm-device-sdrplay` | SDRplay RSP receivers through the vendor API, loaded at runtime |
| `sdrmm-device-rtltcp` | Direct `rtl_tcp` client |
| `sdrmm-device-spyserver` | Direct SpyServer client |
| `sdrmm-device-sdrconnect` | SDRplay SDRconnect over its WebSocket API |
| `sdrmm-device-kiwisdr` | KiwiSDR over its WebSocket API |
| `sdrmm-device-cr8` | Dragon Labs CR-8 through the vendor SDK, loaded at runtime |
| `sdrmm-channels` | Analog demodulators, protocol decoders, their descriptors, and signal synthesis |
| `sdrmm-recorder` | SigMF writing, reading, scanning, and export |
| `sdrmm-orbit` | SGP4, pass prediction, and Doppler |
| `sdrmm-tools` | Antenna calculator and NanoVNA |
| `sdrmm-cps` | Codeplug reading, writing, and conversion |
| `sdrmm-test-support` | Allocation and timing helpers for tests |
| `sdrmm-engine` | Device supervision, channelization, scanning, streams, recording, and state snapshots |
| `sdrmm-tunnel` | Outbound relay client, device key, and pairing for app.sdrmm.com |
| `sdrmm-server` | REST, WebSocket, MCP, persistence, band plans, auth, phones, remote access, and embedded assets |
| `sdrmm-mobile-core` | Pairing, pinned TLS client, missions, and pose fusion for the phone apps |

`apps/sdrmm` is the CLI and owns the process. `apps/desktop` starts the same server on a random
loopback port and opens it in a Tauri window. Both probe SoapySDR in a short-lived child process.
`apps/ios` and `apps/android` are native phone apps on `sdrmm-mobile-core`.

The dependency rules:

- `dsp` does no I/O and depends on no project crate.
- `modem` builds reusable modulation algorithms on `dsp` only.
- `channels` depends on `dsp`, `modem`, `wire`, and `codec2`.
- `mobile-core` depends on `wire` only. Phone builds pull no engine, server, DSP, USB, FFmpeg,
  codec2, or SQLite crate.
- `test-support` and `modem-test-support` never reach the `sdrmm` dependency graph.

`cargo xtask check` enforces the dependency rules.

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
reported and recordings fail loudly. Some decoders allocate for variable-size results.

Spectrum, audio, and video travel as binary WebSocket frames; browser audio is Opus. Decoder
events are typed JSON. After a WebSocket invalidation, clients fetch durable state over REST.

`cargo xtask perf` measures DSP throughput, allocation, decoder searches, and publication.

## Arrays

Every capture block carries the index and time of its first sample, so hardware gaps are
visible. Each radio lane has a dormant tap. An Array node wakes the taps of its lanes; the
engine's array runtime aligns them in time, solves delay, phase and gain on the noise source or a
pilot, and corrects each lane with one filter. A lane that slips is realigned and reported.

Array processors run on the aligned block, inline or on worker threads. Their lane outputs, such
as a beam or a stitched band, are virtual lanes of the first member radio, so channels, recorders
and scopes use them like any radio lane.

The Array never opens hardware. Device nodes own the radios; the Array holds the tuning and gain of
its lanes while it exists.

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
| DVB-S2X LDPC addresses, constellations and interleavers | `crates/channels/src/datv/dvbs2/s2x/` |
| VL-SNR header sequence | `crates/channels/src/datv/dvbs2/vlsnr.rs` |
| DVB-T continual pilot and TPS carriers | `crates/channels/src/datv/dvbt/en300744.rs` |
| DVB-T2 pilots, reserved carriers, P1, L1 and LDPC tables | `crates/channels/src/datv/dvbt/t2/en302755/` |

Sources: ETSI EN 300 401 (DAB), TS 102 563 (DAB+), EN 300 421 (DVB-S), EN 302 307-1 and -2
(DVB-S2/S2X), EN 300 744 (DVB-T), EN 302 755 (DVB-T2), TS 102 606 (GSE), and ES 201 980 (DRM).

`s2x_tables.py` and `dvbs2_spec_check.py` in `crates/modem-test-support/scripts/` generate the
DVB-S2/S2X tables from the ETSI PDF text and check them against it. `dvbt_tables.py` does the same
for DVB-T and DVB-T2. The CI `etsi` job runs all three. The VL-SNR seed and Walsh-Hadamard rows
are typed from the standard.

Tests catch transcription errors by checking independent properties: puncturing density,
polynomial roots, published CRC values, and parity of encoded words.
