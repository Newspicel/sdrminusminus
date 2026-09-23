# Propagation map

The **Propagation map** plots where FT8, FT4, and WSPR signals came from, and estimates a lower
bound on the maximum usable frequency (MUF). It only reads decoder events.

## Build one

1. Add FT8, FT4, or WSPR channels and a **Propagation map**.
2. Wire each channel's `events` to the map.
3. Wire [GPS position](position.md) to `position`. A fixed position works.

On opening, the map loads the last six hours of logged decodes.

## Layers

| Layer | Shows |
|---|---|
| Activity | Estimated reflection points, weighted by count and age |
| MUF | Estimated MUF lower bound per Maidenhead square |
| Paths | Great-circle paths by station and band. Off by default. |

Only messages with a locator add a path. Reports and `73` usually do not.

Each path is split into hops. A one-hop path reflects at its midpoint. Points lose half their
weight every **Half-life**, from five minutes to twelve hours.

## Measured MUF

A decoded signal proves its path carried that frequency at that moment. The map scales it to a
3000 km hop:

```text
MUF(3000) ≥ f × M(3000) / M(D / hops)
```

`f` is the received frequency, `D` the path length, `M` the obliquity factor for a thin layer over
a round Earth. At a 300 km layer height, `M(3000)` is about 3.28.

Read the result as a lower bound:

- Paths under 500 km count as activity but not towards MUF.
- No decodes on a band does not mean the band was closed.
- Layer height matters. Use 300 km for F2, 110 km for sporadic E.

## Ionosondes

**Ionosondes** overlays GIRO and INGV soundings from [prop.kc2g.com](https://prop.kc2g.com/),
cached for fifteen minutes. The footer compares your estimates with the soundings within 3000 km.
If the feed fails, your own decodes stay on the map. Turn it off to stop the requests.
