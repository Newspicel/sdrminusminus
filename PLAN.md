# sdr-- — Project Plan

A modular, client–server SDR application. Rust server (all DSP/decoding happens here),
web-technology client (React) shipped as a Tauri desktop app *and* served directly by the
server for browser access. Runs as a single local app on a laptop, or split with the server
on a Raspberry Pi and the client anywhere on the network.

Working name: **sdr--** ("sdrminusminus"), crate/binary prefix `sdrmm`.
Personal project (not planned for public release). License: **MIT**.

> **Scope of this document (slimmed post-M5).** M0–M5 shipped; the workspace and the running
> code now document their own structure, and `PROGRESS.md` records what was built and how it
> was verified. This plan keeps what code cannot say: the idea (§1–§2), the binding rules and
> invariants, everything not yet built (§8a, §8b, §12a, §13, §19, §20), and the decision log
> (§18). Section numbers are stable — code comments cite them as `PLAN §N`.

---

## 1. Goals & non-goals

### Goals
- **RX first.** Full receive chain: device → channelizer → demodulators/decoders → audio + data.
- **Feature target: SDRangel's RX feature set**, reached incrementally via a stable plugin API
  (see §13 roadmap). SDRangel is ~a decade of plugins — parity is a roadmap, not a milestone.
  The architecture is judged by how cheaply a new decoder can be added.
- **Backend-driven:** the server is the single source of truth for state, settings, and type
  definitions. The client renders what the server describes. Adding a device setting or a new
  channel type requires zero hand-written frontend DTOs.
- **Default hardware:** RTL-SDR and HackRF. Everything else via SoapySDR for free.
- **Many radios at once:** unlimited simultaneous device sets (SDRangel-style), and
  cross-device features on top (scanner spanning devices, multi-VOR fix, diversity,
  DoA/TDoA later).
- **Coherent arrays as a first-class, hardware-agnostic concept:** N synchronized
  receivers grouped into a *coherent device set*. KrakenSDR is just one way to populate
  it (5× RTL-SDR on a shared clock); the same abstraction accepts any clock-synced set —
  future multi-channel SDRs (e.g. Dragon/DragonOS-class boards), a phase-locked bank of
  RTL/Airspy dongles, or an incoming coherent network stream. The DSP (calibration, DoA,
  passive radar) is written against the *array abstraction*, not against Kraken.
- **Platforms:** Linux (x86_64 + aarch64) and macOS (arm64). **Raspberry Pi 4 is the
  performance floor** — every DSP budget decision is measured against it. Windows explicitly ignored.
- **Decoders in Rust** — self-written where reasonable; where a proven library just works
  (digital voice codecs, DAB+ audio), use it via FFI without ceremony (each dependency
  vetted for Linux-arm64 + macOS support, see §13 table).

### Non-goals (for now)
- TX (architecture leaves room: the device trait has a TX half from day one — see §12a for
  exactly where it stops today).
- Windows support.
- Browser-side DSP / WebUSB driving hardware from the client. The server does the work — that's the point.
- Multi-user accounts (LAN-trust + optional token instead, see §12).

---

## 2. Architecture overview

```
┌─────────────────────────── client (React, one codebase) ───────────────────────────┐
│  Tauri desktop app (spawns embedded local server OR connects to remote)            │
│  Browser on any LAN device (UI served by the server itself)                        │
│                                                                                    │
│  TanStack Query ◄── generated typed client ◄── openapi.json                        │
│  WebSocket: state events (JSON) + binary streams (spectrum, audio, IQ taps)        │
│  WebGL2 waterfall/spectrum · AudioWorklet playback · MapLibre for geo decoders     │
└──────────────────────────────────────┬─────────────────────────────────────────────┘
                          REST (control) + one WebSocket (push/streams)
┌──────────────────────────────────────┴─────────────────────────────────────────────┐
│  sdrmm-server (axum)                                                               │
│   REST API (utoipa/OpenAPI) · WS hub · static UI assets · persistence (SQLite)     │
├────────────────────────────────────────────────────────────────────────────────────┤
│  sdrmm-engine — one "device set" per opened device                                 │
│   device thread → ring buffer → DSP thread:                                        │
│     IQ/DC correction → spectrum tap → N × [DDC (shift+decimate) → channel plugin]  │
│   commands in via queue · state/telemetry out via broadcast                        │
├────────────────────────────────────────────────────────────────────────────────────┤
│  sdrmm-channels: ChannelRx plugins (NFM, AM, SSB, ADS-B, POCSAG, …)                │
│  sdrmm-dsp: filters, mixers, resamplers, PLLs, FEC — pure Rust, no I/O             │
├────────────────────────────────────────────────────────────────────────────────────┤
│  sdrmm-device: SdrDevice trait + capability model + shared capture machinery       │
│   backends: soapy (default) · rtlsdr-native · hackrf-native · file/siggen (virtual)│
└────────────────────────────────────────────────────────────────────────────────────┘
```

Key properties:
- The **server is authoritative**. All clients (Tauri window, three browsers, a Python script)
  see the same state and converge via the WS event stream.
- **Control plane and data plane are separate.** REST mutates; the WS pushes. High-rate data
  (spectrum, audio) is binary and per-client throttled; low-rate decoder output is typed JSON.
- The **Tauri app and the browser load the same UI from the same origin model** — the desktop
  app just also knows how to spawn a local server and manage saved remote connections.

---

## 3. Repository layout

Cargo workspace + pnpm workspace in one repo; the workspace manifests and each crate's
`lib.rs` header are the layout documentation now. In one line:
`crates/` — `wire` · `dsp` · `usb-stream` · `device` · `device-soapy` · `device-rtlsdr` ·
`device-hackrf` · `device-virtual` · `engine` · `channels` · `recorder` · `server` —
plus `apps/` (`sdrmm` headless, `desktop` Tauri), `web/` (React; `src/generated/` is
OpenAPI output, checked in, CI-verified), `xtask/`, and `fixtures/`.

Modularity rules (binding):
- `dsp` depends on nothing internal; `channels` depends on `dsp` + `wire` only.
- `wire` depends on nothing internal (serde/utoipa only) so anything can use it.
- A new decoder touches: one module in `channels`, its settings struct in `wire`,
  optionally one React panel. Nothing else.
- Device backends are feature flags; `--no-default-features --features rtl-native` must
  build a Soapy-free binary (matters for minimal Pi images).
- `server` is a library (`start(cfg) -> ServerHandle`); the binaries are thin wrappers.
- Anything a second radio backend would have to write again lives in `crates/device` (§18),
  which stays I/O-free by default (USB transport behind its `usb` feature).

---

## 4. Shared types & codegen (the "no two DTOs" pipeline)

Everything on the wire is defined **once, in Rust**, in `crates/wire` — REST bodies, WS
message enums, channel settings, capability descriptors — with serde + utoipa derives. WS
messages are tagged enums → discriminated unions in TypeScript, exhaustively `switch`-able.
`cargo xtask codegen` regenerates `web/src/generated/` from the OpenAPI; CI re-runs codegen
and fails on diff, so generated code can never drift.

Rules:
- Hand-writing a TS interface that mirrors a Rust struct is a review-blocking offense.
- Binary frame layouts (§5) live in `wire` as documented consts with a Rust encoder +
  one small TS decoder — the one deliberate exception to "generated only".
- `openapi.json` + Swagger UI served at `/api/docs` = free scripting API (Python/curl),
  same story as SDRangel's REST API but typed end-to-end.

---

## 5. Transport & protocols

### REST (control plane)
`axum` + `utoipa`, resource-oriented, mirroring the state model: state snapshot, device
probe, device sets with nested device/channels/record/scanner actions, recordings, decoder
log (+ CSV/JSON export), presets, bookmarks, templates, auth, clients, doctor. The
authoritative, always-current surface is the OpenAPI at `/api/docs` — this document no
longer mirrors the route list. Two recorded route decisions: the record endpoint carries no
format field until a second format exists (M3, YAGNI — SigMF cf32 is fixed), and the
decoder-log export format is a path segment, not a query field, because
`serde_urlencoded` cannot flatten the filter struct shared by all three log endpoints (M4).

### WebSocket (push + data plane) — one socket per client, `/api/ws`
Text frames = JSON `ServerEvent` / `ClientCommand` (from `wire`):
- `StateChanged { scope }` → client invalidates matching TanStack Query keys.
  This is the *only* cache-invalidation mechanism; no polling.
- Decoder output events travel as typed JSON (`Decoded { DecodedRecord }`) on their **own
  broadcast**, not the `StateChanged` control stream: ADS-B alone can emit hundreds of
  frames a second, and a lagging control receiver resyncs with a full-state refetch — a
  cost that must never be triggered by decode traffic. Clients append them to a local ring;
  `StateChanged { DecoderLog }` fires only when the *stored* log changes structurally. The
  DSP plane hands frames over a bounded queue and the drops are reported as
  `DecodedLost { count }` (M4 decision).
- Scanner progress (`ScannerUpdate`) is its own event for the same reason: a running sweep
  retunes several times a second. `DeviceSet.scanner` in the snapshot stays authoritative;
  a `StateChanged` fires when a scan starts or stops (M5).
- Stream subscriptions are per-connection — a phone can ask for 10 fps/1024 bins while a
  desktop gets 30 fps/4096.

Binary frames (kinds: SPECTRUM, AUDIO_OPUS, IQ_F32; layout documented in `wire/frame.rs`,
§4) carry **sample-count timestamps from day one** — cheap now, required later for scanner
accuracy, recordings alignment, and (far future) multi-device coherence.

Backpressure: UI streams are drop-oldest per connection (a slow phone never stalls the DSP);
recording and decoder paths are lossless (bounded queue → hard error, never silent loss).

### MCP server
The server also speaks **MCP** (`rmcp`, streamable-HTTP at `/mcp`, same token auth):
list/tune devices, create and configure channels, drive the scanner, query decoder logs,
grab spectrum snapshots, start/stop recordings. It reuses the same typed service layer as
REST — LLM agents get the same contract as every other client, no parallel implementation.
(Shipped at M5; implementation decisions in §18.)

---

## 6. Device layer

`crates/device` owns the trait model — `DeviceDriver` (probe/open) and `SdrDevice`
(capabilities, settings, rx_start/rx_stop, duplex, gated TX half) — plus the shared
machinery every backend would otherwise re-write (§18). The traits themselves live in code;
the rules below are what binds.

**Duplex is a device rule, not a backend's.** `Duplex` + `DuplexState` in `crates/device` own
it once: an RTL-SDR is `RxOnly`, the HackRF is `Half` (one transceiver, one data path), a
USRP/Lime/Pluto-class radio is `Full`. A direction is claimed while its stream runs and released
when it ends — releasing one never touches the other, so tearing down a capture cannot silence a
transmit burst. Backends supply the mechanism (which register selects which path); whether they
are *allowed* to is decided here, and is unit-tested without hardware.

**Backends supply what differs, and nothing else.** A capture is `CaptureRadio` (arm/disarm the
radio's stream) plus a `SampleConverter` (its ADC coding); the thread, the tier-1 restart
supervisor, the silent-stall detector, the block splitting and the stop/join teardown are
`crates/device`'s and shared. Adding a USB SDR should be a driver, a 256-entry table, a
capability translation and an `arm`.

`Capabilities` is the backbone of backend-driven UI: frequency ranges, sample rates,
named gain stages with ranges, antennas, bandwidths, plus **typed extra settings**
(bool/enum/range with labels). The client auto-renders controls from this — a new
device setting needs zero frontend work. Well-known settings (frequency, gain, rate)
get first-class custom UI; the rest render generically.

### Backends
- **`device-soapy`** (default in dev/server builds): instantly covers Airspy, SDRplay,
  LimeSDR, PlutoSDR, BladeRF, USRP… wherever a Soapy module exists. A documented C
  dependency — never a launch dependency of release artifacts (§15 packaging rule).
  Binding decided at M1 (§18: `soapysdr` 0.5, seify rejected).
- **`device-rtlsdr` / `device-hackrf`** (native, features `rtl-native`/`hackrf-native`):
  each owns its radio driver in-tree under `src/driver/`, on the shared `usb-stream`
  transport (§18). They exist to expose what Soapy hides or half-hides — per-stage gain
  tables, exact PPM, bias-T, direct sampling (HF!), sweep mode — and to make release
  binaries self-contained.
- **`device-virtual`** (always on): signal generator + SigMF/IQ file playback. This is how
  CI, the demo mode, and decoder golden tests run without hardware.
- **Later network/audio backends** (Phase 4, each small): `device-audio` (sound-card/rig
  audio via `cpal`), KiwiSDR network client, rtl_tcp/SpyServer client (§13). Everything
  else rides Soapy.

Same physical device visible via both Soapy and native: native driver claims priority in
the probe merge; duplicates are collapsed by serial.

### Coherent arrays (`crates/device` — array abstraction, not a driver)
A **`CoherentArray`** groups M `SdrDevice`s that share a clock and can be sample-aligned. It
exposes: per-channel gain/phase calibration, a sync/alignment step (noise-source or pilot
correlation), and a combined multi-channel `RxSink` delivering time-aligned `cf32` lanes with
common timestamps (§5). Populated by:
- **KrakenSDR** — 5× RTL-SDR + noise-source switching. Consume its Heimdall DAQ network
  stream first (already coherent/calibrated); direct hardware drive later.
- **A generic synced bank** — any N receivers on a shared reference clock (self-alignment
  via the calibration step). This is the path for *future* multi-channel SDRs
  (Dragon/DragonOS-class boards, USRP MIMO, phase-locked RTL/Airspy banks).
- **Network coherent source** — aligned multi-lane IQ from another sdr-- node or a DAQ.

Everything downstream (DoA via MUSIC/ESPRIT, passive radar, beamforming, diversity combine)
targets `CoherentArray`, so adding new coherent hardware later is a backend, not a rewrite.
Phase 4+.

---

## 7. DSP engine

### Data flow (per device set)
```
[device thread]  blocking reads → SPSC ring (rtrb, cf32)
[dsp thread]     drain ring → DC/IQ imbalance correction
                 ├─ spectrum tap: overlap FFT (rustfft) → avg/peak-hold → u8 bins → WS hub
                 ├─ recorder tap (SigMF sink, lossless)
                 └─ per channel: DDC (NCO mix → polyphase decimate) → ChannelRx::process()
                        outputs: audio (→ Opus encode → WS), events (→ WS), IQ taps
```

### Rules
- Sample format `Complex<f32>` end-to-end; conversion (u8/i8/i16) at the device edge only.
- **No locks or allocation in the hot path.** Settings changes go through a command queue,
  applied between blocks; state snapshots out via `arc-swap`/watch channels.
- Control plane (tokio) and DSP (dedicated OS threads) never share mutable state directly.
- Start with all channels of a device set on one DSP thread (a Pi 4 handles several narrow
  channels fine); the design allows moving hot channels to their own threads later. A
  polyphase channelizer (PFB) is a marked future optimization for many-channels-same-band —
  not built until profiling demands it.
- Per-block processing (e.g. 8–16k samples) — batch, SIMD-friendly (`f32` slices, let
  autovectorization + `std::simd` where it matters; benchmark on a real Pi, see §14).

The primitive inventory lives in `crates/dsp` — pure Rust, no I/O, external dependency only
`rustfft`/`realfft` — and every primitive ships with golden-vector tests (§14): this crate
is the foundation everything else trusts.

---

## 8. Channel plugin system

`ChannelRx` in `crates/channels` is the plugin surface: consume decimated IQ, produce
audio/events/low-rate IQ taps; a `ChannelDescriptor` carries id, name, required bandwidth
and the settings-schema reference that drives the "add channel" UI. The trait and the
registry live in code — the rules:

- **Static registry** (feature-gated inventory at compile time). No dynamic `.so` loading —
  Rust has no stable ABI and it's not worth the pain. If third-party plugins ever matter,
  the escape hatch is WASM (wasmtime) or out-of-process via the network protocol — decided later.
- Each channel type owns: DSP module in `channels`, settings struct in `wire`
  (→ generated TS type), optional dedicated React panel (ADS-B table+map, RDS display…);
  channels without one get the generic schema-rendered settings form + audio/scope panel.
- Channel taxonomy mirrors SDRangel: channels (attached to a device set, consume IQ) vs
  **features** (device-independent: satellite tracker, map, rotator control, APRS collector) —
  features are server-side services with the same settings/codegen treatment, minus the DSP.

---

## 8a. Frequency-allocation database ("what is this frequency?")

A first-class **band-plan / allocation layer** overlaid on the spectrum and searchable — hover
or click any frequency and see who it's allocated to, the service name, and a suggested mode.

- **Data model:** regions → entries `{ start_hz, end_hz, service, allocation, mode_hint,
  channel_step?, notes, source, region }`. Stored in SQLite, shipped as versioned seed data,
  user-extendable and override-able (local edits layered over the shipped set).
- **Layered scopes**, most-specific-wins at a given frequency:
  1. **World** — ITU allocation baseline (ITU Regions 1/2/3) + common global services
     (airband, marine VHF, ADS-B, AIS, ISM, satellite bands).
  2. **Germany** — Bundesnetzagentur **Frequenzplan** as the authoritative national layer
     (source: data.bundesnetzagentur.de Frequenzplan PDF/dataset — parsed into our schema by
     an `xtask` importer; we ship the derived table, not the document).
  3. **Future national layers** — US (FCC allocation table / ULS), UK (Ofcom), and more,
     each a pluggable importer producing the same schema. Region chosen in settings (or auto
     from GPS §GPS-heatmap).
- **UI:** band ruler under the spectrum (colored allocation blocks), click-to-identify
  popover, a searchable **band explorer** ("show me marine VHF", "70cm ham"), and one-click
  "tune here with the suggested mode" — which is also how templates (§10) and the beginner
  band-plan explorer are powered. Amateur band plans (IARU R1) included as an overlay.
- **Importers are `xtask` subcommands**, re-runnable when authorities publish updates;
  provenance (`source`, version, retrieved-date) stored per row so entries are auditable.

---

## 8b. HackRF / PortaPack / Flipper Zero feature coverage (RX)

Goal: whatever the **HackRF + PortaPack (Mayhem firmware)** and **Flipper Zero Sub-GHz** can
do *on the receive side*, sdr-- can do too — usually better, because we have a real CPU,
big FFTs, logging, and maps. TX-only tricks are noted and deferred with the rest of TX.

**Directly covered (map to existing/planned channels & features):**
- Wideband spectrum sweep / "close-call"-style strongest-signal finder → HackRF sweep +
  scanner (§13); **signal-strength "hunt" mode** (Geiger-style audio/visual as you near a
  transmitter — great for fox-hunting and finding rogue emitters).
- ADS-B, AIS, POCSAG, ACARS, RDS, APRS, weather/TPMS/ISM sensors (rtl_433-style),
  radiosonde → all in §13.
- **Sub-GHz OOK/ASK/FSK remotes** (315/433/868/915 MHz: garage doors, TPMS, weather stations,
  doorbells, many key fobs) → a generic **OOK/FSK capture+decode channel** (**shipped**, wave 2)
  that recognizes common encodings (pulse-width — the PT2262/EV1527/Princeton family — and
  Manchester) and logs frames; unknown signals get raw timing capture for inspection. This is
  the Flipper "read Sub-GHz" experience. The chip is deliberately *not* named: an EV1527's 24
  data bits and a PT2262's 12 tri-state symbols are the same pulse train, so a frame that fits
  carries both readings and the operator decides.
- **RF replay-capture (RX half):** record the exact IQ of a burst (garage remote, sensor) to
  the IQ time machine / SigMF, annotate and analyze it. (Re-transmitting it is TX — deferred.)
- Morse, RTTY, SSTV(RX), radio clock, VOR/ILS — already planned.

**Nature/analysis features (PortaPack parity):** waterfall, audio RX (AM/NFM/WFM/SSB), signal
recording, frequency manager/bookmarks, band-plan awareness → all core sdr-- already.

**Explicitly TX / deferred** (documented so scope is honest): sub-GHz *replay/brute/jam*,
BLE/OOK *spam*, RF *transmit* of any kind, "spoof" tools. These need TX — which exists at the
device layer and is gated shut above it (§12a) — and several are legally restricted.

A **"Sub-GHz workbench" template** (§10) bundles the OOK/FSK channel + capture + a decoder log
into a one-click Flipper-replacement layout.

---

## 9. Streaming specifics

The pipelines are built (spectrum, Opus audio, IQ taps); what binds is the shape of what goes
over the wire:

- **Spectrum:** server-side averaged FFT, reduced to ≤4096 display bins by max-decimation and
  quantized to u8 over an adaptive dB window carried in the frame header. Rate and bin count
  are per-connection, never global — a phone and a desktop watching the same device set ask
  for different things. Zoom is a client-side crop; a true zoom-FFT belongs to the channel
  analyzer, not the device spectrum.
- **Audio:** demods emit 48 kHz PCM → Opus (20 ms frames) → WS binary. Mixing is client-side:
  the server ships streams, not a mix, so N listeners on one channel cost one encode. Browser
  autoplay policy means playback unlocks on a user gesture.
- **Channel analyzer taps:** hard-decimated IQ/scope frames only. **Full-rate IQ never goes to
  the browser** — that is what recordings are for.

---

## 10. Client

One React codebase, loaded identically by the Tauri window and by any browser on the LAN; the
desktop app's only extra job is spawning a local server and remembering remote connections.
The stack is settled and lives in `web/package.json` + CLAUDE.md, not here. What binds:

- **Server state discipline:** TanStack Query is the *only* holder of REST data; WS
  `StateChanged` events invalidate keys — no polling, no manual refetch. High-rate binary
  streams bypass Query entirely → Zustand/refs → canvas.
- **Workspaces & tabs (M6, shipped):** server-persisted **workspaces** — exactly one active at
  a time, unlimited **tabs** per workspace, each tab a dockable panel layout (`dockview`:
  splitting, floating, drag-rearrange). Workspaces live in SQLite next to presets — your
  station layout is part of the station config, not browser state, so every client sees the
  same setup. What binds:
  - **The layout tree is ours, in `wire`** (§4), not the dock library's serialization: templates
    author layouts in Rust, and a dock-library major must not invalidate stored workspaces. The
    client compiles that tree into its dock and maps the dock's state back.
  - **A panel names no device set and no channel.** Engine ids are per-run and reused after a
    restart, so a stored panel pinned to one would silently bind to a different radio. Panels
    follow the client's active set; per-panel pinning waits for stable device identity.
  - **Sizes are relative, and a viewport that cannot honour the layout does not write it back.**
    Below the phone breakpoint every panel is one stack and nothing is persisted — otherwise the
    dock's minimum sizes would flatten a layout authored on a desktop.
  - Layout writes are debounced to the end of a gesture and carry the revision they were read
    at; a stale one is refused rather than overwriting another client's arrangement.
- **Maps:** MapLibre GL on OpenFreeMap tiles (free, no key). Offline/self-contained mode is
  still open: drop a region **`.pmtiles`** file next to the server and it serves the tiles
  itself — a Pi in a field needs no internet. Globe projection for satellite views, openAIP
  aviation overlays and optional satellite imagery via a user key, later.
- **Design language (explicitly: no AI slop):** professional instrumentation aesthetic — the
  reference points are pro audio tools and lab equipment, not landing pages. Dark-first with a
  maintained light theme; a mono face with tabular numerals for frequencies and data columns;
  a large, digit-scrollable frequency readout; restrained neutral palette, one accent,
  semantic status colors only; colorblind-safe waterfall colormaps; 4-px grid discipline;
  keyboard-first (tune step, mode, squelch, tab switching all bound); zero decorative
  gradients/glassmorphism/emoji. The 60 fps waterfall is the centerpiece and every panel earns
  its pixels. **A design pass is part of every UI milestone's definition of done.**
- **Beginner-friendly, expert-deep:** the first-run wizard and template gallery ship; a
  template is device + channels + (later) layout + a short "what am I looking at" explainer,
  built from presets, never from special engine code. Still open: layouts in templates, and a
  band-plan explorer that suggests mode and settings when you click a band (§8a). Expert mode
  hides none of the knobs.
- The UI must stay usable on a phone — it is the remote control for a server in another room.

---

## 11. Persistence (server-side)

- `config.toml` / flags / env: port, bind address, token, paths, backend options.
- SQLite (`rusqlite`, bundled — zero system deps) holds presets, bookmarks, the decoder log,
  the recordings index, and later workspaces (§10) and the frequency-allocation DB (§8a).
- **Files on disk are the source of truth for recordings**; the SQLite row is an index that is
  reconciled against the SigMF files, never the other way round.
- **Decoder output is persisted by the server, never by the engine** — the crate boundary is
  the rule: the engine broadcasts typed events, the server decides what is kept, pruned and
  exported (CSV/JSON), so decoder history is queryable instead of scroll-back-only.
- Nothing lives in the browser except UI preferences (theme, layout) in localStorage.

---

## 12. Security model

- Default: bind `0.0.0.0`, no auth — LAN-trusted, same posture as SDRangel/rtl_tcp.
- Optional single shared token, required on REST, WS and MCP alike; one middleware, never a
  per-route decision. The UI prompts for it and stores it per saved connection.
- CORS locked to same-origin by default (dev mode relaxes it).
- Explicit docs note: exposing an SDR server to the internet is your VPN's job (Tailscale
  et al.), not ours. No TLS termination in v1 (reverse-proxy if needed).
- Multi-user accounts and roles are a non-goal (§1); if that ever changes it is a new section,
  not a widening of this one.

---

## 12a. TX & RF security research (future phase — RX ships first)

The device trait carries a TX half from day one. When TX is built, it's a **general-purpose,
legitimate** transmit + RF-research toolkit — the same class of capability SDRangel already
ships (modulators) and every licensed SDR operator uses. It is *not* a catalog of attack
presets.

**In scope — the RF security-testing platform (all behind the gate below):**
- **Signal generator / arbitrary waveform + IQ playback-to-air** — generated tones, modulated
  test signals, or captured/edited SigMF IQ files. General primitive; "replay a captured
  signal" is a special case.
- **Modulators** paired with the demods (NFM/AM/SSB/WFM, digital modes) for two-way / beacon /
  test use on licensed bands.
- **Sub-GHz security testing** (core RF pentest): capture → decode → replay of OOK/ASK/FSK;
  **fixed-code analysis & generation** incl. de Bruijn sequences; **rolling-code capture and
  implementation analysis** (RollJam-style research on your DUT — window/resync/counter flaws).
- **Interference / jam-susceptibility testing** — configurable noise / sweep / CW / tone on a
  chosen band and bandwidth, to characterize how a device under test behaves under interference
  (does the alarm/sensor/link detect it and fail safe, or silently drop?). Delivered into a
  **direct connection, dummy load, or shielded setup**.
- **Flood / spam / malformed-broadcast testing** — generate and transmit high-rate or malformed
  frame streams (incl. BLE-advertising-style floods) at a **device under test over a contained
  link** to probe robustness and crash/DoS resilience. This is the contained form of the
  "spam" tooling: aimed at your DUT on coax/dummy-load/cage, not radiated at bystanders.
- **Targeted protocol fuzzing** — malformed/mutated frames at a specified DUT (sub-GHz, and
  2.4 GHz/BLE-adjacent where the SDR reaches) to find robustness bugs.
- **Bench loopback testing** — TX into your own RX to validate decoders.
- **RX/offline analysis** — decode, dissect, mutate, re-analyze captured frames; encoding ID.

**What exists today, and where it stops.** The device abstraction carries the TX half, as §6
always specified: `SdrDevice::duplex` reports what a radio can do, and `SdrDevice::tx_start`
hands back a `TxStream` (write samples with a burst boundary; stop). `device-hackrf` implements
both over a working bulk-OUT path — burst queueing and the firmware's end-of-burst marker on the
shared transport, plus transmit VGA control — because the radio has it and a driver that omitted
half its hardware would have to be rewritten later. Every other backend inherits the default:
`Duplex::RxOnly` and a `tx_start` that returns `Unsupported`.

It stops there, one layer lower than the gate itself. `Capabilities` reports `tx_capable: false`
on every backend, nothing in `engine`, `server`, MCP or the web UI calls `tx_start`, and there is
no wire type through which a client could ask for one — so no request reaching this process can
key a transmitter. The transmit VGA is written to 0 dB when the device is opened, so a mode
change alone cannot make the radio radiate at drive, and it has no wire setting either. What is
unbuilt is everything above: the authorized-use gate below, and every feature behind it.

**Controlled-environment gate (real safety UX, not a checkbox):**
All TX — including every security-testing tool above — is **hard-disabled by default**.
Enabling it requires an explicit **"controlled RF environment / authorized test"**
acknowledgment in settings, where the operator affirms a contained setup (direct connection,
dummy load, or shielded/Faraday enclosure), an authorized engagement, and responsibility for
legal compliance (region, band, power). While off, no code path can key the transmitter. Sane
defaults reinforce it: minimum TX power on enable, an on-air indicator, and a session
time-box. Treated honestly — the acknowledgment is a deliberate speed-bump and a record of
intent, not proof of containment; keeping the RF actually contained is the operator's job.

**Operating principle (written into the repo):** these are test instruments for *contained,
authorized* assessment — direct-connect, dummy load, or shielded, against devices you're
authorized to test. The project ships them framed and gated that way; it does not ship
presets whose purpose is uncontrolled over-the-air disruption of third-party systems. With the
containment the operator attests to, jam-susceptibility and flood/spam testing deny service to
nothing but the DUT — which is the whole point of the test.

---

## 13. Feature roadmap — SDRangel RX parity, phased

Policy: **self-written pure Rust first** (portable by construction), but pragmatic — where a
proven library just works (mbelib/DSDcc for digital voice, fdk-aac for DAB+ audio), use it
via FFI without ceremony, verified on Linux-arm64 + macOS. Licensing stance (MIT, personal
project): GPL projects (SDRangel, SDR++, DSDcc…) are fair game **as reference** for
algorithms, parameters, and behavior — **no direct code copying**.

Each decoder lands with: settings struct in `wire`, a golden IQ fixture test (§14), and a UI
panel or the generic fallback. Definition of done includes running on a Pi. The tables below
are the *remaining* work — what shipped is in `PROGRESS.md`, and §19 measures the rest against
SDRangel plugin by plugin.

### Phase 1 — analog core
Shipped at M0–M3: spectrum/waterfall, NFM/WFM/AM/SSB, squelch/AGC, Opus audio, SigMF
record/playback, presets and bookmarks. Remaining:

| Feature | Implementation |
|---|---|
| Notch / audio filters per channel | self |
| Frequency tracker / AFC | self |

### Phase 2 — data decoders wave 1
Shipped at M4–M5: RDS (stereo still open, §18), ADS-B + map, AIS + map, POCSAG, AX.25/APRS
(Mic-E still open), RTTY, Morse, frequency scanner. Shipped in wave 2 (post-M6): NAVTEX
(SITOR-B), ACARS, and the sub-GHz OOK/FSK capture-and-decode channel (§8b). Remaining:

| Feature | Notes |
|---|---|
| **HF WEFAX** (weather fax) | self — blocked on transport, not on DSP: a fax page is an image, and §5 has no frame kind for one. Needs an `IMAGE` binary frame plus a canvas panel before the demod is worth writing |
| **Signal-strength hunt mode** (fox-hunting / find-the-transmitter) | app; uses RSSI + audio/visual feedback |
| Channel analyzer (scope, constellation) | IQ taps §9 |
| Demod analyzer (scope/spectrum on demodulated audio) | self |
| Heat map channel · channel power meter (RSSI + logging) | self; pair with each other |
| CTCSS/DCS detection on NFM · Selcall (CCIR/ZVEI) | small |
| Per-channel sinks: audio recording, baseband file (SigMF/raw), UDP out | self; UDP feeds external tools (multimon-ng et al.) |
| APRS *feature* (station/position collection, distinct from the channel) | app |
| Spectrum annotations (band plans / editable frequency DB overlaid on spectrum) | app; §8a |

### Phase 3 — digital voice & harder modems
| Feature | Notes |
|---|---|
| **WFM stereo** | the open half of `demodbfm` (§18): two-channel PCM/Opus/worklet path |
| **M17** | fully open protocol; Codec2 has a pure-Rust port (`codec2` crate — verify quality, else C FFI) |
| **FreeDV** | Codec2 modes + FDMDV modems |
| **DSD suite: DMR, D-Star, YSF, NXDN, P25, dPMR** | **default-on, voice included.** Fast path: DSDcc + mbelib via FFI (the proven combo SDRangel uses) — adopt whichever existing library just works; only self-write framing/FEC in Rust where the libraries fall short. Patent caveat on AMBE/IMBE noted and accepted (personal, non-distributed project) |
| **FT8/FT4** | LDPC(174,91) + 8-GFSK; port from the published spec — sizeable but well specified |
| **ChirpChat (LoRa)** | self, community-documented |
| **Radiosonde** (RS41 …) | self, informed by radiosonde_auto_rx |
| **DAB/DAB+** | OFDM+Viterbi+RS self; DAB+ audio is HE-AAC → likely `fdk-aac` FFI (license-check) — the big one of this phase |
| **VOR**, **ILS** demods | small and fun (30 Hz phase / tone depth) |
| **Radio clock** (DCF77/WWVB/MSF/JJY) | tiny |
| **DSC**, **Pager wave 2 (FLEX?)** | self |
| **ATV** (analog TV) | self, medium |
| **NOAA APT** | self; pairs with satellite tracker |
| Audio cleanup: spectral noise reduction, noise blanker, auto-notch (SDRangel's wdsprx/denoiser territory) | `nnnoiseless` (pure-Rust RNNoise) + self DSP |
| **Meshtastic** / **MeshCore** (LoRa mesh) | self, built on the ChirpChat engine |
| **End-of-Train** (EOT) telemetry | self, small |
| **VOR localizer** (multi-VOR position fix on map) | self |
| **Inmarsat** (STD-C / AERO) | self; JAERO/Scytale-C as reference |
| **PSK31/63**, **WSPR** | self |
| **VDL Mode 2** (ACARS successor, D8PSK 31.5k) | self; dumpvdl2 as reference |
| **ISM sensors** (rtl_433-style: weather stations, TPMS, utility meters — top protocols) | self, rtl_433 as reference; escape hatch: UDP sink → rtl_433 binary |

### Phase 4 — features & advanced
| Feature | Notes |
|---|---|
| **Satellite tracker** | `sgp4` crate (well-maintained), TLE fetch, pass prediction, doppler-corrects linked channels |
| **Rotator control** (GS-232, rotctld) + **rig ctl server** (rigctld protocol compat) | self |
| **Star tracker / radio astronomy** (integrating radiometer, spectral line) | self |
| **Sky map** (celestial view, companion to star tracker) | self, client-side render |
| **Map feature** (consolidated), **SID monitor**, **noise figure**, **PER tester** | self |
| **GPS position source** (gpsd / NMEA serial): live station position for maps & trackers, geotagged mobile heat map (drive-around coverage), auto grid locator | parity — SDRangel supports external GPS dongles for mobile heat maps; ours adds auto grid locator + geotagged recordings |
| **Antenna tools** (dipole/λ calculators) · **3D spectrogram** view | trivial / WebGL eye-candy |
| **Remote sink/source** between sdr-- instances; **rtl_tcp / SpyServer client devices**; **KiwiSDR client**; **audio-input device** (`cpal`) | cheap wins, huge reach |
| **rtl_tcp server** (serve our devices to other apps) + local sink/input routing between device sets | complements the client side |
| **Meteor M-2 LRPT** (digital weather-sat imagery: QPSK, Viterbi+RS) | self; SatDump as reference — pairs with pass automation |
| **Trunking following** (P25 / DMR Tier III: decode control channel, auto-steer voice channels; multi-dongle aware) | SDRTrunk as reference |
| **CW skimmer** (decode every CW signal in the passband simultaneously) | self |
| **TETRA** (clear-mode RX only) | candidate; RX legality varies by country |
| **Coherent array DoA**: bearings on the map (MUSIC/ESPRIT), multi-station triangulation; **passive radar** (range-Doppler); **beamforming / diversity combine** | targets the `CoherentArray` abstraction (§6) — KrakenSDR today, any synced N-RX later; stretch |
| **NanoVNA integration** (USB serial, documented protocol): antenna sweeps, SWR/Smith-chart panels, saved antenna profiles | tools tab |
| **DATV (DVB-S/S2)** | stretch; FFI candidates (leandvb-style) or long-term self |
| MIMO: interferometer, **DOA2 direction finding** | needs coherent hardware; timestamping from §5 is the prerequisite |

---

## 14. Testing strategy

- **`dsp`:** unit tests against analytically generated signals + golden vectors (filter
  responses, PLL lock behaviour). `criterion` benches for the hot paths are still open.
- **Decoders (the crown jewels):** every decoder ships with short IQ fixtures + expected
  decoded output — building the fixture *is* part of building the decoder.
  - **Reference modulators** live once, in `channels::testgen` behind the crate's
    `test-signals` feature. One encoder per protocol feeds all three consumers — the decoder's
    unit tests, the engine's end-to-end runs through `device-virtual`, and `xtask fixtures` —
    without duplicating protocol encoding, and without `channels` gaining a dependency (§3:
    it still depends only on `dsp` + `wire`; the feature is test-only).
  - Where a generator and a decoder could share a mistake and cancel it out, they are written
    from different derivations on purpose (ADS-B's CPR/Gillham/callsign encoders are closed
    form against the decoder's tables) — a mistyped constant must fail a test, not disappear.
  - Synthesized fixtures prove the decoder against the *specification*. Off-air captures prove
    it against the *world* and land per decoder as hardware sessions produce them; a decoder
    without one says so in `PROGRESS.md` rather than pretending coverage it does not have.
- **Engine:** end-to-end through `device-virtual` — siggen or replayed SigMF → channel →
  assert audio RMS / decoded events. **No hardware in CI, ever.**
- **Server:** axum handler tests via `tower::ServiceExt`; OpenAPI snapshot test; codegen-drift
  check (regenerate → `git diff --exit-code`).
- **Web:** `tsgo` strict and vitest for stores/utils; still open is one Playwright smoke flow
  against the real server with `device-virtual`.
- **Hardware is the owner's test bench, not CI's.** Field sessions are run against the built
  release artifact and written down in `PROGRESS.md` with what was measured — that is the only
  record that a driver, a gain table or a decoder survived contact with a real radio.
- **Performance gates (open):** criterion benches in CI for regression tracking, plus a manual
  on-Pi soak checklist per release (X channels at Y rate → CPU%, thermals) — the Pi 4 is the
  floor (§1), so a budget decision is only settled when it is measured there.

---

## 15. Build, packaging, CI

- **Toolchain:** **pinned Rust nightly** (`rust-toolchain.toml`) with the next-gen borrow
  checker (`-Zpolonius=next`). The pin is bumped deliberately, never floating, so builds stay
  reproducible; CI uses the same pin. `cargo xtask` is the only entry point, locally and in
  CI — a gate that cannot be run locally does not exist.
- **CI (GitHub Actions)** mirrors `cargo xtask check` + `cargo xtask test` on ubuntu-x86_64 and
  macos-arm64, including the codegen-drift gate and the Soapy-free build. It grows with the
  project; the local command grows first.
- **Release artifacts:** `sdrmm` headless (linux x86_64 + aarch64, macOS arm64, UI embedded),
  Tauri desktop bundles, and a multi-arch Docker image (`--device /dev/bus/usb`) for Pi/NAS.
- **The hard packaging rule: release artifacts just run.** The default hardware (RTL-SDR,
  HackRF) is compiled in via the native backends over pure-Rust USB, so a release binary links
  no C radio library and needs nothing installed. SoapySDR is optional *extra* coverage, never
  a launch dependency: a missing libSoapySDR costs exotic-device support, not startup. What
  static linking cannot fix stays out of scope and honest — OS USB permissions (udev rules)
  and vendor daemons (SDRplay); `sdrmm --doctor` prints what is found and what to fix.

---

## 16. Milestones (implementation order)

M0 walking skeleton · M1 real hardware · M2 listen · M3 record & replay · M4 decoders wave 1 ·
M5 ops & UX polish (scanner, auto-reconnect, token auth, MCP, templates, native backends,
packaging, docs, `--doctor`) — **all shipped; `PROGRESS.md` records what each one built and how
it was verified.**

- **M6 — the UI shell ✅ shipped.** Workspaces → tabs → dockview panel layouts, server-persisted
  (§10); templates gained layouts. `PROGRESS.md` records what it built, how it was verified and
  the gaps it left.
- **Decoders wave 2 ✅ shipped** (post-M6): NAVTEX (SITOR-B), ACARS and the sub-GHz OOK/FSK
  channel, each with a reference modulator, unit tests, an engine end-to-end run and a playable
  fixture. `PROGRESS.md` records what it built and the gaps it left.
- **M7+ — Phase 3/4 waves** per §13, prioritized by demand, plus the open Phase 1/2 items.

The milestone rule that outlives the list: a milestone is done when its tests are green
*and* its gaps are written down (§14) — not when the feature runs once.

---

## 17. Risks & mitigations

| Risk | Mitigation |
|---|---|
| Pi CPU budget (many channels, spectrum) | per-connection throttling, decimation discipline, benches + on-Pi gates, PFB channelizer in back pocket |
| Scope explosion (SDRangel parity) | plugin API stability is the real product; parity accretes decoder-by-decoder and the architecture is judged by the cost of the next one |
| Decoders that pass their fixtures and fail the air | off-air proof tracked per decoder in `PROGRESS.md`; RDS is the live example — correct against the spec, silent against real broadcast, cause still unresolved |
| Browser audio (latency, autoplay) | AudioWorklet + jitter buffer + gesture unlock; WebCodecs fast path |
| AMBE/IMBE patents, fdk-aac license | accepted — personal, non-distributed project; default-on |
| Nightly toolchain / `-Zpolonius=next` breakage | pinned nightly, bumped deliberately with CI green as the gate; the flag is one line to drop |
| Tauri v2 churn, macOS signing | desktop is a thin shell over the always-working web path; bundles ship unsigned until Apple secrets exist |
| Vendored radio drivers are ours to maintain | deliberate, and the reason is in §18 — two upstream crates had two divergent, both-wrong USB error policies. One shared transport with librtlsdr's policy replaced them; "always newest versions" does not apply to code with no upstream |
| `soapysdr` binding maintenance | our own trait isolates it (§18); Soapy is optional extra coverage, never the launch path (§15) |

---

## 18. Decision log

Settled long ago and not re-litigated — the manifests, `CLAUDE.md` and the code carry them, so
they are recorded here in one line rather than a row each: the name `sdr--` with the crate and
binary prefix `sdrmm`; MIT, a personal project, GPL projects as *reference* only and never
copied; REST + one WebSocket, with `crates/wire` as the single source of truth through OpenAPI
codegen (plain Rust structs and serde/utoipa — no Protobuf); one frontend, loaded by both the
Tauri app and any browser; LAN-trusted access with an optional shared token; Raspberry Pi 4 as
the performance floor; pinned Rust nightly with `-Zpolonius=next`; TypeScript 7 + Biome +
Oxlint and shadcn/Base UI on Tailwind v4, with no ESLint or Prettier; GitHub Actions mirroring
`cargo xtask`.

What follows is the rest — the choices with a rejected alternative or a reason behind them.

| Decision | Choice |
|---|---|
| Digital voice (DMR/P25 …) | default-on, voice included; use proven libs (DSDcc/mbelib) via FFI |
| MQTT / Home Assistant export | rejected — not wanted |
| UI shell | Workspaces (one active) → unlimited tabs → dockview panel layouts, server-persisted (M6) |
| Workspace layout model (M6) | Our own tree in `wire` (`LayoutNode` = split/group, weights in **permille**), not the dock library's JSON: templates author layouts in Rust, stored workspaces survive a dockview major, and integer weights make a load→save cycle a fixed point instead of drifting. Panels carry no device-set or channel binding — engine ids are per-run and *reused*, so a stored pin would silently attach a panel to a different radio; panels follow the client's active set. One snapshot blob per row like presets (written atomically, read whole, never queried by inner field), with `tabs` denormalized so a layout this build cannot parse breaks opening *that* workspace, never the switcher. Concurrent clients converge via a revision-checked update (409, refetch, re-apply) rather than last-write-wins |
| Maps | MapLibre GL; OpenFreeMap default (no key), self-hosted PMTiles for offline, optional satellite-imagery key |
| MCP server | yes — `rmcp` over streamable HTTP at `/mcp`, same token auth (M5) |
| Onboarding | template gallery + first-run wizard + band-plan explorer (M5) |
| Coherent arrays | generic `CoherentArray` abstraction (§6), NOT a Kraken-specific driver — KrakenSDR is one populator, any synced N-RX (future Dragon-class boards, RTL banks) works the same; DoA + passive radar + beamforming (stretch) |
| Decoder events (M4) | typed `DecoderEvent` in `wire`, emitted by channels as owned values (never JSON on the DSP thread); own broadcast + bounded hand-off queue with reported drops (§5); persisted by the server, never by the engine (crate boundary) |
| BFM stereo vs RDS (M4) | RDS is a `wfm` param, not a second channel type — one FM demod, one filter chain. WFM **stereo** is deliberately *not* built: it changes the whole audio path to two channels (PCM, Opus, frame `ch_layout`, AudioWorklet) and is tracked as the remaining half of the §19 `demodbfm` row |
| RTTY/Morse channel rate (M4) | 8 kHz DDC output, not 48 kHz: a 400 Hz CW filter at 48 kHz needs ~2 700 taps to keep its shape factor, which blows the Pi 4 budget for one channel (§14 performance floor) |
| Wideband channels vs the DDC (M4) | A rate conversion costs bandwidth: the DDC delivers only 80% of the output rate flat, the rest being the guard band that stops folding. A mode occupying more than that — ADS-B fills its entire 2 MHz channel — cannot be resampled into place, so the engine **refuses** it unless the device runs at exactly the channel rate, naming that rate. Found by the M4 end-to-end run: at 2.4 Msps the pulses were smeared and the decoder produced nothing, which is indistinguishable from an empty sky. **Follow-up:** a wideband DDC mode that trades the guard band for bandwidth would let ADS-B run at any device rate; until then 1090 MHz means tuning the device to 2 Msps |
| Frequency-allocation DB | layered World/ITU → Germany (BNetzA Frequenzplan) → future US/UK; overlaid on spectrum + searchable (§8a) |
| HackRF/PortaPack/Flipper RX parity | in scope for the RX half (§8b); Sub-GHz OOK/FSK channel + capture |
| TX & RF security testing (future) | in scope behind a default-off "controlled RF environment / authorized test" gate: siggen, IQ-to-air, modulators, bench loopback, **sub-GHz capture/replay/fixed-code (de Bruijn)/rolling-code analysis**, **jam-susceptibility testing**, **flood/spam/malformed-broadcast testing against a DUT**, **targeted fuzzing** — all framed for contained (direct-connect/dummy-load/shielded) authorized use (§12a) |
| NanoVNA | planned as tools-tab integration via USB serial (P4) |
| Soapy binding (M1 evaluation) | `soapysdr` 0.5 over seify: seify duplicates our device/capability abstraction, its production path is the same libSoapySDR, and it had 3 breaking releases in 6 weeks; its native drivers are self-declared experimental. Binding gaps worked around: no `setFrequencyCorrection` wrapper (PPM via the `"CORR"` frequency component), no `getSettingInfo` (per-driver extra-settings tables in `device-soapy`) |
| Native backends: buy, then build | M5 shipped `rs-rtl` 0.4.2 + `hackrf-nusb` 0.3, both pure Rust over `nusb`, so a release artifact links no C library (§15). Rejected then: `rtl-sdr-rs`/`seify-rtlsdr`/`rtlsdr_mt`/`waverave-hackrf`/`seify`. Re-evaluated on technical merit afterwards and **measured** rather than argued — the RTL2832U's test-mode counter ramp makes every lost byte countable: a single-transfer-at-a-time reader (`rtlsdr-pure`) is fine on an idle machine but loses ~2.2% of the stream under 16 spinning threads, while a 15-transfer queue delivers 100.0% under the same load. sdr-- runs a full DSP pipeline on those cores, so the queue depth is the whole decision |
| Native drivers taken in-tree (post-M5) | **Both native backends now own their radio driver outright**, one crate each — `crates/device-rtlsdr/src/driver/` (RTL2832U + R82xx) and `crates/device-hackrf/src/driver/` (HackRF) — over the shared `crates/usb-stream` bulk transport, both directions. The trigger was correctness, not maintenance: each upstream crate hand-rolled its own USB transfer-error policy and both were wrong in different ways (rs-rtl counted cancellations toward a 5-error threshold with 15 transfers in flight, so one stalled pipe read as "disconnected"; hackrf-nusb 0.3.0 closed the whole stream on a *single* errored transfer). Two divergent, both-wrong policies is the defect; one shared transport with **librtlsdr's** policy is the fix — a cancellation never counts, only genuine errors do, the threshold *is* the queue depth, any success clears it. Layering per backend: `driver` (radio, no wire types, no arbitration) → `convert` (the radio's sample table) → `caps` (pure wire translation) → `SdrDevice` over `crates/device`'s shared capture machinery. Recovery is two-tier: the shared supervisor restarts the stream in place (~1–7 ms measured on both radios) and only an exhausted restart budget reaches the engine's destructive fault path (~9 s). `device-soapy` keeps tier 2 only — librtlsdr *inside* SoapySDR already absorbs transient stalls, so an error surfacing through Soapy means that driver gave up; `device-virtual` gets none of it. Owning the drivers also made PPM correction and (next) direct sampling ours to add. The code is no longer a tracked fork of anything, which is why "always newest versions" does not apply to it |
| Shared device machinery, not per-backend copies | The first cut of the native backends left each one owning its capture loop, and the two copies were ~105 identical lines that had already diverged on a bug. The rule now: **anything a second radio would have to write again lives in `crates/device`.** That is `Duplex`/`DuplexState` (§6), `Capture` + `CaptureRadio`/`CaptureStream` (the thread, the tier-1 restart supervisor, the silent-stall detector, block splitting, stop-and-join teardown), `LutConverter` (only the *table* is a radio's own), `Worker`, `RestartPolicy` and `lock`; the bulk-OUT queue moved to `crates/usb-stream` beside bulk-IN. `crates/device` stays I/O-free by default — the USB `CaptureStream` impl is behind its `usb` feature. Two bugs the split fixed by construction: `rx_stop` used to switch the transceiver off unconditionally, which would silence a live transmit burst, and a stop racing a tier-1 restart could leave the radio armed with no thread |
| MCP server (M5 implementation) | `rmcp` streamable-HTTP at `/mcp`, **stateless** (`legacy_session_mode = false`, `json_response = true`): no session to garbage-collect, nothing lost across a restart, and the tools need no server-initiated notifications. rmcp's DNS-rebinding host guard is disabled because it defaults to localhost-only and would 403 every LAN client — the shared token is what gates the endpoint, matching §12's posture for REST |
| Frequency scanner (M5) | App-level and control-plane only: the unit of work is a *device tuning*, not a target, so one dwell measures every target inside the passband off the existing spectrum tap — no extra DSP, which is what makes it affordable on a Pi 4. A running scan owns its set's centre frequency and client retunes are refused while it does, rather than the two fighting |
| Token auth (M5) | One `route_layer` middleware over the routed API + WS + MCP, deliberately *not* the SPA fallback (the login UI must load unauthenticated, and an unmatched `/api/*` stays a typed 404 instead of a 401). Accepted as `Authorization: Bearer` **or** `?token=`, because the browser WebSocket API cannot set headers and the decoder-log export is a plain navigation. `/api/auth`, `/api/openapi.json` and `/api/docs` stay public: they describe the API's shape, never its data |
| Decoders wave 2 (post-M6) | Three channels, one rule each. **NAVTEX** emits only what sits between `ZCZC` and `NNNN`: a broadcast station idles for minutes, and a decoder that logged everything it sliced would bury the messages in phasing signal — the CCIR 476 chart is stored as a *code → ITA2* map so the alphabet stays defined once, in `rtty`. **ACARS** repairs nothing: parity and the ARINC 618 CRC both have to pass or the block is dropped, because the payload is free text and a plausible-but-wrong message is worse than a missing one (`acarsdec`'s syndrome-table repair is a deliberate non-goal, noted in PROGRESS). **Sub-GHz** names no chip — an EV1527's 24 data bits and a PT2262's 12 tri-state symbols are the same pulse train, so `encoding` says `pwm` and both readings ride along; repeats inside 500 ms collapse into one event with a count, and a better-classified frame supersedes a held one only while that one is still a single sighting, which is what keeps a capture that started mid-burst from logging its fragment |
| WEFAX deferred (wave 2) | Not a DSP problem: a fax page is an image, and §5's frame kinds are spectrum, audio and IQ. Shipping it means adding an `IMAGE` binary frame, a server-side page store and a canvas panel — a transport decision, not a decoder, so it waits for one rather than being smuggled in as base64 in a decoder-log row |
| Templates (M5) | A static Rust table, not seeded SQLite rows: templates ship with the binary, so rows would need a migration per edit and a user could delete an entry the next release restores. Presets remain the writable, device-bound half of the same idea |

---

## 19. Appendix — SDRangel parity ledger

Verified against the SDRangel master plugin tree (Aug 2026). This is the authoritative
checklist of what parity *means*; the phase tables in §13 are the plan view of the same list.
Statuses: ✅ planned (phase noted) · ⏭ deliberately skipped · 🔵 covered structurally (not a
discrete plugin for us). **Shipped plugins are listed once, in prose, and detailed in
`PROGRESS.md`** — the tables below are the work that remains.

### Channel RX (44 plugins)

Shipped: `demodam` · `demodnfm` · `demodssb` · `demodwfm` (mono) · `demodadsb` · `demodais` ·
`demodpager` · `demodrtty` · `demodnavtex` · `freqscanner` · `sigmffilesink` at device level.
Beyond SDRangel's channel list, wave 2 also shipped **ACARS** and the **sub-GHz OOK/FSK**
channel (§8b), neither of which is an SDRangel plugin. Shipped in part,
so still in the table: `demodbfm` (RDS landed as a `wfm` param; stereo open), `demodpacket`
(AX.25/APRS landed; Mic-E open), `filesink` (per-channel sinks open).

| SDRangel plugin | Ours | Phase |
|---|---|---|
| chanalyzer | ✅ channel analyzer | P2 |
| channelpower | ✅ channel power meter | P2 |
| demodapt | ✅ NOAA APT | P3 |
| demodatv | ✅ ATV | P3 |
| demodbfm | ✅ WFM **stereo** — the open half of the `wfm` channel RDS already rides on | P3 |
| demodchirpchat | ✅ ChirpChat/LoRa | P3 |
| demoddab | ✅ DAB/DAB+ | P3 |
| demoddatv | ✅ DATV | P4 (stretch) |
| demoddsc | ✅ DSC | P3 |
| demoddsd | ✅ DSD suite, default-on incl. voice | P3 |
| demodendoftrain | ✅ End-of-Train | P3 |
| demodfreedv | ✅ FreeDV | P3 |
| demodft8 | ✅ FT8/FT4 | P3 |
| demodils | ✅ ILS | P3 |
| demodinmarsat | ✅ Inmarsat STD-C/AERO | P3 |
| demodm17 | ✅ M17 | P3 |
| demodmeshcore | ✅ MeshCore | P3 |
| demodmeshtastic | ✅ Meshtastic | P3 |
| demodpacket | ✅ Mic-E position encoding, the one AX.25 form left undecoded | P2 |
| demodradiosonde | ✅ Radiosonde | P3 |
| demodvor / demodvormc | ✅ VOR | P3 |
| filesink | ✅ per-channel baseband sinks (the device-level recorder ships) | P2 |
| freqtracker | ✅ frequency tracker | P1 |
| heatmap | ✅ heat map (+ GPS mobile mode, P4) | P2 |
| localsink | ✅ local routing between device sets | P4 |
| noisefigure | ✅ noise figure | P4 |
| radioastronomy | ✅ radio astronomy | P4 |
| radioclock | ✅ radio clock | P3 |
| remotesink | ✅ remote sink between sdr-- instances | P4 |
| remotetcpsink | ✅ rtl_tcp server | P4 |
| udpsink | ✅ UDP sink (external tools) | P2 |
| wdsprx | ✅ as advanced audio processing in every voice channel (NR/NB/ANF/AGC), not a separate channel type | P3 |

### Features (23 plugins)

Shipped: `morsedecoder`, and `ais` structurally (the AIS channel plus the map and decoder-log
database do its job). Shipped in part, so still in the table: `aprs` and `map`.

| SDRangel plugin | Ours | Phase |
|---|---|---|
| afc | ✅ AFC | P1 |
| ambe | 🔵 mbelib FFI in DSD; hardware AMBE dongle/server support optional later | P3 |
| antennatools | ✅ antenna calculators | P4 |
| aprs | ✅ station-collection feature (the channel, map and log ship) | P2 |
| demodanalyzer | ✅ demod analyzer | P2 |
| denoiser | ✅ audio NR (`nnnoiseless`) | P3 |
| freqdisplay | ⏭ big-frequency readout — just part of our normal UI |  |
| gs232controller | ✅ rotator control (GS-232 + rotctld) | P4 |
| jogdialcontroller | ⏭ keyboard/scroll-wheel tuning covers it |  |
| limerfe | ⏭ hardware-specific; via Soapy settings if ever |  |
| map | ✅ further layers on the shipped MapLibre map: sondes, satellites, beacons, MUF | P3→P4 |
| pertester | ✅ PER tester | P4 |
| radiosonde (feature) | ✅ radiosonde map/log | P3 |
| remotecontrol | ⏭ smart-plug/instrument control — out of scope (same call as MQTT) |  |
| rigctlserver | ✅ rigctld-compatible server | P4 |
| satellitetracker | ✅ satellite tracker (sgp4, doppler-linked channels) | P4 |
| sid | ✅ SID monitor | P4 |
| simpleptt | ⏭ deferred with TX |  |
| skymap | ✅ sky map | P4 |
| startracker | ✅ star tracker | P4 |
| vorlocalizer | ✅ VOR localizer | P3 |

### RX devices (27 plugins)

Shipped: `rtlsdr` and `hackrfinput` (Soapy plus the in-tree native drivers), `fileinput` /
`sigmffileinput` / `testsource` (`device-virtual`), and the whole Soapy-covered fleet below —
`device-soapy` is the contract, so any radio with a Soapy module is reachable without a plugin
of ours.

| SDRangel plugins | Ours |
|---|---|
| airspy, airspyhf, bladerf1/2, fcdpro(+), fobos, limesdr, perseus, plutosdr, sdrplay(v3), usrp, xtrx, aaroniartsa, soapysdrinput | 🔵 via `device-soapy` — module availability varies by platform |
| remotetcpinput | ✅ rtl_tcp/SpyServer client (P4) |
| kiwisdr | ✅ KiwiSDR client (P4) |
| remoteinput / localinput | ✅ sdr-- remote/local routing (P4) |
| audioinput | ✅ `device-audio` via cpal (P4) |
| androidsdrdriverinput | ⏭ Android-specific |

### MIMO channels (3 plugins)

| SDRangel plugin | Ours |
|---|---|
| interferometer | ✅ stretch (coherent hardware; §5 timestamps are the prerequisite) |
| doa2 | ✅ stretch — direction finding, same prerequisite |
| beamsteeringcwmod | ⏭ TX, deferred |

### Beyond parity (ours, not in SDRangel)

MCP server for LLM agents (M5), the decoder-log database with export (M4), and the
frequency-allocation DB (§8a, unbuilt). GPS is *not* beyond parity — SDRangel supports external
GPS dongles for mobile heat maps, so it is tracked in the P4 table above. The consolidated idea
backlog lives in §20.

---

## 20. Future ideas backlog

Not committed to a phase — the running list of "would be cool", so nothing is lost. Grouped,
roughly best-first within each group. Promote to §13 when we decide to build one.

### Capture, analysis & workflow
- **IQ time machine** — rolling per-device-set ring buffer; retro-record the last N seconds to
  disk *after* you hear something, dashcam-style. (High priority; cheap; unique.)
- **Inspectrum-style IQ viewer** — zoomable offline analysis of recordings in the browser
  (spectrogram, cursors, symbol/measurement tools).
- **Signal-ID assistant** — snapshot the spectrum/audio and match against a signal catalog
  (sigidwiki-style fingerprints) to answer "what is this?"; later an ML classifier.
- **Recording scheduler + satellite pass automation** — tracker auto-records/decodes
  APT/LRPT/NOAA/Meteor passes unattended; wake to fresh imagery.
- **Band occupancy analytics** — long-term activity heatmaps over time from scanner/heat-map
  data ("what's alive on this band, and when").
- **Session/replay sharing** — export a recording + workspace + annotations as one bundle a
  friend can open and see exactly what you saw.
- **Annotated recordings** — mark/label events on the timeline of a SigMF capture.

### RF / DSP / multi-receiver
- **Coherent-array DoA / passive radar / beamforming** — on the `CoherentArray` abstraction
  (§6); Kraken today, any synced N-RX later. Bearings + triangulation on the map.
- **TDoA geolocation** across distributed sdr-- nodes (the §5 sample timestamps are the enabler).
- **Diversity combine / noise-canceling** with a second receiver (reference antenna subtracts
  local QRM) — dramatically better HF copy.
- **Adaptive/auto DSP** — auto-notch, auto-squelch, auto-gain, click/noise removal per mode.
- **Wideband recording + offline re-channelization** — record a whole band once, mine many
  channels from it later.
- **GPU spectrum path** (wgpu) for very large FFTs / many channels on capable hosts.

### Decoders / signals (candidate list)
- **Sub-GHz OOK/FSK workbench** (§8b) with a growing protocol library (Flipper-style).
- SSTV RX, DRM30 / DRM+, FLEX/ERMES pagers, LoRaWAN frame parsing, VDL2 (planned P3),
  HFDL, STANAG modems (ID only), Iridium bursts, GSM downlink (grgsm-style, RX/analysis),
  Tetrapol, DMR/P25 trunking (planned P4), OsmocomBB-style GSM monitoring.
- **BLE advertisements** and 2.4 GHz survey (HackRF), Wi-Fi channel occupancy (energy only).
- **GNSS educational decode** — GPS L1 C/A acquisition + ephemeris (learning tool, not nav).
- **Radiosonde fleet** beyond RS41 (DFM, M10/M20, iMet), APRS weather aggregation.
- **Aircraft/ship enrichment** — cross-reference ADS-B/AIS logs with offline databases.

### Platform, UX & integrations
- **Plugin SDK via WASM** (wasmtime) — third-party decoders/panels without our stable ABI risk.
- **Scripting/automation API** — the REST+MCP surface already enables Python control; ship
  recipes (scanner bots, alerting: "ping me when this callsign/ICAO appears").
- **Alerting/notifications** — rule engine on decoder events → desktop/push/webhook.
- **NanoVNA tools tab** (planned P4) + antenna profile library; later **TinySA** spectrum-analyzer
  import, **RTL-SDR-Blog / Airspy bias-T** presets, **rig CAT control** (Hamlib) to slave a real radio.
- **Multi-user / roles** (viewer vs operator) if this ever grows past personal use.
- **Cloud/remote fleet** — one client managing several remote Pi nodes (map of your receivers).
- **Theme/skin system** + layout marketplace for shared workspaces.
- **Accessibility pass** — screen-reader labels, high-contrast, audio cues for the visually impaired.
- **Localization** (DE/EN first) — pairs naturally with the per-region frequency plans.

### Data / reference
- **Frequency-allocation DB expansion** (§8a) — US (FCC), UK (Ofcom), EU CEPT, more; community
  overlays; live "band plan of the day".
- **Offline reference bundles** — ship band plans, satellite TLE snapshots, callsign prefixes,
  ISM protocol catalog for fully-offline field use (pairs with PMTiles maps).
