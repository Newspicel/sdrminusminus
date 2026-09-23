# Channels

A channel takes a Device's IQ and turns one frequency into audio, messages, or pictures. Which
channel you pick decides the mode: AM, WFM, ADS-B, and so on. The [Decoders](decoders.md) page
lists them all.

## Add a channel

1. Press **+ Node** and pick a mode.
2. Wire Device `iq` to the channel's `iq`.
3. Set the channel frequency.
4. Wire the outputs you need:

| Output | Wire to | You get |
|---|---|---|
| `audio` | Speaker | Live sound |
| `audio` | Audio recorder | A WAV file |
| `events` | Readout | Current state: station text, aircraft table |
| `events` | Decoder log | Message history |
| `events` | Map | Positions |
| `events` | Export | CSV or JSON of logged rows |
| `video` | Video | ATV frames or SSTV pictures |
| `baseband` | Baseband scope, recorder, or Network IQ | The channel's filtered IQ |

To swap the mode, right-click the channel and choose **Replace with…**. The frequency and squelch
stay; wires the new mode has no port for are dropped. `m` and `M` cycle the analog modes.

## Which radio hears a channel

A Device on auto tuning moves its window to cover its channels. A Device tuned by hand only
carries the channels inside its window. The rest stay configured and resume when the radio
covers them again.

Wire a channel to more than one Device and it runs on whichever radio hears it. Radios on auto
tuning split their channels between them so as many as possible are heard. A channel wired to
only one radio stays on that radio. The channel face names the radio carrying it.

While a scanner, signal hunt, recording, or network export uses a channel, it stays on its radio
until that stops.

## Tune

Use the channel dial, drag its marker on the Scope, or use the [keyboard](keyboard.md). Typed
frequencies are in MHz unless you add `kHz`, `MHz`, or `GHz`. The step buttons move by 5 or
25 kHz.

The lock beside a dial freezes that frequency. Locking a channel does not lock its Device.

You can set channel frequencies before a radio is connected. The Device opens over them.

## Sample rate

Each channel runs at its own fixed rate. The Device's IQ is resampled to match, so any Device
rate works for any channel, as long as the channel's full bandwidth fits inside the Device's
window. If it does not fit, retune the Device, move the channel, or raise the sample rate.

When the Device rate equals the channel rate, resampling is skipped: ADS-B runs at 2.4 MS/s, DAB
and GNSS at 2.048 MS/s, ATV at 16 MS/s. Use the lowest rate that covers your signals. It saves
USB bandwidth and CPU.

## Squelch

Squelch mutes audio when nothing is there. Only channels with audio have it. Data decoders use
their own detection threshold.

| Mode | Opens |
|---|---|
| Off | Always |
| Manual | Above a fixed level |
| Auto | A set number of dB above the measured noise floor |

The level meter marks the threshold. Auto learns the floor while the channel is quiet, so a
signal that never stops can be mistaken for noise. Once squelch is open, the floor cannot rise
and cut off a long transmission. Switching back to Manual restores your last manual level.

NFM also has tone squelch:

| Setting | Behaviour |
|---|---|
| Detect | Shows the CTCSS tone or DCS code, never mutes |
| CTCSS | Opens only for the chosen tone |
| DCS | Opens only for the chosen code |

**Compander** expands audio 2:1 for links that compress it. Leave it off for ordinary NFM.

## Audio processing

The **Audio** block runs these stages in order. All are off by default except AGC on AM and SSB.

| Stage | Does |
|---|---|
| Blanker | Removes impulse noise from the IQ before filtering. A lower threshold removes more, but can damage the signal. |
| De-click | Removes short clicks from the audio. |
| Passband | Cuts audio below and above two frequencies. |
| Notches | Removes up to four chosen tones, each with its own width. |
| Auto notch | Finds and removes steady tones. |
| Denoise | Attenuates noise between 0 and 20 dB. Steady carriers can be treated as noise. |
| AGC | Levels the volume. Slow suits SSB speech, fast suits tuning around. |

## Where decoded events go

Every event carries its source, frequency, and time. Use **Readout** for what is happening now,
**Decoder log** for history, **Map** for positions, and **Export** to save rows. The log keeps a
bounded history.

### Filter events

Put an **Event filter** between a decoder and its outputs. Every rule you set must match.

| Mode | Passes |
|---|---|
| Keep | Only matching events |
| Drop | Everything except matching events |

A rule that does not apply to an event is ignored: a talkgroup rule never judges an aircraft. A
drop filter with no rules drops nothing. Chain filters to combine them, for example keep POCSAG,
then drop messages containing `TEST`.

A filter only affects events that arrive after it is set. Rows already in the log stay.
