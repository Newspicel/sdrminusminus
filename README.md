<p align="center">
  <img src="assets/icon.svg" alt="SDR-- logo" width="96" height="96">
</p>

# SDR--

A software-defined radio application for listening, decoding, and recording. Connect radios,
channels, and displays in **Patch** view, then pin your everyday controls to **Rack** view.

Run the desktop app with a local SDR, or place the server near your antenna and connect through
a browser. Both use the same receiver engine and interface.

<p align="center">
  <img src="assets/screenshots/patch.png" alt="A receiver patch with three channels, a speaker, recording, and network IQ output">
</p>

## Install

Download a desktop installer or portable server from
[GitHub Releases](https://github.com/Newspicel/sdrminusminus/releases).
The [installation guide](https://sdrmm.newspicel.dev/getting-started/install.html)
covers macOS, Windows, Linux, Homebrew, WinGet, APT, DNF, AUR, Nix, and Docker.

On macOS:

```sh
brew tap newspicel/tap
brew install --cask sdrminusminus
```

For a headless server on macOS or Linux, install `sdrmm` from the same tap:

```sh
brew install sdrmm
brew services start sdrmm
```

Open <http://localhost:8080>. For remote access, configure
[authentication and HTTPS](https://sdrmm.newspicel.dev/server/configuration.html).

## Start with an RTL-SDR

1. Plug in the RTL-SDR and pick it on the **Device** node. Set the rate to **2.4 MS/s**.
2. Add a **WFM** channel from **+ Node** and set it to a local FM station.
3. Wire Device `iq` to WFM `iq`, and WFM `audio` to the Speaker.
4. Start the Speaker. Press `p` on a node to pin it to the Rack.

[Your first receiver](https://sdrmm.newspicel.dev/getting-started/first-receiver.html) walks
through it. [Radios](https://sdrmm.newspicel.dev/hardware.html) covers other hardware.

## What it supports

- **Listening:** AM, NFM, broadcast FM with stereo and RDS, SSB, and digital voice.
- **Decoding:** aircraft, ships, amateur radio, pagers, sensors, images, and more.
- **Displays:** spectrum, waterfalls, maps, decoded messages, and video.
- **Recording:** device IQ, channel baseband, and audio, with SigMF playback.
- **Radio tools:** scanning, signal identification, coherent arrays, direction finding, and passive radar.
- **Automation:** REST, WebSocket, MCP, network IQ export, and event forwarding.

SDR-- is under active development. The
[decoder catalog](https://sdrmm.newspicel.dev/user-guide/decoders.html#catalog) shows how well
each mode is tested.

## Screenshots

These captures use debug-build signal sources and repository IQ fixtures. Regenerate them with
`cargo xtask screenshots`.

| Spectrum and waterfall | Rack view |
|---|---|
| ![Spectrum with the tuned channel marked](assets/screenshots/spectrum.png) | ![Three receivers in the rack](assets/screenshots/rack.png) |

| FT8 decoding | Signal identification |
|---|---|
| ![Decoded messages from a recorded 20 m FT8 slot](assets/screenshots/ft8.png) | ![Signal measurements and candidate protocols](assets/screenshots/ident.png) |

| Aircraft positions | Ship positions |
|---|---|
| ![ADS-B aircraft and decoder log](assets/screenshots/adsb.png) | ![AIS position in Hamburg harbour](assets/screenshots/ais.png) |

| Slow-scan television | Amateur television |
|---|---|
| ![Robot 36 SSTV picture](assets/screenshots/sstv.png) | ![625-line ATV test image](assets/screenshots/atv.png) |

| Pager messages | Broadcast FM |
|---|---|
| ![POCSAG messages with webhook output](assets/screenshots/pocsag.png) | ![RDS station name, text, and alternate frequencies](assets/screenshots/rds.png) |

## Build and contribute

```sh
git clone https://github.com/Newspicel/sdrminusminus.git
cd sdrminusminus
python3 scripts/build-media.py
export FFMPEG_DIR="$(python3 scripts/build-media.py --print-prefix)"
pnpm --dir web install --frozen-lockfile
pnpm --dir web build
cargo run -p sdrmm
```

Open <http://localhost:8080>, or run `cargo xtask dev --watch` and open <http://localhost:5173>
for hot reload. `cargo xtask check` and `cargo xtask test` are the main gates.

The [build guide](https://sdrmm.newspicel.dev/development/building.html) lists prerequisites and
every check. Read [Contributing](CONTRIBUTING.md) and the
[architecture](https://sdrmm.newspicel.dev/development/architecture.html) before a pull request.

## Documentation and API

- [User and developer guide](https://sdrmm.newspicel.dev/introduction.html)
- Swagger UI: `/api/docs` on a running server
- OpenAPI: `/api/openapi.json` or [openapi.json](openapi.json)

## License

Copyright (C) 2026 Julian Haag.

Licensed under the [GNU General Public License, version 3 or later](LICENSE).
[Third-party notices](THIRD_PARTY_NOTICES.md) and license texts are also available in the app's
About panel.
