# Passive radar

Passive radar compares a broadcast transmitter's direct signal with its echoes off aircraft. It
measures how much further each echo travelled and its Doppler shift. Two lanes on a shared clock
are enough. Phase calibration is only needed for echo bearings.

## Wire it

1. Build an [Array](arrays.md) with at least two lanes, tuned **Together**.
2. Add **Passive radar** and wire the Array's `array` to it.
3. Point one antenna at the transmitter. Under **Ref**, pick it as **Reference**. The others are
   **Surveillance**.
4. For the map, wire a GPS to the Array, and a GPS set to **Fixed** at the transmitter to `tx`.
5. Optional: wire an ADS-B channel's `events` to `adsb`. Tracks that match an aircraft show its
   call sign.

## Illuminators

Pick one under **Illum**.

| Illuminator | Notes |
|---|---|
| FM | Strongest echoes. **Clean** with CMA removes the programme's multipath. |
| DAB | Wider band, finer range. **Clean** with DAB remod rebuilds a clean reference. |
| DVB-T | Uses the slice of the channel the lanes cover, set by **Bandwidth** |
| Custom | Any signal, with your own **Bandwidth** |

**Offset** is the transmitter's frequency minus the Array's centre.

## Read the view

The plot shows bistatic range against Doppler. Bright cells are echoes; detections are marked.
An echo seen over several looks becomes a track in the table: range, speed, bearing, SNR and the
matched ADS-B aircraft. A single flash is often noise.

| Readout | Shows |
|---|---|
| Targets | Confirmed tracks |
| Clutter | Direct path and clutter removed |
| Load | Share of real time spent |
| Ref | Reference quality: `Raw`, `Lost`, or CMA or DAB with its dB |
| CPI | Integration time in use |
| Drops | Lost samples or looks, shown when not zero |

**Clear** forgets every track. The phone's [Radar mission](phones.md#missions) shows the same
view.

The range is **bistatic**: the extra distance the echo travelled compared with the direct path.
One echo gives an ellipse on the map, not a point. A tracked echo with a bearing, or an ADS-B
match, gets a position.

## Settings

| Setting | Does |
|---|---|
| Range | How far out and how fast (**Speed**) to search |
| CPI | Integration time. Longer finds weaker echoes but blurs fast ones. **Overlap** reuses part of each. |
| AoA | Bearing per echo. Needs a calibrated array. |
| GPU | **Auto** uses the GPU when it is faster |
| Clutter | How the direct path and ground clutter are removed: ECA-B, ECA-S, NLMS, Block NLMS or Off |
| Pfa | False alarm chance per cell |
| Detect | The CFAR detector (CA, OS or GO) and **Min SNR** |
| Track | How many looks start a track (**Start**), and how many misses end it (**Coast**) |

## Limits

- The lanes must see the transmitter and the sky. A strong direct path in the surveillance
  antennas limits range.
- `no tx`, `No transmitter` or `No array position`: wire the GPS nodes from step 4.
- `Overloaded` means the host cannot keep up: shorten **Range** or **CPI**, or set **GPU** to
  **Auto**.
- `Outside cal table` means the Array's measured table does not cover the carrier. Bearings then
  use the ideal geometry.
- A lane rate below the illuminator's bandwidth cuts range resolution.
