# Nodes and wires

Everything in SDR-- is a node, and every flow between nodes is a wire you can see. The set of
nodes, wires, and settings is a **workspace**. It lives on the server and every client shares it.

## Patch and Rack

**Patch** shows every node and wire. Use it to build and change a receiver.

**Rack** shows only the nodes you pinned. Use it for everyday operation. Select a node and press
`p` to pin or unpin it, and `v` to switch views. Moving a node in the Rack leaves its wires alone.

## Ports

A wire joins an output port to an input port that carries the same kind of data:

| Port | Carries | Typical wire |
|---|---|---|
| `iq` | Raw radio samples | Device → channel, Scope, recorder |
| `audio` | Demodulated sound | Channel → Audio FX, Speaker, Audio recorder |
| `events` | Decoded messages | Channel → Readout, Decoder log, Map |
| `baseband` | One channel's filtered IQ | Channel → Baseband scope, recorder, Network IQ |
| `video` | Pictures and video | Channel → Video |
| `control` | Tuning commands | Scanner, Signal hunt, Satellite → channel |
| `position` | Station location and heading | GPS position → Map, Array, Satellite, ADS-B |
| `array` | Aligned lanes of an antenna array | Array → Direction finder, Beamformer, Passive radar |

## Node types

| Group | Nodes |
|---|---|
| Sources | Device, Recording, Signal generator, GPS position |
| Decoders | AM, NFM, WFM, ADS-B, DMR, and every other [decoder](../user-guide/decoders.md), by family |
| Tools | Scanner, Signal hunt, Satellite, Spectrum monitor, DMR trunk system, Event filter, Audio FX, Array, Direction finder, Beamformer, Passive radar, Stitch, Spatial spectrum, Correlator, Polarimeter, Triangulation |
| Outputs | Scope, Baseband scope, Speaker, Readout, Decoder log, Map, Video, Signal survey, Propagation map, Recorder, Audio recorder, Baseband recorder, Time machine, Network IQ, Event output, Export |

**+ Add** lists what the running server offers. To add a node at the cursor, double-click the
canvas, or right-click it and pick **Add node here**. Hover an entry to see what it does.

## Devices and channels

A **Device** opens one radio. A **channel** decodes one frequency from the Device's IQ.

You tune channels, not the radio. The Device moves its window to cover as many wired channels as
its sample rate can hold, and its header counts them: `5/5` is green, `3/5` yellow, `0/5` red. See
[Tuning](../user-guide/channels.md#tuning).

A Device node remembers which radio it holds. Unplug it and the node, wires, and settings stay.
Plug the same radio back in and it reconnects, or press **Open radio**. **Forget radio** frees the
node for another one.

## Changes apply live

Edits apply as you make them: radios open, settings restore, channels update, and anything the
workspace no longer uses closes. A node that could not apply says why on its face.

Tuning, switching workspaces, and applying templates affect every connected client. If two
clients edit the same revision at once, the second gets a conflict instead of overwriting the
first. See [Workspaces and presets](../user-guide/workspaces.md) for saving, undo, and sharing.
