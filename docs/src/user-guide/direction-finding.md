# Direction finding

A Direction finder estimates a signal's arrival direction from phase differences across a
[coherent array](arrays.md). It displays a bearing, confidence, and angular response so you can
see competing peaks.

## Wire one up

1. Add a multi-lane **Device**, or an [Array node](arrays.md#radios-you-wired-together-yourself)
   for separate radios sharing a clock.
2. Add a **Direction finder**. Set **Geometry** to your antenna layout: a circle with a radius,
   a line with element spacing, or explicit element positions. Set **Elements** to the antenna count.
3. Connect every source lane to the corresponding `iq`, `iq2`, `iq3`… input. All lanes must come
   from the same Device or Array. Applying an incomplete set of connections reports an error.
4. Connect a **GPS position** source to `position` to place bearings on a map or use triangulation.
5. Set **Offset** and **Bandwidth** to select the signal within the source's tuned span.

## Algorithm

| Algorithm | Behaviour |
|---|---|
| Beamformer | Broader angular response; useful as a baseline with limited covariance data |
| MUSIC | Sharper peaks; depends on an accurate source count |

**Sources** sets the number of arrivals MUSIC should assume. Start with one for a single source.

## The compass and what it is telling you

The compass shows the angular response, selected bearing, and confidence. A strip below it shows
calibration quality for each lane. Bearings run clockwise from north at 0°.

When calibration reports **phase unknown**, the node neither displays nor publishes a bearing.
Check the array's clock connections and calibration reference.

## The beam output

The `beam` output sums the elements toward a selected bearing. Connect it to a channel to listen
in that direction.

| Beam | Behaviour |
|---|---|
| Follow bearing | Tracks the current estimated bearing |
| Fixed azimuth | Holds a chosen direction |

Switching to fixed azimuth starts at the beam's current direction.

## Crossing bearings from several finders

Add a **Triangulation** node and connect each Direction finder's `events` output. Bearings from
different positions constrain the transmitter's estimated location.

Each finder needs its own position source. Use GPS for a moving receiver, or a **GPS position**
node with fixed latitude and longitude for a stationary one.

The Triangulation node shows the position estimate, error ellipse, guidance, and the age of each
finder's latest report. **Clear** resets the accumulated estimate.

A Direction finder works without triangulation, but then provides no position estimate, driving
guidance, or event announcing a converged fix.

## On the map

Connect a Direction finder's `events` output to a **Map** to draw bearing rays that fade with age.
Connect Triangulation events to add the combined position estimate, uncertainty ellipse,
contributing stations, and suggested next waypoint.

## Guidance

When the estimate has a long, narrow error ellipse, guidance suggests moving **across** the bearing
to improve the intersection angle. Once the estimate converges, it switches to **approach**.

The first converged fix publishes a decoded event. Connected webhook, MQTT, or Matrix outputs can
forward it. Use [field mode](field-mode.md) for the phone interface and navigation.
