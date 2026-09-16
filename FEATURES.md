# Feature roadmap

If something is shipped, then remove it.

## 1. Engine — many radios, arrays and cross-cutting DSP

- More Native Drivers: RX-888 / Mk2 native driver
- ESPRIT alongside the correlative and MUSIC estimators. On the circular array this defaults to
  it needs the beamspace form, which is a good deal more than a third estimator
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
- DECT: the survey reads the A-field only. The B-field is left — ADPCM/G.726 voice off unencrypted
  bearers — as are the extended fixed part capability messages (QH = 4, C, E) that carry the newer
  DSAA2/DSC2 security bits, and scanning the ten carriers from one wideband capture

### Broadcast & wideband digital

- DVB-S2X Annex E superframe formats 2–7 and non-default superframe scrambling/WH codes.
  Formats 0 and 1, including their VL-SNR framing, and all 55 additional normal/short-frame
  MODCODs are implemented.
- DRM FAC, SDC and MSC, service selection and audio. The current channel only acquires the
  cyclic prefix and reports lock, SNR and frequency error.
- Antenna and fading-channel validation. DAB, DVB-T and DVB-S/S2 playback is tested with
  synthetic IQ, including independent Python reference transmitters for DVB-T and DVB-S2X,
  codec golden vectors, virtual radios and browser playback. These do not establish performance
  with real broadcast transmitters, fading or adjacent-channel interference.

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

- Codeplug programming reaches the objects every radio shares — channels, contacts, group lists,
  zones, scan lists and radio IDs — and preserves every byte it does not model, so a read/write
  round trip changes nothing. What it does not read at all is the rest of the AnyTone map: the
  general-settings block, GPS and both APRS flavours, roaming zones and channels, encryption keys,
  DTMF/2-tone/5-tone signalling, satellite and boot settings, and the per-channel long tail
  (custom CTCSS, talkaround, call confirm, ranging, scrambler, TX colour code). Radtel RT-4D covers
  the same common objects and its DMR ID; its settings blocks, keys and message templates are read
  but not modelled. One AnyTone field is still unproven: the radio read to derive the map holds no
  channel with a transmit shift, so the direction bits are a reading, not a measurement, and the
  conversion report says so on any channel that uses one
- More radios: the AnyTone GD32 family (D868/D878/D578) shares the serial protocol already here and
  needs only its own memory map
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
