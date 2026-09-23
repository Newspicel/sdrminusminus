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
| `audio` | Demodulated sound | Channel → Speaker, Audio recorder |
| `events` | Decoded messages | Channel → Readout, Decoder log, Map |
| `baseband` | One channel's filtered IQ | Channel → Baseband scope, recorder, Network IQ |
| `video` | Pictures and video | Channel → Video |
| `control` | Tuning commands | Scanner, Satellite → channel |
| `position` | Station location | GPS position → Map, Recorder, ADS-B |

## Node types

| Group | Nodes |
|---|---|
| Sources | Device, Recording, Signal generator, GPS position |
| Decoders | AM, NFM, WFM, ADS-B, DMR, and every other [decoder](../user-guide/decoders.md) |
| Tools | Array, Scanner, Signal hunt, Spectrum monitor, Satellite, DMR trunk system, Event filter, Direction finder, Triangulation, Passive radar, Combiner |
| Outputs | Scope, Baseband scope, Speaker, Readout, Decoder log, Map, Video, Signal survey, Propagation map, recorders, Network IQ, Event output, Export |

**+ Add** lists what the running server offers. Double-click or right-click the canvas to add a
node at the cursor. Hover an entry to see what it does.

## How a Device and its channels share a radio

A **Device** opens one radio. A **channel** decodes one frequency from the Device's IQ.

By default a Device tunes itself. It places its window over as many wired channels as its sample
rate can hold, and keeps its own DC spike off them. The Device header counts how many it covers:
`5/5` is green, `3/5` yellow, `0/5` red. To cover more, raise the sample rate or move some channels
to another radio.

To tune by hand, press the radar button on the Device or just turn its dial. The Device then stays
put, and channels outside its window wait until it covers them again.

A channel wired to several Devices runs on whichever one hears it, and names that radio on its
face. See [Channels](../user-guide/channels.md#which-radio-hears-a-channel).

## Radios come back

A Device node remembers which radio it holds. Unplug it and the node, wires, and settings stay.
Plug the same radio back in and it reconnects. **Forget this radio** frees the node for another
one.

## Applying changes

Edits apply on their own. Applying opens radios, restores settings, updates channels, and closes
anything the workspace no longer uses. If a node says the saved layout and the running receiver
differ, press **Apply patch**.

## Shared by everyone

Tuning, switching workspaces, and applying templates affect every connected client. If two
clients edit the same revision at once, the second gets a conflict instead of overwriting the
first. See [Workspaces and presets](../user-guide/workspaces.md) for saving, undo, and sharing.
