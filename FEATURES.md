# Feature roadmap

If something is shipped, then remove it.

## 1. Engine — many radios, arrays and cross-cutting DSP

- More Native Drivers: RX-888 / Mk2 native driver
- `CoherentArray` — N clock-synced receivers as one hardware-agnostic array with per-channel gain/phase calibration, noise-source/pilot alignment, and time-aligned multi-lane output (so support for e.g. KrakenSDR, dragensdr, and any other multi-channel SDR with a shared reference clock)
- Generic synced bank — any N receivers on a shared reference clock
- Direction finding (MUSIC/ESPRIT) with bearings on the map
  - with driving arround and automatic navigation like krakensdr
  - with multi-station triangulation
- Passive radar (range-Doppler)
- Beamforming, diversity combine, and noise cancelling against a reference antenna
- Neural noise reduction on the listen path: DeepFilterNet3
- Interferometer
- A floor that jumps up in one step is read as a signal until the channel next falls quiet, which is the deliberate half of the auto-squelch trade; a smarter estimator would tell the two apart

## 2. Spectrum, tuning & navigation

- Wideband skimmer/auto-detect
- Server-side zoom of the device spectrum

## 3. Recording, replay & measurement

- recording scheduler + unattended satellite-pass automation
- Demod analyzer
- Noise figure; PER tester; SID monitor
- export to rtl_433 tcp/udp, beast adsb, etc.

## 4. Decoders & protocols

- Tetrapol
- STANAG modem ID
- GSM downlink analysis
- OsmocomBB-style monitoring
- TETRA
- NOAA APT; Meteor M-2 LRPT
- Radiosonde (RS41 …) + map/log feature, DFM, M10/M20, iMet
- HF WEFAX — the DSP is the easy half; the picture store SSTV shipped already holds a picture that
  takes minutes to arrive, so what is left is the decoder and the mode's own line geometry
- APRS weather aggregation

### Sub-GHz, ISM & IoT

- Rolling-code analysis
- More of the ISM sensor library like rtl_433 
- ChirpChat / LoRa, Meshtastic, MeshCore
- End-of-Train (EOT) telemetry
- LoRaWAN frame parsing
- BLE advertisements, 2.4 GHz survey, Wi-Fi channel occupancy (energy only)

## 5. Transmit & legitimate security research

- Signal generator / arbitrary waveform + IQ playback-to-air
- Modulators for the remaining modes, over the shared frame/bit codec each protocol module owns in both directions — for two-way, beacon and test use
- Sub-GHz capture → decode → replay; fixed-code analysis and generation including de Bruijn sequences; rolling-code capture and implementation analysis against your own DUT
- Interference / jam-susceptibility testing into a contained link
- Flood / spam / malformed-broadcast testing at a DUT over a contained link
- Targeted protocol fuzzing
- Bench loopback — TX into your own RX to validate decoders (note: this is the point at which the graph's no-cycle proof stops being sufficient)
- Simple PTT
- Beam-steering CW modulator (TX MIMO)

## 6. Station services & hardware integration

- Satellite tracker (TLE fetch, pass prediction, Doppler-corrected channels)
- Rotator control (GS-232, rotctld); rigctld-compatible rig control server
- Saved antenna profiles — the NanoVNA tool sweeps, plots SWR and a Smith chart and
  calibrates, but a sweep is never stored against a named antenna
- Map layers — sondes, satellites, beacons
- TinySA import, Hamlib CAT control
- Radio astronomy; star tracker; sky map

## 7. API, automation & access

- Alerting/notifications — rule engine on decoder events → desktop, push
- Plugin SDK via WASM?
- Offline reference bundles — band plans, TLE snapshots, callsign prefixes, PMTiles maps
