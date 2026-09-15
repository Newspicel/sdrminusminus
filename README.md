<p align="center">
  <img src="assets/icon.svg" alt="sdr-- logo" width="96" height="96">
</p>

# sdr--

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
The [installation guide](https://newspicel.github.io/sdrminusminus/getting-started/install.html)
covers macOS, Windows, Linux, Homebrew, Nix, and Docker.

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
[authentication and HTTPS](https://newspicel.github.io/sdrminusminus/server/configuration.html).

## Start with an RTL-SDR

1. Attach an antenna and plug the RTL-SDR into the computer running sdr--.
2. Select it on the **Device** node. Set the sample rate to **2.4 MS/s** and tune to a local FM station.
3. Add a **WFM** channel from **+ Node** and set it to the station's frequency.
4. Connect Device `IQ` to WFM `IQ`, then WFM `audio` to Speaker `audio`.
5. Start playback on the Speaker. Select a node and press `p` to pin it to the Rack.

[Your first receiver](https://newspicel.github.io/sdrminusminus/getting-started/first-receiver.html)
walks through tuning, gain, audio, and RDS. See the
[hardware guide](https://newspicel.github.io/sdrminusminus/hardware.html) for other receivers and
package-specific driver requirements.

## What it supports

- **Listening:** AM, NFM, broadcast FM with stereo and RDS, SSB, and digital voice.
- **Decoding:** aircraft, ships, amateur radio, pagers, sensors, images, and more.
- **Displays:** spectrum, waterfalls, maps, decoded messages, and video.
- **Recording:** device IQ, channel baseband, and audio, with SigMF playback.
- **Radio tools:** scanning, signal identification, coherent arrays, direction finding, and passive radar.
- **Automation:** REST, WebSocket, MCP, network IQ export, and event forwarding.

sdr-- is under active development. The
[channel catalog](https://newspicel.github.io/sdrminusminus/user-guide/channels.html#channel-catalog)
lists each mode's test coverage and limitations, including partial experimental decoders.

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

Use the repository's pinned Rust toolchain, a C/C++ compiler, CMake, Node 26, and pnpm 11.

```sh
git clone https://github.com/Newspicel/sdrminusminus.git
cd sdrminusminus
pnpm --dir web install --frozen-lockfile
pnpm --dir web build
cargo run -p sdrmm
```

Open <http://localhost:8080>. For frontend hot reload and backend watching, run
`cargo xtask dev --watch` and open <http://localhost:5173>.

The signal generator and synthetic test radios are available in debug builds. Release builds
support real receivers and recording playback.

| Command | Purpose |
|---|---|
| `cargo xtask check` | Format, lint, type-check, build, and check generated files |
| `cargo xtask test` | Rust and frontend tests without hardware |
| `cargo xtask smoke` | Browser tests against the server |
| `cargo xtask codegen` | Generate OpenAPI and TypeScript types |
| `cargo xtask audit` | Dependency checks |

Read [Contributing](CONTRIBUTING.md), the
[build guide](https://newspicel.github.io/sdrminusminus/development/building.html), and
[architecture](https://newspicel.github.io/sdrminusminus/development/architecture.html)
for prerequisites, crate boundaries, and tests.

## Documentation and API

- [User and developer guide](https://newspicel.github.io/sdrminusminus/)
- Swagger UI: `/api/docs` on a running server
- OpenAPI: `/api/openapi.json` or [openapi.json](openapi.json)

## License

Copyright (C) 2026 sdr-- contributors.

Licensed under the [GNU General Public License, version 3 or later](LICENSE).
[Third-party notices](THIRD_PARTY_NOTICES.md) and license texts are also available in the app's
About panel.
