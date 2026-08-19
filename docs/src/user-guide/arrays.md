# Coherent arrays

Several antennas are worth more than one only if their samples belong to the same moment. What
they share decides what you can measure, so sdr-- makes that a property of the radio rather than
an assumption:

| Tier | What the hardware shares | What it buys |
|---|---|---|
| `phase_coherent` | Reference clock and synthesizer | Bearings, beamforming, and everything below |
| `time_sync` | Reference clock only | Delay between elements: passive radar, and bearings only with a phase reference |
| `none` | Nothing | Nothing coherent; a bank of separate receivers |

A time-synced bank has a real, constant delay between its elements, but each retune leaves the
phase between its tuners at a new random value. That is why bearings on such a bank need something
known to solve against — an injected noise source, or a continuous pilot carrier named in the
calibration settings. Without one the direction finder reports `phase unknown` and emits no
bearings at all, rather than plausible ones that are wrong.

Radios that do not share a reference clock drift apart continuously. They can be a bank of
receivers; they can never be an array.

## Radios that are already an array

A receiver whose lanes come out of one device — a Dragon Labs CR-8, an RSPduo in dual-tuner mode,
a multi-channel SoapySDR device — needs nothing special. Add one **Device** node and wire its
`iq`, `iq2`, `iq3`… outputs wherever they are going. The driver declares its own tier.

## Radios you wired together yourself

Two or more separate receivers on a shared clock become one array with an **Array** node.

1. Add a **Device** node per radio and pick its receiver, as usual.
2. Add an **Array** node and wire each radio's `iq` into one of its inputs. The array always draws
   one input more than it has radios, so there is somewhere to put the next one.
3. Set **Wired as** to what you actually cabled: shared clock, or shared clock and LO.

The order of the inputs is the element numbering. If the antennas were cabled in a different order
than you expected, move the wires rather than editing a list anywhere.

The array hands every element back out as a lane on its own `iq` outputs. That is what a direction
finder, a combiner, a channel or a recorder wires to.

### What belongs to the array

A radio an array has taken is opened and tuned by the array. Its own node says which array holds
it instead of offering a tuner that would fight the array's, and unwiring it gives it back.

What the array owns is the **centre frequency and sample rate** — they have to be shared or it is
not an array. Everything inside that tuned span stays free: wire a channel, a recorder or a scope
to any of the array's lanes and it works while the array runs.

Two rules follow from the calibration rather than the plumbing:

- Do not run AGC on an element. A gain that moves invalidates the calibration continuously.
- Fixed gains that differ between elements are fine. Calibration measures each lane's amplitude
  and phase and corrects them.

## Calibration

Every coherent node has a **Calibrate** button and a calibration readout. Calibration measures a
delay and a complex weight per lane and corrects them before any processor sees the samples.

| Cal source | Use it when |
|---|---|
| Signal | Any strong signal every element can hear, which is the ordinary case on the air |
| Noise | A noise burst injected through a splitter, for a bench calibration with nothing on |

Phase is only solved against something known: an injected noise source, or the pilot frequency you
name. On a `phase_coherent` radio phase is meaningful without either, and calibration only trims
what the cabling added.

The readout says which of three states the array is in: solved, still solving, or phase unknown.
The last means the tier and the calibration source together cannot justify a bearing.

## Combining antennas

A **Combiner** node takes the lanes of one coherent radio and writes their sum onto the beam lane,
where an ordinary channel decodes it. It has two modes:

| Mode | What it does |
|---|---|
| Combine | Turns every antenna into step and adds them. Two antennas are worth about 3 dB. |
| Cancel | Keeps the first antenna and subtracts what the others hear. |

Cancel is how a reference antenna takes a local noise source out of a receiver you cannot move
away from it: point the first antenna at what you want and the others at the noise. Both modes
solve their weights from the covariance between the antennas, so nothing is tuned by hand.

Both need phase, so a time-synced bank needs its pilot or noise reference here too.
