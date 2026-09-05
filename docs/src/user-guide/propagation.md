# Propagation map

The Propagation map uses FT8, FT4, and WSPR decodes to show reception paths and estimate a lower
bound on maximum usable frequency (MUF). It processes decoder events and adds no DSP load.
The reflection points and MUF values are estimates based on a configurable ionospheric layer model.

## Build one

1. Add one or more FT8, FT4, or WSPR channels.
2. Add a **GPS position** source and a **Propagation map** from **+ Node**.
3. Connect each channel's `events` output to the map's `events` input.
4. Connect GPS `position` to the map's `position` input.

A receiver position is required to calculate paths. For a stationary receiver without GPS, choose
**Receiver that never moves?** on the GPS position node and enter its latitude and longitude.

When opened, the map also loads the last six hours of decoder-log history for its connected channels.

## What is plotted

A decode containing a Maidenhead grid square defines a path from your receiver to that square.
The model divides the path into hops and estimates their reflection points. For a single hop,
the reflection point is the midpoint.

Reflection points are grouped into Maidenhead squares. Each decode adds weight that decays with
age: half remains after one **Half-life**, a quarter after two. Set the half-life between five
minutes and twelve hours depending on how much history you want to see.

Messages without a grid square do not add a path. This includes signal reports, `RRR`, `RR73`,
and `73`, so many messages in an ongoing contact contribute no new map data.

## Measured MUF

Receiving a signal at frequency *f* shows that its path supported that frequency at that time.
The model scales this observation to a 3000 km reference hop:

```text
MUF(3000) ≥ f × M(3000) / M(D / hops)
```

Here, *D* is path length and `M` is the obliquity factor (`sec φ`) for a thin reflecting layer over
a spherical Earth. At the default layer height of 300 km, `M(3000)` is about 3.28. A 3000 km
single-hop path therefore reports the received frequency; shorter hops scale it upward.

Read this as a model-dependent lower bound, not a measurement of the highest open band:

- Paths shorter than 500 km contribute to activity but are excluded from MUF estimates. Ground-wave
  and near-vertical paths do not fit this calculation reliably.
- The estimate depends on which bands you monitored. No 10 m decodes cannot establish that 10 m
  was closed. A value below the forecast does not by itself disprove the forecast.
- The assumed layer height changes the result. Use 300 km for F2 estimates or 110 km when modelling
  sporadic-E.

## Comparing against the ionosonde network

Enable **Ionosondes** to fetch sounding data from GIRO and INGV through the
[prop.kc2g.com feed](https://prop.kc2g.com/). The server caches the response for fifteen minutes.
The map shows each station's MUF(3000 km) and compares each measured square with an
inverse-distance interpolation of sounding sites within 3000 km. The footer gives the number of
squares above the forecast and the median difference.

If the feed is unreachable, the map reports the reason and continues displaying local decodes.
Disable **Ionosondes** to stop these feed requests. Basemap requests are separate.

## Reading the map

| Layer | Display |
|---|---|
| **Activity** | Estimated reflection points, weighted by decode count and age |
| **MUF** | One labelled point per square showing its estimated MUF lower bound |
| **Paths** | Great-circle paths to stations, one per station and band, newest first |

**Paths** is off by default to keep busy bands readable. The table below the map ranks squares by
activity weight.
