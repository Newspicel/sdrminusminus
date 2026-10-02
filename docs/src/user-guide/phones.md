# Phones

An iPhone or Android phone is a remote head for field work. It runs missions from the active
workspace and sends its position and heading to the server. It does not edit the graph.

## Get the app

Neither app is in a store. Build it from source.

| Phone | Needs | Install |
|---|---|---|
| iPhone | iOS 18 | Build with Xcode, see [Phone apps](../development/phone-apps.md#iphone) |
| Android | Android 10 | Build the APK, see [Phone apps](../development/phone-apps.md#android), then `adb install` it or open the file on the phone |

To look around without a server, pick **Try without a server** under **Demo**.

## Allow phones

Phones need a direct HTTPS endpoint whose key they pin at pairing.

- Open **Library → Phones** and turn on **Allow phones**. This opens a phone **Port**, `8443` by
  default, with the server's own self-signed certificate.
- A server started with `--tls-self-signed` on a LAN address also takes phones on its main port.

The status line reads `Ready on 8443` when phones can reach the server, and `Discovery on` when
nearby phones can find it. Your firewall must let the port in.

Reach the phone port over your LAN or a Tailscale IP. Tunnels that end HTTPS, such as Tailscale
Serve or a Cloudflare Tunnel, carry only the browser.

## Pair

1. Press **Pair phone**. A QR code, an 8-digit **Code** and a **Key** appear for five minutes.
2. On the phone, **Scan QR**. Or pick the server under **Nearby** and type the code, or type
   `host:port` and the code under **Manual**.
3. Check the phone shows the same **Key**, then **Trust**.

Allow local network access when the phone asks, or **Nearby** stays empty. On a server without a
browser, `sdrmm phone` prints the QR code in the terminal, see
[Configuration](../server/configuration.md#phones).

Five wrong codes end the offer with `Too many tries`. Each phone gets its own token: rename or
revoke the phone in the list. Revoking disconnects it at once.

## Missions

The phone lists what the active workspace offers. Its top bar shows the link: `Online`,
`Connecting`, `Offline` or `Refused`.

| Mission | Needs | Shows | Controls |
|---|---|---|---|
| Hunt | A Signal hunt wired to a channel | Level, `Listening`, `Warmer`, `Colder` or `On top` | Start, Tune, Sweep, Mark, **Clicks**, **Haptics** |
| DF drive | A Direction finder, a Triangulation, or both | Bearing rose, guidance, map with rays, heat, ellipse | Navigate, **Auto** or **Direct**, Calibrate, Clear, Tune, Fit |
| Radar | A Passive radar | Range Doppler image and tracks | |
| Survey | A Signal survey with a GPS on `position` | A trail coloured by level | Record, Stop, Clear, Fit |

A mission that cannot run is dimmed and says why, such as `Not running`. The phone can also switch
the active workspace, for every client.

**Navigate** routes to the target and follows it when it moves, with a `New target` alert. The
iPhone guides turn by turn itself; Android hands the target to Google Maps or another map app.
**Direct** guides straight to the fix instead of crossing the bearings first.

## Position and heading

Add a **GPS position** node, pick the **Phone** tab and the phone. Wire it where it is needed:

| Wire to | For |
|---|---|
| Array `position` | Where the array stands and which way it points |
| Signal hunt `position` | Sweep bearings with a handheld antenna |
| Signal survey `position` | Where each level was measured |
| Triangulation `position` | Guidance to the target |
| Passive radar `tx` | Where the transmitter stands |

The phone sends its pose only while a GPS node uses it. Set it up under **Settings → Heading**:

| Setting | Does |
|---|---|
| Source | **Auto** fuses compass, gyro and GPS course. **Compass** or **GPS course** force one. |
| Mount | **Flat** on a seat or dash, **Upright** in a holder |
| Offset | A fixed angle between phone and car |
| Align with car | Drive straight when asked; the app learns the **Offset** |

During a mission the app keeps sending its position with the screen off. On iPhone, press
**Allow always** in **Settings → Location**. On Android, a `Background off` chip means Android
refused: keep the app on screen.

## In the car

**CarPlay** and **Android Auto** show the DF drive map, a **Missions** list, and a DF panel with
the bearing, guidance, **Calibrate** and **Clear**. **Navigate** starts navigation.

CarPlay in a car needs a build signed with Apple's CarPlay navigation entitlement; the simulator
works without it. Android Auto hides sideloaded apps: in Android Auto, tap **Version** ten times,
then turn on **Unknown sources** in **Developer settings**.

## Android chips

| Chip | Means |
|---|---|
| Online, Connecting, Offline | Link to the server, or why it refused |
| Sharing position | The server uses this phone's position |
| Fused ±4° | Heading source and accuracy |
| No GPS fix | No position yet |
| Approx. location | Precise location is off |
| Location off | Location permission or service is off |
| No heading, No compass | No usable heading |
| Calibrate compass | Wave the phone in a figure eight |
| Background off | Android refused to run in the background |
| Alerts off | Notifications are off |
| No map tiles | The map is offline |
