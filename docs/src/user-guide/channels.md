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
| Amateur and text | APRS/AX.25, RTTY, Morse/CW |
| Paging and telemetry | POCSAG, generic sub-GHz OOK/PWM frames |
| Digital voice | DMR, D-STAR, System Fusion, NXDN, P25 Phase 1, dPMR, M17 |
| Video | Analog television luma |

Decoder coverage varies by protocol. A listed mode means the signal path and documented frame
layers are implemented; it does not imply every optional signalling service, trunking system, or
vendor extension is supported. Consult the repository's
[feature roadmap](https://github.com/Newspicel/sdrminusminus/blob/main/FEATURES.md) for known gaps.

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

## Decoder output

Decoder events are typed on the server and timestamped with source and frequency information.
Choose the view that matches the job:

- **Readout** follows changing state, such as RDS text or a table of tracked aircraft.
- **Decoder log** keeps independent messages and frames in SQLite for filtering and review.
- **Map** retains recent position tracks for supported decoders.
- **Export** downloads the stored rows wired into it.

Decoder log history is bounded so a busy unattended receiver cannot grow the database forever.
