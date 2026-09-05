# Understand the workspace

A workspace saves your receiver setup: nodes, connections, positions, rack layout, and regional
band plan.

## Patch and rack

The **Patch** view is where you build and troubleshoot signal flow. Ports are typed, so the canvas
only permits meaningful connections: IQ feeds channels, scopes, and recorders; audio feeds a
speaker; decoder events feed maps, readouts, logs, and exports; scanner control drives a device.

The **Rack** view collects the controls and displays you use most. Select a node and press `p`
to pin or unpin it. Moving or resizing a face on the rack does not change its signal connections.

## Node types

| Group | Nodes | Role |
|---|---|---|
| Sources | Device, GPS position | Radio IQ or a live station position |
| Decoders | AM, NFM, WFM, SSB, ADS-B, DMR, and the rest of the channel catalog | Select and process one signal from device IQ |
| Tools | Array, Direction finder, Passive radar, Combiner, Scanner, Signal hunt, DMR trunk, Event filter, Triangulation | Control radios, process arrays, or filter decoder events |
| Outputs | Scope, Map, Signal survey, Readout, Decoder log, Video, Speaker, Recorder, Audio recorder, Baseband recorder, Time machine, Network IQ, Export | Display, play, record, or export signals and events |

The server supplies the node palette and channel catalog, so the interface shows the options
available in the running build.

## Live position wiring

Add a **GPS position** node and select a source. Sources read by the server use hardware or network
endpoints reachable from the server machine. Browser location uses the device displaying the UI.

| Choice | Source |
|---|---|
| Listed receiver | A detected serial NMEA receiver, searchable by path or device name |
| **Receiver not listed?** | A manually entered serial device path |
| **This device's location** | Browser or desktop WebView location, where available |
| **GPS on the network?** | A gpsd JSON endpoint; default `127.0.0.1:2947` |
| **Receiver that never moves?** | Fixed latitude and longitude |

For serial sources, configure baud rate and maximum published update rate after selecting the
receiver. The node validates GGA and RMC sentences. The update limit controls publication of fixes;
NMEA receivers send sentences without polling. Browser location requests continuous high-accuracy
updates. **Forget source** returns to the source picker.

Connect `position` to the nodes that need it:

- **ADS-B** uses it as the local CPR position reference.
- **Map** shows the station position, recent route, and visited-location heatmap.
- **Recorder** writes latitude, longitude, altitude, and fix time into SigMF capture segments.

One position output can feed several nodes. The GPS display also shows a six-character Maidenhead
locator. When a source loses its fix, the node reports why and consumers stop using the stale
coordinate. Serial and gpsd sources reconnect automatically.

## Drive a signal survey

Add a **Signal survey** display, wire one Device IQ output and one GPS position output into it,
then choose an offset inside the incoming IQ span and a measurement width. The −25k, −5k, +5k,
and +25k controls move the measured slice relative to the IQ centre; they do not retune the radio.
Start the survey only after the face reports both a spectrum level and a GPS fix.

The map records the peak spectrum level in that width at each new fix. Readings are in dBFS, not
dBm: keep receiver gain, antenna, cable, and measurement width unchanged if you want locations to
be comparable. Nearby fixes are combined into roughly ten-metre cells, so time spent stationary
does not make one place appear stronger. Pause before changing the radio setup, and export the
current cells as CSV when the drive is complete.

## Device identity and reconnection

A device node stores a durable hardware reference, not a temporary engine number. If you unplug a
named receiver, the node stays in place with its wires and settings intact. Plug the same receiver
back in and the workspace binds to it again; it does not silently substitute another device.

**Forget this radio** releases the device and clears that durable reference while preserving the
node and its wires.

## Applying a patch

Editing the graph saves the desired workspace. Applying reconciles that graph with live engine
state: it opens attached devices, restores their settings, creates or updates channels, and removes
live objects that no longer belong to the active patch.

Most edits apply automatically. An explicit **Apply patch** button appears when the desired graph
and live state differ in a way that needs your attention.

## Multiple workspaces and clients

The active workspace lives on the server, so every connected browser sees the same receiver state.
Changing the active workspace, tuning a radio, or applying a template affects other clients too.
Use separate workspaces for different activities, not as private per-browser tabs.

Only one client can edit a particular saved revision successfully. If two clients change the same
workspace at once, the server reports a conflict rather than silently overwriting one operator's
layout.
