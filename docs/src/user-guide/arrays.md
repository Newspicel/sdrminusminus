# Coherent arrays

An array processes several antenna signals together. Shared clocks determine whether those
signals can support direction finding, beamforming, or passive radar.

| Tier | Shared hardware | Supported operations |
|---|---|---|
| `phase_coherent` | Reference clock and synthesizer | Bearings, beamforming, combining, passive radar |
| `time_sync` | Reference clock | Passive radar; phase-dependent operations need calibration |
| `none` | No shared reference | Independent reception |

A shared clock is required. Independent receivers drift apart even when tuned to the same frequency.

## Radios that are already an array

Add one **Device** for a multi-lane receiver such as KrakenSDR, CR-8, RSPduo in dual-tuner mode,
or a multi-channel SoapySDR radio. Its driver reports the coherence tier. Connect its `iq`, `iq2`,
and subsequent outputs directly to processing nodes.

### KrakenSDR

KrakenSDR has five lanes; KerberosSDR has four. Their shared clock provides `time_sync` coherence,
but tuner phases change after every retune.

Set **Cal source** to **Noise**. sdr-- switches the built-in noise source on when calibration is
needed, including after a retune or a press of **Calibrate**, then returns to the antennas.
During calibration, the display shows `noise source in` and suppresses bearings.

Use fixed gain and equal-length antenna cables. Calibration pauses while scanning or hunting.

## Radios you wired together yourself

Use an **Array** node for separate receivers physically connected to a shared clock.

1. Add a Device for each receiver.
2. Set matching sample rates and, for shared tuning, matching centre frequencies.
3. Connect each Device's `iq` output to an Array input. Inputs expand as members are added.
4. Set **Wired as** to match the hardware: shared clock, or shared clock and local oscillator.
5. Connect the Array outputs to your processor, channels, or recorders.

Input order sets antenna numbering. Use fixed gain on every receiver; calibration can correct
different fixed gains, but AGC changes invalidate it.

### Tuning and membership

Tune and change sample rate through Array to keep members aligned. Independently tuned arrays
provide a frequency control per lane. Disconnect the array before scanning or hunting.

Device nodes keep ownership of their radios and existing outputs. Removing Array leaves those
running. Removing a member removes the dependent array. A disconnected member faults the array;
processing reconnects when all members recover.

## Calibration

Press **Calibrate** on the coherent processor. It measures delay, amplitude, and phase corrections
for each lane.

| Cal source | Required signal |
|---|---|
| Signal | A strong signal received by every element |
| Noise | Noise injected into every lane, from the radio or an external splitter |

A `time_sync` array needs injected noise or a specified pilot frequency to resolve phase after
retuning. Built-in noise sources switch automatically. Inject an external reference before pressing
**Calibrate**. On `phase_coherent` hardware, calibration corrects cable and other path differences.

The display reports **solved**, **still solving**, or **phase unknown**. Phase unknown means the
reference is insufficient for bearings or beamforming.

## Combining antennas

Connect one coherent source to a **Combiner**, then connect its beam output to an ordinary channel.

| Mode | Effect |
|---|---|
| Combine | Align and sum signals; two antennas can improve SNR by about 3 dB under suitable conditions |
| Cancel | Use the other antennas as noise references for the first antenna |

For cancellation, place the wanted signal on the first antenna and receive the local noise on the
others. Both modes need known relative phase; `time_sync` arrays require a pilot or noise reference.
