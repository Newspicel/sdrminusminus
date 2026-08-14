## 1. Platform & deployment

- **[planned]** Desktop app connecting to a *remote* server, and saved remote connections — the shell only ever spawns its own local one
- **[planned]** A native Save-As dialog for downloads in the desktop shell. The shell installs no `on_download` handler, so wry's default applies: a recording lands silently in the OS download directory (`~/Downloads`, `$XDG_DOWNLOAD_DIR`, `%USERPROFILE%\Downloads`), deduplicated as `name (1)`, with no dialog and no progress — and on Windows wry's `SetHandled(true)` suppresses even WebView2's own flyout. The gap that matters is failure: an export aborts its body rather than truncate, and that abort is invisible here. A Rust-side `tauri-plugin-dialog` handler would keep the shell's no-IPC stance, but a blocking dialog on the main thread needs care

## 2. Device support

- **[shipped]** SoapySDR is the canonical local-hardware layer, with directional RX/TX
  capabilities, explicit multi-channel streams, generic module settings, reconnect supervision,
  and pinned private runtimes in desktop installers and containers
- **[shipped]** Base packages include RTL-SDR, HackRF, Airspy/AirspyHF, bladeRF, LimeSDR,
  PlutoSDR/libiio, and SoapyRemote modules; SDRplay RSP devices are supported through a
  user-installed SDRplay API and SoapySDRPlay3 module, while UHD remains an optional pack
- **[planned]** KiwiSDR client device
- **[planned]** Remote source/sink between sdr-- instances; local routing between device sets
- **[planned]** Audio-input device (`cpal`) — soundcard as a receiver

## 3. Many radios at once & coherent arrays

- **[planned]** Cross-device features: a scanner spanning devices, multi-VOR fix, diversity
- **[planned]** `CoherentArray` — N clock-synced receivers as one hardware-agnostic array with per-channel gain/phase calibration, noise-source/pilot alignment, and time-aligned multi-lane output
- **[planned]** KrakenSDR support — via its Heimdall DAQ network stream first, direct hardware drive later
- **[planned]** Generic synced bank — any N receivers on a shared reference clock
- **[planned]** Network coherent source — aligned multi-lane IQ from another sdr-- node or a DAQ
- **[planned]** Direction finding (MUSIC/ESPRIT) with bearings on the map; multi-station triangulation
- **[planned]** Passive radar (range-Doppler)
- **[planned]** Beamforming and diversity combine
- **[planned]** Interferometer
- **[planned]** TDoA geolocation across distributed sdr-- nodes

## 4. Spectrum, tuning & navigation

- **[planned]** Better Frequency scanner
- **[planned]** Hardware-assisted wideband sweep — the scanner still sweeps by retuning; a
  firmware sweep delivers blocks stamped with their own frequency rather than a stream at one
  tuning, so the scanner's device-set spectrum tap has nothing to read it with yet
- **[planned]** Strongest-signal "close-call" finder
- **[planned]** Signal-strength **hunt mode** — Geiger-style audio/visual feedback as you close on a transmitter
- **[planned]** Percentile-anchored waterfall colour range — the range is the frame's own min…max today, so a high noise floor washes the display out
- **[planned]** Server-side zoom — zooming re-frames bins that already arrived rather than resolving finer; the readout is honest about it
- **[planned]** Pinch-zoom on touch pointers
- **[planned]** 3D spectrogram view
- **[planned]** Band occupancy analytics over time

## 5. Frequency-allocation database — "what is this frequency?"

The dial and the plot were built so this could hang off them without rework, and it did.

- **[planned]** The FCC importer — one PDF yields ITU Regions 1/2/3 *and* the US table, but `pdftotext -layout` lays each page out independently, so its columns shift between a header page and its continuation. It needs word coordinates (`-bbox-layout`); until then ITU and US stay curated
- **[planned]** A CEPT importer — EFIS publishes the ECA table and may expose it machine-readably (`efis.cept.org`, unreachable from the network this was written on). CEPT and IARU R1 are curated meanwhile
- **[planned]** User-extendable and override-able entries

## 6. Recording, capture & replay

- **[planned]** Per-channel sinks — audio recording, baseband file, UDP out to external tools
- **[planned]** RF replay-capture workflow — record a burst, annotate it, analyze it
- **[planned]** **IQ time machine** — rolling ring buffer, retro-record the last N seconds after the fact
- **[planned]** Inspectrum-style offline IQ viewer in the browser
- **[planned]** Annotated recordings; recording scheduler + unattended satellite-pass automation
- **[planned]** Wideband recording + offline re-channelization
- **[planned]** Session/replay sharing as one openable bundle

## 7. UI, workspaces & onboarding

- **[planned]** Node kinds whose backends do not exist yet: GPS source, UDP sink, WAV sink, and the `iq-tap`/`position` port types that go with them
- **[planned]** A scope on a channel tap — a scope only takes a device today
- **[planned]** Theme/skin system and a layout marketplace

## 8. Voice & analog channels

- **[planned]** ATV colour and the sound subcarrier — luma only today; chroma is left where it is in the video band
- **[planned]** Notch and audio filters per channel
- **[planned]** Selcall (CCIR/ZVEI)

## 9. Digital voice

- **[planned]** NXDN SACCH/FACCH addressing and YSF callsigns — the signalling layers below
  each mode's voice framing
- **[planned]** FreeDV
- **[planned]** Trunking following — P25 / DMR Tier III control channel decode with auto-steered
  voice channels. Needs the control-channel payloads above first
- **[planned]** Hardware AMBE dongle/server support

## 10. Aviation & marine

- **[planned]** VOR, VOR localizer (multi-VOR fix), ILS, DSC
- **[planned]** Inmarsat STD-C / AERO
- **[planned]** VDL Mode 2; HFDL; Iridium bursts
- **[planned]** ADS-B / AIS log enrichment against offline aircraft and ship databases

## 11. Data, text & paging

- **[planned]** APRS *feature* — station/position collection, distinct from the channel
- **[planned]** FLEX and further pager formats, ERMES
- **[planned]** CW skimmer — every CW signal in the passband at once
- **[planned]** Tetrapol, STANAG modem ID, GSM downlink analysis, OsmocomBB-style monitoring
- **[planned]** Off-air proof — as above, all four are specification-proven only

## 12. Sub-GHz, ISM & IoT

- **[shipped]** No chip is named — a 24-bit frame carries both the EV1527 reading (address + button) and the PT2262 tri-state string where every bit pair is a legal symbol
- **[shipped]** Repeats inside 500 ms collapse into one counted event, and a better-classified frame supersedes a held one only while that one is a single sighting — which is what stops a capture that started mid-burst from logging its fragment
- **[planned]** Rolling-code *analysis* — a KeeLoq-style remote decodes today as a structureless 66-bit PWM frame; analysis is gated TX-phase work (§20)
- **[planned]** Protocol library — the encoding is classified and the bits handed back; a table of known payload layouts (weather stations, TPMS, meters) is data work, not DSP
- **[planned]** ISM sensor suite (rtl_433-style); escape hatch is a UDP sink into the rtl_433 binary
- **[planned]** ChirpChat / LoRa, Meshtastic, MeshCore
- **[planned]** End-of-Train (EOT) telemetry
- **[planned]** LoRaWAN frame parsing
- **[planned]** BLE advertisements, 2.4 GHz survey, Wi-Fi channel occupancy (energy only)
- **[planned]** Off-air proof — never yet tested against a real remote

## 13. Weather, satellite & imaging

- **[planned]** NOAA APT; Meteor M-2 LRPT
- **[planned]** Radiosonde (RS41 …) + map/log feature; later DFM, M10/M20, iMet
- **[planned]** HF WEFAX — the DSP is the easy half; the picture transport ATV shipped (§8) is now the half that exists, so what is left is the decoder plus a server-side page store for a mode whose picture takes minutes rather than milliseconds
- **[planned]** SSTV RX; APRS weather aggregation

## 14. Broadcast & wideband digital

- **[planned]** DAB / DAB+
- **[planned]** DATV (DVB-S / S2)
- **[planned]** TETRA
- **[planned]** DRM30 / DRM+

## 15. Amateur & weak-signal

- **[planned]** FT8 / FT4
- **[planned]** PSK31 / PSK63; WSPR
- **[planned]** Radio clock (DCF77 / WWVB / MSF / JJY)

## 16. Analysis & measurement

- **[planned]** Channel analyzer (scope, constellation) — also the prerequisite for wiring a scope to a channel tap
- **[planned]** Demod analyzer; channel power meter; heat map channel
- **[planned]** Noise figure; PER tester; SID monitor
- **[planned]** Radio astronomy; star tracker; sky map
- **[planned]** Signal-ID assistant — match a spectrum/audio snapshot against a signal catalog, later an ML classifier
- **[planned]** GNSS educational decode
- **[planned]** Signal generator / arbitrary waveform + IQ playback-to-air
- **[planned]** Seeing stuff like 4fsk directly without being behind a decoder. 

## 17. Audio processing

- **[planned]** Spectral noise reduction, noise blanker, auto-notch, AGC as advanced processing inside **every** voice channel rather than a separate channel type
- **[planned]** Adaptive/auto DSP — auto-notch, auto-squelch, auto-gain, per-mode click and noise removal

## 18. Station services & hardware integration

- **[planned]** Satellite tracker (TLE fetch, pass prediction, Doppler-corrected channels)
- **[planned]** Rotator control (GS-232, rotctld); rigctld-compatible rig control server
- **[planned]** GPS position source (gpsd / NMEA) — station position for maps and trackers, geotagged mobile heat map, auto grid locator, geotagged recordings
- **[planned]** NanoVNA over USB serial — sweeps, SWR and Smith-chart panels, saved antenna profiles
- **[planned]** Antenna calculators
- **[planned]** Map layers — sondes, satellites, beacons, MUF
- **[planned]** TinySA import, bias-T presets, Hamlib CAT control

## 19. API, automation & access

- **[planned]** Scripting recipes on the existing REST + MCP surface (scanner bots, "ping me when this callsign appears")
- **[planned]** Alerting/notifications — rule engine on decoder events → desktop, push, webhook
- **[planned]** Plugin SDK via WASM
- **[planned]** Multi-user roles; remote fleet management across several Pi nodes
- **[planned]** Offline reference bundles — band plans, TLE snapshots, callsign prefixes, PMTiles maps
- **[planned]** Output Nodes for like Discord

## 20. Legitimate Security research

- **[planned]** Signal generator / arbitrary waveform + IQ playback-to-air
- **[planned]** Modulators for the remaining modes, over the shared frame/bit codec each protocol module owns in both directions — for two-way, beacon and test use on licensed bands
- **[planned]** Sub-GHz capture → decode → replay; fixed-code analysis and generation including de Bruijn sequences; rolling-code capture and implementation analysis against your own DUT
- **[planned]** Interference / jam-susceptibility testing into a contained link
- **[planned]** Flood / spam / malformed-broadcast testing at a DUT over a contained link
- **[planned]** Targeted protocol fuzzing
- **[planned]** Bench loopback — TX into your own RX to validate decoders (note: this is the point at which the graph's no-cycle proof stops being sufficient)
- **[planned]** Offline frame workbench — dissect, mutate, re-analyze captured frames; encoding identification
- **[planned]** Simple PTT
- **[planned]** Beam-steering CW modulator (TX MIMO)

## 21. Cross-cutting engine capabilities

- **[planned]** GPU spectrum path (wgpu) for very large FFTs or many channels
- **[planned]** Diversity combine / noise cancelling with a reference antenna
