# Field mode

Field mode puts signal hunting, direction finding, and passive radar on your phone. Set up the
workspace on a desktop first, then connect the phone to the same server.

## Connect a phone

Open **Library → Field** and open the link on the phone. It carries the server token, which field
mode saves and removes from the address bar. If the desktop is on `localhost`, the link uses a LAN
address instead. You can also open `/field` in any browser that reaches the server.

The phone must reach the server. The desktop app and `sdrmm --bind 127.0.0.1:<port>` only listen
on this computer; run `sdrmm` with the default `--bind 0.0.0.0:8080`.

Phone location needs HTTPS. Away from home, use
[a tunnel](../server/tunnels.md).

## Missions

Each mission drives one node in the active workspace:

| Mission | Needs | Shows |
|---|---|---|
| Fox hunt | Signal hunt | Level, rising or falling, clicks that speed up |
| DF drive | Direction finder | Compass, guidance, map |
| Radar watch | Passive radar | Range-Doppler display and tracks |

Fullscreen and keep-screen-on work where the browser supports them.

## DF drive

The compass turns with your GPS heading, not the phone's compass. A
[Triangulation](direction-finding.md#triangulate) node adds guidance on where to drive. Without
it, you still get bearings.

With a [routing service](../server/configuration.md#turn-by-turn-routing) configured, you also
get turn-by-turn directions. Spoken directions start after you tap the screen.

| Nav mode | Drives to |
|---|---|
| Auto | A crossing point until the estimate settles, then the estimate |
| Direct | The current estimate |
| Off | Nowhere; heading guidance only |

**Navigate in Maps** hands the target to your phone's navigation app. Tap it again when the
target moves; the browser cannot update an open navigation session.

## Offline maps

Put `basemap.pmtiles` next to the server database for maps without internet. Without it, field
mode uses the online map, or a blank background if that is unreachable.
