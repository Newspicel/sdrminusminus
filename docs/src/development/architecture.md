# Architecture

The desktop app and headless binary share the Rust server and receiver engine. Both serve the
same React interface.

```text
React client ↔ REST / WebSocket / MCP ↔ Server control plane
                                              ↓ commands
Radio / network / recording → DSP engine → audio, events, spectrum, IQ
```

## Crate boundaries

| Crate | Responsibility |
|---|---|
| `sdrmm-dsp` | Allocation-free signal-processing primitives; no I/O or internal project dependencies |
| `sdrmm-modem` | Reusable modem algorithms depending only on DSP |
| `sdrmm-modem-test-support` | Modem measurement catalogs, simulations, and baseline tooling; tests and developer tools only |
| `sdrmm-wire` | Shared settings, DTOs, events, patch graph, and OpenAPI schemas |
| `sdrmm-device` | Hardware-independent device traits, capabilities, settings, and registry |
| `sdrmm-device-virtual` | Signal generators and SigMF playback |
| `sdrmm-device-rtlsdr` | Native RTL-SDR driver |
| `sdrmm-device-airspy`, `sdrmm-device-airspyhf` | Native Airspy drivers |
| `sdrmm-device-hackrf` | Native HackRF driver |
| `sdrmm-device-ad936x` | AntSDR, PlutoSDR and other AD936x boards, speaking iiod over ethernet or USB |
| `sdrmm-device-soapy` | Local hardware through SoapySDR |
| `sdrmm-device-sdrplay` | SDRplay RSP receivers through the vendor API, loaded at runtime |
| `sdrmm-device-rtltcp` | Direct `rtl_tcp` client |
| `sdrmm-device-spyserver` | Direct SpyServer client |
| `sdrmm-device-cr8` | Dragon Labs CR-8 through the vendor SDK, loaded at runtime |
| `sdrmm-device-array` | Already-open streams composed as logical lanes; no hardware opens |
| `sdrmm-channels` | Analog demodulators, protocol decoders, and their descriptors |
| `sdrmm-recorder` | SigMF writing, reading, scanning, and export |
| `sdrmm-engine` | Device supervision, channelization, scanning, streams, recording, and state snapshots |
| `sdrmm-server` | REST, WebSocket, MCP, persistence, band plans, auth, and embedded assets |

`apps/sdrmm` owns CLI configuration and process lifetime. `apps/desktop` starts the server on
an ephemeral loopback port and opens a Tauri WebView. Both isolate SoapySDR discovery in a
short-lived child process.

## One source of truth for wire types

Define REST bodies, WebSocket messages, settings, and patch types in `crates/wire`. OpenAPI
schemas derive from those types; `cargo xtask codegen` generates TypeScript declarations.

The client reads device capabilities, channel descriptors, and the node palette from the server,
keeping controls aligned with the running build.

## Data plane and control plane

The DSP path uses command queues for settings and bounded snapshots or buffers for output.
It performs no I/O, locking, allocation, or async work in hot processing.

The control plane owns HTTP handlers, SQLite, workspace reconciliation, subscriptions, recording
indexes, and serialization. It may allocate or block as needed.

Spectrum, audio, and video use binary WebSocket frames; browser audio is Opus-compressed.
Decoder events use typed JSON. Durable state is fetched through REST after WebSocket invalidations.

## Coherent processing

Each capture block carries its first sample index, including gaps from reported hardware loss.
Coherent processing taps each lane into a ring and selects the sample range common to all lanes.
After a gap, it advances to the next shared index before applying calibrated delays and weights.

Beamforming sums weighted lanes into a normal capture ring. Channels, recorders, and scopes
consume that beam through the ordinary single-lane path.

An Array node combines streams already owned by Device nodes. `device-array` provides logical
ingress lanes; the engine forwards corrected IQ, coordinates tuning, and handles member recovery.
The array adapter never opens hardware.

Media and recording outputs cross preallocated single-producer/single-consumer buffer pools.
Workers allocate transport payloads and publish them. Full queues never block DSP: media loss
is reported and recordings fail explicitly. Shutdown drains pending buffers. Some decoder
algorithms still allocate variable-sized results.

`channels` depends on `dsp`, `modem`, and `wire`. Shared modem algorithms belong in `modem`.
Allocation, throughput, and modem measurement tooling belongs in test-support crates outside the
application dependency graph. `cargo xtask check` enforces boundaries; `cargo xtask perf` checks
DSP throughput, allocation, decoder searches, and engine publication.

## Workspaces and live engine state

The workspace graph describes desired state. Applying it binds saved Device references to
discovered radios, restores settings, and reconciles channels and engine objects.

Saved references use backend, serial, key, and variant identity. Engine IDs are temporary and
never stored in the graph. Disconnected radios retain their nodes and settings until reconnection.

## Failure and backpressure

Queues are bounded. Overruns, dropped frames, recording faults, truncated exports, WebSocket lag,
and reconnection state surface to clients. Slow consumers cannot block capture or grow memory
without a limit.

## Testing layers

| Layer | Coverage |
|---|---|
| DSP | Analytic and golden vectors, allocation and throughput gates |
| Decoders | Recorded IQ and expected output, plus generated vectors |
| Engine | End-to-end virtual-device tests |
| Server | Handlers, persistence, streams, authentication, OpenAPI, codegen drift |
| Client | Unit tests and browser smoke flows |

CI builds release configurations without enumerating host radios. Test at the narrowest layer
that proves the behaviour, adding end-to-end coverage for cross-layer workflows.

## Standard tables and their provenance

Some decoder constants come directly from specifications:

| Constants | Location |
|---|---|
| DAB puncturing and protection profiles | `crates/channels/src/dab/protection.rs` |
| DAB phase reference | `crates/channels/src/dab/ofdm.rs` |
| DVB-S puncturing and Reed–Solomon parameters | `crates/channels/src/datv/dvbs.rs` |
| DVB-S2 LDPC accumulator addresses | `crates/channels/src/datv/dvbs2/tables/` |
| VL-SNR header sequence | `crates/channels/src/datv/dvbs2/vlsnr.rs` |

Sources are ETSI EN 300 401 (DAB), TS 102 563 (DAB+), EN 300 421 (DVB-S), EN 302 307-1 and -2
(DVB-S2/S2X), TS 102 606 (GSE), and ES 201 980 (DRM).

Table values were cross-checked against [welle.io](https://github.com/AlbrechtL/welle.io)
(GPL-2.0-or-later) and GNU Radio's [gr-dtv](https://github.com/gnuradio/gnuradio)
(GPL-3.0-or-later). This attribution concerns table verification, not copied decoder code.
The 7,378 DVB-S2 accumulator addresses were transformed mechanically. The VL-SNR 896-bit seed
and Walsh–Hadamard rows were transcribed from the standard; their sixteen generated patterns
match gr-dtv's tables.

Tests check independent properties such as puncturing density, polynomial roots, published CRC
values, and parity checks on encoded words. These checks help detect transcription errors.
