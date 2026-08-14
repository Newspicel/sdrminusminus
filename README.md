<img src="assets/icon.svg" alt="" width="96" height="96">

# sdr--

A modular, client–server software-defined radio receiver.

A Rust server owns the hardware and does all the DSP — channelization, demodulation,
decoding, spectrum, recording. A React client renders what the server describes and never
touches a sample of IQ. The same frontend ships two ways: as a Tauri desktop app that embeds
the server in-process, and as static assets served by the server itself, so a Raspberry Pi on
the roof and a browser on the couch run the identical UI.

## Quickstart

Tagged releases publish `sdrmm` archives (Linux x86_64/aarch64, macOS arm64/x86_64, Windows
x86_64), a multi-arch container image at `ghcr.io/newspicel/sdrminusminus`, and desktop
installers (`.dmg`, `.deb`, `.AppImage`, `.msi`, `.exe`).

The same artifacts are built nightly from `main` and published to the rolling
[`nightly`](https://github.com/Newspicel/sdrminusminus/releases/tag/nightly) prerelease
(`ghcr.io/newspicel/sdrminusminus:nightly`), on the nights `main` actually moved. Unstable by
definition — its version is the build date, `YY.M.D`.

SoapySDR is the canonical local-hardware layer. Desktop installers and containers include a
private SoapySDR 0.8.1 runtime, SoapyRTLSDR, SoapyHackRF, and the curated modules listed in
[`packaging/soapy/environment.yml`](packaging/soapy/environment.yml); do not install SoapySDR
separately for those artifacts. Portable `sdrmm` headless archives compile the same backend but
use the host runtime: install SoapySDR 0.8.1 (module ABI 0.8) plus the matching module first;
the release baseline is SoapyRTLSDR 0.3.3 and SoapyHackRF 0.3.4. SDRplay RSP receivers are
supported through SoapySDRPlay3 0.5.2 after installing SDRplay API 3.15 or newer; the proprietary
API and its module are not bundled. See the [hardware guide](docs/src/hardware.md#sdrplay).

To build from source you need the pinned nightly Rust toolchain (`rust-toolchain.toml` —
`rustup` installs it on the first build), Node 26, pnpm 11, and SoapySDR 0.8 development files:
`libsoapysdr-dev` on Debian/Ubuntu or `brew install soapysdr` on macOS. A deliberate
virtual/network-only build remains available with `--no-default-features --features net-client`.

```sh
git clone https://github.com/Newspicel/sdrminusminus
cd sdrminusminus
pnpm --dir web install --frozen-lockfile
pnpm --dir web build          # the server embeds these assets
cargo run -p sdrmm            # serves on 0.0.0.0:8080
```

Open <http://localhost:8080>, open the **Signal Generator (virtual)** device, and add an NFM
channel at +300 kHz — you will hear a 1 kHz tone without owning a radio.

For UI work use `cargo xtask dev` instead: it runs the server plus a Vite dev server with HMR
on <http://localhost:5173>, proxying `/api` and the WebSocket to the server.

## Commands

`cargo xtask` is the entry point for everything; every gate CI runs is runnable locally first.

| Command | Does |
|---|---|
| `cargo xtask dev` | Server + Vite dev server with HMR |
| `cargo xtask codegen` | Regenerate `openapi.json` + `web/src/generated` (run after changing `crates/wire`) |
| `cargo xtask check` | The full gate: formatting, frontend lint/typecheck, clippy, minimal and Soapy release-shaped builds, web build, codegen drift |
| `cargo xtask test` | Rust + web test suites (uses `device-virtual`, no hardware) |
| `cargo xtask smoke` | The Playwright browser flow against the real binary (needs `pnpm --dir web exec playwright install chromium`) |
| `cargo xtask audit` | Check the dependency graph against RustSec (`deny.toml`; needs `cargo install --locked cargo-deny`) |
| `cargo xtask fixtures` | Regenerate the synthesized SigMF decoder fixtures in `fixtures/` |
| `cargo xtask licenses` | Re-harvest the third-party notices from the lockfiles (run after changing a dependency) |
| `cargo xtask icons` | Re-render every icon (favicons, desktop `.ico`/`.icns`) from `assets/icon.svg` |
| `cargo xtask dist [--target <triple>]` | The release archive for that target into `dist/` — exactly what the release pipeline uploads |
| `cargo xtask desktop [--bundles <list>]` | The Tauri shell: compile gate by default, installers with `--bundles` (needs `cargo install --locked tauri-cli`) |
| `cargo xtask soapy-bundle-check` | Assert a staged desktop runtime contains the core, RTL-SDR/HackRF modules, and notices |
| `cargo xtask set-version <semver>` | Stamp a release version across the workspace; the release pipeline runs this from the tag |

## Documentation

- Full documentation: <https://newspicel.github.io/sdrminusminus/> (sources in `docs/`,
  built with mdBook)
- API reference: `/api/docs` on a running server (Swagger UI over the generated OpenAPI
  document)

## License

MIT — see [`LICENSE`](LICENSE).

Third-party components are listed in [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md), which
`cargo xtask licenses` regenerates from the lockfiles. Their full license texts ship inside the
binary and are readable in the app's About panel; installers additionally carry each bundled
hardware package's own texts in `soapy/licenses`.

Two components need more than their SPDX id. `codec2` is LGPL-2.1-only and statically linked —
publishing sdr--'s complete source is what satisfies the relink right the LGPL reserves for
users. `librtlsdr` and `libhackrf` are GPL-2.0-or-later and shipped in installers as SoapySDR
modules, loaded at runtime through SoapySDR's own plugin API rather than linked, so the GPL
applies to those libraries and not to this product.
