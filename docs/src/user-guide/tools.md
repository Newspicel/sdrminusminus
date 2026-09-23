# Tools

**Library → Tools** opens instruments and calculators that are not part of a receiver.

| Tool | Does |
|---|---|
| Antenna calculator | Dimensions for dipoles, folded and inverted-V dipoles, end-fed half-waves, ground planes, 5/8 verticals, J-poles, quad loops, and Yagis |
| NanoVNA | Sweeps an antenna over USB and shows SWR, impedance, and a Smith chart. Calibrates, and exports Touchstone files. |
| Radio programmer | Reads, edits, and writes codeplugs, and copies them between radios |

## Radio programmer

Supported radios: AnyTone AT-D890UV and Radtel RT-4D.

It edits what every radio shares: channels, contacts, group lists, zones, scan lists, and radio
IDs. Everything else in the codeplug is kept byte for byte. When writing, it reads the radio
first and writes back only the blocks that changed.

**Copy to** fits a codeplug to another radio model and stores it as a new one.

The NanoVNA and the radio programmer need the device plugged into the server, not the client.
