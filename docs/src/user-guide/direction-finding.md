# Direction finding

A **Direction finder** turns an [Array](arrays.md) into bearings. **Triangulation** crosses
bearings from several places, or from one moving array, into a position.

## Set up

1. Build and [calibrate](arrays.md#calibrate) an Array, and wire a GPS to its `position`.
2. Add **Direction finder** and wire the Array's `array` to it.
3. Set **Offset** and **Width** to cover the signal.
4. Pick a **Method**.

| Method | Use |
|---|---|
| Bartlett | Plain beam scan, robust |
| Capon | Sharper beam scan |
| MUSIC | Splits close sources |
| Root-MUSIC, ESPRIT | Grid free, need a line or a circle |

**Sources** counts transmitters with **Auto**, or takes a fixed number. **Squelch** drops weak
peaks and **Station** names the bearings this finder sends. **More** holds the rest, such as
**Smooth** against reflections and **Yaw gate**.

## The rose

The rose shows the response around the array, each bearing, and its spread. **Rel** is relative to
the array's forward direction. **True** is from true north and needs a heading or a fixed azimuth
on the Array.

| Row | Shows |
|---|---|
| Bearing, ± | The strongest bearing and its one sigma |
| Fit | Share of the signal the bearings explain |
| SNR | Signal to noise |
| Src | Sources found |

| Chip | Means |
|---|---|
| Squelch | Peak below **Squelch** |
| No heading | Wire a GPS with heading to the Array |
| No position | Wire a GPS to the Array |
| Aliased | Antennas more than half a wavelength apart |
| Mode alias | Too high for phase modes on this circle |
| Mirror | A line cannot tell front from back. **Side** picks one. |
| Rotating | Turning faster than **Yaw gate**, blocks skipped |
| Singular | Covariance could not be inverted |
| Table off | Outside the calibration table |
| Stale | No reading for three report periods |

## Listen in one direction

Wire the finder's `events` to a [Beamformer](arrays.md#beamformer) `steer` input, and `beam` to
a channel.

## Triangulate

1. Add **Triangulation** and wire each finder's `events` to it.
2. Give each finder's Array its own position. A moving array sends bearings from wherever it was.

A [Signal hunt](scanning.md#hunt-a-transmitter) with a phone sends bearings too.

| Row | Shows |
|---|---|
| Estimate | The most likely position |
| Spread | The one sigma error ellipse |
| Guidance | Where to drive next: `Drive across` or `Drive at it` |
| Bearings | Bearings in use |

The table lists each station's last bearing, its spread and age. **Fade** sets how fast old
bearings lose weight: **Auto** picks **Fixed** or **Moving** from the stations, **Set** takes your
half life. **Min conf** refuses weak bearings and **Emitters** caps how many transmitters it
finds. **Clear** throws away every bearing.

Wire a vehicle's GPS to Triangulation `position` for guidance. **Guide** picks **Auto**, which
crosses the bearings first and then drives at the fix, **Direct** or **Off**. **Probe** sets how
far to drive across a single bearing.

## On the map

Wire finder and Triangulation `events` to a **Map**. It draws bearing rays, a heat layer of
likely positions, the estimate and its ellipse. The first fix is an event that an
**Event output** can forward.

## In a car

Mount the array on the car, pair a [phone](phones.md) and wire its GPS to the Array with
**Heading**. The phone's DF drive mission shows the bearing and guidance and starts navigation to
the target.
