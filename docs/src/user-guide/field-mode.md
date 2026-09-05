# Field mode

Field mode is a phone interface for signal hunting, direction finding, and passive radar.
It uses the active workspace on the same server. Build the workspace in the desktop interface,
then open `/field` from a browser that can reach the server.

## Get it onto a phone

Open **Library → Field** and scan the QR code with the phone's camera. The link includes the
server token. Field mode stores it and removes it from the address bar.

When the desktop browser uses `localhost`, the QR code offers a server LAN address that the phone
can reach instead.

## Missions

`/field` lists the missions available from nodes in the active workspace. Each mission controls
one node.

| Mission | Required node | Controls and displays |
|---|---|---|
| Fox hunt | Signal hunt | Signal level, rising/falling indication, variable-rate click track, start/stop |
| DF drive | Direction finder | Compass, guidance, and map |
| Radar watch | Passive radar | Range–Doppler surface and tracked echoes |

Missions offer fullscreen mode and keep the screen awake where the browser supports it.

## Driving to a signal

DF drive rotates the compass to the vehicle's GPS course over ground. It does not use the phone's
compass sensor. Guidance suggests crossing the bearing while the estimate is uncertain, then
approaching it after convergence.

Guidance requires a connected [Triangulation node](direction-finding.md#crossing-bearings-from-several-finders).
Without one, the bearing compass still works and the screen reports that guidance is unavailable.

### Turn-by-turn

With a routing backend configured, DF drive shows a route, the next manoeuvre, and its distance.
Spoken directions become available after the first touch interaction.

The mission requests a new route when you leave the route, the target moves, or guidance changes
between crossing and approaching. It does not poll for routes on a timer.

| Nav mode | Behaviour |
|---|---|
| Auto | Route to a crossing waypoint until the fix converges, then to the estimate |
| Direct | Route to the current estimate |
| Off | Compass and direction arrow only |

If routing is unconfigured or unavailable, the mission reports the problem and uses heading
guidance. **Navigate in Maps** opens the current target in the phone's navigation app. Use it again
when the target changes; the browser cannot update the foreground native app automatically.

See [server configuration](../server/configuration.md#turn-by-turn-routing) for backend options.
The routing API key stays on the server.

## Maps without internet

Place an archive named `basemap.pmtiles` beside the database to use an offline basemap.
Otherwise, field mode uses the online map style. If neither is available, bearings, routes, and
markers remain visible on a blank background.
