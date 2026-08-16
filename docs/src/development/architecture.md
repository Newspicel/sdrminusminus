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
