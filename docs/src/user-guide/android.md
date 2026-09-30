# Android app

The Android app runs [missions](phones.md#missions) in the field. It needs Android 10 or later.

## Install

The app is not in the Play Store. Build the APK as described in
[Android development](../development/android.md), then install it with `adb install` or open the
file on the phone and allow the install.

To look around without a server, pick **Try without a server** under **Demo**. **Leave demo**
ends it.

## Pair

1. On the server, turn on **Allow phones** and press **Pair phone** in **Library → Phones**, or run
   `sdrmm phone`. See [Phones](phones.md#pair).
2. In the app, **Scan QR**.
3. Or pick the server under **Nearby** and type the 8-digit code, or type `host:port` and the code
   under **Manual**.
4. Check the **Key** and press **Trust**.

On Android 17, allow local network access when asked, or no server on your LAN can be reached.

## Missions

The list shows the server's missions under **Hunt**, **DF drive**, **Radar** and **Survey**. The
top bar shows the link: `Online`, `Connecting`, `Offline` or `Refused`.

| Mission | Shows | Buttons |
|---|---|---|
| Hunt | Level, `Warmer` or `Colder` | Start, Tune, Sweep, Mark; **Clicks** and **Haptics** |
| DF drive | Bearing rose, guidance, map **Layers** | Navigate, Calibrate, Clear, Tune, Fit |
| Radar | Range Doppler image and **Tracks** | |
| Survey | A trail coloured by level | Record, Stop, Clear, Fit |

**Navigate** hands the target to Google Maps, or asks for a map app. When the target moves, a
`New target` notification appears; tap it to navigate again.

## Chips

| Chip | Means |
|---|---|
| Sharing position | The server uses this phone's position |
| Fused ±4° | Heading source and accuracy |
| No GPS fix | No position yet |
| Approx. location | Precise location is off |
| Location off | Location permission or service is off |
| No heading, No compass | No usable heading |
| Calibrate compass | Wave the phone in a figure eight |
| Background off | Android refused to run in the background: keep the app on screen |
| Alerts off | Notifications are off |
| No map tiles | The map is offline |

## Settings

- **Heading:** **Source** (Auto, Compass, GPS course), **Mount** (Flat or Upright), **Offset**.
  **Align with car** learns the phone's angle to the car while you drive straight.
- **Navigation app:** Google Maps or Ask.
- **Display:** **Keep screen on**, **Map tiles**.
- **Servers:** add another server or forget one.

## Android Auto

The car shows the DF drive map, a **Missions** list, and a DF panel. **Navigate** starts the car's
navigation app.

The app is sideloaded, so Android Auto hides it by default. In Android Auto, tap **Version** ten
times, then turn on **Unknown sources** in **Developer settings**.
