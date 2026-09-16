# Channels and decoding

A channel receives one frequency from a Device's IQ stream. Retuning the Device preserves channel
frequencies. Channels outside its reception range stay configured and resume when the radio covers
them again.

## Add a channel

Choose a mode from **+ Node** and connect Device `IQ` to channel `IQ`. Set the channel frequency,
then connect the outputs you need:

| Output | Destination | Result |
|---|---|---|
| `audio` | Speaker | Live audio |
| `events` | Readout | Station text, aircraft tables, and other current state |
| `events` | Decoder log | Stored message history |
| `events` | Map | Decoded positions |
| `events` | Export | CSV or JSON of stored rows |
| `video` | Video | ATV frames or an SSTV picture |

## Channel catalog

The **Decoders** palette lists modes available in the running build. Support and test coverage
vary by mode:

| Group | Channels | Maturity |
|---|---|---|
| Analog voice | AM, NFM, SSB, WFM (broadcast, with stereo and RDS) | tested on air |
| Digital voice | DMR | tested on air |
| Digital voice | FreeDV 1600 | tested on air |
| Digital voice | D-STAR, System Fusion, NXDN, P25 Phase 1, dPMR, M17 | fixture-only |
| Aviation | ADS-B (1090ES) | tested on air |
| Aviation | ACARS, VDL Mode 2, HFDL, Inmarsat Classic Aero | fixture-only |
| Aviation | VOR, ILS localizer / glideslope | experimental |
| Marine | AIS, NAVTEX, Digital Selective Calling, Inmarsat STD-C / EGC | fixture-only |
| Amateur data and HF | APRS / AX.25, RTTY, PSK (31, 63, 125, 250 baud), Morse (CW), CW skimmer, FT8, FT4, WSPR | fixture-only |
| Paging and telemetry | POCSAG | tested on air |
| Paging and telemetry | FLEX, ERMES, Selcall (CCIR/ZVEI), Sub-GHz OOK/FSK frames, ISM sensors, radio clocks (DCF77, WWVB, MSF, JJY) | fixture-only |
| Video | ATV, SSTV | fixture-only |
| Wideband digital | DAB / DAB+, DATV (DVB-S / S2), DRM30 / DRM+ | experimental |
| Utility | Signal identifier, Iridium bursts, DECT base station survey | fixture-only |
| Utility | GNSS lab (GPS L1 C/A) | experimental |

Optional services, trunking variants, and vendor extensions may be unsupported. Check the
mode-specific limits below.

## What the maturity labels mean

| Label | Evidence |
|---|---|
| **tested on air** | Live reception verified through the receiver and decoder integration |
| **fixture-only** | Generated IQ, reference vectors, or recordings tested; live integration unverified |
| **experimental** | Partial acquisition, decoding, or measurement support |

Fixture tests catch decoding errors but provide limited evidence for drift, interference,
transients, and multipath. Labels apply only to the tested services.

The [fixture library](https://github.com/Newspicel/sdrminusminus/blob/main/fixtures/README.md)
lists recording origins and expected output, including DMR, ADS-B, FreeDV 1600, and FT8.
Some modes also use published protocol vectors. Iridium uses off-air bits in a synthetic waveform.

VDL Mode 2, HFDL, Inmarsat Classic Aero, Inmarsat STD-C, and DSC use
[xng](https://github.com/airframesio/xng). Their labels describe the SDR-- integration's coverage.

## ISM sensors

A Sub-GHz channel decodes known sensor payloads and displays raw frames for other signals.
Supported devices are grouped by pulse coding:

| Coding | Devices |
|---|---|
| Pulse position | Nexus-T/TH, Rubicson (also Solight TE44, EMOS E0107T), Acurite 609TXC, Acurite 606TX, Prologue-TH, inFactory-TH, Kedsum-TH, Springfield soil probe |
| Pulse width | LaCrosse TX141TH-Bv2, Fine Offset WH2, Auriol HG02832, Geevon TX16-3, WS2032 weather mast, EMOS E6016 rain gauge, Rubicson 48942 pool, WT0124 pool, Opus XT300 soil probe |
| Manchester | Ambient Weather F007TH |
| Pulse code (FSK) | Ambient Weather WH31E, Renault TPMS, Toyota TPMS |
| Differential Manchester | WT450-TH |

Readings can include temperature, humidity, soil moisture, wind speed and direction, rainfall,
tyre pressure, and power. Renault TPMS adds Manchester coding after framing; Toyota TPMS adds
differential Manchester.

The decoder checks pulse timings and the device's checksum, digest, or parity before reporting
a reading. Unrecognised bursts remain available in the raw timing view. FSK sensors use bit periods
of 55–58 µs; the default minimum pulse width admits these signals.

Pulse slicing, payload layouts, validation rules, and CRC/LFSR digest routines follow
[rtl_433](https://github.com/merbanan/rtl_433), licensed GPL-2.0-or-later.

## Experimental mode limits

| Mode | Available output | Missing or limited functionality |
|---|---|---|
| DAB / DAB+ | Modes I–IV, FIC and MSC decoding, classic DAB MPEG Layer II audio, CRC-checked DAB+ access units | DAB+ audio and data services remain unavailable |
| DATV | DVB-S/S2 transport packets, programme tables, MPEG Layer II audio, or generic-stream datagrams | AAC, AC-3 and video decoding remain unavailable; DVB-T is not implemented |
| DRM30 / DRM+ | Acquisition, lock, SNR, and frequency error | No FAC, SDC, or MSC decoding; no service labels or media |
| GNSS lab | GPS L1 C/A acquisition and NAV telemetry | No position solution |
| VOR / ILS | Radial or difference in depth of modulation | Tested only against analytically generated signals |

To add on-air coverage, contribute a short IQ capture restricted to the relevant band, with its
expected decoded output. See [Build and test](../development/building.md) and the
[contribution guide](https://github.com/Newspicel/sdrminusminus/blob/main/CONTRIBUTING.md).

## Pager text

POCSAG uses seven-bit text. Some German networks substitute umlauts and ß using DIN 66003.
SDR-- applies that mapping inside words next to lowercase letters: `M}nchen` becomes `München`
and `Stra~e` becomes `Straße`.

Other text stays ASCII, including `[ALARM]` and entirely uppercase messages. There is no manual
character-set setting.

## Sample rate and passband

Keep the channel's full occupied bandwidth inside the Device's reception range. If it does not
fit, retune the Device, move the channel, or increase the sample rate.

Most channels resample IQ internally. These modes require a specific device rate:

| Channel | Device rate |
|---|---|
| ADS-B | 2–4 MS/s |
| ATV | 2–20 MS/s |
| GNSS lab | 2.048 MS/s |

The channel reports incompatible rates and offers a suitable choice. Use the lowest rate that
covers your signals to reduce USB traffic and CPU load.

## Tuning and squelch

Tune through the channel dial, its Scope marker, or keyboard shortcuts. Direct entry accepts MHz
by default, or an explicit `kHz`, `MHz`, or `GHz` suffix. Step buttons adjust by −25, −5, +5, or +25 kHz.

The lock beside a dial prevents changes to that frequency. A locked channel does not lock its
source Device. You can set channel frequencies before connecting a radio; an untuned Device
initially opens over its connected channels.

### Squelch

| Mode | Behaviour |
|---|---|
| Off | Pass all signals |
| Manual | Open above a fixed level; lower thresholds open more easily |
| Auto | Open a chosen number of dB above the measured noise floor |

The level meter marks the opening threshold. Auto learns during quiet periods, so a continuous
signal can be mistaken for noise. Once open, the floor cannot rise and suppress a long transmission.
Returning to Manual restores the previous manual threshold.

NFM also supports tone squelch:

| Setting | Behaviour |
|---|---|
| Detect | Report CTCSS or DCS without gating audio |
| CTCSS | Open only for the selected tone |
| DCS | Open only for the selected code |

**Compander** applies 2:1 audio expansion for links using matching compression. Leave it off for
ordinary NFM. Expansion stops 20 dB below the reference level; sub-audible tones are excluded from
level tracking.

## Audio processing

The **Audio** block processes stages in this order. All are off by default except AM and SSB AGC.

| Stage | Effect and controls |
|---|---|
| **Blanker** | Removes IQ impulses before the channel filter. Lower thresholds remove more impulses but can also damage the wanted signal. |
| **De-click** | Removes short audio impulses after demodulation. Detection compares each sample with the surrounding level and neighbours; width is set by mode. |
| **Passband** | Sets low and high audio cutoffs. Narrow the range to the audio you need. |
| **Notches** | Removes up to four selected frequencies, each with an adjustable width. |
| **Auto notch** | Suppresses steady carriers without manual frequency selection. |
| **Denoise** | Tracks the noise floor in each spectral bin and attenuates bins without a detected signal. Strength ranges from no attenuation at 0 to 20 dB at 100. Continuous carriers can be treated as noise. |
| **AGC** | Levels audio. Slow suits SSB speech, fast suits tuning, and medium provides an intermediate response. |

Blanker acts on IQ before filtering to reduce impulse ringing. The remaining stages process audio.

## Identifying a signal

Add **Signal identifier** and select a span up to 192 kHz wide. It reports detected transmissions,
loudest first, with modulation, frequency, bandwidth, symbol rate, deviation, burst timing, and
OFDM timing where measurable.

Candidates combine four kinds of evidence:

| Evidence | Contribution |
|---|---|
| Waveform | Modulation and measured timing |
| Frequency | Likely services for the band |
| Bursts | Distinguishes signals with similar modulation |
| Decoder checks | Confirms candidates through valid frames, checksums, or digital-voice sync |

**Confirmed** candidates outrank waveform matches. Confirmation is available where an integrated
decoder can run at the identifier's rate.

**Interval** sets the observation length. **Threshold** sets the required level above noise.
Results settle across recent windows to reduce changes caused by one noisy measurement.

The identifier can recognise some wider signals from a partial slice, but cannot detect
spread-spectrum signals below noise or resolve densely packed 50 Hz HF signals.
For fixture comparisons, run `cargo xtask ident-matrix`.

## Slow-scan television

Tune SSTV to the SSB carrier. It receives the 1000–2600 Hz video subcarrier above that frequency.
Pictures take roughly 36 seconds to four and a half minutes, depending on mode.

| Setting | Effect |
|---|---|
| Follow VIS | Read the transmitted mode header automatically |
| Manual mode | Decode using the selected mode when the header is missed or damaged |
| Slant correction | Track line sync to correct sample-clock differences; normally leave enabled |
| Keep unfinished pictures | Save partial images after a fade or interrupted transmission |

Supported modes are Robot 36/72, Martin M1/M2, Scottie S1/S2/DX, PD50/90/120/180, and Wraase SC2-180.

Connect `video` to **Video** to watch reception line by line. Finished and retained partial images
are saved as PNGs on the server, including while no client is connected. The channel panel lists
them. Retention is 24 hours, capped at 512 images.

## Surveying a DECT network

The `dect` channel surveys identity, configuration, and security signalling on one carrier.
It reads the A-field, excluding call audio and user data in the B-field.

Use a receiver covering the DECT band with at least 2.304 MS/s. An RTL-SDR cannot reach the band;
HackRF and SDRplay can. Carriers occupy 1.728 MHz.

| Setting | Choice |
|---|---|
| Band | Europe: 1880–1900 MHz; US: 1920–1930 MHz |
| Side | Base, Handset, or Both |

European carrier 0 is 1897.344 MHz; carrier numbers descend in 1.728 MHz steps to 1881.792 MHz.
US carriers count upward from 1921.536 MHz.

Bursts are grouped by slot timing to separate base stations sharing a carrier. Each A-field must
pass its R-CRC check. Records include:

| Field | Contents |
|---|---|
| RFPI | Base identity, access-rights class, operator or manufacturer, and cell identifiers |
| System information | Carrier, frequency, slot pair, transceiver count, available carriers, scan carrier |
| Capabilities | Slot types, frequency control, handover, connectionless and higher-layer services |
| Security | Advertised DSAA authentication and DSC ciphering, observed encryption negotiation, key index when present |
| Handsets | PMIDs seen in encryption handshakes and the fixed part's FMID |

Burst and error counts appear per station. Encryption is marked active after an observed grant.
Advertised support does not prove encryption was used, and missing signalling does not prove a
call was unencrypted.

## Following a DMR trunk system

Add **DMR trunk system**, connect Device `iq`, and enter the control-channel frequency in MHz.
Choose a system type or auto-detect. The node manages the required DMR decoders.

| System | Channel discovery |
|---|---|
| Tier III, including Capacity Max | Learns logical channel definitions and follows voice grants |
| Capacity Plus | Uses Repeater outputs or Search to find carriers sharing rest-channel changes; follows both timeslots |
| Hytera XPT | Uses the same discovery approach with XPT signalling |

Following continues on the server without an open browser. Traffic channels must fit in the
Device passband; out-of-range grants report a failure. Increase the sample rate or retune as needed.

**Record calls** buffers completed calls and audio in memory. Encrypted calls retain metadata only.
Disable it to follow traffic without audio buffering.

## Where decoder output goes

Events include source, frequency, and timestamp. Use **Readout** for current state, **Decoder log**
for message history, **Map** for positions, and **Export** for saved rows.

Decoder-log retention is bounded. SSTV images use a separate picture store: the log records
arrival, while `GET /api/images` serves the pictures.

## DAB and DVB audio

For classic DAB, choose **Generation → DAB** and connect `audio` to a **Speaker** node.
**Auto** selects the first audio service, which may use DAB+ and therefore have no playback yet.
**Transmission** selects I, II, III or IV; existing channels default to I. All modes require
2.048 MS/s at the channel input. Changing mode resets acquisition and service selection.

DVB-S/S2 plays the first MPEG Layer II audio stream in the selected programme. Mono is copied
to both output channels; other supported sample rates are converted to 48 kHz. The decoder log
shows audio-frame counts and decode or queue failures separately from radio lock. Unsupported
AAC, AC-3 and DAB+ audio is reported explicitly.

Mode and audio tests include independent synthetic waveforms, an independent MPEG Layer II
PCM reference, and virtual-device playback through the normal audio output. These checks do
not establish reception quality under real antenna fading.
