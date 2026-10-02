# Arrays

An **Array** node turns radio lanes into one antenna array. Every array processor reads its `array`
output: [Direction finder](direction-finding.md), Beamformer, Stitch, Spatial spectrum,
Correlator, Polarimeter and [Passive radar](passive-radar.md).

## Make an array

On a multi-lane radio such as a KrakenSDR, press **Make array** on the Device. It adds an Array
and wires every lane to it. The button is greyed out when the lanes share no clock. A playing
array recording has the same button.

For separate radios on one clock:

1. Add a Device for each radio.
2. Wire each Device `iq` to the Array in antenna order: `lane1`, `lane2`, and so on.
3. Open **Tier** and pick what the radios share: **Shared clock**, or **Shared LO** for clock and
   local oscillator.

A lane belongs to one Array. The Device then shows **Array** on that lane, and tuning and gain
move to the Array.

## Tiers

| Tier | Lanes share |
|---|---|
| Shared LO | Clock and local oscillator |
| Shared clock | Sample clock |
| None | Nothing, not an array |

On one Shared LO radio the lanes start lined up in time. Otherwise the Array syncs them first.
It also measures clock drift between radios. When the tier ends up below what you declared,
**Tier** reads `capped`.

| Radio | Tier | Noise source |
|---|---|---|
| KrakenSDR | Shared clock | Built in, antennas cut off |
| KerberosSDR | Shared clock | Built in, antennas stay on |
| RSPduo dual tuner | Shared clock | None |
| LimeSDR, USRP B210, bladeRF 2, AD936x boards | Shared LO | None |
| CR-8 | Shared LO | None |
| Other multi-channel SoapySDR radios | Shared clock | None |
| Several radios in one Array | What you declare, capped by any radio that gives more than one lane | None |
| [Array recording](recording.md#play-an-array-recording) | As recorded | Recorded noise windows |

## Lane tuning

Pick it under **Tuning**.

| Mode | Lanes | For |
|---|---|---|
| Together | All on one frequency | Every processor except Stitch |
| Spread | Side by side, **Span** shows the band | Stitch |

## Lay out the antennas

Open **Geometry**. Angles run clockwise from the array's forward direction.

| Shape | Set |
|---|---|
| UCA | A circle: **Radius**, **First** (where lane 1 sits, 0 is forward), **Winding** |
| ULA | A line: **Spacing**, **Axis** (from lane 1 to the last lane) |
| Custom | Each lane's x, y and z in metres |

**Max f** is the highest frequency before the spacing aliases.

## Point it

Open **Forward** and pick a **Mode**.

- **Fixed:** type the **Azimuth** of the array's forward direction, from true north.
- **Heading:** wire a GPS with heading, such as a [phone](phones.md), to the Array's `position`.
  **Mount** is the array's forward direction relative to that heading.

Wire a GPS to `position` in either mode. Direction finders need it to send bearings.

## Calibrate

Pick the **Cal source**:

| Source | Uses |
|---|---|
| Noise | The radio's built-in noise source |
| Pilot | A carrier every antenna hears, at a known **Offset** and **Width** |
| Emitter | A transmitter at a known true **Bearing**, **Offset** and **Width**. Needs **Forward** set. |
| Off | Nothing |

The Array calibrates delay, phase and gain on start and after every retune or gain change.
**Calibrate** runs it now. Processors that need phase wait until it is done.

Open **Calibration** for the rest. **Check** rechecks the phase on a timer. **EQ** flattens each
lane across the band. **Warm start** begins from the last calibration stored for this radio and
band.

The **Gain** row sets one gain for every lane. **Auto** steps it from the lane levels and needs a
cal source.

## Read the face

| Row | Shows |
|---|---|
| Sync | `Idle`, `Syncing`, `Locked`, `Drifting` or `Lost` |
| Tier | What the lanes share |
| Cal | `No cal`, `Waiting`, `Calibrating`, `Calibrated`, `Warm`, `Stale` or `Cal failed` |
| Heading | Array forward, true north |
| Gaps, Realigns, Drops, Lost | Lost or resynced samples, shown when not zero |

`Calibrated`, `Warm` and `Stale` show their age. Each lane row shows its quality, gain and phase.
When the array stops, the face says why, such as `Noise clips, lower gain`.

A processor that cannot run says why on its own face: `Syncing`, `Calibrating`, `Needs cal`,
`Retuning`, `Not coherent` or `Wrong tuning`.

**Rec** [records](recording.md#record-an-array) every lane into one SigMF collection.
[Play it back](recording.md#play-an-array-recording) into an Array like a radio.

## Beamformer

Combines the lanes into one `beam` lane. Channels, scopes and recorders use it like a radio lane.

| Mode | Does |
|---|---|
| MRC | Best SNR from every lane |
| Beam | Steers with equal weights |
| MVDR | Steers and suppresses the rest |
| Nulls | Steers with nulls on the azimuths you list, or that **Auto** finds |
| GSC | Steers and cancels the rest as it moves |
| Cancel | Subtracts what the **Refs** lanes hear from the **Main** lane |
| CMA | Locks onto a signal with a steady envelope |

**Steer** picks **DF**, a Direction finder wired to `steer`, or a **Fixed** azimuth from array
forward. The plot shows the beam pattern. **Gain** is the gain over one lane.

## Stitch

Joins spread lanes into one `wide` lane at the lane rate times the lane count. It needs the Array
on **Spread**: **Spread array** on its face sets it. **Blend** handles overlaps: **SNR** favours
the cleaner lane, **Equal** averages.

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
