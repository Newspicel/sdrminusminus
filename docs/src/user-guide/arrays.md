# Arrays

An **Array** node turns radio lanes into one antenna array. Every array processor reads its `array`
output: [Direction finder](direction-finding.md), Beamformer, Stitch, Spatial spectrum,
Correlator, Polarimeter and [Passive radar](passive-radar.md).

## Make an array

On a multi-lane radio such as a KrakenSDR, press **Make array** on the Device. It adds an Array
and wires every lane to it. The button is greyed out when the lanes share no clock.

For separate radios on one clock:

1. Add a Device for each radio.
2. Wire each Device `iq` to the Array in antenna order: `lane1`, `lane2`, and so on.
3. Open **Tier** and pick what the radios share: **Shared clock**, or **Shared LO** for clock and
   local oscillator.

A lane belongs to one Array. The Device then shows **Array** on that lane, and tuning and gain
move to the Array.

## Tiers

| Tier | Shared | Needs |
|---|---|---|
| Shared LO | Clock and local oscillator | Calibration once, or a warm start |
| Shared clock | Clock only | Calibration after every retune and gain change |
| None | Nothing | Not an array |

The Array measures clock drift between radios. A declared tier that does not hold is capped and
**Tier** reads `capped`. [Radios](../hardware.md#coherence-tiers) lists the tier of each radio.

## Lay out the antennas

Open **Geometry**. Angles run clockwise from the array's forward direction.

| Shape | Set |
|---|---|
| UCA | A circle: **Radius**, **First** (where lane 1 sits, 0 is forward), **Winding** |
| ULA | A line: **Spacing**, **Axis** (from lane 1 to the last lane) |
| Custom | Each lane's position in metres |

**Max f** is the highest frequency before the spacing aliases.

## Point it

Open **Orientation**.

- **Fixed:** type the **Azimuth** of the array's forward direction, from true north.
- **Heading:** wire a GPS with heading, such as a [phone](phones.md), to the Array's `position`.
  **Mount** is the array's forward direction relative to that heading.

Wire a GPS to `position` in either mode. Direction finders need it to send true bearings.

## Calibrate

Press **Calibrate**. The Array lines up delay, phase and gain on every lane. Processors that need
phase wait until it has. Open **Calibration** to pick the **Source**:

| Source | Uses |
|---|---|
| Noise | The radio's built-in noise source |
| Pilot | A carrier every antenna hears, at a known **Offset** and **Width** |
| Emitter | A transmitter at a known **Bearing**, **Offset** and **Width** |
| Off | Nothing |

**Check** rechecks the phase on a timer. **EQ** flattens each lane across the band.
**Warm start** begins from the last calibration stored for this radio and band.

Open **Gain** to set one gain for every lane. **Auto** runs the radios' AGC instead. Calibrate at
the gain you use.

## Read the face

| Row | Shows |
|---|---|
| Sync | `Syncing`, `Locked`, `Drifting` or `Lost` |
| Tier | What the lanes share |
| Cal | `Calibrated` and its age, `Calibrating`, `Warm`, `Stale` or `Cal failed` |
| Heading | Array forward, true north |
| Gaps, Realigns, Drops, Lost | Lost or resynced samples, shown when not zero |
| Fault | Why the array stopped, such as `Noise clips, lower gain` |

A processor that cannot run says why on its own face: `Syncing`, `Calibrating`, `Needs cal`,
`Not coherent` or `Wrong tuning`.

**Rec** [records](recording.md#record-an-array) every lane into one SigMF collection.

## Lane tuning

| Mode | Lanes | For |
|---|---|---|
| Together | All on one frequency | Every processor except Stitch |
| Spread | Side by side, **Span** shows the band | Stitch |

## Beamformer

Combines the lanes into one `beam` lane. Channels, scopes and recorders use it like a radio lane.

| Mode | Does |
|---|---|
| MRC | Best SNR from every lane |
| Beam | Steers with equal weights |
| MVDR | Steers and suppresses the rest |
| Nulls | Steers with nulls on the azimuths you list |
| GSC | Steers and cancels the rest as it moves |
| Cancel | Subtracts what the **Refs** lanes hear from the **Main** lane |
| CMA | Locks onto a signal with a steady envelope |

**Steer** picks **DF**, a Direction finder wired to `steer`, or a **Fixed** azimuth. The plot
shows the beam pattern. **Gain** is the gain over one lane.

## Stitch

Joins spread lanes into one `wide` lane at the lane rate times the lane count. Set the Array to
**Spread** first. **Blend** handles overlaps: **SNR** favours the cleaner lane, **Equal**
averages. Use equal, fixed gain on every lane.

## Spatial spectrum

Shows bearing over frequency. **Map** draws bearing against frequency. **Trail** draws frequency
over time, coloured by bearing. Pick **Rel** or **True** bearings.

## Correlator

Correlates every pair of lanes. For the chosen **Baseline** it plots amplitude and phase over
frequency and reads the **Delay** from the phase slope and the coherence **Coh**.

## Polarimeter

Needs two crossed antennas: pick the **H** and **V** lanes. It reads the Stokes values, the
polarised share **Pol**, **Tilt**, **Ellip** and **Hand**. **Output** sets what `beam` carries:
**Matched** to the wave or **Cross** to it.
