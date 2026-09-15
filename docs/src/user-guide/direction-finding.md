# Direction finding

A **Direction finder** estimates signal arrival direction from a [coherent array](arrays.md).
It shows a bearing, confidence, and angular response. Triangulation combines bearings into a
position estimate.

## Set up a finder

1. Add a multi-lane Device or an [Array](arrays.md#radios-you-wired-together-yourself).
2. Add **Direction finder**. Set **Geometry** to your antenna layout and **Elements** to its count.
3. Connect every lane to the matching `iq`, `iq2`, and subsequent inputs. All must come from one source.
4. Set **Offset** and **Bandwidth** to cover the signal.
5. [Calibrate the array](arrays.md#calibration).
6. Connect GPS `position` for map output or triangulation.

Geometry supports a circle with radius, a line with element spacing, or explicit element positions.

## Algorithm

| Algorithm | Use |
|---|---|
| Beamformer | Broad response; useful with limited covariance data |
| MUSIC | Sharper peaks; requires an accurate source count |

For one transmitter, start with **Sources** set to one.

## Read the compass

The compass shows response peaks, the selected bearing, and confidence. Bearings run clockwise
from north at 0°. The strip below shows calibration quality per lane.

**Phase unknown** suppresses bearings. Check clock connections and the calibration reference.

## Listen along a bearing

Connect `beam` to a channel. **Follow bearing** steers toward the current estimate.
**Fixed azimuth** holds a chosen direction and starts at the beam's current bearing.

## Crossing bearings from several finders

1. Add **Triangulation** and connect the finders' `events` outputs.
2. Give each finder its own position source, using GPS or fixed coordinates.
3. View the estimate, error ellipse, guidance, and age of each bearing. **Clear** resets the estimate.

Bearings from different positions constrain the transmitter location. A finder alone provides
bearings; position estimates and driving guidance require Triangulation.

## On the map

Connect finder `events` to **Map** for bearing rays that fade with age. Connect Triangulation
events for the estimated location, uncertainty ellipse, contributing stations, and next waypoint.

## Guidance

For a long, narrow uncertainty ellipse, guidance suggests moving across the bearing to improve
the intersection angle. Once the estimate converges, it suggests approaching the location.

The first converged fix emits an event that connected webhook, MQTT, or Matrix outputs can forward.
Use [field mode](field-mode.md) for phone guidance and navigation.
