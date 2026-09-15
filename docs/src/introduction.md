<p align="center">
  <img src="icon.svg" alt="sdr-- logo" width="96" height="96">
</p>

# Welcome to sdr--

sdr-- is a software-defined radio application. Listen, decode, view, and record signals from an
SDR, a network receiver, or an IQ recording.

Build a receiver by connecting nodes in **Patch** view. Pin frequently used controls and displays
to **Rack** view. Start with an RTL-SDR and a local FM station.

## Get started

1. [Install sdr--](getting-started/install.md).
2. Build [your first receiver](getting-started/first-receiver.md).
3. Learn the [workspace controls](getting-started/workspace.md).

## Find a guide

| Task | Guide |
|---|---|
| Connect a radio | [Radios and hardware](hardware.md) |
| Listen or decode | [Channels and decoding](user-guide/channels.md) |
| Save and replay signals | [Recording and playback](user-guide/recording.md) |
| Operate over a network | [Containers and remote radios](server/deployment.md) |
| Use a phone in the field | [Field mode](user-guide/field-mode.md) |
| Fix a problem | [Troubleshooting](troubleshooting.md) |
| Develop sdr-- | [Build and test](development/building.md) |

## How it runs

The server handles the radio and signal processing. The desktop app and browser provide the same
interface. Run everything on one computer, or place the server near the antenna and connect over
the network. All clients share the active workspace.

sdr-- is under active development. The [channel catalog](user-guide/channels.md#channel-catalog)
lists supported modes, test coverage, and experimental limits.
