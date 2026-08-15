# Feature roadmap

If something is shipped, then remove it.

## 1. Platform and desktop shell

- Desktop app connecting to a *remote* server, and saved remote connections — the shell only ever spawns its own local one
- A native Save-As dialog for downloads in the desktop shell. The shell installs no `on_download` handler, so wry's default applies: a recording lands silently in the OS download directory (`~/Downloads`, `$XDG_DOWNLOAD_DIR`, `%USERPROFILE%\Downloads`), deduplicated as `name (1)`, with no dialog and no progress — and on Windows wry's `SetHandled(true)` suppresses even WebView2's own flyout. The gap that matters is failure: an export aborts its body rather than truncate, and that abort is invisible here. A Rust-side `tauri-plugin-dialog` handler would keep the shell's no-IPC stance, but a blocking dialog on the main thread needs care

## 2. Engine — many radios, arrays and cross-cutting DSP

- `CoherentArray` — N clock-synced receivers as one hardware-agnostic array with per-channel gain/phase calibration, noise-source/pilot alignment, and time-aligned multi-lane output (so support for e.g. KrakenSDR)
- Generic synced bank — any N receivers on a shared reference clock
- Network coherent source — aligned multi-lane IQ from another sdr-- node or a DAQ
- Direction finding (MUSIC/ESPRIT) with bearings on the map; multi-station triangulation
- Passive radar (range-Doppler)
- Beamforming, diversity combine, and noise cancelling against a reference antenna
- Interferometer
- A floor that jumps up in one step is read as a signal until the channel next falls quiet, which is the deliberate half of the auto-squelch trade; a smarter estimator would tell the two apart

## 3. Spectrum, tuning & navigation

- Better frequency scanner, including one that spans several devices
- Hardware-assisted wideband sweep — the scanner still sweeps by retuning; a
  firmware sweep delivers blocks stamped with their own frequency rather than a stream at one
  tuning, so the scanner's device-set spectrum tap has nothing to read it with yet
- Strongest-signal "close-call" finder
- Signal-strength hunt mode — Geiger-style audio/visual feedback as you close on a transmitter
- Server-side zoom of the *device* spectrum — zooming a device scope re-frames bins
  that already arrived rather than resolving finer; the readout is honest about it. A channel's
  baseband scope resolves properly, being a transform of that channel's own samples
- Pinch-zoom on touch pointers
- 3D spectrogram view — a height-mapped surface reads worse than the 2D waterfall for
  finding signals (the near ridge occludes the far one); the persistence display above shows the
  same third dimension honestly. A perspective tilt of the existing waterfall, and a range–Doppler
  surface for passive radar (§2), are the two cases that would earn it

## 4. Recording, replay & measurement

- The rest of the per-channel sinks — a baseband file, and a per-channel network tap;
  the network export node carries a device's IQ only
- IQ time machine — rolling ring buffer, retro-record the last N seconds after the fact
- Inspectrum-style offline IQ viewer in the browser
- Annotated recordings; recording scheduler + unattended satellite-pass automation
- Wideband recording + offline re-channelization
- Session/replay sharing as one openable bundle
- Demod analyzer
- Noise figure; PER tester; SID monitor

## 5. Decoders & protocols

### Digital voice

- YSF callsigns — the signalling layer below its voice framing
- Hardware AMBE dongle/server support

### Aviation & marine

- VOR, VOR localizer (multi-VOR fix), ILS, DSC
- Inmarsat STD-C / AERO
- VDL Mode 2; HFDL; Iridium bursts

### Data, text & paging®

- FLEX and further pager formats, ERMES
- CW skimmer — every CW signal in the passband at once
- Tetrapol, STANAG modem ID, GSM downlink analysis, OsmocomBB-style monitoring

### Sub-GHz, ISM & IoT

- Rolling-code *analysis* — a KeeLoq-style remote decodes today as a structureless 66-bit PWM frame; analysis is gated TX-phase work (§6)
- Protocol library — the encoding is classified and the bits handed back; a table of known payload layouts (weather stations, TPMS, meters) is data work, not DSP
- ISM sensor suite (rtl_433-style); escape hatch is a UDP sink into the rtl_433 binary
- ChirpChat / LoRa, Meshtastic, MeshCore
- End-of-Train (EOT) telemetry
- LoRaWAN frame parsing
- BLE advertisements, 2.4 GHz survey, Wi-Fi channel occupancy (energy only)

### Weather, satellite & imaging

- NOAA APT; Meteor M-2 LRPT
- Radiosonde (RS41 …) + map/log feature; later DFM, M10/M20, iMet
- HF WEFAX — the DSP is the easy half; the picture transport ATV shipped is now the half that exists, so what is left is the decoder plus a server-side page store for a mode whose picture takes minutes rather than milliseconds
- SSTV RX; APRS weather aggregation

### Broadcast & wideband digital

- TETRA
- The multiplex and media layers above the shipped DAB, DATV and DRM acquisition —
  DAB FIC/MSC and DAB+ audio, DVB-S/S2 FEC + MPEG-TS and video, DRM FAC/SDC/MSC and audio

### Proof against the air

- Off-air proof — Tetrapol, STANAG modem ID, GSM downlink and OsmocomBB-style
  monitoring are specification-proven only, and no sub-GHz remote decoder has yet been tested
  against a real transmitter

## 6. Transmit & legitimate security research

- Signal generator / arbitrary waveform + IQ playback-to-air
- Modulators for the remaining modes, over the shared frame/bit codec each protocol module owns in both directions — for two-way, beacon and test use on licensed bands
- Sub-GHz capture → decode → replay; fixed-code analysis and generation including de Bruijn sequences; rolling-code capture and implementation analysis against your own DUT
- Interference / jam-susceptibility testing into a contained link
- Flood / spam / malformed-broadcast testing at a DUT over a contained link
- Targeted protocol fuzzing
- Bench loopback — TX into your own RX to validate decoders (note: this is the point at which the graph's no-cycle proof stops being sufficient)
- Offline frame workbench — dissect, mutate, re-analyze captured frames; encoding identification
- Simple PTT
- Beam-steering CW modulator (TX MIMO)

## 7. Station services & hardware integration

- Satellite tracker (TLE fetch, pass prediction, Doppler-corrected channels)
- Rotator control (GS-232, rotctld); rigctld-compatible rig control server
- Saved antenna profiles — the NanoVNA tool sweeps, plots SWR and a Smith chart and
  calibrates, but a sweep is never stored against a named antenna
- Map layers — sondes, satellites, beacons, MUF
- TinySA import, Hamlib CAT control
- Radio astronomy; star tracker; sky map

## 8. API, automation & access

- Scripting recipes on the existing REST + MCP surface (scanner bots, "ping me when this callsign appears")
- Alerting/notifications — rule engine on decoder events → desktop, push, webhook
- Plugin SDK via WASM
- Multi-user roles; remote fleet management across several Pi nodes
- Offline reference bundles — band plans, TLE snapshots, callsign prefixes, PMTiles maps
