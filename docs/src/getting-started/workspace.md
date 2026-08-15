# Understand the workspace

A workspace is a saved receiver bench. Its patch records which devices and processing nodes you
want, how they are connected, where they are placed, which faces are pinned to the rack, and which
regional band plan is active.

## Patch and rack

The **Patch** view is where you build and troubleshoot signal flow. Ports are typed, so the canvas
only permits meaningful connections: IQ feeds channels, scopes, and recorders; audio feeds a
speaker; decoder events feed maps, readouts, logs, and exports; scanner control drives a device.

The **Rack** view is an operating surface for the nodes you use most. Select a node and press `p`
to pin or unpin it. Moving or resizing a face on the rack does not change its signal connections.

## Node types

| Group | Nodes | Role |
|---|---|---|
| Sources | Device, GPS position (device, GPSD, or NMEA serial) | Radio IQ or a live station position |
| Channels | AM, NFM, WFM, SSB, decoders | Select and process one signal from device IQ |
| Displays | Scope, Map, Signal survey, Readout, Decoder log, Video | Visualize spectrum, position, or channel output |
| Sinks | Speaker, Recorder, Export | Play audio, save IQ, or export decoded rows |
| Features | Scanner | Drive a device through a frequency range |

The server supplies the palette and channel catalog. If a build gains or loses a backend or
channel type, the interface follows it rather than maintaining a second hard-coded catalog.

## Live position wiring

Position is a typed stream in the patch, not a workspace setting. Add a **GPS position** node,
choose its position source, and wire its **position** output only to the consumers that need it:

- **Device GPS** uses the desktop WebView's location provider and appears in the palette only when
  that provider exists. The application requests high-accuracy, continuously updated fixes.
- **GPSD** connects to a gpsd JSON endpoint. The default is `127.0.0.1:2947`; edit the address on
  the node when gpsd runs elsewhere.
- **NMEA serial** lists the serial devices detected on the machine running the `sdrmm` server and
  reads checked GGA and RMC sentences from the selected device. A manual path is still accepted
  for devices the operating system does not enumerate. Baud and the maximum live update rate are
  configurable; the rate limits published fixes because NMEA receivers push sentences rather
  than being polled.
  The device is on the server machine, not the machine displaying a remote browser.

An ADS-B position input supplies the moving local CPR reference without writing each fix into its
channel settings. A map position input draws the current station, its bounded route, and a heat
map of the places visited. The GPS face shows the current six-character Maidenhead grid locator.
A recorder position input writes latitude, longitude, altitude, and fix time into SigMF capture
segments while IQ is recorded. One GPS output can fan out to all of these consumers.

When a provider loses its fix, its node reports the reason and consumers stop using the previous
coordinate. Reconnects are automatic for gpsd and NMEA serial sources.

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
