# Position and GPS

The **GPS position** node tells other nodes where the station is and, if the source knows it, which
way it points. One node can feed many.

## Pick a source

| Tab | Source |
|---|---|
| Receiver | A serial NMEA GPS on the server. Set the baud rate. |
| Network | gpsd, default `127.0.0.1:2947` |
| Fixed | Latitude and longitude you type in |
| Phone | A paired [phone](phones.md): position and heading |

Serial and gpsd sources must be reachable from the server, and reconnect on their own. **Forget
source** picks a different one.

The node shows the fix, your Maidenhead locator, accuracy, speed, and heading when there is one.
A lost fix is reported, and old coordinates stop being used. A phone that drops out reads
**Offline**.

## Heading

Heading is true north.

| Source | Heading from |
|---|---|
| Phone | Compass, gyro and GPS course, fused on the phone |
| NMEA | `HDT` and `THS` sentences. Magnetic headings are ignored. |
| gpsd | `ATT` reports |

An NMEA or gpsd heading older than two seconds is dropped. An [Array](arrays.md#point-it) set to
**Heading** turns with it.

## What uses it

| Node | Uses position for |
|---|---|
| ADS-B | Decoding aircraft positions |
| Array | Where the array stands and which way it points |
| Map | Your station and its GPS trail |
| Passive radar `tx` | Where the transmitter stands, from a Fixed source |
| Recorder | Location in the recording's metadata |
| Satellite | Pass and Doppler prediction |
| Signal hunt | Where you stand and which way you point, for sweeps |
| Signal survey | Where each measurement was taken |
| Triangulation | The vehicle to guide |
| Propagation map | Your station on the map |
