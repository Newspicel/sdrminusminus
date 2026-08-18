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

A channel face warns when its audio or video output goes nowhere. NFM is the exception: its event
output is commonly unused unless CTCSS or DCS detection is enabled.

## Channel catalog

The node palette splits channels into **Modes**, which produce audio, and **Decoders**, which
produce events. The server reports the exact catalog for the running build; this is the current
list.

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
| Utility | Signal identifier, Iridium bursts | fixture-only |
| Utility | GNSS lab (GPS L1 C/A) | experimental |

Coverage varies by protocol. A listed mode means the signal path and the documented frame layers
are implemented; it does not mean every optional signalling service, trunking system or vendor
extension is supported. The [feature roadmap](https://github.com/Newspicel/sdrminusminus/blob/main/FEATURES.md)
records known gaps.

## What the maturity labels mean

| Label | What it means |
|---|---|
| **tested on air** | Decoded live from a real transmitter, through the whole stack from the receiver to the decoder log. |
| **fixture-only** | Decodes a golden IQ fixture rendered by sdr--'s own modulator, plus the worked examples the standard publishes. The frame layers are proven; the receiver has not been held against a real transmitter. |
| **experimental** | Acquisition, lock, or measurement only — no payload decoded — or a lab implementation rather than an operational one. |

Most decoders are fixture-only. A fixture proves that the decoder undoes what our own modulator
did, which catches real bugs but says nothing about transmitter drift, keying transients,
adjacent-channel splatter, or multipath. Treat a fixture-only mode as a decoder that should work
rather than one that is known to.

Two of the on-air modes also carry a committed capture, so the proof survives as a regression
test. DMR reads `dmr_call_48k`, a direct-mode call on PMR446 captured with an RTL-SDR, which is
the only signal in the tree that keys off between bursts the way a real TDMA transmitter does.
FreeDV 1600 reads the FreeDV project's own receive test recording. The rest were confirmed against
live traffic — broadcast FM with its RDS station identity, ADS-B aircraft with solved positions,
and commercial POCSAG paging — but a capture of that traffic is not redistributable, so it is not
committed.

Where a standard publishes worked examples — ADS-B position and identification frames, the APRS
compressed-position examples, the CCIR 476 alphabet, the radio-clock golden minutes — the decoders
are checked against those too, but a published vector is still not a transmitter.

Iridium is a middle case: its test replays a bit sequence taken off the air, re-modulated by the
project's own transmitter, so the framing is real traffic while the waveform is synthetic. VDL
Mode 2, HFDL, Inmarsat Classic Aero, Inmarsat STD-C and Digital Selective Calling use decoders
from the [xng](https://github.com/airframesio/xng) project, which are exercised against real
traffic upstream; the label describes this integration, not that work.

## ISM sensors

A Sub-GHz channel reads named sensor payloads as well as raw frames. Ten devices are recognised
across five pulse codings:

| Coding | Devices |
|---|---|
| Pulse position | Nexus-T/TH, Rubicson (also Solight TE44, EMOS E0107T), Acurite 609TXC, Acurite 606TX, Prologue-TH, inFactory-TH, Kedsum-TH, Springfield soil probe |
| Pulse width | LaCrosse TX141TH-Bv2, Fine Offset WH2, Auriol HG02832, Geevon TX16-3, WS2032 weather mast, EMOS E6016 rain gauge, Rubicson 48942 pool, WT0124 pool, Opus XT300 soil probe |
| Manchester | Ambient Weather F007TH |
| Pulse code (FSK) | Ambient Weather WH31E, Renault TPMS, Toyota TPMS |
| Differential Manchester | WT450-TH |

Beyond temperature and humidity a reading can carry soil moisture, wind speed and direction,
rainfall, tyre pressure, and power, so a weather mast, a soil probe and a tyre sensor all land in
the same decoder log. Two of the tyre sensors stack a second coding on top of their bits —
Manchester for the Renault, differential Manchester for the Toyota — which is undone after
framing, the way the flexible decoder in rtl_433 expresses it.

Each device is matched on its own pulse timings, then accepted only if its checksum, digest or
parity closes, so an unrecognised burst still falls through to the raw timing view rather than
being reported as a reading. FSK sensors clock at 55-58 µs a bit, which is finer than the
default minimum pulse width the channel debounces at; the default is set low enough to admit
them.

The pulse slicers, ISM sensor payload layouts, their validation heuristics, and the CRC and LFSR
digest routines those layouts check with follow [rtl_433](https://github.com/merbanan/rtl_433)
(GPL-2.0-or-later), which documents the pulse timings and field positions each of these sensors
transmits.

The wideband digital channels are **acquisition only**. They report waveform lock, SNR, frequency
error, and the configured symbol rate where one applies. They do not decode DAB FIC/MSC, DVB
transport streams, DRM FAC/SDC/MSC, programme audio, or DATV pictures. A missing service label
means the multiplex layer was not decoded, not that the station has no name.

The GNSS lab acquires GPS L1 C/A and reads NAV telemetry for study; it is not a positioning
receiver. VOR and ILS report a radial and a difference in depth of modulation rather than decoding
a frame, and both are so far checked only against analytically generated signals.

An off-air capture that promotes a mode to *tested on air* is among the most useful contributions
the project can receive. Keep it to a few seconds, stripped to the band of interest, and pair it
with the decoded output it should produce — see
[Build and test](../development/building.md) and the
[contribution guide](https://github.com/Newspicel/sdrminusminus/blob/main/CONTRIBUTING.md).

## Sample rate and passband

A channel's occupied band must fit inside its source device's current passband. If it does not,
move the channel closer to center, raise the device sample rate, or retune the device.

Most channels are resampled to whatever rate they prefer, so the device rate does not matter. A few
decode the device's own samples and constrain it:

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

**Auto** hands the threshold to the channel: it measures the channel's own noise floor and holds
the gate a chosen number of decibels above it, so a quiet channel and a noisy one are configured
the same way. The level meter draws a notch where the gate currently sits. Two limits are worth
knowing:

- The floor is learned while the channel is quiet. A channel that is never quiet has no floor to
  find, so the gate only opens on something louder than what is already there.
- The floor is never raised while the gate is open, so a long transmission cannot squelch itself.

The manual threshold is kept, and the gate returns to it when auto is switched off.

NFM adds tone squelch:

- **Detect** reports any recognized CTCSS tone or DCS code without gating audio.
- **CTCSS** opens only for a selected standard tone.
- **DCS** opens only for a selected standard code.

**Compander** expands the audio 2:1 about a fixed reference, undoing the 2:1 compression that
commercial and trunked NFM gear applies before it transmits. Every decibel a quiet passage sat
below the reference on the air is turned back into two, which puts the hiss between syllables back
down where the transmitter's compressor found it. Turn it on only for a link that is actually
compandered: on plain NFM it widens the dynamic range that was never narrowed, so quiet speech
falls away. The same switch compresses on transmit, so two channels set the same way are a matched
pair. Expansion stops 20 dB below the reference, and the sub-audible tone is kept out of the level
the expander follows whether or not tone squelch is in use.

## Audio processing

Every channel that produces audio has the same processing chain in its **Audio** block. All of it
is off by default except AGC on AM and SSB, which have no levelling of their own.

| Stage | What it does |
|---|---|
| **AGC** | Levels audio to a fixed target at one of three speeds. *Slow* suits SSB speech, *fast* suits tuning across a band, *medium* is a reasonable default. |
| **Blanker** | Cuts impulse noise — ignition, switching supplies — out of the IQ before the channel filter, while a pulse is still a pulse. The threshold is a multiple of the channel's average level: lower blanks more, and low enough blanks the signal too. |
| **De-click** | Removes impulses created after the demodulator: FM discriminator clicks and the static crashes an AM or SSB detector passes through. A sample must stand out both from the audio's level and from its neighbours before it is replaced, so loud speech is left alone. The click width is set per mode and is not a control. |
| **Denoise** | Spectral noise reduction. It tracks what each part of the spectrum does at its quietest, decides bin by bin whether anything is speaking over that, and holds down the rest. Strength sets how far a quiet bin may be pulled: nothing at 0, 20 dB at 100. It gives up gain gradually rather than in steps, so what is left sounds like steady hiss instead of the warbling that a hard subtraction leaves behind. Anything genuinely unchanging counts as noise, including an unbroken carrier. |
| **Auto notch** | Removes steady carriers, such as an adjacent heterodyne, without being told where they are. Several at once cost no more than one. |
| **Passband** | A low and high cut on the audio. Narrow it until only the voice is left. |
| **Notches** | Up to four operator-placed nulls, each with its own frequency and width. A narrow one removes a whistle and leaves the voice around it. |

The chain runs in that signal order: blanker on the IQ, then de-click, passband, notches, auto
notch, denoise and AGC on the audio. Impulses are removed first, because a filter turns one into
ringing.

## Slow-scan television

An SSTV picture scans out over 36 seconds to four and a half minutes, so the channel behaves more
like a video source than a log line. Tune it as you would any audio-band mode: put the dial on the
SSB carrier, and the channel takes the 1000–2600 Hz above it where the video subcarrier lives.

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

## Following a DMR trunk system

The **DMR trunk system** node turns a control channel into the traffic channels it grants. Wire a
radio's `iq` output into it, name the control channel in MHz, and choose the system type, or leave
it on auto-detect and let the signalling identify itself. The node runs its own decoders, so no DMR
channel has to be drawn for it.

| System | How it is followed |
|---|---|
| Tier III (including Capacity Max) | Logical channels are learned from the system's own channel definitions; a receiver opens when a voice grant names one. |
| Capacity Plus | No frequency is granted, so every repeater output is itself a traffic channel. List them under **Repeater outputs** and both timeslots of each are followed once the system is recognized. |
| Hytera XPT | As Capacity Plus, using XPT's own signalling. |

The receivers it opens belong to the server, not to the patch, so following continues while no
browser is connected. They obey the same passband rule as any other channel: a traffic channel
outside the radio's current sample rate cannot be opened, and the node says so rather than failing
quietly. Widen the sample rate, retune so the traffic channels fall inside it, or give the system a
second radio.

With **Record calls** on, completed calls are buffered in memory, audio included. Encrypted
transmissions keep only their metadata. Turn it off to follow traffic without buffering any audio.

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
