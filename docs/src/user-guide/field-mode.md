# Field mode

Field mode provides phone controls for signal hunting, direction finding, and passive radar.
Prepare the active workspace on a desktop, then connect the phone to the same server.

## Connect your phone

Open **Library → Field** and scan the QR code. The link includes the server token, which field mode
stores and removes from the address bar. When the desktop uses `localhost`, the QR code offers
a reachable LAN address.

You can also open `/field` directly from a browser that can reach the server.

## Missions

Available missions depend on the active workspace. Each controls one node.

| Mission | Required node | Controls |
|---|---|---|
| Fox hunt | Signal hunt | Level, rising/falling indication, variable-rate clicks, start/stop |
| DF drive | Direction finder | Compass, guidance, map |
| Radar watch | Passive radar | Range–Doppler surface and tracks |

Fullscreen and screen wake lock are available where supported by the browser.

## Driving to a signal

DF drive orients the compass using GPS course over ground, not the phone's compass sensor.
A connected [Triangulation node](direction-finding.md#crossing-bearings-from-several-finders)
provides crossing and approach guidance. Without it, the bearing display still works.

### Turn-by-turn

Configure a [routing backend](../server/configuration.md#turn-by-turn-routing) for routes,
next manoeuvres, and distances. Spoken directions become available after a touch interaction.
The routing key stays on the server.

| Nav mode | Destination |
|---|---|
| Auto | Crossing waypoint until convergence, then the location estimate |
| Direct | Current location estimate |
| Off | Heading guidance only |

Routes update when you leave the route, the target moves, or the guidance phase changes.
If routing is unavailable, the screen reports the reason and keeps heading guidance.

**Navigate in Maps** opens the target in the phone's navigation app. Open it again when the target
changes; the browser cannot update an already open native navigation session.

## Maps without internet

Place `basemap.pmtiles` beside the server database for an offline basemap. Otherwise, field mode
uses the online style. Without either map, bearings, routes, and markers appear on a blank background.
