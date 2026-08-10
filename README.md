# sdr--

A modular, client–server software-defined radio receiver.

A Rust server owns the hardware and does all the DSP — channelization, demodulation,
decoding, spectrum, recording. A React client renders what the server describes and never
touches a sample of IQ. The same frontend ships two ways: as a Tauri desktop app that embeds
the server in-process, and as static assets served by the server itself, so a Raspberry Pi on
the roof and a browser on the couch run the identical UI.

Every wire type is defined once, in Rust (`crates/wire`), and the TypeScript is generated
from the resulting OpenAPI document. There are no hand-written frontend DTOs.

`PLAN.md` is the source of truth for architecture and scope; `PROGRESS.md` records what is
actually built.

## Hardware

| Device | How |
|---|---|
| RTL-SDR | Native pure-Rust backend (tuner gain table, bias tee, tuner AGC), or SoapySDR |
| HackRF | Native pure-Rust backend (per-stage LNA/VGA gain, RF amp, antenna power), or SoapySDR |
| Airspy, SDRplay, LimeSDR, PlutoSDR, BladeRF, USRP, … | SoapySDR, wherever a Soapy module exists |
| Recordings and the built-in signal generator | `device-virtual`, always compiled in, no hardware needed |

## Quickstart

Tagged releases publish `sdrmm` tarballs (linux x86_64/aarch64, macOS arm64), a multi-arch
container image at `ghcr.io/newspicel/sdrminusminus` and desktop bundles.

To build it yourself you need the pinned nightly Rust toolchain (`rust-toolchain.toml` —
`rustup` installs it on the first build), Node 24 and pnpm 11. SoapySDR is only needed for the
default build: `libsoapysdr-dev` on Debian/Ubuntu, `brew install soapysdr` on macOS; build
with `--no-default-features --features rtl-native,hackrf-native` to skip it entirely — that is
the shape release artifacts ship in, and it needs no C library at all.

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
| `cargo xtask check` | The full gate: fmt, clippy `-D warnings`, Soapy-free build, the release-shaped native build, `biome ci`, type-aware `oxlint`, `tsgo`, web build, codegen-drift |
| `cargo xtask test` | Rust + web test suites (uses `device-virtual`, no hardware) |
| `cargo xtask fixtures` | Regenerate the synthesized SigMF decoder fixtures in `fixtures/` |
| `cargo xtask dist [--target <triple>]` | Build the self-contained release binary (web build, then `--no-default-features --features rtl-native,hackrf-native`) into `dist/` |

## Documentation

- Full documentation: <https://newspicel.github.io/sdrminusminus/> (sources in `docs/`,
  built with mdBook)
- API reference: `/api/docs` on a running server (Swagger UI over the generated OpenAPI
  document)

## License

MIT — see [`LICENSE`](LICENSE).
