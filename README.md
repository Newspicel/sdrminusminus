<p align="center">
  <img src="assets/icon.svg" alt="SDR-- logo" width="96" height="96">
</p>

# SDR--

A software-defined radio application for listening, decoding, and recording. Connect radios,
channels, and displays in **Patch** view, then pin your everyday controls to **Rack** view.

Run the desktop app with a local SDR, or place the server near your antenna and connect through
a browser. Both use the same receiver engine and interface.

Questions or ideas? Join the [Discord](https://discord.gg/dYaRyGwBNw).

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: light)" srcset="assets/screenshots/patch-light.png">
    <img src="assets/screenshots/patch-dark.png" alt="A receiver patch with three channels, a speaker, recording, and network IQ output">
  </picture>
</p>

## Install

Download a desktop installer or portable server from
[GitHub Releases](https://github.com/Newspicel/sdrmm/releases), or
[build](https://sdrmm.com/docs/development/building) it yourself.
The [installation guide](https://sdrmm.com/docs/getting-started/install)
covers macOS, Windows, Linux, Homebrew, APT, DNF, Nix, and Docker.

## Start with an RTL-SDR

1. Plug in the RTL-SDR and pick it on the **Device** node. Set the rate to **2.4 MS/s**.
2. Add a **WFM** channel from **+ Node** and set it to a local FM station.
3. Wire Device `iq` to WFM `iq`, and WFM `audio` to the Speaker.
4. Start the Speaker. Press `p` on a node to pin it to the Rack.

[Your first receiver](https://sdrmm.com/docs/getting-started/first-receiver) walks
through it. [Radios](https://sdrmm.com/docs/hardware) covers other hardware.

## What it supports

- **Listening:** [AM, NFM, WFM with stereo and RDS, SSB](https://sdrmm.com/docs/user-guide/channels), digital voice
  (DMR, P25, NXDN, D-STAR, FreeDV, M17), and [noise reduction](https://sdrmm.com/docs/user-guide/channels#audio-fx).
- **Decoding:** [aircraft, ships, pagers, amateur modes, weather satellites, radiosondes, DAB, DRM,
  and DVB](https://sdrmm.com/docs/user-guide/decoders#catalog).
- **Displays:** [spectrum, waterfalls, maps, decoded messages, and video](https://sdrmm.com/docs/getting-started/workspace).
- **Recording:** [device IQ, channel baseband, audio, and each call](https://sdrmm.com/docs/user-guide/recording),
  with a time machine and SigMF playback.
- **Finding signals:** [scanning, signal identification, band monitoring, and transmitter
  hunting](https://sdrmm.com/docs/user-guide/scanning).
- **Arrays:** [coherent arrays](https://sdrmm.com/docs/user-guide/arrays),
  [direction finding](https://sdrmm.com/docs/user-guide/direction-finding), and
  [passive radar](https://sdrmm.com/docs/user-guide/passive-radar).
- **Location:** [GPS](https://sdrmm.com/docs/user-guide/position), [satellite tracking](https://sdrmm.com/docs/user-guide/satellites),
  and a [propagation map](https://sdrmm.com/docs/user-guide/propagation).
- **Phones:** [iPhone and Android remote](https://sdrmm.com/docs/user-guide/phones) with CarPlay and Android Auto.
- **Tools:** [antenna calculator, NanoVNA, and radio programmer](https://sdrmm.com/docs/user-guide/tools).
- **Automation:** [REST, WebSocket, and MCP](https://sdrmm.com/docs/server/api), plus
  [network IQ, webhooks, MQTT, and databases](https://sdrmm.com/docs/user-guide/network-iq).

## Screenshots

These captures use debug-build signal sources and repository IQ fixtures. Regenerate them with
`cargo xtask screenshots`.

<table>
<tr><th width="50%">Spectrum and waterfall</th><th width="50%">Rack view</th></tr>
<tr><td><picture><source media="(prefers-color-scheme: light)" srcset="assets/screenshots/spectrum-light.png"><img width="100%" src="assets/screenshots/spectrum-dark.png" alt="Spectrum with the tuned channel marked"></picture></td><td><picture><source media="(prefers-color-scheme: light)" srcset="assets/screenshots/rack-light.png"><img width="100%" src="assets/screenshots/rack-dark.png" alt="Three receivers in the rack"></picture></td></tr>
<tr><th width="50%">FT8 decoding</th><th width="50%">Signal identification</th></tr>
<tr><td><picture><source media="(prefers-color-scheme: light)" srcset="assets/screenshots/ft8-light.png"><img width="100%" src="assets/screenshots/ft8-dark.png" alt="Decoded messages from a recorded 20 m FT8 slot"></picture></td><td><picture><source media="(prefers-color-scheme: light)" srcset="assets/screenshots/ident-light.png"><img width="100%" src="assets/screenshots/ident-dark.png" alt="Signal measurements and candidate protocols"></picture></td></tr>
<tr><th width="50%">Aircraft positions</th><th width="50%">Ship positions</th></tr>
<tr><td><picture><source media="(prefers-color-scheme: light)" srcset="assets/screenshots/adsb-light.png"><img width="100%" src="assets/screenshots/adsb-dark.png" alt="ADS-B aircraft and decoder log"></picture></td><td><picture><source media="(prefers-color-scheme: light)" srcset="assets/screenshots/ais-light.png"><img width="100%" src="assets/screenshots/ais-dark.png" alt="AIS position in Hamburg harbour"></picture></td></tr>
<tr><th width="50%">Slow-scan television</th><th width="50%">Amateur television</th></tr>
<tr><td><picture><source media="(prefers-color-scheme: light)" srcset="assets/screenshots/sstv-light.png"><img width="100%" src="assets/screenshots/sstv-dark.png" alt="Robot 36 SSTV picture"></picture></td><td><picture><source media="(prefers-color-scheme: light)" srcset="assets/screenshots/atv-light.png"><img width="100%" src="assets/screenshots/atv-dark.png" alt="625-line ATV test image"></picture></td></tr>
<tr><th width="50%">Pager messages</th><th width="50%">Broadcast FM</th></tr>
<tr><td><picture><source media="(prefers-color-scheme: light)" srcset="assets/screenshots/pocsag-light.png"><img width="100%" src="assets/screenshots/pocsag-dark.png" alt="POCSAG messages with webhook output"></picture></td><td><picture><source media="(prefers-color-scheme: light)" srcset="assets/screenshots/rds-light.png"><img width="100%" src="assets/screenshots/rds-dark.png" alt="RDS station name, text, and alternate frequencies"></picture></td></tr>
</table>

## Documentation and API

- [User and developer guide](https://sdrmm.com/docs/)
- Swagger UI: `/api/docs` on a running server
- OpenAPI: `/api/openapi.json` or [openapi.json](openapi.json)

## Thanks

[KrakenRF](https://www.krakenrf.com), [Airspy](https://airspy.com) and
[AntSDR](https://www.microphase.cn/) provided hardware for development and testing.

Making a radio? Write to [hi@jhaag.me](mailto:hi@jhaag.me) to get it supported and tested.

## License

Copyright (C) 2026 Julian Haag.

Licensed under the [GNU Affero General Public License, version 3 or later](LICENSE).
Need a commercial license or custom work? Write to [hi@jhaag.me](mailto:hi@jhaag.me).
[Third-party notices](THIRD_PARTY_NOTICES.md) and license texts are also available in the app's
About panel.
