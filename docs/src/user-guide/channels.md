# Channels

A channel takes a Device's IQ and turns one frequency into audio, messages, or pictures. The
channel you pick decides the mode: AM, WFM, ADS-B, and so on. [Decoders](decoders.md) lists them
all.

## Add a channel

1. Press **+ Add** and pick a mode.
2. Wire Device `iq` to the channel's `iq`.
3. Set the channel frequency.
4. Wire the outputs you need:

| Output | Wire to | You get |
|---|---|---|
| `audio` | Speaker | Live sound |
| `audio` | Audio recorder | A WAV file |
| `audio` | Audio FX | Filtered, denoised or levelled sound |
| `events` | Readout | Current state: station text, aircraft table |
| `events` | Decoder log | Message history |
| `events` | Map | Positions |
| `events` | Export | CSV or JSON of logged rows |
| `events` | Event output | Events sent out, call audio saved |
| `video` | Video | ATV, DVB, SSTV, WEFAX and weather satellite pictures |
| `baseband` | Baseband scope, Baseband recorder, or Network IQ | The channel's filtered IQ |

To swap the mode, right-click the channel and choose **Replace with…**. The frequency and squelch
stay; wires the new mode has no port for are dropped. `m` and `M` cycle NFM, WFM, AM and SSB.

## Tuning

Use the channel dial, drag its marker on the Scope, or use the [keyboard](keyboard.md). Scroll or
press the arrow keys over a dial digit to step it. Typed frequencies are in MHz unless you add
`Hz`, `kHz`, or `GHz`. You can set frequencies before a radio is connected.

### Auto and manual

Every Device starts on **Auto**, shown by the lit radar button next to its dial. It places its
window over as many wired channels as its sample rate can hold, and keeps its own DC spike off
them. The count on the Device, such as `2/3`, shows how many it hears. To hear more, raise the
sample rate or move some channels to another radio.

Tuning the Device itself, from its dial, the keyboard, or the Scope, asks first and then switches
it to **Manual**. The radio then stays put, and channels outside its window wait until it covers
them again. Press the radar button to return to Auto. Use Manual to watch a fixed band, or when no
channel is wired.

The lock beside a dial freezes that frequency. Locking a channel does not lock its Device.

### Several radios

Wire a channel to more than one Device and it runs on whichever radio hears it. Radios on Auto
split their channels so as many as possible are heard. The channel face names the radio carrying
it. While a scanner, signal hunt, recording, or network export uses a channel, it stays on its
radio until that stops.

## Sample rate

Each channel runs at a rate set by its mode. The Device's IQ is resampled to match, so any Device
rate works as long as the channel's full bandwidth fits inside the Device's window.

When the Device rate equals the channel rate, resampling is skipped: ADS-B runs at 2.4 MS/s, DAB
and GNSS at 2.048 MS/s, ATV at 16 MS/s. Use the lowest rate that covers your signals. It saves
USB bandwidth and CPU.

## Squelch

Squelch mutes audio when nothing is there. Only channels with audio have it.

| Mode | Opens | Set with |
|---|---|---|
| Off | Always | Slider at far left |
| Manual | Above the slider level | The slider |
| Auto | **Margin** dB above the measured noise floor, 2 to 40 | The Auto button |

The level meter marks the threshold. Auto learns the floor while the channel is quiet, so a
signal that never stops can be mistaken for noise. Once squelch is open, the floor cannot rise
and cut off a long transmission. Turning Auto off keeps the threshold it reached. Analog channels
report calls on `events` only with squelch on.

NFM also has **Tone** squelch:

| Setting | Behaviour |
|---|---|
| Detect | Shows the CTCSS tone or DCS code, never mutes |
| CTCSS | Opens only for the chosen tone |
| DCS | Opens only for the chosen code |

**Scrambler** undoes voice inversion: set the **Carrier**, 1.5 to 4.5 kHz, or pick **Auto**.
**Compander** expands audio 2:1 for links that compress it. Leave it off for ordinary NFM.

## Noise blanker

The **Blanker** on audio channels removes impulse noise from the IQ before filtering. A lower
threshold removes more, but can damage the signal.

## Audio FX

Wire a channel's `audio` through an **Audio FX** node to process what you hear. The raw audio
stays available on the channel's other wires, so a recorder can keep it while a Speaker plays the
cleaned version. Chain several nodes to stack effects. Stages run in this order, all off by
default:

| Stage | Does |
|---|---|
| De-click | Removes short clicks |
| Passband | Cuts audio below and above two frequencies |
| Notches | Removes up to four chosen tones, each with its own width |
| Auto notch | Finds and removes steady tones |
| Denoise | **Spectral** cuts noise by up to 20 dB, light on CPU. **RNNoise** is a small built-in speech model. **DPDFNet** is the strongest speech model, downloaded on first use. |
| AGC | Levels the volume: Slow, Med or Fast. AM and SSB channels already level their own. |

RNNoise and DPDFNet are trained on speech. Leave them off for music, data tones and CW.

Pick a DPDFNet model, then press the download button next to it. Base and DPDFNet 2 suit small
machines; 4 and 8 sound better and cost more CPU. NB models suit 8 kHz narrowband voice such as
SSB and NFM, HR models full 48 kHz audio such as broadcast speech. The button next to a downloaded
model removes it.

## Filter events

Put an **Event filter** between a decoder and its outputs. Every rule you set must match.

| Mode | Passes |
|---|---|
| Keep | Only matching events |
| Drop | Everything except matching events |

A rule that does not apply to an event is ignored: a talkgroup rule never judges an aircraft. A
drop filter with no rules drops nothing. Chain filters to combine them, for example keep POCSAG,
then drop messages containing `TEST`. A filter only affects events that arrive after it is set.
