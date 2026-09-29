# Passive radar

Passive radar compares a broadcast transmitter's direct signal with its echoes off aircraft. It
measures how much further each echo travelled and its Doppler shift. Two lanes on a shared clock
are enough; phase calibration is only needed for echo bearings.

## Wire it

1. Build an [Array](arrays.md) with at least two lanes, tuned **Together**.
2. Add **Passive radar** and wire the Array's `array` to it.
3. Point one antenna at the transmitter and pick it as **Reference**. The others are
   **Surveillance**.
4. For the map, wire a GPS to the Array, and a GPS set to **Fixed** at the transmitter to `tx`.
5. Optional: wire an ADS-B channel's `events` to `adsb`. Tracks that match an aircraft show its
   call sign.

## Illuminators

| Illuminator | Notes |
|---|---|
| FM | Strongest echoes. **Cleaning** with CMA removes the programme's multipath. |
| DAB | Wider band, finer range. **DAB remod** rebuilds a clean reference. |
| DVB-T | Uses the slice of the channel the lanes cover |
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
| Ref | Reference quality |
| Drops | Lost samples or looks, shown when not zero |

**Clear** forgets every track.

The range is **bistatic**: the extra distance the echo travelled compared with the direct path.
One echo gives an ellipse on the map, not a point. A tracked echo with a bearing, or an ADS-B
match, gets a position.

## Settings

| Setting | Does |
|---|---|
| Range, Speed | How far out and how fast to search |
| CPI | Integration time. Longer finds weaker echoes but blurs fast ones. |
| AoA | Bearing per echo. Needs a calibrated array. |
| GPU | Uses the GPU when it is faster |
| Clutter | How the direct path and ground clutter are removed |
| Detect | The CFAR detector, its false alarm rate **Pfa** and minimum SNR |
| Track | How many looks start a track, and how many misses end it |

## Limits

- The lanes must see the transmitter and the sky. A strong direct path in the surveillance
  antennas limits range.
- **Overloaded** means the host cannot keep up: shorten **Range** or **CPI**, or turn on **GPU**.
- A lane rate below the illuminator's bandwidth cuts range resolution.
- The phone's Radar mission shows the same view and tracks.
