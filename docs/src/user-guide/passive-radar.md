# Passive radar

Passive radar borrows a transmitter that is already on the air — a broadcast station, usually —
and looks for what its signal bounced off. One antenna watches the transmitter, another watches
the sky, and what makes an echo visible is that it arrived later and shifted in frequency.

It runs on a [time-synced array](arrays.md): the delay between the two lanes is what is being
measured, so a shared clock is enough and phase is not needed.

## Wire one up

1. Have a coherent radio with at least two lanes.
2. Add a **Passive radar** node. Its two inputs are named rather than numbered:

| Input | Antenna |
|---|---|
| `ref` | Pointed at the illuminator |
| `surv` | Pointed at the sky the targets are in |

3. Wire a **GPS position** node into `position` if you want the echoes drawn on a map.

## What it does with them

| Step | Purpose |
|---|---|
| ECA | Cancels the direct path and the zero-Doppler clutter from the surveillance lane |
| CAF | Correlates what is left against the reference across every Doppler hypothesis |
| CFAR | Sets a threshold from each cell's own neighbourhood and reports what stands above it |
| Cluster | Merges cells that touch, so one bright patch is one echo rather than four |
| Track | Follows echoes from one integration to the next and names the ones that keep coming back |

**Integration** is how long one coherent processing interval takes: longer sees weaker echoes and
blurs anything moving. **Range bins** sets how far out the surface reaches, and **Doppler span**
how fast a target may be closing or opening.

## Reading the surface

The face paints range against Doppler, with detections marked. A detection that the tracker has
not made up its mind about carries no name; one it has seen often enough is labelled with its
target number and keeps it while it moves. An echo that appears once and never again is noise, and
saying so is the tracker's whole job.

## Echoes on the map

A passive radar measures **bistatic range**: how much further the echo travelled than the direct
path. That does not put a target at a point or on a bearing — it puts it somewhere on an ellipse
with the transmitter and the receiver at its foci.

Tick **Transmitter** in the settings and type the illuminator's latitude, longitude and frequency,
and the map draws that ellipse for every echo. Without it there is nothing to draw an ellipse
around, so nothing is drawn.

There is no tracker on the map: what is followed is followed in range and Doppler, not on the
ground.

## In the field

The **Radar watch** mission puts the surface fullscreen on a phone with what is being followed
listed under it. See [field mode](field-mode.md).
