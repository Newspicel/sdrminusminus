<p align="center">
  <img src="icon.svg" alt="sdr-- logo" width="96" height="96">
</p>

# Welcome to sdr--

sdr-- receives and decodes radio signals. Connect an SDR, a network receiver, or an IQ recording
to channels and displays on a canvas. Use the rack view for the controls you operate regularly.

The Rust server runs the hardware, signal processing, decoders, and recordings. The desktop app
and browser use the same interface to control it:

```text
SDR or recording → Rust server → desktop app or browser
```

Run the server on your computer or on a separate machine near the antenna. All connected clients
share the active receiver. A built-in signal generator lets you learn the controls without hardware.

## Start here

- [Install sdr--](getting-started/install.md), then build [your first receiver](getting-started/first-receiver.md).
- For a remote installation, read [Configuration and security](server/configuration.md) and
  [Containers and remote radios](server/deployment.md).
- To contribute, start with [Build and test](development/building.md) and
  [Architecture](development/architecture.md).

## Project status

sdr-- is under active development. The [channel catalog](user-guide/channels.md#channel-catalog)
lists supported modes and their maturity. Most decoders are tested with generated fixtures;
that does not establish how well they handle signals from real transmitters.

Nightly builds follow `main` and may change saved-data formats without migration guarantees.
Use stable releases for persistent installations.
