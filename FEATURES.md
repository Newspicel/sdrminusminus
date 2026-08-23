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

### Broadcast & wideband digital

The multiplexes now decode: DAB down to CRC-checked DAB+ access units, DVB-S and DVB-S2 down to
MPEG-TS packets and a program table. DVB-S2 covers the whole EN 302 307-1 MODCOD table and the
S2X very-low-SNR frames, carries generic streams as well as transport streams, and picks one
input stream out of a multi-stream carrier. What is left above it is the media, and DRM's whole
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
- The rest of DVB-S2X: 8APSK, the 64/128/256APSK constellations, the finer code rates between
  the S2 ones, and the super-frame formats of its annex E. What is here is every S2 MODCOD plus
  the VL-SNR set, which is the part a receiver meets on a real transponder
- Generic streams go no further than the GSE reader. Datagrams come out whole and CRC-checked
  with their protocol and label, and the status names what is riding on the carrier, but nothing
  hands them to a network interface
- DRM's multiplex — FAC, SDC and MSC. The channel still only acquires: it reports lock, SNR and
  frequency error off the cyclic prefix and reads nothing. It needs the per-mode cell mapping,
  pilot-based channel estimation, the multilevel coding the MSC uses, and then the audio
- Signal quality worth trusting on a real antenna. Every chain here is proven against a
  transmitter written beside it, which catches structural mistakes but shares any misreading of a
  standard — except DVB-S2's tables and VL-SNR framing, which are checked against both
  EN 302 307-2 and an unrelated implementation. DAB resynchronizes per frame off the null with no
  timing loop. DVB-S2 does better: it takes a coarse carrier estimate off the sync word, filters
  it across frames, anchors phase on the whole PLHEADER and interpolates between pilot blocks —
  so it rides out a carrier offset, at the cost of losing the frames it spends acquiring one.
  Neither has met a fading channel

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
