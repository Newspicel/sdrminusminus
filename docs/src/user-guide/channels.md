# Channels and decoding

A channel selects one signal inside a device's sampled passband. Its offset is relative to the
device center frequency, so retuning the device moves every attached channel together while
changing a channel offset moves only that channel.

## Connect a channel

Add a channel from **+ Node**, then wire the Device's `IQ` output into the channel's `IQ` input.
Connect each output you want to use:

| Channel output | Connect to | Result |
|---|---|---|
| `audio` | Speaker | Browser audio |
| `events` | Readout | Accumulated state such as a station or aircraft table |
| `events` | Decoder log | Stored, filterable message history |
| `events` | Map | Positions from ADS-B, AIS, or APRS |
| `events` | Export | CSV or JSON download of stored rows |
| `video` | Video | Completed ATV frames |

A channel face warns when its audio or video output goes nowhere. The NFM event output is an
exception because it is commonly unused unless CTCSS or DCS detection is enabled.

## Available channel types

The server reports the exact catalog for the running build. Current channel families include:

| Family | Modes |
|---|---|
| Analog audio | AM, NFM, SSB, broadcast WFM with stereo and RDS |
| Aviation and marine | ADS-B 1090ES, ACARS, AIS, NAVTEX |
| Amateur and text | APRS/AX.25, RTTY, Morse/CW, CCIR-1 and ZVEI-1 Selcall |
| Paging and telemetry | POCSAG, generic sub-GHz OOK/PWM frames |
| Digital voice | DMR, D-STAR, System Fusion, NXDN, P25 Phase 1, dPMR, M17, FreeDV 1600 |
| Video | Analog television luma |
| Digital broadcast acquisition | DAB/DAB+ Mode I, DVB-S/S2 DATV at 100 kBd–1 MBd, DRM30/DRM+ |

Decoder coverage varies by protocol. A listed mode means the signal path and documented frame
layers are implemented; it does not imply every optional signalling service, trunking system, or
vendor extension is supported. Consult the repository's
[feature roadmap](https://github.com/Newspicel/sdrminusminus/blob/main/FEATURES.md) for known gaps.

The digital-broadcast channels currently report RF acquisition only: waveform lock, SNR,
frequency error, and configured symbol rate where applicable. They do not yet decode DAB FIC/MSC,
DVB transport streams, DRM FAC/SDC/MSC, programme audio, or DATV pictures. A missing service label
therefore means the multiplex layer has not been decoded; it is not an empty station name.

## Passband and sample-rate rules

The channel's occupied band must fit inside its source device's current passband. If it does not,
move the channel closer to center, increase the device sample rate, or retune the device.

Most channels are resampled to their preferred input rate. ADS-B is different: its half-microsecond
pulses are decoded from the device's own samples and require a device rate between 2 and 4 MS/s.
The channel face offers a compatible rate when the current one cannot work.

Use the lowest rate that covers the signals you need. Higher rates increase USB traffic, FFT work,
and CPU load without improving a narrow channel.

## Tuning and squelch

You can tune a channel by editing its offset, dragging its marker on a connected Scope, or using
keyboard controls while the channel is selected. The displayed absolute frequency is the source
stream's center plus the channel offset.

Audio channels can gate output with squelch. A lower threshold opens more easily; setting squelch
off passes audio continuously. NFM additionally supports:

- **Detect**, which reports any recognized CTCSS tone or DCS code without gating audio;
- **CTCSS**, which opens only for a selected standard tone;
- **DCS**, which opens only for a selected standard code.

## Following a DMR trunk system

The **DMR trunk system** node turns a control channel into the traffic channels it grants. Wire
the events output of one or more DMR decoders into it and choose the system type, or leave it on
auto-detect and let the signalling identify itself.

- **Tier III / Capacity Max** learns where each logical channel is from the system's own channel
  definitions and opens a receiver when a voice grant names one.
- **Capacity Plus** grants no frequency: every carrier you wire in is itself a traffic channel, so
  both timeslots of each are followed as soon as the system is recognized.

The receivers it opens are the server's, not the patch's, so following continues while no browser
is connected. They obey the same passband rule as any other channel: a traffic channel outside the
radio's current sample rate cannot be opened, and the node says so instead of failing quietly.
Widen the sample rate, retune so the traffic channels fall inside it, or give the system a second
radio.

Completed calls are buffered in memory for the retention you choose on the node — audio included,
unless the transmission was encrypted, in which case only its metadata is kept. Set retention to
off to follow traffic without buffering any audio.

## Decoder output

Decoder events are typed on the server and timestamped with source and frequency information.
Choose the view that matches the job:

- **Readout** follows changing state, such as RDS text or a table of tracked aircraft.
- **Decoder log** keeps independent messages and frames in SQLite for filtering and review.
- **Map** retains recent position tracks for supported decoders.
- **Export** downloads the stored rows wired into it.

Decoder log history is bounded so a busy unattended receiver cannot grow the database forever.
