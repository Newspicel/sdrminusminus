<p align="center">
  <img src="icon.svg" alt="sdr-- logo" width="96" height="96">
</p>

# Welcome to sdr--

sdr-- is a modular software-defined radio receiver. It turns an SDR, a network receiver, or a
recording into a visual signal-processing workspace you can operate from a desktop app or web
browser.

The project separates the real-time radio work from the interface:

```text
SDR or recording → Rust DSP server → REST, WebSocket, and MCP → desktop app or browser
```

The server owns hardware access, tuning, channelization, demodulation, decoding, spectrum,
scanning, and recording. The client renders the server's capabilities and connects those pieces
on a patch canvas. This design lets a small computer sit beside the antenna while you operate it
from somewhere more comfortable.

## Highlights

- **Visual receiver building.** Connect a radio to channels, scopes, speakers, maps, logs,
  recorders, scanners, and exports.
- **Useful on the first launch.** The built-in signal generator exercises the entire receive
  path without radio hardware.
- **Analog and digital reception.** Listen to common analog modes and decode aviation, marine,
  amateur, paging, telemetry, sub-GHz, video, and digital voice signals.
- **Repeatable setups.** Workspaces save the whole bench; templates configure common activities;
  presets and bookmarks capture settings you want to reuse.
- **Record once, inspect again.** Capture device IQ as SigMF and reopen it as a source through the
  same processing graph.
- **Automation-ready.** A typed REST API, WebSocket event stream, generated OpenAPI document, and
  MCP server expose the same engine used by the interface.

## Choose a path

If this is your first time using sdr--, start with [Install sdr--](getting-started/install.md) and
[Your first receiver](getting-started/first-receiver.md).

If you are deploying a receiver beside an antenna, read
[Configuration and security](server/configuration.md) and
[Containers and remote radios](server/deployment.md).

If you want to contribute, begin with [Build and test](development/building.md), then read the
[architecture guide](development/architecture.md).

## Project status

sdr-- is under active development. Nightly builds track the latest `main` branch and can change
without migration guarantees. Stable releases are the better choice for saved stations and
unattended deployments. The repository's [feature roadmap](https://github.com/Newspicel/sdrminusminus/blob/main/FEATURES.md)
distinguishes shipped work from future ideas.

Always follow the radio regulations that apply where you operate, especially around restricted
traffic, recording, and transmission. sdr-- currently focuses on reception.
