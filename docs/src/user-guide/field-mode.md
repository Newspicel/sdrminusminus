# Field mode

sdr-- is a desktop application. Field mode is the deliberate exception: one touch-first route,
served by the same server from the same workspace, for the times the receiver is in a car and you
are not at the desk.

Open `/field` in any browser on the same network. There is no separate application and nothing
extra to install.

## Get it onto a phone

**Library ▸ Field** shows a QR code. Point the phone's camera at it and it opens field mode
already carrying the token.

If you are browsing on `localhost`, that address means nothing to a phone, so the QR offers one of
this machine's LAN addresses instead. The token arrives in the URL and is stripped out of the
address bar once stored.

## Missions

`/field` lists what the active workspace can drive. Each mission runs against one node.

| Mission | Needs | What you get |
|---|---|---|
| Fox hunt | A channel | One large level meter, a click track that speeds up as the signal rises, and a trail on the map |
| DF drive | A direction finder | A compass turned to the vehicle's heading, guidance, and the map underneath |
| Radar watch | A passive radar | The range–Doppler surface fullscreen with what the tracker is following |

The screen stays awake while a mission is open, there is a fullscreen toggle, and the layout keeps
clear of the phone's own cutouts.

## Driving to a signal

DF drive shows the live bearing on a compass rotated to your course over ground, which comes from
the GPS rather than from the phone's own sensors. Under it is what to do next: cross the bearing
while the estimate is a long ellipse, close in once it has tightened.

Guidance comes from the [triangulation node](direction-finding.md#crossing-bearings-from-several-finders)
the finder is wired into. Without one the compass still works and the mission says so.

### Turn-by-turn

If the server has a routing backend configured, the mission draws the route, shows the next
manoeuvre with a distance countdown, and speaks it. Voice arms itself on your first touch, which
is what phones require.

It asks for a new route only when something actually changed: you left the road, the target moved,
or the guidance switched between crossing and closing. Never on a timer, so a free-tier key is not
a problem.

| Nav mode | Behaviour |
|---|---|
| Auto | Crossing waypoint until the fix converges, then the estimate |
| Direct | Always the current estimate |
| Off | Compass and arrow only |

With no routing backend, no internet, or a backend that is down, the mission falls back to heading
guidance and says so. **Navigate in Maps** hands the current target to the phone's own navigation
app; it re-arms whenever the target changes, because a browser cannot re-open it on its own while
the native app is in front.

See [server configuration](../server/configuration.md) for the routing backend and its key, which
stays on the server.

## Maps without internet

Put a `.pmtiles` archive next to the database in the data directory and field mode draws a basemap
from it with no network at all. Without one it uses the online style, and without that a blank
background — the bearings, route and markers are drawn either way.
