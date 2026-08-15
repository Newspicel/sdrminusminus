# Feature roadmap

This file tracks work that has shipped and ideas that remain planned. It is a roadmap, not a
promise or release schedule. For current installation and usage instructions, read the
[documentation](https://newspicel.github.io/sdrminusminus/); for the exact channel catalog in a
particular build, open **+ Node** or request `GET /api/channeltypes` from that server.

- **shipped** means the behavior is available in the current codebase;
- **planned** means it is not implemented yet, even when the note describes a possible design.

## 1. Platform and deployment

- **[planned]** Desktop app connecting to a *remote* server, and saved remote connections — the shell only ever spawns its own local one
- **[planned]** A native Save-As dialog for downloads in the desktop shell. The shell installs no `on_download` handler, so wry's default applies: a recording lands silently in the OS download directory (`~/Downloads`, `$XDG_DOWNLOAD_DIR`, `%USERPROFILE%\Downloads`), deduplicated as `name (1)`, with no dialog and no progress — and on Windows wry's `SetHandled(true)` suppresses even WebView2's own flyout. The gap that matters is failure: an export aborts its body rather than truncate, and that abort is invisible here. A Rust-side `tauri-plugin-dialog` handler would keep the shell's no-IPC stance, but a blocking dialog on the main thread needs care


## 2. Many radios at once & coherent arrays

- **[planned]** Cross-device features: a scanner spanning devices, multi-VOR fix, diversity
- **[planned]** `CoherentArray` — N clock-synced receivers as one hardware-agnostic array with per-channel gain/phase calibration, noise-source/pilot alignment, and time-aligned multi-lane output (so support for e.g. KrakenSDR)
- **[planned]** Generic synced bank — any N receivers on a shared reference clock
- **[planned]** Network coherent source — aligned multi-lane IQ from another sdr-- node or a DAQ
- **[planned]** Direction finding (MUSIC/ESPRIT) with bearings on the map; multi-station triangulation
- **[planned]** Passive radar (range-Doppler)
- **[planned]** Beamforming and diversity combine
- **[planned]** Interferometer

## 3. Spectrum, tuning & navigation

- **[planned]** Better Frequency scanner
- **[planned]** Hardware-assisted wideband sweep — the scanner still sweeps by retuning; a
  firmware sweep delivers blocks stamped with their own frequency rather than a stream at one
  tuning, so the scanner's device-set spectrum tap has nothing to read it with yet
- **[planned]** Strongest-signal "close-call" finder
- **[planned]** Signal-strength **hunt mode** — Geiger-style audio/visual feedback as you close on a transmitter
- **[planned]** Server-side zoom of the *device* spectrum — zooming a device scope re-frames bins
  that already arrived rather than resolving finer; the readout is honest about it. A channel's
  baseband scope resolves properly, being a transform of that channel's own samples
- **[planned]** Pinch-zoom on touch pointers
- **[planned]** 3D spectrogram view — a height-mapped surface reads worse than the 2D waterfall for
  finding signals (the near ridge occludes the far one); the persistence display above shows the
  same third dimension honestly. A perspective tilt of the existing waterfall, and a range–Doppler
  surface for passive radar (§2), are the two cases that would earn it


## 5. Recording, capture & replay

- **[planned]** The rest of the per-channel sinks — a baseband file, and a per-channel network tap;
  the network export node carries a device's IQ only
- **[planned]** **IQ time machine** — rolling ring buffer, retro-record the last N seconds after the fact
- **[planned]** Inspectrum-style offline IQ viewer in the browser
- **[planned]** Annotated recordings; recording scheduler + unattended satellite-pass automation
- **[planned]** Wideband recording + offline re-channelization
- **[planned]** Session/replay sharing as one openable bundle


## 8. Digital voice

- **[planned]** YSF callsigns — the signalling layer below its voice framing
- **[planned]** Hardware AMBE dongle/server support

## 9. Aviation & marine

- **[planned]** VOR, VOR localizer (multi-VOR fix), ILS, DSC
- **[planned]** Inmarsat STD-C / AERO
- **[planned]** VDL Mode 2; HFDL; Iridium bursts
- **[planned]** ADS-B / AIS log enrichment against offline aircraft and ship databases

## 10. Data, text & paging

- **[planned]** FLEX and further pager formats, ERMES
- **[planned]** CW skimmer — every CW signal in the passband at once
- **[planned]** Tetrapol, STANAG modem ID, GSM downlink analysis, OsmocomBB-style monitoring
- **[planned]** Off-air proof — as above, all four are specification-proven only

## 11. Sub-GHz, ISM & IoT

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

## 12. Weather, satellite & imaging

- **[planned]** NOAA APT; Meteor M-2 LRPT
- **[planned]** Radiosonde (RS41 …) + map/log feature; later DFM, M10/M20, iMet
- **[planned]** HF WEFAX — the DSP is the easy half; the picture transport ATV shipped is now the half that exists, so what is left is the decoder plus a server-side page store for a mode whose picture takes minutes rather than milliseconds
- **[planned]** SSTV RX; APRS weather aggregation

## 13. Broadcast & wideband digital

- **[planned]** TETRA
- **[planned]** The multiplex and media layers above the shipped DAB, DATV and DRM acquisition —
  DAB FIC/MSC and DAB+ audio, DVB-S/S2 FEC + MPEG-TS and video, DRM FAC/SDC/MSC and audio

## 15. Analysis & measurement

- **[planned]** Demod analyzer
- **[planned]** Noise figure; PER tester; SID monitor
- **[planned]** Radio astronomy; star tracker; sky map
- **[planned]** Signal generator / arbitrary waveform + IQ playback-to-air

## 16. Audio processing

- **[planned]** A floor that jumps up in one step is read as a signal until the channel next falls quiet, which is the deliberate half of the auto-squelch trade; a smarter estimator would tell the two apart

## 17. Station services & hardware integration

- **[planned]** Satellite tracker (TLE fetch, pass prediction, Doppler-corrected channels)
- **[planned]** Rotator control (GS-232, rotctld); rigctld-compatible rig control server
- **[planned]** Saved antenna profiles — the NanoVNA tool sweeps, plots SWR and a Smith chart and
  calibrates, but a sweep is never stored against a named antenna
- **[planned]** Map layers — sondes, satellites, beacons, MUF
- **[planned]** TinySA import, Hamlib CAT control

## 18. API, automation & access

- **[planned]** Scripting recipes on the existing REST + MCP surface (scanner bots, "ping me when this callsign appears")
- **[planned]** Alerting/notifications — rule engine on decoder events → desktop, push, webhook
- **[planned]** Plugin SDK via WASM
- **[planned]** Multi-user roles; remote fleet management across several Pi nodes
- **[planned]** Offline reference bundles — band plans, TLE snapshots, callsign prefixes, PMTiles maps

## 19. Legitimate Security research

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

## 20. Cross-cutting engine capabilities

- **[planned]** Diversity combine / noise cancelling with a reference antenna
