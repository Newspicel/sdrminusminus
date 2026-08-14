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
| Sources | Device | A local SDR, network receiver, virtual generator, or recording |
| Channels | AM, NFM, WFM, SSB, decoders | Select and process one signal from device IQ |
| Displays | Scope, Map, Readout, Decoder log, Video | Visualize spectrum or channel output |
| Sinks | Speaker, Recorder, Export | Play audio, save IQ, or export decoded rows |
| Features | Scanner | Drive a device through a frequency range |

The server supplies the palette and channel catalog. If a build gains or loses a backend or
channel type, the interface follows it rather than maintaining a second hard-coded catalog.

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
