<p align="center">
  <img src="assets/icon.svg" alt="sdr-- logo" width="96" height="96">
</p>

# sdr--

A modular software-defined radio receiver for the desktop, the browser, and small remote servers.

sdr-- keeps radio hardware and real-time DSP in a Rust server while a React interface handles
tuning, visualization, and control. Run both together as a desktop app, serve the same interface
from a Raspberry Pi or home server, or connect directly to `rtl_tcp` and SpyServer receivers.

## What it can do

- Build a receiver visually from device, channel, display, scanner, recorder, and output nodes.
- Listen to AM, narrowband FM, broadcast FM, and SSB.
- Decode ADS-B, AIS, APRS/AX.25, POCSAG, ACARS, NAVTEX, RTTY, Morse, DCF77/WWVB/MSF/JJY radio
  clocks, educational GPS L1 C/A acquisition and NAV telemetry, sub-GHz frames, and several
  digital voice modes.
- Acquire DAB/DAB+, narrow-band DVB-S/S2 DATV, and DRM30/DRM+ carriers with lock, SNR, and
  frequency-error diagnostics.
- Receive SSTV in twelve scanning modes, watch each picture build up line by line, and keep every
  one that arrives in the server's picture store.
- Display live spectrum and waterfall views, decoded readouts, position maps, logs, and ATV video.
- Scan frequency ranges, save workspaces and presets, search regional band plans, and record IQ as
  SigMF for later playback.
- Automate the receiver through a typed REST API, WebSocket events, OpenAPI, or MCP.

The built-in signal generator means you can explore the complete receive path without owning an
SDR.

## Get started

Download the desktop installer or portable server for your platform from
[GitHub Releases](https://github.com/Newspicel/sdrminusminus/releases). Nightly builds are
available from the rolling [nightly release](https://github.com/Newspicel/sdrminusminus/releases/tag/nightly).

To try the server with Docker:

```sh
docker compose up -d
```

Open <http://localhost:8080>. In the starter workspace, choose **Signal Generator (virtual)** on
the Device node. The existing Scope will immediately show synthetic signals. Add an NFM channel,
wire the Device's IQ output to it, wire its audio output to the Speaker, and tune the channel to
`+300 kHz` for a 1 kHz test tone.

For a real receiver, choose it instead of the signal generator. Desktop installers and containers
bundle SoapySDR support for RTL-SDR, HackRF, Airspy/AirspyHF, bladeRF, LimeSDR, PlutoSDR, and
SoapyRemote. See the
[hardware guide](https://newspicel.github.io/sdrminusminus/hardware.html) for setup and USB
troubleshooting.

### Nix

On NixOS or another Linux system with flakes enabled, install the Tauri desktop application
directly from GitHub:

```sh
nix --extra-experimental-features 'nix-command flakes' \
  profile install github:Newspicel/sdrminusminus
sdrmm-desktop
```

The flake supports x86_64 and aarch64 Linux. It does not bundle SoapySDR hardware modules; NixOS
users select those in their system configuration and enable the corresponding device permissions.
See the [installation guide](https://newspicel.github.io/sdrminusminus/getting-started/install.html#nix)
for an RTL-SDR and HackRF example.

## Build from source

You need the pinned Rust toolchain, a C/C++ compiler, CMake, Node 26, pnpm 11, and SoapySDR 0.8
development files. On Debian or Ubuntu, install `build-essential cmake libsoapysdr-dev`; on macOS,
run `xcode-select --install` and `brew install cmake soapysdr`.

```sh
git clone https://github.com/Newspicel/sdrminusminus.git
cd sdrminusminus
pnpm --dir web install --frozen-lockfile
pnpm --dir web build
cargo run -p sdrmm
```

Then open <http://localhost:8080>. For development, `cargo xtask dev` runs the auto-reloading Rust
server and a Vite dev server with hot reload at <http://localhost:5173>.

A virtual and network-only build does not require SoapySDR:

```sh
cargo run -p sdrmm --no-default-features --features net-client
```

## Project layout

| Path | Purpose |
|---|---|
| `apps/sdrmm` | Headless server binary with the web UI embedded |
| `apps/desktop` | Tauri desktop shell around the same server and UI |
| `crates/engine` | Real-time device, DSP, channel, scanner, and recording orchestration |
| `crates/channels` | Demodulators and protocol decoders |
| `crates/device-*` | SoapySDR, network, and virtual device backends |
| `crates/wire` | Shared REST, WebSocket, settings, and generated-client types |
| `crates/server` | HTTP, WebSocket, MCP, persistence, and embedded frontend |
| `web` | React application |
| `docs` | mdBook documentation |

## Development commands

`cargo xtask` is the local entry point for the same gates used in CI.

| Command | Purpose |
|---|---|
| `cargo xtask dev` | Run the auto-reloading server and frontend dev server |
| `cargo xtask check` | Format, lint, type-check, build, and check generated-code drift |
| `cargo xtask test` | Run Rust and frontend tests without real hardware |
| `cargo xtask smoke` | Run the Playwright flow against the real server binary |
| `cargo xtask codegen` | Regenerate OpenAPI and TypeScript API types |
| `cargo xtask audit` | Check dependencies with `cargo-deny` |
| `cargo xtask fixtures` | Regenerate synthesized decoder fixtures |
| `cargo xtask licenses` | Regenerate third-party notices |
| `cargo xtask dist` | Build a portable server archive for the current target |
| `cargo xtask desktop` | Check or bundle the Tauri desktop app |

See the [development guide](https://newspicel.github.io/sdrminusminus/development/building.html)
for prerequisites, architecture, testing, and release workflows.

## Documentation and API

- [User and developer guide](https://newspicel.github.io/sdrminusminus/)
- [Contribution guide](CONTRIBUTING.md)
- [Feature roadmap](FEATURES.md)
- Swagger UI at `/api/docs` on any running server
- Generated OpenAPI document at `/api/openapi.json` or [in the repository](openapi.json)

## License

Copyright (C) 2026 sdr-- contributors.

sdr-- is free software licensed under the [GNU General Public License, version 3 or
later](LICENSE). Distributed dependencies and bundled hardware components are documented in
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md); their complete license texts are also available
from the app's About panel.
