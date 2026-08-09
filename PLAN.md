# sdr-- — Project Plan

A modular, client–server SDR application. Rust server (all DSP/decoding happens here),
web-technology client (React) shipped as a Tauri desktop app *and* served directly by the
server for browser access. Runs as a single local app on a laptop, or split with the server
on a Raspberry Pi and the client anywhere on the network.

Working name: **sdr--** ("sdrminusminus"), crate/binary prefix `sdrmm`.
Personal project (not planned for public release). License: **MIT**.

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
- TX (architecture leaves room: the device trait has a TX half from day one, unimplemented).
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
│  sdrmm-device: SdrDevice trait + capability model                                  │
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

Cargo workspace + pnpm workspace in one repo.

```
sdrminusminus/
├── Cargo.toml                    # workspace
├── crates/
│   ├── wire/           # ALL wire types: REST DTOs, WS messages, settings structs.
│   │                   # serde + utoipa::ToSchema derives. The single source of truth.
│   │                   # (Plain Rust structs → OpenAPI. NOT Protobuf, despite no "proto".)
│   ├── dsp/            # DSP primitives. Pure functions/structs, no I/O, no async.
│   ├── device/         # SdrDevice traits, capability model, device registry.
│   ├── device-soapy/   # SoapySDR backend (default feature).
│   ├── device-rtlsdr/  # native RTL-SDR backend (feature "rtl-native").
│   ├── device-hackrf/  # native HackRF backend (feature "hackrf-native").
│   ├── device-virtual/ # signal generator + IQ file playback (always on; used by tests/demo).
│   ├── engine/         # flowgraph runtime: device sets, threads, rings, channel hosting.
│   ├── channels/       # ChannelRx trait + built-in demods as feature-gated modules.
│   │                   # Heavy/optional decoders graduate to own crates (e.g. channels-dsd).
│   ├── recorder/       # SigMF IQ recording; playback lives in device-virtual.
│   └── server/         # axum app as a LIBRARY (start(cfg) -> ServerHandle) + REST/WS/static.
├── apps/
│   ├── sdrmm/          # headless server binary (Pi target). Thin wrapper over crates/server.
│   └── desktop/        # Tauri v2 app: embeds crates/server in-process, connection manager.
├── web/                # React + Vite + TS + TanStack Query
│   └── src/generated/  # OpenAPI-generated types+client. Checked in, CI-verified.
├── xtask/              # cargo xtask codegen | dev | fixtures | dist
└── fixtures/           # small recorded IQ samples per decoder (golden tests)
```

Modularity rules:
- `dsp` depends on nothing internal; `channels` depends on `dsp` + `wire` only.
- `wire` depends on nothing internal (serde/utoipa only) so anything can use it.
- A new decoder touches: one module in `channels`, its settings struct in `wire`,
  optionally one React panel. Nothing else.
- Device backends are feature flags; `--no-default-features --features rtl-native` must
  build a Soapy-free binary (matters for minimal Pi images).

---

## 4. Shared types & codegen (the "no two DTOs" pipeline)

Everything on the wire is defined **once, in Rust**, in `crates/wire`:

- REST request/response bodies, WS message enums, channel settings, device capability
  descriptors — all `#[derive(Serialize, Deserialize, ToSchema)]`.
- WS messages are tagged enums: `#[serde(tag = "type", content = "data")] enum ServerEvent { … }`
  → discriminated unions in TypeScript, exhaustively `switch`-able.
- `utoipa` assembles them into OpenAPI; WS-only types are force-registered as schema
  components so they're generated too.

Pipeline (`cargo xtask codegen`):
1. Emit `openapi.json` by calling `ApiDoc::openapi()` directly (no running server needed).
2. `openapi-typescript` → `web/src/generated/schema.d.ts` (pure types, incl. WS enums).
3. `openapi-fetch` provides the typed client (tiny runtime, full inference from schema.d.ts).
4. A thin handwritten adapter (~50 lines, written once) exposes `queryOptions`/mutation
   helpers for TanStack Query keyed by path — not per-endpoint code.
5. CI re-runs codegen and fails on diff → generated code can never drift.

Rules:
- Hand-writing a TS interface that mirrors a Rust struct is a review-blocking offense.
- Binary frame layouts (§5) live in `wire` as documented consts with a Rust encoder +
  one small TS decoder (~100 lines total, changes rarely — the one deliberate exception).
- Bonus: `openapi.json` + Swagger UI served at `/api/docs` = free scripting API
  (Python/curl), same story as SDRangel's REST API but typed end-to-end.

---

## 5. Transport & protocols

### REST (control plane)
`axum` + `utoipa`. Resource-oriented, mirrors the state model:

```
GET    /api/state                     # full snapshot (initial load)
GET    /api/devices                   # discovered hardware (probe results)
POST   /api/devicesets               { device_id }          # open device → create device set
DELETE /api/devicesets/{ds}
PATCH  /api/devicesets/{ds}/device   { freq?, rate?, gains?, … }
POST   /api/devicesets/{ds}/channels { type, settings }
PATCH  /api/devicesets/{ds}/channels/{ch}   # typed per-channel settings
DELETE /api/devicesets/{ds}/channels/{ch}
POST   /api/devicesets/{ds}/record   { action: start|stop }   # format fixed to SigMF cf32
                                     # (M3 decision: a format field returns when a second
                                     #  format exists — YAGNI until then)
GET/DELETE /api/recordings           # index reconciled from SigMF files on disk (§11)
GET/DELETE /api/decoderlog           # stored decoder frames, filterable (kind/set/time/text)
GET    /api/decoderlog/export/{fmt}  # csv|json download of the same filter (§11)
                                     # (M4 decision: format is a path segment, not a query
                                     #  field — serde_urlencoded cannot flatten a shared
                                     #  filter struct, and the filter is shared by all three)
POST   /api/devicesets/{ds}/scanner  { action: start|stop, settings? }   # M5 frequency scanner
GET    /api/templates                # built-in station templates (§10)
POST   /api/templates/{id}/apply     { device_set }
GET    /api/auth                     # { token_required } — unauthenticated by design (§12)
GET    /api/clients                  # connected WebSocket clients (M5 multi-client)
GET    /api/doctor                   # the same report `sdrmm --doctor` prints (§15)
GET/POST /api/presets, /api/bookmarks …
GET    /api/openapi.json · /api/docs
```

### WebSocket (push + data plane) — one socket per client, `/api/ws`
Text frames = JSON `ServerEvent` / `ClientCommand` (from `wire`):
- `StateChanged { scope }` → client invalidates matching TanStack Query keys.
  This is the *only* cache-invalidation mechanism; no polling.
- Decoder output events (ADS-B aircraft, POCSAG message, RDS text, APRS packet…): typed JSON
  (`Decoded { DecodedRecord }`). They travel on their **own broadcast**, not the `StateChanged`
  control stream: ADS-B alone can emit hundreds of frames a second, and a lagging control
  receiver resyncs with a full-state refetch — a cost that must never be triggered by decode
  traffic. Clients append them to a local ring; `StateChanged { DecoderLog }` fires only when
  the *stored* log changes structurally (cleared, pruned). The DSP plane hands frames over a
  bounded queue and the drops are reported as `DecodedLost { count }` (M4 decision).
- Scanner progress (`ScannerUpdate { ds, status }`) is its own event for the same reason as
  decoder output: a running sweep retunes several times a second, and one `StateChanged` per
  step would cost every client a full-state refetch at that rate. `DeviceSet.scanner` in the
  snapshot stays authoritative; a `StateChanged` fires when a scan starts or stops (M5).
- Client → server: stream subscriptions (`SubscribeSpectrum { ds, fps, bins }`,
  `SubscribeAudio { ch }`), which are per-connection — a phone can ask for 10 fps/1024 bins
  while a desktop gets 30 fps/4096.

Binary frames (little-endian, header defined in `wire`):
```
u8 ver | u8 kind | u16 stream_id | u32 seq | u64 timestamp(sample-count)
kind: SPECTRUM   → f64 center_hz, f32 span_hz, f32 db_min, f32 db_max, u16 n, u8[n] bins
      AUDIO_OPUS → u8 ch_layout, opus packet (20 ms)
      IQ_F32     → interleaved cf32 (channel analyzer taps, low rate only)
```
Sample-count timestamps from day one — cheap now, required later for scanner accuracy,
recordings alignment, and (far future) multi-device coherence.

Backpressure: UI streams are drop-oldest per connection (a slow phone never stalls the DSP);
recording and decoder paths are lossless (bounded queue → hard error, never silent loss).

### MCP server
The server also speaks **MCP** (official Rust SDK `rmcp`, streamable-HTTP transport mounted
at `/mcp` on the same axum app, same token auth). Tools: list/tune devices, create and
configure channels, drive the scanner, query decoder logs ("which aircraft did you see in
the last hour?", pager messages, APRS stations), grab spectrum snapshots, start/stop
recordings. It reuses the same typed service layer as REST — LLM agents get the same
contract as every other client, no parallel implementation. (Lands at M5.)

---

## 6. Device layer

### Trait model (`crates/device`)
```rust
trait DeviceDriver: Send + Sync {
    fn id(&self) -> &'static str;                  // "soapy", "rtlsdr", "hackrf"
    fn probe(&self) -> Vec<DeviceInfo>;
    fn open(&self, info: &DeviceInfo) -> Result<Box<dyn SdrDevice>>;
}
trait SdrDevice: Send {
    fn capabilities(&self) -> &Capabilities;       // serialized to client as-is
    fn apply(&mut self, s: &DeviceSettings) -> Result<()>;
    fn rx_start(&mut self, sink: RxSink) -> Result<()>;   // pushes cf32 into ring
    fn rx_stop(&mut self);
    // TX half declared, unimplemented for now.
}
```

`Capabilities` is the backbone of backend-driven UI: frequency ranges, sample rates,
named gain stages with ranges, antennas, bandwidths, plus **typed extra settings**
(bool/enum/range with labels). The client auto-renders controls from this — a new
device setting needs zero frontend work. Well-known settings (frequency, gain, rate)
get first-class custom UI; the rest render generically.

### Backends
- **`device-soapy`** (default in dev/server builds): via the `soapysdr` crate. Instantly covers
  Airspy, SDRplay, LimeSDR, PlutoSDR, BladeRF, USRP… wherever a Soapy module exists. C dependency
  documented per platform (`apt: libsoapysdr-dev soapysdr-module-all`, `brew: soapysdr
  soapyrtlsdr soapyhackrf`). Never a launch dependency of release artifacts (§15 packaging rule).
  Risk: the Rust binding's maintenance — fallback is a minimal own FFI layer or the `seify`
  crate (FutureSDR's abstraction; evaluated at M1 → rejected, see §18 — we keep our own
  trait either way because the capability-schema is ours).
- **`device-rtlsdr`** (native, feature): exposes what Soapy hides or half-hides — direct
  sampling (HF!), offset tuning, bias-T, exact PPM, tuner-specific gain tables, USB buffer
  tuning. Prefer a pure-Rust driver over `nusb`/`rusb` (e.g. `rtl-sdr`-style crates, evaluate;
  else thin `librtlsdr` FFI).
- **`device-hackrf`** (native, feature): amp enable, antenna power, sweep mode (fast wideband
  scanning — a marquee feature Soapy's API can't express), clean gain model (LNA/VGA).
- **`device-virtual`** (always on): signal generator (tones, FM/AM-modulated test signals,
  noise) + SigMF/raw IQ file playback with loop & rate control. This is how CI, the demo
  mode, and decoder golden tests run without hardware.
- **Later network/audio backends** (Phase 4, each small): `device-audio` (sound-card/rig
  audio via `cpal` — decode FT8/RTTY straight from a transceiver), `device-kiwi` (KiwiSDR
  network client), rtl_tcp/SpyServer client (§13). Everything else rides Soapy.
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

Same physical device visible via both Soapy and native: native driver claims priority in
the probe merge; duplicates are collapsed by serial.

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

### `crates/dsp` inventory (all pure Rust, self-written unless noted)
NCO/quadrature mixer · windowed-sinc FIR design + polyphase decimator · half-band chains ·
rational resampler (evaluate `rubato`, else self) · biquads/DC blocker · AGC · power+hysteresis
squelch · FM quadrature discriminator · PLL/Costas · symbol timing recovery (Gardner, M&M) ·
correlators · CRC/Golay/BCH/Hamming · Viterbi + interleavers (for DAB/M17 later) ·
measurement helpers (SNR, power). External: `rustfft`/`realfft` (pure Rust, SIMD, portable).

Every primitive gets golden-vector unit tests (§14) — this crate is the foundation everything
else trusts.

---

## 8. Channel plugin system

```rust
trait ChannelRx: Send {
    fn descriptor() -> &'static ChannelDescriptor where Self: Sized;
        // id, name, required bandwidth, settings schema ref → drives the "add channel" UI
    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self> where Self: Sized;
    fn apply(&mut self, settings: ChannelSettings) -> Result<()>;
    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs);
        // out: audio_pcm(&[f32], rate) · event(ServerEvent) · iq_tap(&[Complex<f32>])
}
```

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
  doorbells, many key fobs) → a generic **OOK/FSK capture+decode channel** that recognizes
  common encodings (PT2262/EV1527/Princeton, Manchester) and logs frames; unknown signals get
  raw timing capture for inspection. This is the Flipper "read Sub-GHz" experience.
- **RF replay-capture (RX half):** record the exact IQ of a burst (garage remote, sensor) to
  the IQ time machine / SigMF, annotate and analyze it. (Re-transmitting it is TX — deferred.)
- Morse, RTTY, SSTV(RX), radio clock, VOR/ILS — already planned.

**Nature/analysis features (PortaPack parity):** waterfall, audio RX (AM/NFM/WFM/SSB), signal
  recording, frequency manager/bookmarks, band-plan awareness → all core sdr-- already.

**Explicitly TX / deferred** (documented so scope is honest): sub-GHz *replay/brute/jam*,
  BLE/OOK *spam*, RF *transmit* of any kind, "spoof" tools. These need TX (the device trait's
  TX half exists but is unimplemented) and several are legally restricted — out of scope now.

A **"Sub-GHz workbench" template** (§10) bundles the OOK/FSK channel + capture + a decoder log
into a one-click Flipper-replacement layout.

---

## 9. Streaming specifics

### Spectrum & waterfall
- Server-side Welch-style averaged FFT; configurable FFT size (1k–64k) and rate; reduced to
  ≤4096 display bins (max-decimation), quantized to u8 over an adaptive dB window
  (range in frame header). ~20 fps default, per-client negotiable.
- Client renders spectrum line + WebGL2 scrolling waterfall texture with client-side colormaps.
  v1 zoom = client-side crop; true zoom-FFT later as a channel-analyzer feature.

### Audio
- Channel demods emit PCM at 48 kHz mono/stereo → Opus (libopus via the `opus` crate,
  vendored build — verified fine on macOS/Linux-arm64) at 20 ms frames → WS binary.
- Client: decode via WebCodecs `AudioDecoder` when available, WASM Opus fallback;
  playback through an AudioWorklet with a 60–100 ms jitter buffer. Multiple clients can
  listen to the same or different channels; mixing happens client-side (it's just streams).
- Browser autoplay policy: audio starts on first user gesture (standard unlock pattern).

### Channel analyzer taps
Low-rate IQ/scope/constellation frames (IQ_F32 kind, decimated hard) for the channel
analyzer UI — never full-rate IQ to the browser.

---

## 10. Client

- **Stack:** React 19 + Vite + **TypeScript 7** (native `tsgo` compiler) strict, TanStack
  Query (server state), Zustand (stream/UI state: waterfall buffers, audio status),
  **shadcn/ui on Base UI primitives** (Base UI is shadcn's default since 07/2026) +
  Tailwind v4, MapLibre GL (ADS-B/AIS/APRS/sat maps), WebGL2 canvases for DSP views.
- **Frontend tooling** (newest, Rust-fast): **Biome** for formatting + import organizing;
  **Oxlint** for linting with **type-aware rules** (via `tsgolint` on the TypeScript-7 Go
  base); **TypeScript 7** (`tsgo`) for typecheck. No ESLint/Prettier. Type-aware Oxlint is
  the slower CI gate; format + non-type lint run pre-commit.
- **Workspaces & tabs (SDRangel-style, done properly):** server-persisted **workspaces**
  — exactly one active at a time, unlimited **tabs** per workspace, and every tab is a
  dockable panel layout (`dockview`: VS-Code-style splitting, floating, drag-rearrange).
  Panels: spectrum/waterfall, channel controls, audio mixer, maps, decoder logs,
  analyzers, tools. Workspaces live in SQLite next to presets — your station layout is
  part of the station config, not browser state, so every client sees the same setup.
- **Maps:** MapLibre GL. Default basemap: **OpenFreeMap** vector tiles (free, no API key,
  no usage caps). Offline/self-contained mode: drop a region **`.pmtiles`** file
  (Protomaps) next to the server and it serves the tiles itself (`pmtiles` crate) — a Pi
  in a field needs no internet. Satellite imagery optional via user-supplied
  MapTiler/Esri key. Globe projection (built into MapLibre v5) for satellite-tracking
  views; openAIP aviation overlays later.
- **Design language (explicitly: no AI slop):** professional instrumentation aesthetic —
  the reference points are pro audio tools and lab equipment, not landing pages.
  Dark-first with a maintained light theme; Geist/Inter for UI + a mono face with
  tabular numerals for frequencies and data columns; a large, digit-scrollable frequency
  readout; restrained neutral palette, one accent, semantic status colors only;
  colorblind-safe waterfall colormaps (viridis/magma + classic heat); 4-px grid
  discipline; keyboard-first (tune step, mode, squelch, tab switching all bound);
  zero decorative gradients/glassmorphism/emoji. The 60 fps waterfall is the centerpiece
  and every panel earns its pixels. A design pass is part of every UI milestone's
  definition of done.
- **Beginner-friendly, expert-deep:** first-run wizard (detect hardware → pick a
  template) plus a **template gallery** — one click configures device + channels +
  workspace layout + a short "what am I looking at" explainer: FM Radio · Airband ·
  Aircraft map (ADS-B) · Ships (AIS) · Pagers · NOAA weather satellites · Radiosonde
  hunting · Ham 2m/70cm · ISM sensors · Shortwave broadcast. Templates are just presets
  + layouts — no special engine code. A band-plan explorer suggests mode/settings when
  you click a band. Expert mode hides none of the knobs.
- **Server state discipline:** TanStack Query is the *only* holder of REST data;
  WS `StateChanged` events invalidate keys (no polling, no manual refetch).
  High-rate binary streams bypass Query entirely → Zustand/refs → canvas.
- **Tauri v2 desktop app** (`apps/desktop`):
  - Embeds `crates/server` in-process (it's a library) on `127.0.0.1:<ephemeral>` and points
    its WebView at it — identical code path to remote browsing, so there is exactly one frontend.
  - Connection manager: "Local (embedded)" + saved remotes (`pi.local:8080` with optional token).
  - Native niceties: menu bar, dock icon, later auto-update. macOS signing/notarization is a
    packaging task at M5, unsigned dev builds until then.
- **Server-served UI:** same built assets embedded into the server binary via `rust-embed` —
  a Pi deployment is one binary; browse to `http://pi.local:8080`. PWA manifest so phones/tablets
  can "install" it. The UI must stay responsive/usable on a phone (it's the remote control).
- Dev mode: `cargo xtask dev` = server with CORS + Vite dev server proxying `/api` (HMR intact).

---

## 11. Persistence (server-side)

- `config.toml`: port, bind address, token, device backend options. Env-var overridable.
- SQLite (`rusqlite`, bundled — zero system deps) for everything else:
  - **Presets/profiles**: full device-set + channels snapshot (JSON blob, versioned schema).
  - **Workspaces**: tabs + dockview panel layouts + panel↔channel bindings (§10).
  - **Templates**: built-in gallery entries are just read-only presets+workspaces (§10).
  - **Bookmarks**: frequency, mode, label, tags, group.
  - **Decoder logs**: ADS-B sightings, APRS positions, POCSAG/RDS messages — queryable and
    exportable (CSV/JSON) instead of scroll-back-only like most SDR UIs.
  - Recordings index (SigMF files on disk + metadata row).
- Nothing in the browser except UI preferences (theme, layout) in localStorage.

---

## 12. Security model

- Default: bind `0.0.0.0`, no auth — LAN-trusted, same posture as SDRangel/rtl_tcp.
- Optional single shared token (`config.toml` or `--token`): required as `Authorization`
  header / WS query param; one axum middleware. UI prompts and stores it per saved connection.
- CORS locked to same-origin by default (dev mode relaxes it).
- Explicit docs note: exposing an SDR server to the internet is your VPN's job (Tailscale
  et al.), not ours. No TLS termination in v1 (reverse-proxy if needed).

---

## 12a. TX & RF security research (future phase — RX ships first)

The device trait carries a TX half from day one (unimplemented in the RX phases). When TX is
built, it's a **general-purpose, legitimate** transmit + RF-research toolkit — the same class
of capability SDRangel already ships (modulators) and every licensed SDR operator uses. It is
*not* a catalog of attack presets.

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

**Controlled-environment gate (your Faraday-cage idea, as real safety UX):**
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

### Phase 1 — analog core (with M0–M2 infrastructure)
| Feature | Implementation |
|---|---|
| Spectrum/waterfall, per-device-set | self (`rustfft`) |
| NFM, WFM (mono), AM, SSB (USB/LSB/CW) demods | self |
| Squelch, AGC, notch/audio filters | self |
| Audio streaming | libopus (vendored) |
| IQ recording + playback (SigMF) | self (spec is simple JSON+data) |
| Presets, bookmarks | app |
| Frequency tracker / AFC | self |

### Phase 2 — data decoders wave 1 (well-documented protocols, all self in Rust)
| Feature | Notes |
|---|---|
| Broadcast FM: stereo + **RDS** | 57 kHz BPSK, group/AF/RT decode |
| **ADS-B** (1090ES) + aircraft map | preamble correlation, Mode S CRC; MapLibre view |
| **AIS** + ship map | GMSK/NRZI, HDLC framing |
| **POCSAG** pager (512/1200/2400) | classic, easy |
| **AX.25 / APRS** (AFSK1200 + 9600 G3RUH) | + APRS feature collecting stations/positions |
| **RTTY**, **Navtex** (SITOR-B), **Morse decoder** | self |
| **Frequency scanner** | app-level, uses fast retune / HackRF sweep |
| Channel analyzer (scope, constellation) | IQ taps §9 |
| Heat map channel | self |
| **ACARS** (VHF, MSK 2400 over AM) | self; aircraft messages into log DB + map |
| **HF WEFAX** (weather fax) | self — easy and satisfying |
| CTCSS/DCS detection on NFM · Selcall (CCIR/ZVEI) | small |
| **Sub-GHz OOK/ASK/FSK capture+decode** (PT2262/EV1527/Manchester; garage/TPMS/sensors) | self; Flipper-replacement §8b |
| **Signal-strength hunt mode** (fox-hunting / find-the-transmitter) | app; uses RSSI + audio/visual feedback |
| Channel power meter (RSSI + logging) | trivial; pairs with heat map |
| Demod analyzer (scope/spectrum on demodulated audio) | self |
| Per-channel sinks: audio recording, baseband file (SigMF/raw), UDP out | self; UDP feeds external tools (multimon-ng et al.) |
| Spectrum annotations (band plans / editable frequency DB overlaid on spectrum) | app |

### Phase 3 — digital voice & harder modems
| Feature | Notes |
|---|---|
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

Each decoder lands with: settings struct in `wire`, golden IQ fixture test (§14), and a
UI panel or the generic fallback. Definition of done includes running on a Pi.

---

## 14. Testing strategy

- **`dsp`:** unit tests against analytically generated signals + golden vectors
  (e.g. filter responses, PLL lock behavior). `criterion` benches for hot paths.
- **Decoders (the crown jewels):** every decoder ships with short IQ fixtures
  (seconds, checked into `fixtures/` or fetched by `xtask fixtures`) + expected decoded
  output. Recording our own fixture library starts at M3 (record/replay milestone) —
  building it *is* part of building each decoder.
  - **Reference modulators** live once, in `channels::testgen` behind the crate's
    `test-signals` feature (M4 decision). One encoder per protocol feeds all three consumers —
    the decoder's unit tests, the engine's end-to-end runs through `device-virtual`, and
    `xtask fixtures` — without duplicating protocol encoding, and without `channels` gaining a
    dependency (§3: it still depends only on `dsp` + `wire`; the feature is test-only).
  - Synthesized fixtures prove the decoder against the *specification*. Off-air captures prove
    it against the *world* and land per decoder as hardware sessions produce them; a decoder
    without one says so in `PROGRESS.md` rather than pretending coverage it does not have.
- **Engine:** end-to-end tests with `device-virtual`: siggen → channel → assert audio
  RMS/decoded events. No hardware in CI, ever.
- **Server:** axum handler tests via `tower::ServiceExt`; OpenAPI snapshot test;
  codegen-drift check (regenerate → `git diff --exit-code`).
- **Web:** `tsc` strict, vitest for stores/utils, one Playwright smoke flow against the
  real server with `device-virtual` (open device → add NFM channel → see spectrum frames).
- **Performance gates:** criterion benches in CI (regression tracking), plus a manual
  on-Pi soak checklist per release (X channels at Y rate → CPU%, thermals).

---

## 15. Build, packaging, CI

- **Toolchain:** **pinned Rust nightly** (`rust-toolchain.toml`, e.g. `nightly-2026-08-01`)
  with the next-gen borrow checker enabled (`RUSTFLAGS=-Zpolonius=next` in
  `.cargo/config.toml`). The pin is bumped deliberately (not floating) so builds stay
  reproducible; CI uses the same pin. `just`/`cargo xtask` as the only entry points
  (`xtask dev`, `codegen`, `test`, `dist`). pnpm for `web/`.
- **Frontend toolchain:** newest across the board; **TypeScript 7** (`tsgo`), **Biome**
  (format + organize imports), **Oxlint** (lint, type-aware via `tsgolint`). No
  ESLint/Prettier. pnpm for `web/`.
- **CI (GitHub Actions)** — added as the project matures (author locally via `xtask`/`just`
  first; a workflow lands with M0 and grows each milestone):
  - Rust: `cargo fmt --check`, `clippy -D warnings`, tests on ubuntu-x86_64 + macos-arm64,
    cross-build `aarch64-unknown-linux-gnu` (cargo-zigbuild or cross).
  - Web: `biome ci` (format + lint), `oxlint` **with type-aware rules** (the slower,
    thorough gate), `tsgo` typecheck, web build.
  - Cross-cutting: OpenAPI codegen-drift gate (regenerate → `git diff --exit-code`).
- **Release artifacts:**
  - `sdrmm` headless server: linux x86_64 + aarch64, macOS arm64 (web UI embedded).
  - Desktop: Tauri bundles — macOS `.dmg` (signing/notarization at M5), Linux AppImage + `.deb`.
  - Multi-arch Docker image (`--device /dev/bus/usb`) for Pi/NAS deployments.
- **System deps** kept honest, under a hard packaging rule: **release artifacts just run.**
  The desktop bundle and prebuilt `sdrmm` binaries ship the default hardware (RTL-SDR,
  HackRF) compiled in via the native backends — pure-Rust USB (`nusb`) preferred, else
  vendored static C libs — and must never require an install to launch. SoapySDR is
  optional *extra* coverage, never a launch dependency: release builds omit the `soapy`
  feature or load it at runtime, so a missing libSoapySDR costs exotic-device support,
  not startup. What static linking cannot fix stays out of scope and honest: OS USB
  permissions (udev rules, Windows WinUSB binding) and vendor daemons (SDRplay) —
  `sdrmm --doctor` prints what's found (Soapy modules, USB permissions, udev hints).

---

## 16. Milestones (implementation order)

- **M0 — Walking skeleton** *(proves every architectural risk before any real DSP)*
  Workspace + wire + codegen pipeline green in CI · axum serving embedded UI · WS hub ·
  `device-virtual` siggen → spectrum frames → WebGL waterfall in the browser · Tauri shell
  boots the same UI via embedded server · state snapshot + StateChanged invalidation working.
- **M1 — Real hardware**
  Soapy backend, probe/open/capability UI, RTL-SDR + HackRF fully controllable (gains,
  rate, PPM, bias-T…), hotplug robustness. Evaluate seify vs own-FFI fallback here.
- **M2 — Listen** *(daily-usable milestone)*
  DDC channels · NFM/AM/SSB/WFM · squelch/AGC · Opus audio in browser+Tauri · presets ·
  bookmarks · phone-usable UI.
- **M3 — Record & replay**
  SigMF recording, file playback device, recordings browser. Start the decoder fixture library.
- **M4 — Decoders wave 1**
  RDS, POCSAG, ADS-B + map, AIS, APRS/AX.25, RTTY, Morse. Decoder-log database + export.
- **M5 — Ops & UX polish**
  Frequency scanner · auto-reconnect on replug (a faulted device set re-opens and restores
  its channels once its device re-enumerates; today recovery is manual close/re-open) ·
  multi-client polish · token auth · **MCP server** · **template gallery + first-run
  wizard** · native `rtl-native`/`hackrf-native` backends → self-contained binaries
  (§15 packaging rule) · Tauri packaging/signing · Docker/Pi image · docs site · `--doctor`.
  (Workspaces/tabs did **not** land earlier: M0–M4 shipped a fixed panel layout and no
  dockview dependency. The §10 workspace/tab shell is deferred to M6 rather than silently
  claimed — the panels it would host all exist, so it is a shell change, not a feature.)
- **M6+ — Phase 3/4 waves** per §13, prioritized by demand.

---

## 17. Risks & mitigations

| Risk | Mitigation |
|---|---|
| `soapysdr` Rust binding maintenance | own trait isolates it; decided at M1: `soapysdr` 0.5 adopted, seify rejected (§18); fallback minimal FFI stays available |
| Pi CPU budget (many channels, spectrum) | per-connection throttling, decimation discipline, benches + on-Pi gates, PFB channelizer in back pocket |
| Browser audio (latency, autoplay) | AudioWorklet + jitter buffer + gesture unlock; WebCodecs fast path |
| AMBE/IMBE patents, fdk-aac license | accepted — personal, non-distributed project; default-on |
| Nightly toolchain / `-Zpolonius=next` breakage or slowdowns | pinned nightly, bumped deliberately with CI green as the gate; the flag is one line to drop if a pin misbehaves |
| Tauri v2 churn, macOS signing | desktop is a thin shell over the always-working web path; signing deferred to M5 |
| Scope explosion (SDRangel parity) | plugin API stability is the real product; M2 is already a usable SDR; parity accretes decoder-by-decoder |

---

## 18. Decision log

| Decision | Choice |
|---|---|
| Name | `sdr--`, crates/binary prefix `sdrmm` |
| Client shipping | Tauri desktop app from day one + server-served web UI (one frontend) |
| Transport | REST (OpenAPI) + one WebSocket (events + binary streams) |
| Type sharing | OpenAPI codegen from Rust — `wire` crate is the single source of truth (plain Rust structs + serde/utoipa; no Protobuf) |
| Access model | LAN-trusted, optional shared token |
| License | MIT; personal project. GPL code as reference only, no direct copying |
| Digital voice (DMR/P25 …) | default-on, voice included; use proven libs (DSDcc/mbelib) via FFI |
| Performance floor | Raspberry Pi 4 |
| Toolchain | pinned Rust nightly + `-Zpolonius=next` (next-gen borrow checker) |
| UI components | shadcn/ui on Base UI primitives, Tailwind v4 |
| Frontend toolchain | TypeScript 7 (`tsgo`) + Biome (format) + Oxlint (lint, type-aware); no ESLint/Prettier; newest versions always |
| CI | GitHub Actions, added incrementally (workflow lands at M0, grows per milestone) |
| MQTT / Home Assistant export | rejected — not wanted |
| UI shell | Workspaces (one active) → unlimited tabs → dockview panel layouts, server-persisted |
| Maps | MapLibre GL; OpenFreeMap default (no key), self-hosted PMTiles for offline, optional satellite-imagery key |
| MCP server | yes — `rmcp` over streamable HTTP at `/mcp`, same token auth (M5) |
| Onboarding | template gallery + first-run wizard + band-plan explorer (M5) |
| Coherent arrays | generic `CoherentArray` abstraction (§6), NOT a Kraken-specific driver — KrakenSDR is one populator, any synced N-RX (future Dragon-class boards, RTL banks) works the same; DoA + passive radar + beamforming (stretch) |
| Decoder events (M4) | typed `DecoderEvent` in `wire`, emitted by channels as owned values (never JSON on the DSP thread); own broadcast + bounded hand-off queue with reported drops (§5); persisted by the server, never by the engine (crate boundary) |
| BFM stereo vs RDS (M4) | RDS is a `wfm` param, not a second channel type — one FM demod, one filter chain. WFM **stereo** is deliberately *not* in M4 (M4 §16 lists RDS only): it changes the whole audio path to two channels (PCM, Opus, frame `ch_layout`, AudioWorklet) and is tracked as the remaining half of the §19 `demodbfm` row |
| RTTY/Morse channel rate (M4) | 8 kHz DDC output, not 48 kHz: a 400 Hz CW filter at 48 kHz needs ~2 700 taps to keep its shape factor, which blows the Pi 4 budget for one channel (§14 performance floor) |
| Wideband channels vs the DDC (M4) | A rate conversion costs bandwidth: the DDC delivers only 80% of the output rate flat, the rest being the guard band that stops folding. A mode occupying more than that — ADS-B fills its entire 2 MHz channel — cannot be resampled into place, so the engine **refuses** it unless the device runs at exactly the channel rate, naming that rate. Found by the M4 end-to-end run: at 2.4 Msps the pulses were smeared and the decoder produced nothing, which is indistinguishable from an empty sky. **Follow-up (M5+):** a wideband DDC mode that trades the guard band for bandwidth would let ADS-B run at any device rate; until then 1090 MHz means tuning the device to 2 Msps |
| Frequency-allocation DB | layered World/ITU → Germany (BNetzA Frequenzplan) → future US/UK; overlaid on spectrum + searchable (§8a) |
| HackRF/PortaPack/Flipper RX parity | in scope for the RX half (§8b); Sub-GHz OOK/FSK channel + capture |
| TX & RF security testing (future) | in scope behind a default-off "controlled RF environment / authorized test" gate: siggen, IQ-to-air, modulators, bench loopback, **sub-GHz capture/replay/fixed-code (de Bruijn)/rolling-code analysis**, **jam-susceptibility testing**, **flood/spam/malformed-broadcast testing against a DUT**, **targeted fuzzing** — all framed for contained (direct-connect/dummy-load/shielded) authorized use (§12a) |
| NanoVNA | planned as tools-tab integration via USB serial (P4) |
| Native backends (M5) | `rs-rtl` 0.4.2 (RTL-SDR) + `hackrf-nusb` 0.3 (HackRF), both pure Rust over `nusb` 0.2 — so a release artifact links no C library and launches with nothing installed (§15). Rejected: `rtl-sdr-rs`/`seify-rtlsdr`/`rtlsdr_mt` (rusb/libusb or librtlsdr — a C dependency), `waverave-hackrf` (stale, pulls nusb 0.1 *and* 0.2), `seify` (wraps hackrf-nusb and duplicates our own device abstraction). Known gaps, documented rather than faked: no PPM or direct-sampling through rs-rtl's public API, no independent baseband-filter bandwidth or hardware sweep through hackrf-nusb's. Registered above Soapy in the serial merge |
| RTL-SDR driver, re-evaluated (post-M5) | Re-checked on technical merit alone after §17 accepted GPL dependencies, so the original licence-based rejection of `rtlsdr-pure` no longer decides anything. **Outcome: keep `rs-rtl` 0.4.2** — it is the only candidate whose read path keeps a queue of USB transfers in flight (`NUM_TRANSFERS = 15`, `Endpoint::submit` × 15 then `wait_next_complete`). `rtlsdr-pure` 0.2.3's sole read API is `read_bytes(&self, len)` over a `BulkReader` that submits **one** buffer and awaits it (`rtl2832.rs:880-881`), leaving the endpoint empty between transfers. **Measured on the NESDR SMArt v5 rather than argued** (RTL2832U test mode emits an 8-bit counter ramp, so every lost byte is a countable discontinuity): idle, rtlsdr-pure is fine — 8 ramp breaks in 131 MB at 2.4 MS/s, full rate delivered, so the "starves the FIFO" claim is *false as stated for an idle machine*. Under 16 spinning threads it loses ~2.2% of the stream (2.048 MS/s: 4.01 of 4.10 MB/s, 241 breaks; 2.4 MS/s: 4.69 of 4.80 MB/s, 254 breaks) — roughly one discontinuity every 0.1 s, which is fatal for decoder framing and, on an odd-length gap, permanently swaps I/Q against `convert.rs`'s cross-block alignment. rs-rtl on the same machine, load, rates and byte count delivered **100.0%** with 0 dropped chunks. The 15-transfer queue is exactly the margin that buys, and sdr-- runs a full DSP pipeline on those cores. rtlsdr-pure would also *regress* two working controls: direct sampling is absent entirely, and the bias tee is unreachable (`set_gpio_bit` is `pub(crate)`), so its PPM support does not pay for it. `rtl-sdr-rs` 0.3.3 / `seify-rtlsdr` 0.0.4 expose the full control set (PPM, direct sampling, offset tuning) but only `read_sync(&self, buf)` over a single blocking `rusb::read_bulk` — same starvation, plus the C dependency. Both remaining gaps are ours to close, not a reason to switch. The transient-stall teardown is a defect in rs-rtl's error accounting and is fixable entirely on our side (see PROGRESS "Known gaps"). PPM and direct sampling need upstream setters, *not* a register escape hatch: `rs_rtl::device::Device` and `rs_rtl::tuner::R82xx` are both `pub` and constructible, but `RtlSdr` hands out neither, and PPM's sample-rate half is computed inside `set_sample_rate` from the private `rtl_xtal_freq` (`rtlsdr.rs:557-563`) while retuning goes through the private `R82xx` built with the private `tuner_xtal_freq` — so driving them downstream means shadowing rs-rtl's own state and losing to it on the next retune. The right change is librtlsdr-shaped `set_freq_correction()` / `set_direct_sampling()` methods on `RtlSdr`; upstream (`xoolive/desperado`) is active and its HEAD is byte-identical to the published 0.4.2, so this is a PR to file, not a fork |
| MCP server (M5 implementation) | `rmcp` 3.1 streamable-HTTP at `/mcp`, **stateless** (`legacy_session_mode = false`, `json_response = true`): no session to garbage-collect, nothing lost across a restart, and the tools need no server-initiated notifications. rmcp's DNS-rebinding host guard is disabled because it defaults to localhost-only and would 403 every LAN client — the shared token is what gates the endpoint, matching §12's posture for REST |
| Frequency scanner (M5) | App-level and control-plane only: the unit of work is a *device tuning*, not a target, so one dwell measures every target inside the passband off the existing spectrum tap — no extra DSP, which is what makes it affordable on a Pi 4. A running scan owns its set's centre frequency and client retunes are refused while it does, rather than the two fighting |
| Token auth (M5) | One `route_layer` middleware over the routed API + WS + MCP, deliberately *not* the SPA fallback (the login UI must load unauthenticated, and an unmatched `/api/*` stays a typed 404 instead of a 401). Accepted as `Authorization: Bearer` **or** `?token=`, because the browser WebSocket API cannot set headers and the decoder-log export is a plain navigation. `/api/auth`, `/api/openapi.json` and `/api/docs` stay public: they describe the API's shape, never its data |
| Templates (M5) | A static Rust table, not seeded SQLite rows: templates ship with the binary, so rows would need a migration per edit and a user could delete an entry the next release restores. Presets remain the writable, device-bound half of the same idea |
| Soapy binding (M1 evaluation) | `soapysdr` 0.5 over seify: seify duplicates our device/capability abstraction, its production path is the same libSoapySDR, and it had 3 breaking releases in 6 weeks; its native drivers are self-declared experimental. Binding gaps worked around: no `setFrequencyCorrection` wrapper (PPM via the `"CORR"` frequency component), no `getSettingInfo` (per-driver extra-settings tables in `device-soapy`) |

---

## 19. Appendix — SDRangel parity ledger

Verified against the SDRangel master plugin tree (Aug 2026). This is the authoritative
checklist; the phase tables in §13 are the plan view of the same list. Statuses:
✅ planned (phase noted) · ⏭ deliberately skipped · 🔵 covered structurally (not a discrete plugin for us).

### Channel RX (44 plugins)

| SDRangel plugin | Ours | Phase |
|---|---|---|
| chanalyzer | ✅ channel analyzer | P2 |
| channelpower | ✅ channel power meter | P2 |
| demodadsb | ✅ ADS-B + map | P2 |
| demodais | ✅ AIS + map | P2 |
| demodam | ✅ AM | P1 |
| demodapt | ✅ NOAA APT | P3 |
| demodatv | ✅ ATV | P3 |
| demodbfm | ✅ folded into `wfm`: RDS landed at M4 as a `wfm` param; **stereo still open** | P2 |
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
| demodnavtex | ✅ Navtex | P2 |
| demodnfm | ✅ NFM | P1 |
| demodpacket | ✅ AX.25 | P2 |
| demodpager | ✅ POCSAG (FLEX later) | P2 |
| demodradiosonde | ✅ Radiosonde | P3 |
| demodrtty | ✅ RTTY | P2 |
| demodssb | ✅ SSB/CW | P1 |
| demodvor / demodvormc | ✅ VOR | P3 |
| demodwfm | ✅ WFM | P1 |
| filesink / sigmffilesink | ✅ per-channel baseband sinks + device recorder | P2–P3 |
| freqscanner | ✅ frequency scanner | P2 |
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

| SDRangel plugin | Ours | Phase |
|---|---|---|
| afc | ✅ AFC | P1 |
| ais (feature) | 🔵 folded into AIS channel + map + decoder-log DB | P2 |
| ambe | 🔵 mbelib FFI in DSD; hardware AMBE dongle/server support optional later | P3 |
| antennatools | ✅ antenna calculators | P4 |
| aprs | ✅ APRS feature (stations, positions, log) | P2 |
| demodanalyzer | ✅ demod analyzer | P2 |
| denoiser | ✅ audio NR (`nnnoiseless`) | P3 |
| freqdisplay | ⏭ big-frequency readout — just part of our normal UI |  |
| gs232controller | ✅ rotator control (GS-232 + rotctld) | P4 |
| jogdialcontroller | ⏭ keyboard/scroll-wheel tuning covers it |  |
| limerfe | ⏭ hardware-specific; via Soapy settings if ever |  |
| map | ✅ map feature (layers grow: aircraft, ships, sondes, sats, beacons; ionosonde/MUF later) | P2→P4 |
| morsedecoder | ✅ Morse decoder | P2 |
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

| SDRangel plugins | Ours |
|---|---|
| airspy, airspyhf, bladerf1/2, fcdpro(+), fobos, limesdr, perseus, plutosdr, sdrplay(v3), usrp, xtrx, aaroniartsa, soapysdrinput | 🔵 via `device-soapy` (module availability varies; Soapy is the contract) |
| rtlsdr, hackrfinput | ✅ Soapy by default + native backends for extra features |
| fileinput, sigmffileinput, testsource | ✅ `device-virtual` |
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

*(Correction, verified by user: SDRangel does support external GPS dongles for mobile
heat maps — GPS is parity, tracked in the P4 table above.)*

The consolidated idea backlog lives in §20 below.

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
