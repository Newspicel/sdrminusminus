# Feature roadmap

If something is shipped, then remove it.

## 1. Platform and desktop shell

- Desktop app connecting to a *remote* server, and saved remote connections — the shell only ever spawns its own local one
- A native Save-As dialog for downloads in the desktop shell. The shell installs no `on_download` handler, so wry's default applies: a recording lands silently in the OS download directory (`~/Downloads`, `$XDG_DOWNLOAD_DIR`, `%USERPROFILE%\Downloads`), deduplicated as `name (1)`, with no dialog and no progress — and on Windows wry's `SetHandled(true)` suppresses even WebView2's own flyout. The gap that matters is failure: an export aborts its body rather than truncate, and that abort is invisible here. A Rust-side `tauri-plugin-dialog` handler would keep the shell's no-IPC stance, but a blocking dialog on the main thread needs care

## 2. Engine — many radios, arrays and cross-cutting DSP

- RX-888 / Mk2 native driver — 16-bit direct sampling over USB 3, no SoapySDR module, so the
  whole shortwave spectrum arrives as one stream for the channelizer to split. The `usb-stream`
  crate already carries the bulk-transfer path; what the device adds is FX3 firmware upload at
  open, an ADC clock the rest of the stack currently assumes is a tuner, and a sample rate high
  enough that the spectrum tap needs to decimate before it ever reaches a scope
- `CoherentArray` — N clock-synced receivers as one hardware-agnostic array with per-channel gain/phase calibration, noise-source/pilot alignment, and time-aligned multi-lane output (so support for e.g. KrakenSDR)
- Generic synced bank — any N receivers on a shared reference clock
- Network coherent source — aligned multi-lane IQ from another sdr-- node or a DAQ
- Direction finding (MUSIC/ESPRIT) with bearings on the map; multi-station triangulation
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

- Inspectrum-style offline IQ viewer in the browser
- Annotated recordings; recording scheduler + unattended satellite-pass automation
- Wideband recording + offline re-channelization
- Session/replay sharing as one openable bundle
- Demod analyzer
- Noise figure; PER tester; SID monitor

## 5. Decoders & protocols

### Data, text & paging

- Tetrapol, STANAG modem ID, GSM downlink analysis, OsmocomBB-style monitoring

### Sub-GHz, ISM & IoT

- Rolling-code *analysis* — a KeeLoq-style remote decodes today as a structureless 66-bit PWM frame; analysis is gated TX-phase work (§6)
- More of the ISM sensor library. Five pulse slicers (PCM, PPM, PWM, Manchester, differential
  Manchester) and ten devices are in, which is the framing rtl_433 uses for ~94% of its
  catalogue; what is left is payload layouts on the same table — TPMS, meters, the wind and rain
  message types, and the remaining weather-station families. Three slicers are unported
  (PIWM, OSV1, RZI/NRZS) because only a handful of devices use them
- Escape hatch for the long tail: a UDP or TCP sink into the rtl_433 binary. The IQ export
  already speaks `cu8`, which is rtl_433's native sample format; what is missing is the transport
  glue and a way to fold its output back into the decoder log
- ChirpChat / LoRa, Meshtastic, MeshCore
- End-of-Train (EOT) telemetry
- LoRaWAN frame parsing
- BLE advertisements, 2.4 GHz survey, Wi-Fi channel occupancy (energy only)

### Weather, satellite & imaging

- NOAA APT; Meteor M-2 LRPT
- Radiosonde (RS41 …) + map/log feature, DFM, M10/M20, iMet
- HF WEFAX — the DSP is the easy half; the picture store SSTV shipped already holds a picture that
  takes minutes to arrive, so what is left is the decoder and the mode's own line geometry
- APRS weather aggregation

### Broadcast & wideband digital

- TETRA
- The multiplex and media layers above the shipped DAB, DATV and DRM acquisition —
  DAB FIC/MSC and DAB+ audio, DVB-S/S2 FEC + MPEG-TS and video, DRM FAC/SDC/MSC and audio

## 6. Transmit & legitimate security research

- Signal generator / arbitrary waveform + IQ playback-to-air
- Modulators for the remaining modes, over the shared frame/bit codec each protocol module owns in both directions — for two-way, beacon and test use
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
- Map layers — sondes, satellites, beacons
- TinySA import, Hamlib CAT control
- Radio astronomy; star tracker; sky map

## 8. API, automation & access

- Scripting recipes on the existing REST + MCP surface (scanner bots, "ping me when this callsign appears")
- Alerting/notifications — rule engine on decoder events → desktop, push, webhook
- Plugin SDK via WASM
- Multi-user roles; remote fleet management across several Pi nodes
- Offline reference bundles — band plans, TLE snapshots, callsign prefixes, PMTiles maps
