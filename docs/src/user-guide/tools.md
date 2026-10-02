# Tools

**Library → Tools** opens instruments and calculators that are not part of a receiver.

| Tool | Does |
|---|---|
| Antenna calculator | Dimensions for dipoles, folded and inverted-V dipoles, end-fed half-waves, ground planes, 5/8 verticals, J-poles, quad loops, and Yagis |
| NanoVNA | Sweeps an antenna over USB and shows SWR, impedance, and a Smith chart. Calibrates, and exports `.s1p` and `.s2p` files. |
| Radio programmer | Reads, merges, and writes codeplugs, and copies them between radios |

## Radio programmer

Supported radios: AnyTone AT-D890UV and Radtel RT-4D.

It handles what both radios share: channels, zones, contacts, group lists, scan lists, and radio
IDs. **Write to radio** reads the radio first and writes back only the blocks that differ, so the
rest of the codeplug stays as it was.

**Copy for that radio** fits a codeplug to the model picked under **Copy to** and stores it as a
new one. **Merge in** takes entries from another stored codeplug.

The NanoVNA and the radio programmer need the device plugged into the server, not the client.
