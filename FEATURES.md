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

### Broadcast & wideband digital

The multiplexes now decode: DAB down to CRC-checked DAB+ access units, DVB-S and DVB-S2 down to
MPEG-TS packets and a program table. What is left above them is the media, and DRM's whole
multiplex.

- DAB+ audio — the superframe hands over HE-AAC v2 access units and nothing turns them into
  sound. It needs an AAC-LC decoder over the 960-sample transform DAB+ uses, then SBR, then
  parametric stereo. `symphonia-codec-aac` cannot stand in: it refuses any stream with SBR and
  only handles 1024-sample frames. The same decoder is what DRM audio needs, so it is worth one
  careful implementation rather than two
- Classic DAB audio — MPEG-1 Layer II, which DVB's audio streams also want
- DAB transmission modes II, III and IV; only Mode I is implemented, which is what is on the air
  in Band III but not what a shortwave or satellite DAB feed would use
- DAB data services — packet mode, MOT slideshow, and the DLS text riding in the PAD
- DVB video — MPEG-2 and H.264. The transport stream is demultiplexed and the PES units come out
  whole with their timestamps; nothing decodes them into pictures yet
- DVB audio — MPEG-1 Layer II, AAC and AC-3 off the same PES units
- DVB-S2 beyond QPSK and 8PSK — 16APSK and 32APSK, 8PSK at rate 3/5 (its bit interleaver twists
  the columns and nothing else does), rates 1/4, 1/3 and 2/5, VL-SNR frames, GSE encapsulation,
  and more than one input stream with adaptive coding
- DRM's multiplex — FAC, SDC and MSC. The channel still only acquires: it reports lock, SNR and
  frequency error off the cyclic prefix and reads nothing. It needs the per-mode cell mapping,
  pilot-based channel estimation, the multilevel coding the MSC uses, and then the audio
- Signal quality worth trusting on a real antenna. Every chain here is proven against a
  transmitter written beside it, which catches structural mistakes but shares any misreading of a
  standard. Sync and equalization are sized for that clean signal: DAB resynchronizes per frame
  off the null with no timing loop, DVB-S2 takes its phase reference from the frame header alone,
  and neither has met a fading channel

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
