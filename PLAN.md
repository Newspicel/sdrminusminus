# sdr-- — Feature List

Everything sdr-- does, will do, or has deliberately decided not to do — collapsed into one
list. Three states only: **[shipped]** works today, **[planned]** is intended, **[skipped]**
is a deliberate no. Within each section, shipped comes first.

---

## 1. Platform & deployment

- **[shipped]** Client–server split — Rust server does all DSP/decoding, React client renders
- **[shipped]** Server is authoritative — every client (desktop window, browsers, scripts) sees the same state and converges over one WebSocket
- **[shipped]** One frontend, two hosts — served by the server for browser access, and bundled into the Tauri desktop app
- **[shipped]** Desktop app spawns an embedded local server *or* connects to a remote one; saved remote connections
- **[shipped]** Run modes — single local app on a laptop, or server on a Raspberry Pi with the client anywhere on the LAN
- **[shipped]** Linux (x86_64 + aarch64) and macOS (arm64); Raspberry Pi 4 is the performance floor
- **[shipped]** Release artifacts just run — no C radio library linked, nothing to install for the default hardware
- **[shipped]** `sdrmm --doctor` — reports what hardware was found and what needs fixing (udev rules, vendor daemons)
- **[shipped]** Headless binary, Tauri desktop bundles, multi-arch Docker image (`--device /dev/bus/usb`)
- **[skipped]** Windows
- **[skipped]** Mobile/phone layouts — desktop-only assumptions (pointer, keyboard, laptop viewport) allowed everywhere; the phone-as-remote-control case is gone

## 2. Device support

- **[shipped]** RTL-SDR — native in-tree driver (RTL2832U + R82xx) over pure-Rust USB
- **[shipped]** HackRF — native in-tree driver, both directions on the transport
- **[shipped]** SoapySDR backend as optional extra coverage: airspy, airspyhf, bladeRF 1/2, FUNcube Pro(+), Fobos, LimeSDR, Perseus, PlutoSDR, SDRplay (v3), USRP, XTRX, Aaronia RTSA — availability varies by platform
- **[shipped]** Virtual devices — file input, SigMF file input, test source / signal generator
- **[shipped]** Auto-rendered device UI — frequency ranges, sample rates, named gain stages, antennas, bandwidths and typed extra settings come from the device's capability model; a new setting needs zero frontend work
- **[shipped]** Native driver wins over Soapy for the same physical radio; duplicates collapsed by serial
- **[shipped]** Stream reliability — in-place restart supervisor (~1–7 ms) and silent-stall detection before anything destructive happens
- **[shipped]** PPM frequency correction
- **[planned]** Direct sampling (HF via RTL-SDR)
- **[planned]** rtl_tcp / SpyServer client device
- **[planned]** KiwiSDR client device
- **[planned]** Remote source/sink between sdr-- instances; local routing between device sets
- **[planned]** Audio-input device (`cpal`) — soundcard as a receiver
- **[planned]** rtl_tcp *server* (remote TCP sink) — serve your radio to other tools
- **[skipped]** Android SDR driver input

## 3. Many radios at once & coherent arrays

- **[shipped]** Unlimited simultaneous device sets — several radios open and running side by side
- **[planned]** Cross-device features: a scanner spanning devices, multi-VOR fix, diversity
- **[planned]** `CoherentArray` — N clock-synced receivers as one hardware-agnostic array with per-channel gain/phase calibration, noise-source/pilot alignment, and time-aligned multi-lane output
- **[planned]** KrakenSDR support — via its Heimdall DAQ network stream first, direct hardware drive later
- **[planned]** Generic synced bank — any N receivers on a shared reference clock (future multi-channel boards, USRP MIMO, phase-locked RTL/Airspy banks)
- **[planned]** Network coherent source — aligned multi-lane IQ from another sdr-- node or a DAQ
- **[planned]** Direction finding (MUSIC/ESPRIT) with bearings on the map; multi-station triangulation
- **[planned]** Passive radar (range-Doppler)
- **[planned]** Beamforming and diversity combine
- **[planned]** Interferometer
- **[planned]** TDoA geolocation across distributed sdr-- nodes

## 4. Spectrum, tuning & navigation

- **[shipped]** Live spectrum + waterfall with averaging and peak hold, throttled per client
- **[shipped]** Frequency manager: presets, bookmarks
- **[shipped]** Frequency scanner — one dwell measures every target inside the passband; a running scan owns its device's tuning
- **[shipped]** Wideband sweep (HackRF) driving the scanner
- **[planned]** Frequency tracker / AFC — lock a channel onto a drifting signal
- **[planned]** Channel power meter (RSSI + logging)
- **[planned]** Heat map channel
- **[planned]** Strongest-signal "close-call" finder
- **[planned]** Signal-strength **hunt mode** — Geiger-style audio/visual feedback as you close on a transmitter (fox-hunting, rogue emitters)
- **[planned]** 3D spectrogram view (WebGL)
- **[planned]** Band occupancy analytics — long-term activity heatmaps from scanner and heat-map data

## 5. Frequency-allocation database — "what is this frequency?"

- **[planned]** Band-plan / allocation layer overlaid on the spectrum and searchable
- **[planned]** Layered scopes, most-specific-wins: **World** (ITU Regions 1/2/3 + global services) → **Germany** (BNetzA Frequenzplan) → **US** (FCC), **UK** (Ofcom), EU CEPT and more as pluggable importers
- **[planned]** Region chosen in settings or auto-selected from GPS
- **[planned]** Band ruler under the spectrum with colored allocation blocks
- **[planned]** Click-to-identify popover — service name, allocation, suggested mode, channel step, notes
- **[planned]** Searchable **band explorer** ("show me marine VHF", "70 cm ham")
- **[planned]** One-click "tune here with the suggested mode"
- **[planned]** Amateur band plans (IARU R1) as an overlay
- **[planned]** User-extendable and override-able entries, layered over the shipped set
- **[planned]** Re-runnable importers with per-row provenance (source, version, retrieval date)
- **[planned]** Community overlays, "band plan of the day"

## 6. Recording, capture & replay

- **[shipped]** Device-level SigMF recorder (lossless), files on disk as the source of truth
- **[shipped]** Recordings index, reconciled against the files
- **[shipped]** Decoder log persisted server-side, queryable rather than scroll-back-only
- **[shipped]** Decoder log export (CSV/JSON) as a plain download
- **[planned]** Per-channel sinks — audio recording, baseband file (SigMF/raw), UDP out to external tools (multimon-ng, rtl_433 …)
- **[planned]** RF replay-capture — record the exact IQ of a burst (garage remote, sensor), annotate and analyze it
- **[planned]** **IQ time machine** — rolling per-device ring buffer, retro-record the last N seconds *after* you hear something
- **[planned]** Inspectrum-style offline IQ viewer in the browser — zoomable spectrogram, cursors, symbol/measurement tools
- **[planned]** Annotated recordings — label events on a capture's timeline
- **[planned]** Recording scheduler + unattended satellite-pass automation
- **[planned]** Wideband recording + offline re-channelization — record a band once, mine channels from it later
- **[planned]** Session/replay sharing — recording + workspace + annotations as one openable bundle

## 7. UI, workspaces & onboarding

- **[shipped]** Patch-graph canvas + pin-board rack — devices are labelled nodes, wires answer "which SDR is this?" spatially
- **[shipped]** Stable device identity — nodes name a backend + serial; an absent device is a visibly disconnected node, never a silent rebind
- **[shipped]** Workspaces ("stations") that apply **additively** — loading one opens the radios it names and creates the channels it draws, and never closes or deletes anyone else's work; survives a restart
- **[shipped]** Canvas refuses invalid wiring — e.g. an ADS-B channel onto a 2.4 Msps receiver, using the same rate rule the engine enforces
- **[shipped]** Generic schema-rendered settings forms for any channel without a dedicated panel
- **[shipped]** Dedicated panels where they earn it (ADS-B table + map, RDS display, …)
- **[shipped]** MapLibre map
- **[planned]** Template gallery + first-run wizard + beginner band-plan explorer
- **[planned]** "Sub-GHz workbench" template — OOK/FSK channel + capture + decoder log in one click
- **[planned]** Map layers — sondes, satellites, beacons, MUF
- **[planned]** Theme/skin system and a layout marketplace for shared workspaces
- **[planned]** Accessibility pass — screen-reader labels, high contrast, audio cues
- **[planned]** Localization (DE/EN first), pairing with per-region frequency plans
- **[skipped]** Big-frequency readout as a separate feature — it's just part of the normal UI
- **[skipped]** Jog-dial controller — keyboard and scroll-wheel tuning cover it

## 8. Voice & analog channels

- **[shipped]** AM
- **[shipped]** NFM
- **[shipped]** SSB
- **[shipped]** WFM (mono)
- **[shipped]** RDS — a parameter of the WFM channel, not a second channel type
- **[planned]** WFM **stereo** — two-channel PCM/Opus/AudioWorklet path
- **[planned]** ATV (analog TV)
- **[planned]** Notch and audio filters per channel
- **[planned]** CTCSS/DCS detection on NFM
- **[planned]** Selcall (CCIR/ZVEI)

## 9. Digital voice

- **[planned]** **DSD suite — DMR, D-Star, YSF, NXDN, P25, dPMR — default-on, voice included**
- **[planned]** M17 (fully open protocol)
- **[planned]** FreeDV (Codec2 modes + FDMDV modems)
- **[planned]** Trunking following — decode the P25 / DMR Tier III control channel and auto-steer voice channels, multi-dongle aware
- **[planned]** Hardware AMBE dongle/server support — optional, mbelib covers the software path

## 10. Aviation & marine

- **[shipped]** ADS-B + map — runs at whatever rate the receiver is set to (2–4 MHz), so any RTL-SDR works
- **[shipped]** AIS + map
- **[shipped]** ACARS — strict, repairs nothing: parity and the ARINC 618 CRC both pass or the block is dropped
- **[shipped]** NAVTEX (SITOR-B) — emits only what sits between `ZCZC` and `NNNN`
- **[planned]** VOR
- **[planned]** VOR localizer — multi-VOR position fix on the map
- **[planned]** ILS
- **[planned]** DSC
- **[planned]** Inmarsat STD-C / AERO
- **[planned]** VDL Mode 2 (D8PSK 31.5k, the ACARS successor)
- **[planned]** HFDL, Iridium bursts
- **[planned]** ADS-B / AIS log enrichment against offline aircraft and ship databases

## 11. Data, text & paging

- **[shipped]** POCSAG paging
- **[shipped]** RTTY
- **[shipped]** Morse decoder
- **[shipped]** AX.25 / APRS channel
- **[planned]** Mic-E position encoding — the one AX.25 form still undecoded
- **[planned]** APRS *feature* — station/position collection, distinct from the channel
- **[planned]** FLEX (and further pager formats), ERMES
- **[planned]** CW skimmer — decode every CW signal in the passband at once
- **[planned]** Tetrapol, STANAG modem identification, GSM downlink analysis (grgsm-style), OsmocomBB-style monitoring

## 12. Sub-GHz, ISM & IoT

- **[shipped]** **Generic sub-GHz OOK/ASK/FSK capture-and-decode channel** (315/433/868/915 MHz — garage doors, TPMS, weather stations, doorbells, key fobs): recognizes pulse-width (PT2262/EV1527/Princeton family) and Manchester encodings, logs frames, raw timing capture for unknown signals. Names no chip — a frame carries every reading that fits and the operator decides. Repeats inside 500 ms collapse into one counted event
- **[planned]** ISM sensor suite (rtl_433-style: weather stations, TPMS, utility meters — top protocols); escape hatch is a UDP sink into the rtl_433 binary
- **[planned]** ChirpChat / LoRa
- **[planned]** Meshtastic (on the ChirpChat engine)
- **[planned]** MeshCore
- **[planned]** End-of-Train (EOT) telemetry
- **[planned]** Growing sub-GHz protocol library (Flipper-style)
- **[planned]** LoRaWAN frame parsing
- **[planned]** BLE advertisements, 2.4 GHz survey (HackRF), Wi-Fi channel occupancy (energy only)

## 13. Weather, satellite & imaging

- **[planned]** NOAA APT
- **[planned]** Meteor M-2 LRPT (QPSK, Viterbi + RS)
- **[planned]** Radiosonde (RS41 …) + radiosonde map/log feature; later DFM, M10/M20, iMet
- **[planned]** HF WEFAX — needs an `IMAGE` binary frame kind, a server-side page store and a canvas panel first
- **[planned]** SSTV RX
- **[planned]** APRS weather aggregation

## 14. Broadcast & wideband digital

- **[planned]** DAB / DAB+ (OFDM + Viterbi + RS, HE-AAC audio)
- **[planned]** DATV (DVB-S / S2)
- **[planned]** TETRA
- **[planned]** DRM30 / DRM+

## 15. Amateur & weak-signal

- **[planned]** FT8 / FT4 (LDPC(174,91) + 8-GFSK)
- **[planned]** PSK31 / PSK63
- **[planned]** WSPR
- **[planned]** Radio clock (DCF77 / WWVB / MSF / JJY)

## 16. Analysis & measurement

- **[planned]** Channel analyzer — scope, constellation
- **[planned]** Demod analyzer — scope/spectrum on demodulated audio
- **[planned]** Noise figure measurement
- **[planned]** PER tester
- **[planned]** SID monitor
- **[planned]** Radio astronomy — integrating radiometer, spectral line
- **[planned]** Star tracker
- **[planned]** Sky map — celestial view, client-side render
- **[planned]** **Signal-ID assistant** — snapshot spectrum/audio and match against a signal catalog (sigidwiki-style), later an ML classifier
- **[planned]** GNSS educational decode — GPS L1 C/A acquisition + ephemeris (learning tool, not navigation)

## 17. Audio processing

- **[shipped]** Opus-encoded audio to the browser with jitter buffering and gesture unlock
- **[planned]** Spectral noise reduction, noise blanker, auto-notch, AGC — available in **every** voice channel as advanced audio processing, not a separate channel type
- **[planned]** Adaptive/auto DSP — auto-notch, auto-squelch, auto-gain, click/noise removal per mode

## 18. Station services & hardware integration

- **[planned]** Satellite tracker — TLE fetch, pass prediction, Doppler correction of linked channels
- **[planned]** Rotator control (GS-232, rotctld)
- **[planned]** rigctld-compatible rig control server
- **[planned]** GPS position source (gpsd / NMEA serial) — live station position for maps and trackers, geotagged mobile heat map for drive-around coverage, automatic grid locator, geotagged recordings
- **[planned]** NanoVNA integration over USB serial — antenna sweeps, SWR and Smith-chart panels, saved antenna profiles
- **[planned]** Antenna tools — dipole / λ calculators
- **[planned]** TinySA import, RTL-SDR-Blog / Airspy bias-T presets, Hamlib CAT control to slave a real radio
- **[skipped]** LimeRFE-specific control (reachable via Soapy settings if ever)

## 19. API, automation & access

- **[shipped]** REST control API with OpenAPI schema and generated typed client
- **[shipped]** One WebSocket for state events (JSON) plus binary spectrum, audio and IQ-tap streams
- **[shipped]** **MCP server** at `/mcp` (stateless streamable HTTP) — AI/agent control of the radio
- **[shipped]** Optional single shared token over REST, WS and MCP alike; accepted as a Bearer header or `?token=`. Public: auth, OpenAPI schema and docs endpoints
- **[shipped]** Default LAN-trusted posture (bind `0.0.0.0`, no auth), same as SDRangel/rtl_tcp; CORS locked to same-origin
- **[shipped]** Config via `config.toml`, flags and env: port, bind address, token, paths, backend options
- **[planned]** Python-and-friends scripting on the existing REST + MCP surface, with shipped recipes (scanner bots, "ping me when this callsign/ICAO appears")
- **[planned]** Alerting/notifications — rule engine on decoder events → desktop, push, webhook
- **[planned]** Plugin SDK via WASM (wasmtime) — third-party decoders and panels
- **[planned]** Multi-user roles (viewer vs operator)
- **[planned]** Cloud/remote fleet — one client managing several remote Pi nodes on a map of your receivers
- **[planned]** Offline reference bundles — band plans, TLE snapshots, callsign prefixes, ISM protocol catalog, PMTiles maps for field use
- **[skipped]** TLS termination — reverse-proxy or VPN it (Tailscale et al.)

## 20. Transmit & RF security research

A general-purpose, legitimate transmit and RF-research toolkit. These are test instruments for
*contained, authorized* assessment — direct-connect, dummy load or shielded — against devices
you are authorized to test. 

- **[shipped]** Device-layer TX half — `Duplex`, `tx_start`, `TxStream`, implemented on the HackRF backend over a working bulk-OUT path with burst queueing and transmit VGA control
- **[shipped]** TX unreachable from outside the device layer — every backend reports `tx_capable: false`, the transmit VGA is written to 0 dB on open, and no engine, server, MCP or UI path (nor any wire type) can key a transmitter
- **[planned]** The authorized-use gate itself, and everything below it
- **[planned]** Signal generator / arbitrary waveform + IQ playback-to-air (generated tones, modulated test signals, captured or edited SigMF files)
- **[planned]** Modulators paired with each demod (NFM/AM/SSB/WFM, digital modes) for two-way, beacon and test use on licensed bands
- **[planned]** Sub-GHz capture → decode → replay of OOK/ASK/FSK
- **[planned]** Fixed-code analysis and generation, including de Bruijn sequences
- **[planned]** Rolling-code capture and implementation analysis (RollJam-style, against your own DUT — window, resync and counter flaws)
- **[planned]** Interference / jam-susceptibility testing — configurable noise, sweep, CW or tone on a chosen band and bandwidth, into a contained link
- **[planned]** Flood / spam / malformed-broadcast testing, including BLE-advertising-style floods, at a DUT over a contained link
- **[planned]** Targeted protocol fuzzing — malformed and mutated frames at a specified DUT
- **[planned]** Bench loopback — TX into your own RX to validate decoders
- **[planned]** Offline frame workbench — decode, dissect, mutate, re-analyze captured frames; encoding identification
- **[planned]** Simple PTT
- **[planned]** Beam-steering CW modulator (TX MIMO)

## 21. Cross-cutting engine capabilities

- **[shipped]** Any number of channels per device set, each with its own DDC, settings and panel
- **[shipped]** Channels that need the radio's own rate (ADS-B) get the device's samples mixed to their offset, unresampled, at 2–4 MHz
- **[shipped]** Typed decoder events on the wire, persisted by the server with reported drops rather than silent loss
- **[shipped]** Per-connection throttling and decimation discipline so a Pi 4 stays the floor, not the ceiling
- **[planned]** GPU spectrum path (wgpu) for very large FFTs or many channels on capable hosts
- **[planned]** Diversity combine / noise cancelling with a reference antenna for HF QRM
