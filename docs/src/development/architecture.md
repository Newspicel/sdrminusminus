# Architecture

The headless binary and desktop app use the same Rust server library and receiver engine.
The browser and desktop window run the same React client.

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
| `sdrmm-modem` | Reusable modem algorithms depending only on DSP |
| `sdrmm-modem-test-support` | Modem measurement catalogs, simulations, and baseline tooling; tests and developer tools only |
| `sdrmm-wire` | Shared settings, DTOs, events, patch graph, and OpenAPI schemas |
| `sdrmm-device` | Hardware-independent device traits, capabilities, settings, and registry |
| `sdrmm-device-virtual` | Signal generators and SigMF playback |
| `sdrmm-device-rtlsdr` | Native RTL-SDR driver |
| `sdrmm-device-hackrf` | Native HackRF driver |
| `sdrmm-device-soapy` | Local hardware through SoapySDR |
| `sdrmm-device-sdrplay` | SDRplay RSP receivers through the vendor API, loaded at runtime |
| `sdrmm-device-net` | Direct `rtl_tcp` and SpyServer clients |
| `sdrmm-device-cr8` | Dragon Labs CR-8 through the vendor SDK, loaded at runtime |
| `sdrmm-device-array` | Already-open streams composed as logical lanes; no hardware opens |
| `sdrmm-channels` | Analog demodulators, protocol decoders, and their descriptors |
| `sdrmm-recorder` | SigMF writing, reading, scanning, and export |
| `sdrmm-engine` | Device supervision, channelization, scanning, streams, recording, and state snapshots |
| `sdrmm-server` | REST, WebSocket, MCP, persistence, band plans, auth, and embedded assets |

`apps/sdrmm` handles CLI configuration and process lifetime. `apps/desktop` binds the server to an
ephemeral loopback port and points a Tauri WebView at it. With SoapySDR enabled, both call
`sdrmm_device_soapy::enable_isolated_probes` during startup. Discovery re-executes the binary as
a short-lived probe helper, isolating the application from crashes in vendor discovery code.

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

An ordinary channel reads one lane. A coherent processor needs aligned samples from every lane
of one source.

Each capture block carries its first sample index. Drivers advance the index over hardware-reported
losses, preserving gaps. While coherent processing is active, each lane also writes to a tap ring
that records any samples it cannot retain.

The aggregator finds the largest sample range common to all lanes. After a gap, it discards samples
up to the next common index, then applies calibrated delays and complex weights. Like `dsp_loop`,
it runs without locks, allocation, or async work.

Processors can return beamforming weights. The aggregator sums the weighted lanes into an ordinary
capture ring after the source's physical lanes. Channels, recorders, and spectrum subscriptions
consume this beam through the normal single-lane path.

A patch **Array** node composes streams from existing Device nodes. `sdrmm-device-array` supplies
logical ingress lanes; the engine forwards the members' corrected IQ through bounded rings and
coordinates tuning. Device nodes retain ownership of their radios and channels. The engine handles
membership changes and member recovery without opening hardware through the array adapter.

Channel media, spectrum, recording blocks, and coherent results cross preallocated SPSC buffer
pools before workers allocate transport payloads or call broadcast senders. Saturation never waits
on the DSP thread: media loss is reported, and recordings fail explicitly. Worker shutdown drains
pending buffers. Decoder algorithms may still allocate their own variable-sized results.

`channels` depends on `dsp`, `modem`, and `wire`; protocol-independent modem algorithms stay in
`modem`. `sdrmm-test-support` contains allocation and throughput measurement helpers.
`modem-test-support` owns the modem measurement harness and JSON baseline tooling. Neither
appears in the application's normal dependency graph. `cargo xtask check` enforces these boundaries;
`cargo xtask perf` runs the DSP allocation and throughput gates, full-slot decoder search baselines,
and engine publication checks.

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
