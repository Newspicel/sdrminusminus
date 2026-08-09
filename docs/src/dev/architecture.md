# Architecture

`PLAN.md` governs. This page is the working map; when the two disagree, the plan wins and the
page is wrong.

## Crate map

```
crates/wire         every REST DTO, WS message and settings struct — serde + utoipa
crates/dsp          DSP primitives: filters, mixers, resamplers, PLLs, FEC, sync, tone
crates/device       SdrDevice / DeviceDriver traits, capability model, driver registry
crates/device-soapy   SoapySDR backend (default feature)
crates/device-rtlsdr  native RTL-SDR backend (feature "rtl-native")
crates/device-hackrf  native HackRF backend (feature "hackrf-native")
crates/device-virtual signal generator + SigMF playback (always on)
crates/channels     ChannelRx trait + demods and decoders
crates/recorder     SigMF writer/reader
crates/engine       device sets, threads, rings, channel hosting, recording, scanner
crates/server       axum app as a LIBRARY: REST, WS hub, static assets, SQLite
apps/sdrmm          headless binary — a thin wrapper over crates/server
apps/desktop        Tauri v2 app embedding crates/server in-process
web                 React 19 + Vite + TypeScript 7 client
xtask               dev · codegen · check · test · fixtures · dist
fixtures            golden IQ fixtures
```

Dependency rules, enforced by review and by the manifests:

- `wire` depends on nothing internal, so anything may use it.
- `dsp` depends on nothing internal, has no I/O and no async.
- `channels` depends on `dsp` + `wire` only. Reference modulators for tests live behind a
  test-only feature so this stays true.
- Device backends are feature flags. `--no-default-features` must build a Soapy-free binary,
  and CI checks it on every change.
- `server` is a library. The headless binary and the desktop app are both thin wrappers, which
  is what makes "the desktop app runs the same code path as a remote browser" true rather than
  aspirational.

A new decoder touches one module in `channels`, one settings struct in `wire`, and optionally
one React panel. If it needs more than that, the design is wrong.

## Two planes

```
control plane (tokio)                    DSP plane (dedicated OS threads)
  REST handlers ── command queue ───────→ device thread → SPSC ring → DSP thread
  WS hub       ←── broadcast channels ──── spectrum, audio, decoded events
  SQLite                                    (no locks, no allocation, no async)
```

They never share mutable state directly. Settings changes travel as commands applied between
blocks; state leaves as snapshots over watch and broadcast channels. Blocking device I/O runs
on `spawn_blocking`, never on a tokio worker, and behind a per-set runtime mutex rather than
an engine-wide lock — one dongle's slow USB transaction must not stall every other device
set.

### The DSP thread

```
drain ring → DC/IQ correction
  ├─ spectrum tap: windowed FFT → dB → broadcast
  ├─ recorder tap: Arc-copy → bounded queue → writer thread (lossless)
  └─ per channel: DDC (NCO mix → polyphase decimate → fractional resample)
                  → channel filter → squelch → ChannelRx::process()
                       → audio PCM → Opus encoder thread → WS
                       → typed events → bounded queue → pump thread → broadcast + SQLite
```

The hot path allocates nothing in steady state and takes no locks. Everything that must not
lose data — recording, decoder events — goes through a bounded queue whose overflow is a
reported error, not a dropped sample. Everything that may lose data — spectrum, audio to a
slow client — is drop-oldest per connection.

Timestamps are sample counts from day one. They are cheap now and are the prerequisite for
scanner accuracy, recording alignment and any future multi-device coherence.

## Types and codegen

Every wire type is defined once, in `crates/wire`, with `Serialize`, `Deserialize` and
`ToSchema`. WebSocket messages are tagged enums, so the generated TypeScript is a
discriminated union that panels can exhaustively `switch` on.

```
crates/wire ──utoipa──→ openapi.json ──openapi-typescript──→ web/src/generated/schema.d.ts
                                     └─ openapi-fetch gives the typed client
```

`cargo xtask codegen` runs it without a server. `cargo xtask check` reruns it and fails on any
diff, so the checked-in artifacts cannot drift.

**Hand-writing a TypeScript type that mirrors a Rust struct is a review-blocking offense.**
The one deliberate exception is the binary frame layout, which has a Rust encoder and a small
TypeScript decoder on either side of a documented header.

## State model

`GET /api/state` is the full snapshot; after that, clients live off `StateChanged` events and
invalidate exactly the query keys the scope names. There is no polling anywhere in the client.

The server is authoritative. A client may update optimistically, but it reconciles against the
next snapshot — every optimistic path in the web client mirrors a server-side merge function
and is unit-tested against it.

High-rate binary streams bypass the query cache entirely: they land in Zustand and refs, then
in a canvas.

## Devices

`DeviceDriver::probe()` returns `DeviceInfo`s; `open()` yields an `SdrDevice`. A device
publishes a `Capabilities` document — frequency ranges, sample rates, named gain stages,
antennas, bandwidths, typed extra settings — and the client renders controls from it. Adding a
device setting requires no frontend code.

Settings are validated against capabilities *before* any hardware setter runs, and extras are
read back and verified afterwards, because at least one popular Soapy module returns success
for keys it ignores.

Probe results from several drivers are merged with the native driver winning and duplicates
collapsed by serial.

## Testing

No hardware in CI, ever.

| Layer | How |
|---|---|
| `dsp` | Analytic signals and golden vectors: filter responses, PLL lock, resampler alias floors |
| Decoders | A reference modulator per protocol in `channels::testgen` (test-only feature) plus expected output |
| Engine | End-to-end through `device-virtual`: siggen or a SigMF fixture → channel → assert audio or decoded events |
| Server | axum handler tests, an OpenAPI snapshot, and the codegen-drift gate |
| Web | `tsgo` strict, vitest for stores and utilities |

The decoder end-to-end runs deliberately use a *different* device rate from the channel rate,
so the DDC is exercised rather than bypassed. That is how the wideband-channel rejection rule
was found: ADS-B decoded nothing at 2.4 Msps, silently.

Reference modulators are written independently of the decoders they test — ADS-B's CPR,
Gillham and callsign encoders use closed forms where the decoder uses tables — so a mistyped
constant fails a test instead of cancelling out.
