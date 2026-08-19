# Direction finding

A direction finder reads the phase differences between the elements of a
[coherent array](arrays.md) and says which way a signal arrived. It answers with a bearing, a
confidence, and the whole circle it read that bearing off, so you can see whether the peak it
picked was the only one.

## Wire one up

1. Have a coherent radio: one Device node for a radio with several lanes, or an
   [Array node](arrays.md#radios-you-wired-together-yourself) for radios you cabled together.
2. Add a **Direction finder** and wire every element into it: `iq`, `iq2`, `iq3`… Every lane must
   come from the same radio, and a half-wired array is refused by name when you apply the patch.
3. Wire a **GPS position** node into its `position` input. Bearings without a place are just
   angles; with one they become rays on the map and can be crossed.
4. Set **Geometry** to what you built — a circle of a given radius, a line of a given spacing, or
   explicit element positions — and **Elements** to how many there are. The node's inputs follow
   that number.

Set **Offset** and **Bandwidth** to the signal you are after inside the tuned span.

## Algorithm

| Algorithm | Behaviour |
|---|---|
| Beamformer | Blunt, and it always answers. The baseline when the covariance is too short for MUSIC. |
| MUSIC | Far sharper, and it needs the source count to be right. |

**Sources** tells MUSIC how many arrivals to assume. One is right far more often than not.

## The compass and what it is telling you

The face draws the pseudospectrum on a compass rose with the bearing over it, the confidence
beside it, and a per-lane calibration quality strip below. Bearings are compass bearings: zero due
north, running clockwise.

If the calibration readout says phase is unknown, no bearing is drawn and none is emitted. That is
the array telling you its tier and calibration source cannot justify one.

## The beam output

A direction finder also sums its elements towards a bearing and writes the result onto a `beam`
output. Wire a channel to it and you listen to whatever the array is pointed at.

| Beam | Behaviour |
|---|---|
| Follow bearing | The beam tracks whatever bearing the array is currently finding |
| Fixed azimuth | The beam stays where you put it |

Choosing a fixed azimuth starts from wherever the beam is pointing now, so pinning it holds the
direction the array just found.

## Crossing bearings from several finders

One finder says which way; two say where. Add a **Triangulation** node and wire the `events`
output of every direction finder into it.

The triangulation node holds the grid where those bearings cross. Its face shows the estimate, the
size of the error ellipse, the guidance, and every finder that has reported with how long ago it
last did. **Clear** throws away everything the grid has learned and starts again.

Each finder needs a position of its own. A receiver that drives gets it from a GPS; one that never
moves gets it from a **Fixed place** node — type the latitude and longitude once and everything
downstream treats it like any other fix.

Without a triangulation node a direction finder still shows bearings and a compass, but there is
no estimate, no guidance, and no event when a fix converges.

## On the map

Wire a finder's `events` into a **Map** to draw its bearing rays, which fade with age. Wire a
triangulation node's `events` in as well and the map adds the fused estimate, its uncertainty
ellipse, the contributing stations, and the leg to the next place worth driving to.

## Guidance

While the estimate is a long thin ellipse, the guidance tells you to drive **across** the current
bearing — two bearings from the same place say nothing, and two from different places cross. Once
the ellipse closes up it switches to **approach** and points at the estimate.

The first time a fix converges, a decoded event is published, so any webhook, MQTT or Matrix
[event output](channels.md) you have wired fires on it.

Driving all of this from a phone is [field mode](field-mode.md).
