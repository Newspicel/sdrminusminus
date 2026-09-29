# iPhone app

The iPhone app runs [missions](phones.md#missions) in the field. It needs iOS 18 or later.

## Install

The app is not in the App Store yet. Build it with Xcode 27 as described in
[Build and test](../development/building.md#phone-apps) and run it on your iPhone.

To look around without a server, pick **Try without a server** under **Demo**. **Leave demo**
ends it.

## Pair

1. On the server, turn on **Allow phones** and press **Pair phone** in **Library → Phones**, or run
   `sdrmm pair`. See [Phones](phones.md#pair).
2. In the app, **Scan QR**. Where the app cannot scan, point the Camera app at the QR code.
3. Or pick the server under **Nearby** and type the code, or type `host:port` and the code under
   **Manual**.
4. Check the **Key** and press **Trust**.

Allow local network access when asked, or **Nearby** stays empty.

## Missions

The list shows the server's missions under **Hunt**, **DF drive**, **Radar** and **Survey**. The
top bar shows the link: `Online`, `Connecting`, `Offline` or `Refused`. The workspace menu
switches the server's active workspace.

| Mission | Shows | Buttons |
|---|---|---|
| Hunt | Level, `Warmer` or `Colder`, a strength bar | Start, Tune, Sweep, Mark; **Clicks** and **Haptics** |
| DF drive | Bearing rose, guidance, map with **Rays**, **Heat**, **Ellipse** | Navigate, Calibrate, Clear, Tune, Fit |
| Radar | Range Doppler image and **Tracks** | |
| Survey | A trail coloured by level | Record, Stop, Clear, Fit |

**Direct** in DF drive guides straight to the fix instead of crossing the bearings first.

## Heading

Open **Settings → Heading**.

- **Source:** **Auto** fuses compass, gyro and GPS course. **Compass** or **GPS course** force one.
- **Mount:** **Flat** on a seat or dash, **Upright** in a holder.
- **Align with car:** drive straight when asked. The app learns the phone's angle to the car and
  shows `Aligned 3°`.

## Navigation

**Navigate** in DF drive routes to the target with Apple Maps data and speaks each turn. When the
target moves, the route follows it and a `New target` alert appears. Voice and units are in
**Settings**.

Before the first route the app shows once:

> YOUR USE OF THIS REAL TIME ROUTE GUIDANCE APPLICATION IS AT YOUR SOLE RISK. LOCATION DATA MAY
> NOT BE ACCURATE.

## CarPlay

CarPlay shows the DF drive map, a **Missions** list, **Navigate**, and a DF panel with the bearing,
guidance, **Calibrate** and **Clear**. It works in the CarPlay simulator. Cars need Apple's
CarPlay navigation approval first.

## In the background

During a mission the app keeps sending its position with the screen off, and hunt clicks keep
playing. iOS shows the blue location pill while it does. For this, press **Allow always** in
**Settings → Location**.
