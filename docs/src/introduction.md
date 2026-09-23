<p align="center">
  <img src="icon.svg" alt="SDR-- logo" width="96" height="96">
</p>

# Welcome to SDR--

SDR-- listens to, decodes, and records radio signals from an SDR, a network receiver, or an IQ
recording.

You build a receiver by wiring nodes together in **Patch** view, then pin the controls you use
to **Rack** view. An RTL-SDR and a local FM station are enough to start.

## Start here

1. [Install SDR--](getting-started/install.md).
2. Build [your first receiver](getting-started/first-receiver.md).
3. Learn how [nodes and wires](getting-started/workspace.md) fit together.

## Find a guide

| Task | Guide |
|---|---|
| Connect a radio | [Radios](hardware.md) |
| Listen to a signal | [Channels](user-guide/channels.md) |
| Decode data | [Decoders](user-guide/decoders.md) |
| Save and replay signals | [Recording and playback](user-guide/recording.md) |
| Run the radio somewhere else | [Deployment](server/deployment.md) |
| Use a phone in the field | [Field mode](user-guide/field-mode.md) |
| Fix a problem | [Troubleshooting](troubleshooting.md) |
| Work on SDR-- | [Build and test](development/building.md) |

## How it runs

A server talks to the radio and does all signal processing. The desktop app and the browser
show the same interface on top of it. Run both on one computer, or put the server next to the
antenna and connect over the network. Every connected client sees the same workspace.

SDR-- is under active development. The [decoder catalog](user-guide/decoders.md#catalog) shows
how well each mode is tested.
