# Propagation map

The Propagation map node turns your own FT8, FT4 and WSPR decodes into a picture of where the
ionosphere is reflecting right now, and derives a measured MUF from what you actually heard. It
is receive-only and adds no DSP: everything it draws comes from decodes the weak-signal channels
already produce.

## Build one

1. Add one or more FT8, FT4 or WSPR channels and let them decode.
2. Add a **GPS position** source and a **Propagation map** from **+ Node**.
3. Wire each decoder's `events` output into the map's `events` input.
4. Wire the GPS `position` output into the map's `position` input.

The position input is not optional. Every path has two ends, and the map measures from where the
receiver stands. If you have no GPS receiver, a GPSD or NMEA source pointed at a fixed
configuration works, since the station does not move.

On mount the node also reads the last six hours out of the decoder log for the channels wired
into it, so the map is populated the moment you open a workspace rather than starting empty.

## What is plotted

Each decode that carries a grid square gives a path from your station to that square. The map
splits the path into the hops it must have taken and marks each reflection point — for a path
short enough to be a single hop, that is the midpoint.

Reflection points are gathered into their own Maidenhead squares. Each square accumulates a
weight that decays exponentially: a decode contributes half as much after one half-life, a
quarter after two. The half-life is a setting, from five minutes for watching a band open to
twelve hours for a whole-day picture.

Not every decode carries a grid. Signal reports, `RRR`, `RR73` and `73` do not, so a busy QSO
contributes only its opening call. This is a property of the mode, not a limitation of the map.

## Measured MUF

A decode at frequency *f* over a path of length *D* proves that the path supported *f*. That is a
lower bound on the maximum usable frequency for that path, and it can be projected onto the
standard 3000 km reference hop:

    MUF(3000) ≥ f × M(3000) / M(D / hops)

`M` is the obliquity factor of a thin reflecting layer, `sec φ`, computed for a spherical earth at
the configured layer height. At the default 300 km, `M(3000)` comes out at 3.28, matching the
`M(3000)F2` the ionosonde network publishes. A path exactly 3000 km long therefore reports its own
frequency; a shorter hop implies a much higher MUF, and a long multi-hop path implies one only
slightly above the band in use.

Two limits are deliberate:

- Paths shorter than 500 km are excluded from the MUF estimate. Those are ground wave or
  near-vertical incidence, and inverting them as if they were an F2 hop would produce a wildly
  high number. They still count towards the activity heatmap.
- The result is always a **floor**. It is the highest frequency you actually decoded, so the real
  MUF sits at or above it. If you were not listening on 10 m, the map cannot know 10 m was open.
  Read a negative difference against the forecast as "I was not listening higher", not as "the
  forecast is wrong".

The layer height is a setting because the assumption changes the answer. Leave it at 300 km for
ordinary F2 work; drop it to 110 km when you are looking at sporadic-E.

## Comparing against the ionosonde network

With **Ionosondes** enabled the server fetches the current MUF(3000 km) from the
[GIRO](https://giro.uml.edu/) and INGV sounding network, aggregated by
[prop.kc2g.com](https://prop.kc2g.com/), and caches it for fifteen minutes — the interval that
map is rebuilt on. Each sounding site is drawn with its own reading, and every square with a
measured MUF is compared against an inverse-distance interpolation of the sites within 3000 km of
it. The footer reports how many squares sit above the forecast and the median difference.

The fetch is the only outbound request the propagation map makes, and it is made by the server,
not the browser. A server with no route to the feed answers with an empty station list and the
reason, and the map carries on showing what this receiver measured on its own. Turn the toggle
off to make no request at all.

## Reading the map

The **Activity** layer is a decayed heatmap of reflection points: where the ionosphere is putting
signals down for you, weighted by how recently. The **MUF** layer replaces it with one dot per
square coloured and labelled by the measured MUF floor.

**Paths** draws the great-circle line to each station heard, one per station and band, newest
first. It is off by default because a busy FT8 band draws a lot of lines.

The table below the map ranks squares by weight, so the top row is where propagation is most
active rather than merely most distant.
