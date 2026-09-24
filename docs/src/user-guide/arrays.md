# Coherent arrays

An array receives several antennas at once, from receivers that share a clock. How much they
share decides what they can do:

| Tier | Shared | Can do |
|---|---|---|
| `phase_coherent` | Clock and local oscillator | Bearings, beamforming, combining, passive radar |
| `time_sync` | Clock only | Passive radar. The rest needs calibration after every retune. |
| `none` | Nothing | Independent reception only |

Receivers without a shared clock drift apart, even on the same frequency.

## Multi-lane radios

KrakenSDR, CR-8, an RSPduo in dual-tuner mode, and multi-channel SoapySDR radios are one
**Device** with several outputs: `iq1`, `iq2`, and so on. The driver reports the tier. Wire the
outputs straight to the processing node.

### KrakenSDR

KrakenSDR has five lanes, KerberosSDR four. They are `time_sync`: the tuner phases change on
every retune.

Set **Cal source** to **Noise**. SDR-- then switches on the built-in noise source whenever it
needs to calibrate, after a retune or when you press **Calibrate**, and switches back to the
antennas. Bearings are hidden while it shows `noise source in`.

Use fixed gain and equal-length cables. AGC stays allowed, but its **Auto** turns amber on lanes a
coherent node or Array uses. Calibration pauses during scans and hunts.

In Auto, the lanes a coherent node uses move as one, to the frequency that suits all their
decoders. A new sample rate restarts the coherent nodes.

## Build your own array

For separate receivers wired to one clock, use an **Array** node.

1. Add a Device for each receiver.
2. Give them the same sample rate, and the same frequency if they share tuning.
3. Wire each Device `iq` to the Array. It grows an input per member.
4. Set **Wired as** to match: shared clock, or shared clock and local oscillator.
5. Wire the Array's outputs to the processor, channels, or recorders.

Input order sets antenna numbering. Use fixed gain: AGC breaks calibration.

Tuning a member, changing its rate, or switching it to Auto moves the whole Array, so the members
stay aligned. A scan or hunt on a member moves the whole Array too. The Devices keep their radios, and
removing the Array leaves them running. If a member drops out, the array pauses until it is back.

## Calibrate

Press **Calibrate** on the processing node. It measures the delay, gain, and phase of each lane.

| Cal source | Needs |
|---|---|
| Signal | A strong signal every antenna receives |
| Noise | Noise fed into every lane, built in or through an external splitter |

A `time_sync` array needs noise or a known pilot to recover phase after each retune. Built-in
noise switches itself. Feed external noise before pressing **Calibrate**. On `phase_coherent`
hardware, calibration corrects cable and path differences.

The node shows **solved**, **still solving**, or **phase unknown**. Phase unknown means there is
not enough reference for bearings or beamforming.

## Combine antennas

Wire a coherent source to a **Combiner** and its `beam` output to an ordinary channel.

| Mode | Does |
|---|---|
| Diversity | Aligns and adds the antennas. Two antennas gain about 3 dB SNR. |
| Cancel | Uses the other antennas to subtract local noise from the first |

For Cancel, point the first antenna at the wanted signal and the others at the noise. Both modes
need the phase: `time_sync` arrays need a pilot or noise reference.

The `beam` output also feeds a Scope and any number of channels. It stays silent until the phase
is solved.

## Stitch lanes into one wide stream

Wire every lane of a radio that tunes each lane on its own into a **Stitch**. Its `wide` output
runs at the lane rate times the lane count, as one more radio lane.

| Mode | Does |
|---|---|
| Auto | Tunes the lanes side by side with a small overlap. Tuning the wide lane moves them all. |
| Manual | Keeps each lane where you tune it. Gaps between lanes stay empty. |

A KrakenSDR at 2.048 MS/s gives about 8.7 MHz in Auto. Overlaps match each lane's gain and phase to
its neighbour while a signal sits in them. A Stitch needs the radio's lanes to itself, so no
Combiner or direction finder can share them. Use fixed, equal gain on every lane.
