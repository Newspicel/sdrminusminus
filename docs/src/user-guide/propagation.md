# Propagation map

Map reception paths from FT8, FT4, and WSPR decodes and estimate a lower bound on maximum usable
frequency (MUF). The map uses decoder events, with no additional signal processing.

## Build one

1. Add FT8, FT4, or WSPR channels and a **Propagation map**.
2. Connect each channel's `events` output to the map.
3. Connect **GPS position** to `position`. For a fixed station, enter coordinates in the GPS node's
   **Fixed** tab.

The map also loads six hours of decoder-log history for connected channels when opened.

## Read the layers

| Layer | Display |
|---|---|
| Activity | Estimated reflection points weighted by decode count and age |
| MUF | Estimated MUF lower bound per Maidenhead square |
| Paths | Great-circle paths by station and band, newest first; off by default |

A message must contain a Maidenhead locator to add a path. Reports, `RRR`, `RR73`, and `73`
usually contribute no new location data.

The model divides each path into hops and estimates reflection points. A single-hop reflection
point is the midpoint. Points are grouped into Maidenhead squares and lose half their weight per
**Half-life**, adjustable from five minutes to twelve hours. The table ranks squares by activity.

## Measured MUF

Receiving a signal proves its path supported that frequency at that time. The model scales it to
a 3000 km reference hop:

```text
MUF(3000) ≥ f × M(3000) / M(D / hops)
```

Here, `f` is received frequency, `D` is path length, and `M` is the obliquity factor (`sec φ`) for
a thin reflecting layer over a spherical Earth. At a 300 km layer height, `M(3000)` is about 3.28.
A single 3000 km hop reports the received frequency; shorter hops scale upward.

Interpret the result as a model-dependent lower bound:

- Paths under 500 km count as activity but do not contribute to MUF.
- Missing decodes on a band do not establish that the band was closed.
- Layer height changes the estimate. Use 300 km for F2 or 110 km for sporadic-E modelling.
- A result below a forecast does not by itself disprove that forecast.

## Comparing against the ionosonde network

Enable **Ionosondes** for GIRO and INGV soundings through
[prop.kc2g.com](https://prop.kc2g.com/). The server caches results for fifteen minutes.

The map shows station MUF(3000 km) and compares local estimates with an inverse-distance
interpolation of sounding sites within 3000 km. The footer reports squares above the forecast
and the median difference.

Feed failures are reported while local decodes remain visible. Disable **Ionosondes** to stop
sounding requests; basemap requests are separate.
