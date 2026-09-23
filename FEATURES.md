# Feature roadmap

Remove an item once it ships.

## 1. Transmit

Already there: TX streams on HackRF, AD936x/Pluto and Soapy, a reserved `tx` input on transmit-capable
Device nodes that takes no wire yet, and modulators for NFM, AM, SSB, APRS, DMR, dPMR, D-Star,
M17, NXDN, P25 and YSF. Only the Signal generator uses the modulators.

### Foundation
- Transmit gate: the only node that may feed a Device's `tx` input
- Engine TX path: modulator to `TxStream`, settings via command queue
- Safety: band limits, TX timeout, a visible on-air state
- PTT: button, hotkey, VOX
- Microphone input into the voice modulators
- TX level metering and spectral mask check

### Sources
- Modulators for the remaining modes, over the frame/bit codec each protocol already owns
- IQ recording playback to air
- Signal generator and arbitrary waveforms to air

### Links
- Satellite uplink on the Doppler-corrected frequency the Satellite node already shows
- Bench loopback: TX into your own RX to validate decoders. The graph's no-cycle proof stops
  being sufficient here
- Beam-steering CW (TX MIMO)

### Security research (own DUT, contained link)
- Sub-GHz capture, decode, replay
- Fixed-code analysis and generation, including de Bruijn sequences
- Rolling-code capture and implementation analysis
- Interference and jam-susceptibility testing
- Flood, spam and malformed-broadcast testing
- Targeted protocol fuzzing

## 2. Hardware
- RX-888 / Mk2 native driver
- Rotator control: GS-232, rotctld
- Rig control: Hamlib CAT client, rigctld-compatible server
- TinySA import
- Saved antenna profiles: store NanoVNA sweeps against a named antenna

### Codeplugs
- AnyTone D890UV: the general-settings block, GPS, both APRS flavours, roaming, encryption keys,
  DTMF/2-tone/5-tone, satellite and boot settings, and the per-channel long tail (custom CTCSS,
  talkaround, call confirm, ranging, scrambler, TX colour code) are not read
- AnyTone: transmit shift direction bits are inferred, not measured. Needs a radio read with a
  shifted channel
- AnyTone GD32 family (D868/D878/D578): same serial protocol, needs its memory map
- Radtel RT-4D: settings blocks, keys and message templates are read but not modelled

## 3. Receive DSP
- ESPRIT next to the correlative and MUSIC estimators. The circular array needs the beamspace form
- Interferometer
- DeepFilterNet3 neural noise reduction on the listen path
- Auto-squelch: tell a floor step from a signal. Today a floor that jumps in one step reads as a
  signal until the channel next falls quiet

## 4. Decoders

### Voice, trunking & cellular
- TETRA, Tetrapol
- GSM downlink analysis, OsmocomBB-style monitoring

### Weather & satellites
- NOAA APT, Meteor M-2 LRPT
- HF WEFAX: the SSTV picture store already fits. Needs the decoder and line geometry
- Radiosondes: RS41, DFM, M10/M20, iMet, with map and log
- APRS weather aggregation

### Broadcast & wideband
- DRM: FAC, SDC, MSC, service selection and audio. Today it only locks and reports SNR and
  frequency error
- DVB-S2X Annex E superframe formats 2 to 7, non-default superframe scrambling and WH codes
- DAB modes II to IV on air
- Real-world validation of DVB-T and DVB-S/S2: real transmitters, fading, adjacent-channel
  interference. Today only synthetic IQ

### Sub-GHz, ISM & IoT
- More rtl_433 sensors
- LoRa (ChirpChat), LoRaWAN frames, Meshtastic, MeshCore
- End-of-Train (EOT) telemetry
- BLE advertisements, 2.4 GHz survey, Wi-Fi channel occupancy (energy only)

### DECT
- B-field voice (ADPCM/G.726 on unencrypted bearers)
- Extended fixed part capability messages (QH = 4, C, E) with DSAA2/DSC2 bits
- All ten carriers from one wideband capture

### Other
- STANAG modem identification

## 5. Recording & measurement
- Recording scheduler and unattended satellite-pass automation
- Noise figure, PER tester, SID monitor
- Radio astronomy, star tracker, sky map

## 6. Map
- Layers for sondes, satellites, beacons

## 7. Field mode
- Redo field mode as its own app

## 8. Automation & API
- Desktop and push notifications from Event filter
- WASM plugin SDK
- Offline bundles for TLE snapshots and callsign prefixes
