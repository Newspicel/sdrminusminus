# Coherent arrays

An array processes samples from several antennas together. Direction finding and beamforming
need stable phase relationships; passive radar needs aligned sample timing. The hardware's
shared clocks determine which operations are available.

| Tier | Shared hardware | Supported measurements |
|---|---|---|
| `phase_coherent` | Reference clock and synthesizer | Bearings, beamforming, combining, and passive radar |
| `time_sync` | Reference clock | Passive radar; phase-dependent operations also need a calibration reference |
| `none` | No shared reference | Independent reception only |

Retuning a `time_sync` array changes the relative tuner phases. Use an injected noise source or
a known pilot carrier to calibrate them. Without that reference, the direction finder reports
`phase unknown` and produces no bearings. Receivers without a shared clock drift apart and cannot
form a coherent array.

## Radios that are already an array

For a multi-lane receiver such as a Dragon Labs CR-8, an RSPduo in dual-tuner mode, or a
multi-channel SoapySDR device, add one **Device** node. Connect its `iq`, `iq2`, `iq3`… outputs
to the processing nodes. The driver reports the receiver's coherence tier.

## Radios you wired together yourself

Use an **Array** node for separate receivers connected to a shared clock.

1. Add a **Device** node for each radio and select its receiver.
2. Set the radios to the same sample rate. For shared tuning, their centre frequencies must match.
3. Add an **Array** node and connect each Device's `iq` output to an array input. A spare input
   appears as you add members.
4. Set **Wired as** to match the physical connections: shared clock, or shared clock and LO.

Input order determines antenna element numbering. Move the wires if that order is wrong.
Connect the Array's output lanes to a direction finder, combiner, channel, or recorder.

### Tuning and membership

Once connected, change frequency and sample rate through the Array node to keep its members
consistent. Independently tuned arrays have a frequency control for each lane. Disconnect the
array before scanning or hunting.

Each Device node retains its radio and existing channels, scopes, and recordings. Removing the
Array leaves those running. Removing a member removes the dependent array. If a member disconnects,
the array reports a fault and reconnects its streams when all members recover.

Use fixed gain on each element. AGC changes the amplitude relationship and invalidates calibration.
Fixed gains may differ: calibration measures and corrects each lane's amplitude and phase.

## Calibration

Press **Calibrate** on the coherent node. Calibration measures a delay and complex weight for
each lane, then applies those corrections before processing.

| Cal source | When to use it |
|---|---|
| Signal | A strong signal received by every element |
| Noise | A noise burst injected through a splitter for bench calibration |

A `time_sync` array needs injected noise or a specified pilot frequency to solve relative phase.
On `phase_coherent` hardware, calibration corrects the additional differences from cabling.

The readout shows **solved**, **still solving**, or **phase unknown**. Phase unknown means the
hardware tier and calibration source do not provide enough information for a bearing.

## Combining antennas

A **Combiner** sums the lanes of one coherent source and sends the result to a beam lane. Connect
an ordinary channel to that lane to decode the combined signal.

| Mode | Effect |
|---|---|
| Combine | Aligns and sums antenna signals. Two antennas can improve SNR by about 3 dB under suitable conditions. |
| Cancel | Uses the other antennas as noise references and subtracts their contribution from the first. |

For cancellation, use the first antenna for the wanted signal and the others to receive the local
noise source. Both modes calculate weights from the covariance between lanes and require known
relative phase. A `time_sync` array therefore needs a pilot or noise reference.
