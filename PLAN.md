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
- Feature target: SDRangel's, Mayhem Firmware, Flipper Zero Momentum feature set of SDR
- Backend-driven: the server is the single source of truth for state, settings, and type
  definitions. The client renders what the server describes. Adding a device setting or a new
  channel type requires zero hand-written frontend DTOs.
- **Many radios at once:** unlimited simultaneous device sets (SDRangel-style), and
  cross-device features on top (scanner spanning devices, multi-VOR fix, diversity,
  DoA/TDoA).
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
  vetted for Linux-arm64 + macOS support, see §13 table). For small libaries shoose to copy the code into the Project and bring it into our style.

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
- **RF replay-capture:** record the exact IQ of a burst (garage remote, sensor) to
  the IQ time machine / SigMF, annotate, analyze it and Re-transmitting.
- Morse, RTTY, SSTV(RX), radio clock, VOR/ILS — already planned.

**Nature/analysis features (PortaPack parity):** waterfall, audio RX (AM/NFM/WFM/SSB), signal
recording, frequency manager/bookmarks, band-plan awareness → all core sdr-- already.

**TX / deferred**: sub-GHz *replay/brute/jam*, BLE/OOK *spam*, RF *transmit* of any kind, "spoof" tools. 

A **"Sub-GHz workbench" template** (§10) bundles the OOK/FSK channel + capture + a decoder log
into a one-click Flipper-replacement layout.

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

## 12a. TX & RF security research

It's a **general-purpose, legitimate** transmit + RF-research toolkit.

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

**Operating principle (written into the docs):** these are test instruments for *contained,
authorized* assessment — direct-connect, dummy load, or shielded, against devices you're
authorized to test. The project ships them framed and gated that way; it does not ship
presets whose purpose is uncontrolled over-the-air disruption of third-party systems. With the
containment the operator attests to, jam-susceptibility and flood/spam testing deny service to
nothing but the DUT — which is the whole point of the test.

---

## 13. Features

| Feature | Implementation |
|---|---|
| Notch / audio filters per channel | self |
| Frequency tracker / AFC | self |
| RDS (stereo still open, §18), ADS-B + map, AIS + map, POCSAG, AX.25/APRS (Mic-E still open), RTTY, Morse, frequency scanner. NAVTEX(SITOR-B), ACARS, and the sub-GHz OOK/FSK capture-and-decode channel (§8b). | self |
| **HF WEFAX** (weather fax) | self — blocked on transport, not on DSP: a fax page is an image, and §5 has no frame kind for one. Needs an `IMAGE` binary frame plus a canvas panel before the demod is worth writing |
| **Signal-strength hunt mode** (fox-hunting / find-the-transmitter) | app; uses RSSI + audio/visual feedback |
| Channel analyzer (scope, constellation) | IQ taps §9 |
| Demod analyzer (scope/spectrum on demodulated audio) | self |
| Heat map channel · channel power meter (RSSI + logging) | self; pair with each other |
| CTCSS/DCS detection on NFM · Selcall (CCIR/ZVEI) | small |
| Per-channel sinks: audio recording, baseband file (SigMF/raw), UDP out | self; UDP feeds external tools (multimon-ng et al.) |
| APRS *feature* (station/position collection, distinct from the channel) | app |
| Spectrum annotations (band plans / editable frequency DB overlaid on spectrum) | app; §8a |
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
| **Satellite tracker** | `sgp4` crate (well-maintained), TLE fetch, pass prediction, doppler-corrects linked channels |
| **Rotator control** (GS-232, rotctld) + **rig ctl server** (rigctld protocol compat) | self |
| **Star tracker / radio astronomy** (integrating radiometer, spectral line) | self |
| **Sky map** (celestial view, companion to star tracker) | self, client-side render |
| **Map feature** (consolidated), **SID monitor**, **noise figure**, **PER tester** | self |
| **GPS position source** (gpsd / NMEA serial): live station position for maps & trackers, geotagged mobile heat map (drive-around coverage), auto grid locator | parity — SDRangel supports external GPS dongles for mobile heat maps; ours adds auto grid locator + geotagged recordings |
| **Antenna tools** (dipole/λ calculators) · **3D spectrogram** view | trivial / WebGL eye-candy |
| **Remote sink/source** between sdr-- instances; **rtl_tcp / SpyServer client devices**; **KiwiSDR client**; **audio-input device** (`cpal`) | cheap wins, huge reach |
| **Meteor M-2 LRPT** (digital weather-sat imagery: QPSK, Viterbi+RS) | self; SatDump as reference — pairs with pass automation |
| **Trunking following** (P25 / DMR Tier III: decode control channel, auto-steer voice channels; multi-dongle aware) | SDRTrunk as reference |
| **CW skimmer** (decode every CW signal in the passband simultaneously) | self |
| **TETRA**  | self |
| **Coherent array DoA**: bearings on the map (MUSIC/ESPRIT), multi-station triangulation; **passive radar** (range-Doppler); **beamforming / diversity combine** | targets the `CoherentArray` abstraction (§6) — KrakenSDR today, any synced N-RX later; stretch |
| **NanoVNA integration** (USB serial, documented protocol): antenna sweeps, SWR/Smith-chart panels, saved antenna profiles | tools tab |
| **DATV (DVB-S/S2)** | stretch; FFI candidates (leandvb-style) or long-term self |
| MIMO: interferometer, **DOA2 direction finding** | needs coherent hardware; timestamping from §5 is the prerequisite |

---

## 15. Build, packaging, CI

- **Release artifacts:** `sdrmm` headless (linux x86_64 + aarch64, macOS arm64, UI embedded),
  Tauri desktop bundles, and a multi-arch Docker image (`--device /dev/bus/usb`) for Pi/NAS.
- **The hard packaging rule: release artifacts just run.** The default hardware (RTL-SDR,
  HackRF) is compiled in via the native backends over pure-Rust USB, so a release binary links
  no C radio library and needs nothing installed. SoapySDR is optional *extra* coverage, never
  a launch dependency: a missing libSoapySDR costs exotic-device support, not startup. What
  static linking cannot fix stays out of scope and honest — OS USB permissions (udev rules)
  and vendor daemons (SDRplay); `sdrmm --doctor` prints what is found and what to fix.


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
| Many WebGL plots on one canvas (M7) | browsers cap live GL contexts — one shared renderer for all visible scope faces, render only on-screen faces, re-render at zoom-adjusted DPR (`CANVAS §7`) |
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
| Onboarding | template gallery + first-run wizard + band-plan explorer (M5) |
| Coherent arrays | generic `CoherentArray` abstraction (§6), NOT a Kraken-specific driver — KrakenSDR is one populator, any synced N-RX (future Dragon-class boards, RTL banks) works the same; DoA + passive radar + beamforming (stretch) |
| Decoder events (M4) | typed `DecoderEvent` in `wire`, emitted by channels as owned values (never JSON on the DSP thread); own broadcast + bounded hand-off queue with reported drops (§5); persisted by the server, never by the engine (crate boundary) |
| BFM stereo vs RDS (M4) | RDS is a `wfm` param, not a second channel type — one FM demod, one filter chain. WFM **stereo** is deliberately *not* built: it changes the whole audio path to two channels (PCM, Opus, frame `ch_layout`, AudioWorklet) and is tracked as the remaining half of the §19 `demodbfm` row |
| RTTY/Morse channel rate (M4) | 8 kHz DDC output, not 48 kHz: a 400 Hz CW filter at 48 kHz needs ~2 700 taps to keep its shape factor, which blows the Pi 4 budget for one channel (§14 performance floor) |
| Wideband channels vs the DDC (M4, **superseded**) | A rate conversion costs bandwidth: the DDC delivers only 80% of the output rate flat, the rest being the guard band that stops folding. A mode occupying more than that — ADS-B fills its entire 2 MHz channel — cannot be resampled into place, so the engine refused it unless the device ran at exactly the channel rate. Found by the M4 end-to-end run: at 2.4 Msps the pulses were smeared and the decoder produced nothing, which is indistinguishable from an empty sky. Superseded by the row below: the follow-up this row asked for turned out not to be a wider DDC |
| Channels that read the radio's own rate (post-M7) | **ADS-B runs at whatever rate the receiver is set to, 2–4 MHz** (`ChannelDescriptor::native_rate_max_hz`): the engine gives such a channel the device's samples mixed to its offset and nothing else, and the decoder's half-chips are 1.024 samples wide on an RTL-SDR instead of 1. The rule it replaces cost the commonest ADS-B receiver there is — no RTL-SDR can produce 2.000 Msps, its nearest rate is 2.048, so "set the device to exactly 2 MHz" was a dead end with no way out. **Why not a wider DDC**, which the M4 row proposed: the filter is not the problem. At 2 Msps a 0.5 µs pulse *is one sample*, so any rate change splits it across two — measured through the production DDC and through an unfiltered interpolation, both decode nothing. The decoder has to meet the radio at its rate, which is what dump1090 does (one hard-coded rate at a time; this one is continuous). The 4 MHz ceiling is a DSP-thread budget, not a DSP limit — the scan costs a magnitude per sample and the Pi 4 is the floor (§1) — and is refused with the range named, never silently. **Amended after the first off-air run:** rate-flexible windows alone still decoded nothing on a real radio, because a transmitter's bit clock owes the receiver's sample grid nothing — and the tests never noticed, because the generator's sample-to-chip mapping *was* the decoder's window arithmetic and every generated frame started phase-0 (a tautology). At a non-integer samples-per-chip the leftover sub-sample phase shifts within the frame, so the slicer now tries eight sub-sample phase tables per candidate (the CRC arbitrates) and reads each half-chip as an overlap-weighted energy rather than a single-sample peak — dump1090's hard-coded 2.4 Msps demodulator, generalized to any rate. The generator band-limits now (16× render, aperture integration), so a decoder test can never share the decoder's arithmetic again. Exactly 2.000 Msps keeps a physical blind spot near half-sample phases — every sample integrates half a pulse and half a gap and reads the same level, so there is nothing left to decode — which the 2.048 real receivers actually produce does not have |
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
| Canvas-first client (M7) | **Supersedes the two M6 rows above** ("UI shell", "Workspace layout model"): the tabs→dockview shell and its `LayoutNode` tree are removed, replaced by a patch-graph canvas + pin-board rack, modelled as `PatchGraph` + `RackLayout` in `wire` (own model, never the canvas library's serialization — same reasoning, new document). Motivation: with several radios, identity must be *spatial* — a labelled device node and the wires leaving it answer "which SDR is this?" structurally, where tabbed UIs (SDRangel) answer it with a dropdown. Full model in `PLAN-CANVAS.md`; M6's shipped work is deleted in its final phase, recorded not hidden |
| Mobile support (M7) | Removed entirely — §10 previously bound phone usability and viewport-guarded layout writes; both are gone. Desktop-only assumptions (pointer, keyboard, laptop-class viewport) are allowed everywhere. Cost accepted and recorded: the phone-as-remote-control use case dies with it |
| Canvas library (M7) | React Flow (`@xyflow/react` 12, MIT — verified Aug 2026) over tldraw (production use needs a license key, the free tier forces a watermark — and it is a whiteboard, not a node graph) and Rete/litegraph (MIT but not React-idiomatic). Node faces are plain React components, so Base UI parts and our tokens carry into every node |
| Stable device identity (M7) | Graduates from deferral (the M6 "panels name no device set" rule) to prerequisite #1: `PatchGraph` names devices by backend + serial; an absent device is a visibly disconnected node, never a silent rebind. Serial-less duplicate clones bind at most one node and `--doctor` suggests programming an EEPROM serial. Built with one addition, `CANVAS §3`: a `key` tie-break consulted only when a backend exposes no serial, without which a patch could not name *which* recording a file-playback node plays |
| Graph applies additively (M7) | `POST /api/workspaces/{id}/apply` opens the radios a station names and creates the channels it draws, and never closes or deletes: removing a node is its own gesture, and a reconciler that also deleted would read "this workspace has fewer nodes than the engine has channels" — the normal state when a second client adds one — as an instruction to close someone's radio. Idempotent, so it runs on every station load, which is what makes a restart come back as a station. Rejected alternative: storing channel settings in the graph so it could be a full desired-state reconciler — one revision-checked blob per workspace means every squelch turn would become a workspace write and two clients editing different channels would 409 each other (`CANVAS §4`) |
| The rate rule on the wire (M7) | `ChannelDescriptor.exact_rate_only` is derived in `channels` from the same `occupied_band` + `resamplable_bandwidth_hz` the engine's admission check uses, and shipped on the wire, so the canvas can refuse an ADS-B wire to a 2.4 Msps receiver where the operator drew it. Rejected: re-deriving the 80% guard-band constant in TypeScript, which is a second implementation of a DSP rule that would drift from the first |

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
