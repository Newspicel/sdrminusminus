# Direction finding

A **Direction finder** estimates where a signal comes from, using a
[coherent array](arrays.md). **Triangulation** crosses bearings from several places into a
position.

## Set up

1. Add a multi-lane Device or an [Array](arrays.md#build-your-own-array).
2. Add **Direction finder**. Set **Geometry** and **Elements** to match your antennas: a circle
   with a radius, a line with a spacing, or explicit positions.
3. Wire every lane to the matching input. All must come from one source.
4. Set **Offset** and **Bandwidth** to cover the signal.
5. [Calibrate](arrays.md#calibrate).
6. Wire [GPS position](position.md) for the map and triangulation.

| Algorithm | Use |
|---|---|
| Beamformer | Broad and robust |
| MUSIC | Sharper, but needs the right number of **Sources**. Start with one. |

## The compass

The compass shows the response, the chosen bearing, and the confidence. 0° is north, clockwise.
The strip below shows calibration per lane. **Phase unknown** hides bearings: check the clock
wiring and the calibration reference.

## Listen in one direction

Wire `beam` to a channel. **Follow bearing** points the beam at the current estimate. **Fixed
azimuth** holds a direction.

## Triangulate

1. Add **Triangulation** and wire each finder's `events` to it.
2. Give each finder its own position, from GPS or fixed coordinates.

It shows the estimate, its error ellipse, the age of each bearing, and where to go next. For a
long thin ellipse it suggests moving across the bearing. Once the estimate settles, it suggests
driving towards it. **Clear** starts over.

Wire finder or Triangulation `events` to a **Map** to see bearing rays, the estimate, and the
next waypoint. The first settled fix emits an event that Event output can forward by webhook,
MQTT, or Matrix. On a phone, use the **DF drive** mission in [field mode](field-mode.md).
