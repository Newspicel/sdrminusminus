# Feature roadmap

If something is shipped, then remove it.

## 1. Platform and desktop shell

- Desktop app connecting to a *remote* server, and saved remote connections — the shell only ever spawns its own local one
- A native Save-As dialog for downloads in the desktop shell. The shell installs no `on_download` handler, so wry's default applies: a recording lands silently in the OS download directory (`~/Downloads`, `$XDG_DOWNLOAD_DIR`, `%USERPROFILE%\Downloads`), deduplicated as `name (1)`, with no dialog and no progress — and on Windows wry's `SetHandled(true)` suppresses even WebView2's own flyout. The gap that matters is failure: an export aborts its body rather than truncate, and that abort is invisible here. A Rust-side `tauri-plugin-dialog` handler would keep the shell's no-IPC stance, but a blocking dialog on the main thread needs care

## 2. Engine — many radios, arrays and cross-cutting DSP

- More Native Drivers: RX-888 / Mk2 native driver
- `CoherentArray` — N clock-synced receivers as one hardware-agnostic array with per-channel gain/phase calibration, noise-source/pilot alignment, and time-aligned multi-lane output (so support for e.g. KrakenSDR, dragensdr, and any other multi-channel SDR with a shared reference clock)
- Generic synced bank — any N receivers on a shared reference clock
- Direction finding (MUSIC/ESPRIT) with bearings on the map
  - with driving arround and automatic navigation like krakensdr
  - with multi-station triangulation
- Passive radar (range-Doppler)
- Beamforming, diversity combine, and noise cancelling against a reference antenna
- Neural noise reduction on the listen path — an opt-in per-channel stage behind the OM-LSA
  denoiser, in a feature-gated crate of its own, since loading a model is I/O and `dsp` has none.
  DeepFilterNet3 is the fit: Rust-native under tract, MIT/Apache-2.0 for code *and* weights, and
  natively 48 kHz. Two things gate it. Its weights are trained on full-band clean speech, while
  radio audio is 300–3000 Hz, companded, and routinely below any SNR in that training set — it wins
  on FM hiss and broadband RFI and invents detail on weak SSB near the floor, so it stays off by
  default and never sits upstream of a decoder, which also keeps it away from CW and the data modes
  it would erase. And it needs fine-tuning on band-limited speech mixed with real captures before it
  beats the classical stage on the signals that actually matter. GTCRN is the cheaper base to
  fine-tune (48.2 k parameters against 2.3 M) at the cost of resampling to 16 kHz and back
- Interferometer
- A floor that jumps up in one step is read as a signal until the channel next falls quiet, which is the deliberate half of the auto-squelch trade; a smarter estimator would tell the two apart

## 3. Spectrum, tuning & navigation

- Wideband skimmer — every signal across the visible span decoded at once and labelled on the
  waterfall, rather than one tuned channel at a time. The identifier already ranks a signal's
  protocol from its bandwidth, symbol rate and deviation; what is missing is a pool of cheap
  detectors fed from the span and a label layer that survives zoom, pan and a moving signal
- Server-side zoom of the *device* spectrum — zooming a device scope re-frames bins
  that already arrived rather than resolving finer; the readout is honest about it. A channel's
  baseband scope resolves properly, being a transform of that channel's own samples

## 4. Recording, replay & measurement

- recording scheduler + unattended satellite-pass automation
- Demod analyzer
- Noise figure; PER tester; SID monitor
- export to rtl_433 tcp/udp, beast adsb, etc.

## 5. Decoders & protocols

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

## 6. Transmit & legitimate security research

- Signal generator / arbitrary waveform + IQ playback-to-air
- Modulators for the remaining modes, over the shared frame/bit codec each protocol module owns in both directions — for two-way, beacon and test use
- Sub-GHz capture → decode → replay; fixed-code analysis and generation including de Bruijn sequences; rolling-code capture and implementation analysis against your own DUT
- Interference / jam-susceptibility testing into a contained link
- Flood / spam / malformed-broadcast testing at a DUT over a contained link
- Targeted protocol fuzzing
- Bench loopback — TX into your own RX to validate decoders (note: this is the point at which the graph's no-cycle proof stops being sufficient)
- Simple PTT
- Beam-steering CW modulator (TX MIMO)

## 7. Station services & hardware integration

- Satellite tracker (TLE fetch, pass prediction, Doppler-corrected channels)
- Rotator control (GS-232, rotctld); rigctld-compatible rig control server
- Saved antenna profiles — the NanoVNA tool sweeps, plots SWR and a Smith chart and
  calibrates, but a sweep is never stored against a named antenna
- Map layers — sondes, satellites, beacons
- TinySA import, Hamlib CAT control
- Radio astronomy; star tracker; sky map

## 8. API, automation & access

- Alerting/notifications — rule engine on decoder events → desktop, push, webhook
- Plugin SDK via WASM?
- Offline reference bundles — band plans, TLE snapshots, callsign prefixes, PMTiles maps
