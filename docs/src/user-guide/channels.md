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

| Group | Channels |
|---|---|
| Analog voice | AM, NFM, SSB, WFM (broadcast, with stereo and RDS) |
| Digital voice | DMR, D-STAR, System Fusion, NXDN, P25 Phase 1, dPMR, M17, FreeDV 1600 |
| Aviation | ADS-B (1090ES), ACARS, VDL Mode 2, HFDL, Inmarsat Classic Aero, VOR, ILS localizer / glideslope |
| Marine | AIS, NAVTEX, Digital Selective Calling, Inmarsat STD-C / EGC |
| Amateur data and HF | APRS / AX.25, RTTY, PSK31, PSK63, Morse (CW), CW skimmer, FT8, FT4, WSPR |
| Paging and telemetry | POCSAG, FLEX, ERMES, Selcall (CCIR/ZVEI), Sub-GHz OOK/FSK frames, radio clocks (DCF77, WWVB, MSF, JJY) |
| Video | ATV, SSTV |
| Wideband digital | DAB / DAB+, DATV (DVB-S / S2), DRM30 / DRM+ |
| Utility | Signal identifier, GNSS lab (GPS L1 C/A), Iridium bursts |

Coverage varies by protocol. A listed mode means the signal path and the documented frame layers
are implemented; it does not mean every optional signalling service, trunking system or vendor
extension is supported. The [feature roadmap](https://github.com/Newspicel/sdrminusminus/blob/main/FEATURES.md)
records known gaps.

The wideband digital channels are **acquisition only**. They report waveform lock, SNR, frequency
error, and the configured symbol rate where one applies. They do not decode DAB FIC/MSC, DVB
transport streams, DRM FAC/SDC/MSC, programme audio, or DATV pictures. A missing service label
means the multiplex layer was not decoded, not that the station has no name.

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

The **DMR trunk system** node turns a control channel into the traffic channels it grants. Wire the
`events` output of one or more DMR decoders into it and choose the system type, or leave it on
auto-detect and let the signalling identify itself.

| System | How it is followed |
|---|---|
| Tier III (including Capacity Max) | Logical channels are learned from the system's own channel definitions; a receiver opens when a voice grant names one. |
| Capacity Plus | No frequency is granted, so every carrier you wire in is itself a traffic channel. Both timeslots of each are followed once the system is recognized. |
| Hytera XPT | As Capacity Plus, using XPT's own signalling. |

The receivers it opens belong to the server, not to the patch, so following continues while no
browser is connected. They obey the same passband rule as any other channel: a traffic channel
outside the radio's current sample rate cannot be opened, and the node says so rather than failing
quietly. Widen the sample rate, retune so the traffic channels fall inside it, or give the system a
second radio.

Completed calls are buffered in memory for the retention set on the node, audio included. Encrypted
transmissions keep only their metadata. Set retention to off to follow traffic without buffering
any audio.

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
