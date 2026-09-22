# Understand the workspace

A workspace saves your nodes, connections, radio settings, rack layout, and regional band plan.
It lives on the server and is shared by every connected client.

## Patch and rack

Use **Patch** to connect nodes and inspect signal flow. Ports accept compatible signals:
IQ feeds channels and scopes, audio feeds speakers, and decoder events feed displays and exports.

Use **Rack** for everyday operation. Select a node and press `p` to pin or unpin its controls.
Moving or resizing it in Rack leaves its connections intact. Press `v` to switch views.

## Node types

| Group | Examples | Purpose |
|---|---|---|
| Sources | Device, GPS position | Supply IQ or station position |
| Decoders | AM, NFM, WFM, ADS-B, DMR | Receive one signal from IQ |
| Tools | Array, Direction finder, Passive radar, Combiner, Scanner, Signal hunt, Satellite, DMR trunk, Event filter, Triangulation | Process signals or control receivers |
| Outputs | Scope, Baseband scope, Map, Readout, Decoder log, Video, Speaker, recorders, Network IQ, Export | Display, play, save, or forward results |

**+ Node** lists the nodes available in the running server. Start with a Device, connect a channel,
and add outputs for its audio or events.

## Live position wiring

Add **GPS position** and select a source:

| Tab | Source |
|---|---|
| Receiver | Serial NMEA receiver, selected from the list or entered as a device path |
| Network | gpsd endpoint; default `127.0.0.1:2947` |
| Fixed | Latitude and longitude entered manually |
| This device | Browser or desktop WebView location, where supported |

Serial and network sources must be reachable from the server. **This device** uses the client
showing the interface. For serial receivers, set the baud rate and maximum published update rate.
The node validates GGA and RMC sentences and displays a six-character Maidenhead locator.

Connect `position` to any consumers that need it:

| Consumer | Uses position for |
|---|---|
| ADS-B | Local CPR decoding reference |
| Map | Station position, route, and visited-location heatmap |
| Recorder | Position and fix time in SigMF capture metadata |
| Direction finder, Passive radar, Propagation map | Geographic results |

One source can feed several nodes. Lost fixes are reported and stale coordinates stop being used.
Serial and gpsd sources reconnect automatically. **Forget source** reopens the source picker.

## Drive a signal survey

1. Add **Signal survey** and connect Device `IQ` and GPS `position`.
2. Choose a frequency offset within the IQ span and a measurement width.
3. Wait for a spectrum level and GPS fix, then start the survey.
4. Pause before changing the receiver setup. Export the results as CSV when finished.

The offset controls move the measured slice without retuning the radio. Each fix records the
peak spectrum level within that slice. Nearby fixes are grouped into roughly ten-metre cells.

Levels are in dBFS. Keep gain, antenna, cable, and measurement width unchanged to compare locations.

## Device identity and reconnection

Device nodes remember the selected receiver's identity. Unplugging it preserves the node,
connections, and settings; reconnecting the same receiver restores the binding.

Use **Forget this radio** to release it and choose a replacement. The node and wires remain.

## Applying a patch

Most edits save and apply automatically. Applying a patch opens devices, restores settings,
updates channels, and removes live objects no longer used by the workspace. Press **Apply patch**
when a node reports that the saved layout and running receiver differ.

## Multiple workspaces and clients

Tuning, switching workspaces, and applying templates affect everyone connected to the server.
Workspaces organise activities; they are not private browser sessions.

Concurrent edits to the same saved revision produce a conflict rather than overwrite another
client's layout. See [Workspaces, templates, and presets](../user-guide/workspaces.md) for
saving, sharing, undo, and reuse.
