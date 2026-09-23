# Position and GPS

The **GPS position** node tells other nodes where the station is. One node can feed many.

## Pick a source

| Tab | Source |
|---|---|
| Receiver | A serial NMEA GPS on the server. Set the baud rate. |
| Network | gpsd, default `127.0.0.1:2947` |
| Fixed | Latitude and longitude you type in |
| This device | The location of the browser or desktop app showing the interface |

Serial and gpsd sources must be reachable from the server, and reconnect on their own. **This
device** needs HTTPS or localhost. **Forget source** picks a different one.

The node shows the fix and your Maidenhead locator. A lost fix is reported, and old coordinates
stop being used.

## What uses it

| Node | Uses position for |
|---|---|
| ADS-B | Decoding aircraft positions |
| Map | Your station, route, and a heatmap of visited places |
| Recorder | Location in the recording's metadata |
| Satellite | Pass and Doppler prediction |
| Signal survey | Where each measurement was taken |
| Direction finder, Passive radar, Propagation map | Placing results on the map |
