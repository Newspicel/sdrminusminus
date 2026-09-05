# Channels and decoding

A channel selects one signal inside a device's sampled passband. Its offset is relative to the
device center frequency: retuning the device moves every attached channel together, while changing
a channel's offset moves only that channel.

## Add a channel

Add a channel from **+ Node**, then wire the Device's `IQ` output into the channel's `IQ` input.
Connect the outputs you need:

| Channel output | Connect to | Result |
|---|---|---|
| `audio` | Speaker | Browser audio |
| `events` | Readout | Accumulated state, such as a station or aircraft table |
| `events` | Decoder log | Stored, filterable message history |
| `events` | Map | Positions from ADS-B, AIS, APRS and other locating decoders |
| `events` | Export | CSV or JSON download of stored rows |
| `video` | Video | ATV frames, and an SSTV picture as it scans out |

## Channel catalog

The node palette lists every channel type under **Decoders**, whether it produces audio or events.
The server reports the exact catalog for the running build; this is the current list.

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

Coverage varies by protocol. The catalog lists implemented signal paths, but optional services,
trunking variants, and vendor extensions may be unsupported. Check the mode-specific limits below.

## What the maturity labels mean

| Label | Evidence |
|---|---|
| **tested on air** | Verified with a real transmitter through the receiver and decoder integration |
| **fixture-only** | Tested with generated IQ and, where available, published reference vectors; this integration has not been verified on air |
| **experimental** | Partial acquisition, decoding, or measurement support; not an operational receiver for the full service |

Generated fixtures catch decoding errors, but do not establish tolerance to transmitter drift,
keying transients, adjacent-channel interference, or multipath. Most decoders have only this coverage.

Committed recordings add regression coverage for DMR, ADS-B, FreeDV 1600, and a busy FT8 slot.
Their origins and expected output are listed in the
[fixture library](https://github.com/Newspicel/sdrminusminus/blob/main/fixtures/README.md).
A recording test does not necessarily verify the whole live receive path, and a maturity label
applies only to the services tested.

Some decoders also use worked examples from their standards, including ADS-B frames, APRS compressed
positions, CCIR 476 characters, and radio-clock minutes. Iridium tests use an off-air bit sequence
with a synthetic waveform, which tests real framing but not real RF conditions.

VDL Mode 2, HFDL, Inmarsat Classic Aero, Inmarsat STD-C, and Digital Selective Calling use decoders
from [xng](https://github.com/airframesio/xng). Their labels describe the sdr-- integration,
separately from upstream testing.

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
| DAB / DAB+ | FIC and MSC decoding, CRC-checked DAB+ access units | No audio codec or playback |
| DATV | DVB-S/S2 transport packets and programme tables, or generic-stream datagrams | No audio or video codec output |
| DRM30 / DRM+ | Acquisition, lock, SNR, and frequency error | No FAC, SDC, or MSC decoding; no service labels or media |
| GNSS lab | GPS L1 C/A acquisition and NAV telemetry | No position solution |
| VOR / ILS | Radial or difference in depth of modulation | Tested only against analytically generated signals |

To add on-air coverage, contribute a short IQ capture restricted to the relevant band, with its
expected decoded output. See [Build and test](../development/building.md) and the
[contribution guide](https://github.com/Newspicel/sdrminusminus/blob/main/CONTRIBUTING.md).

## Pager text

POCSAG uses seven-bit characters. Some German networks use DIN 66003, which replaces ASCII
brackets and related punctuation with umlauts and ß.

sdr-- applies this mapping when the affected character appears inside a word beside a lowercase
letter: `M}nchen` becomes `München`, and `Stra~e` becomes `Straße`. Otherwise it keeps ASCII, so
`[ALARM]` retains its brackets. Entirely uppercase pages remain ASCII. There is no manual setting.

## Sample rate and passband

A channel's occupied band must fit inside its source device's current passband. If it does not,
move the channel closer to center, raise the device sample rate, or retune the device.

Most channels resample device IQ to their processing rate, provided their occupied band fits in
the device passband. The following channels process samples at the device rate and require:

| Channel | Required device rate |
|---|---|
| ADS-B | 2–4 MS/s |
| ATV | 2–20 MS/s |
| GNSS lab | 2.048 MS/s |

The channel face says so when the current rate cannot work, and offers a compatible one.

Otherwise use the lowest rate that covers the signals you need. Higher rates increase USB traffic,
FFT work and CPU load without improving a narrow channel.

## Tuning and squelch

Tune a channel by editing its offset, dragging its marker on a connected Scope, or using the
keyboard while the channel is selected. The displayed absolute frequency is the source stream's
center plus the channel offset.

**Freq** next to the offset takes an absolute frequency instead and works the offset out for you.
A bare number is read as megahertz; a `kHz`, `MHz` or `GHz` suffix is honoured. Frequencies the
current sample rate cannot reach are rejected, and the reachable span is shown below the field.

Audio channels can gate their output with squelch. A lower threshold opens more easily; turning
squelch off passes audio continuously.

**Auto** sets the threshold a chosen number of decibels above the channel's measured noise floor.
The level meter marks the current threshold.

The channel learns the noise floor during quiet periods. A continuous signal may be mistaken for
the floor, requiring a stronger signal to open the gate. Once the gate opens, the floor cannot rise
and suppress a long transmission.

Disabling Auto restores the saved manual threshold.

NFM adds tone squelch:

- **Detect** reports any recognized CTCSS tone or DCS code without gating audio.
- **CTCSS** opens only for a selected standard tone.
- **DCS** opens only for a selected standard code.

**Compander** applies 2:1 audio expansion to receive signals transmitted with matching compression.
Enable it only for a companded NFM link; ordinary NFM speech can become too quiet with expansion.
The corresponding transmit setting applies compression. Expansion stops 20 dB below the reference
level, and sub-audible tones are excluded from level tracking.

## Audio processing

Audio channels share an **Audio** block. Processing is off by default except for AGC on AM and SSB.
The stages run in the order shown below.

| Stage | Effect and controls |
|---|---|
| **Blanker** | Removes IQ impulses before the channel filter. Lower thresholds remove more impulses but can also damage the wanted signal. |
| **De-click** | Removes short audio impulses after demodulation. Detection compares each sample with the surrounding level and neighbours; width is set by mode. |
| **Passband** | Sets low and high audio cutoffs. Narrow the range to the audio you need. |
| **Notches** | Removes up to four selected frequencies, each with an adjustable width. |
| **Auto notch** | Suppresses steady carriers without manual frequency selection. |
| **Denoise** | Tracks the noise floor in each spectral bin and attenuates bins without a detected signal. Strength ranges from no attenuation at 0 to 20 dB at 100. Continuous carriers can be treated as noise. |
| **AGC** | Levels audio. Slow suits SSB speech, fast suits tuning, and medium provides an intermediate response. |

The blanker runs on IQ; the remaining stages run on audio. Removing impulses before filtering
reduces the ringing they would otherwise cause.

## Slow-scan television

An SSTV picture takes 36 seconds to four and a half minutes to receive, depending on mode.
Tune to the SSB carrier; the channel processes the 1000–2600 Hz video subcarrier above it.

A transmission names its own mode in the VIS header that precedes it. **Follow VIS**, the default,
reads that header and recognizes Robot 36 and 72, Martin M1 and M2, Scottie S1, S2 and DX, PD50,
PD90, PD120 and PD180, and Wraase SC2-180. Pick a mode by hand when the header was missed or
corrupted; the decoder then starts on any header it sees and scans it as the mode you chose.

**Slant correction** tracks each line's sync pulse instead of free-running from the header, which
keeps the picture upright when your sample clock and the transmitter's disagree. Leave it on unless
you are diagnosing the sync itself.

**Keep unfinished pictures** decides what happens when a transmission fades or is cut short. On,
the lines that did arrive are kept; off, only a picture that scanned to its last line is.

Wire the channel's `video` output into a Video node to watch a picture build up line by line. Every
finished picture, and every kept partial, is also stored on the server as a PNG and listed in the
channel's own panel, so a picture that arrived while no browser was connected is still there. The
store holds 24 hours of pictures, capped at 512 of them.

## Surveying a DECT network

The `dect` channel surveys base stations on one DECT carrier. It reads the 64-bit A-field in each
burst for identity, configuration, and authentication or ciphering signalling. It does not decode
the B-field containing call audio and user data.

A DECT carrier is 1.728 MHz wide and the channel runs at 2.304 MHz, so the receiver needs at least
that much bandwidth and must reach the band: 1880–1900 MHz in Europe, 1920–1930 MHz in the US.
An RTL-SDR tops out below the band and cannot be used; a HackRF or an SDRplay can.

Set **Band** so carrier numbers resolve to frequencies, and set **Side** to `Base` if you only want
the fixed part, `Handset` for portables, or `Both`. Carrier 0 is the *highest* frequency in the
European band (1897.344 MHz) and they count downwards in 1.728 MHz steps to carrier 9 at
1881.792 MHz; the US band counts upwards from 1921.536 MHz.

Each base station transmits a dummy bearer once per 10 ms frame in a fixed slot, cycling through
the identity and system-information messages. The decoder groups bursts by their slot timing, so
several base stations sharing one carrier stay apart, and folds each one into a single record:

- **RFPI** — the 40-bit Radio Fixed Part Identity, broadcast on the Nt channel. It splits into the
  access rights class (A residential, B private multi-cell, C public, D GSM/UMTS, E direct), the
  manufacturer, installer or operator code, the fixed part number and sub-number, and the radio
  fixed part number that separates cells within one system. Class C and D encode single-cell versus
  multi-cell in the low bit of the RPN.
- **System information** — the carrier the base is on and its frequency, which slot pair it uses,
  how many transceivers it has, which of the ten carriers it says are available, and its primary
  scan carrier number.
- **Capabilities** — the fixed part capabilities broadcast, decoded bit by bit: slot types,
  frequency control, handover, the connectionless services, and the higher-layer services.
- **Security** — whether the base advertises **standard authentication (DSAA)** and **standard
  ciphering (DSC)**, and, separately, whether encryption was actually negotiated on the air. MAC
  encryption-control messages are followed through request, confirm and grant, so a bearer shows as
  encrypted only once the grant is seen. A cipher key index is reported when the base uses the
  keyed variant.
- **Handsets** — the PMIDs seen in encryption handshakes, plus the FMID of the fixed part.

Each A-field must pass its R-CRC check. Burst and error counts appear beside each station.

Advertised ciphering support does not establish whether a call uses encryption. The reported
encryption state follows observed request, confirm, and grant messages; missing signalling is
not proof that a call is unencrypted.

## Following a DMR trunk system

Add a **DMR trunk system** node, connect a Device's `iq` output, and enter the control-channel
frequency in MHz. Select a system type or use auto-detect. The node manages its own DMR decoders.

| System | Channel discovery |
|---|---|
| Tier III, including Capacity Max | Learns logical channel definitions and opens traffic channels named in voice grants |
| Capacity Plus | Uses **Repeater outputs**, or **Search** to find carriers that announce and follow the same rest-channel changes; follows both timeslots |
| Hytera XPT | Uses the same approach as Capacity Plus with XPT signalling |

Following runs on the server even when no browser is connected. Traffic channels must fit in the
source radio's passband. If a grant falls outside it, the node reports the failure. Increase the
sample rate or retune to include the required frequencies.

Enable **Record calls** to buffer completed calls and their audio in memory. Encrypted calls retain
metadata only. Disable it to follow traffic without buffering audio.

## Where decoder output goes

Decoder events are typed on the server and timestamped with source and frequency information.
Choose the destination that matches the job:

| Node | Use it for |
|---|---|
| Readout | Changing state, such as RDS text or a table of tracked aircraft |
| Decoder log | Independent messages and frames, stored in SQLite for filtering and review |
| Map | Recent position tracks from locating decoders |
| Export | Downloading the stored rows wired into it |

Decoder log history is bounded, so a busy unattended receiver cannot grow the database forever.

Pictures are not decoder-log rows. A completed SSTV picture writes one line to the log recording
what arrived, while the pixels go to the picture store and are served from `GET /api/images`.
