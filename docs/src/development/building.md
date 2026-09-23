# Build and test

CI and local development run the same `cargo xtask` commands.

## Prerequisites

| Tool | Version |
|---|---|
| Rust | Pinned in `rust-toolchain.toml`; rustup installs it |
| Node | 26 |
| pnpm | 11, exact version in `web/package.json` |
| Native | C/C++ compiler, Clang/libclang, CMake, GNU Make, NASM, Python 3.12+ |

```sh
sudo apt-get install -y build-essential cmake clang libclang-dev python3 nasm   # Debian, Ubuntu
brew install cmake python nasm                                                  # macOS
```

The workspace needs the pinned nightly for `-Zpolonius=next`. SoapySDR loads at runtime, so no
development package is needed.

## Build and run

```sh
git clone https://github.com/Newspicel/sdrminusminus.git
cd sdrminusminus
python3 scripts/build-media.py
export FFMPEG_DIR="$(python3 scripts/build-media.py --print-prefix)"
pnpm --dir web install --frozen-lockfile
pnpm --dir web build
cargo run -p sdrmm
```

Open <http://localhost:8080>.

The media script builds the few FFmpeg 9.0.1 codecs SDR-- needs, from checksummed source, into
`.media/<target>`. Keep `FFMPEG_DIR` set for every Cargo command. For cross builds, pass
`--target <triple>` to the script. Nix uses its own FFmpeg.

The server embeds `web/dist`, so build the frontend first. Without it, backend-only builds get a
placeholder page.

For hot reload of the frontend and automatic backend restarts:

```sh
cargo xtask dev --watch
```

Open <http://localhost:5173>. Vite forwards API and WebSocket traffic to port 8080.

### Windows

Build from a Visual Studio developer shell with LLVM and MSYS2 Make installed; the CI media action
shows the setup. Set `MEDIA_SHELL_BIN` to the folders holding `bash`, `make`, and `clang-cl`. The
script adds them only for the tools it runs, so MSYS2's `link.exe` never hides the MSVC linker.

ARM64 builds the codecs without assembly, because FFmpeg's ARM assembler tools do not ship with
the toolchain. xtask retries Cargo up to three times on Windows, since the
`ffmpeg-sys-the-third` build script sometimes crashes loading libclang
([issue 145](https://github.com/shssoichiro/ffmpeg-the-third/issues/145)).

## Feature flags

The server enables `soapy`, `sdrplay`, `cr8`, `rtlsdr`, `hackrf`, `airspy`, `airspyhf`, `ad936x`,
`net-client`, and `gpu-fft` by default. Packaged releases use a subset.

```sh
cargo run -p sdrmm --no-default-features                        # no radio drivers
cargo run -p sdrmm --no-default-features --features net-client  # network radios only
```

Recording playback and the signal generator work in every build.

## Test without a radio

Add a **Signal generator** node, pick a signal, and wire it to a matching channel and a Speaker.
Debug builds also list synthetic radios on the Device node: a four-lane coherent array and test
transceivers.

## Checks

| Command | Runs |
|---|---|
| `cargo xtask check` | Format, Clippy, frontend lint and type-check, release builds, generated-file drift |
| `cargo xtask test` | Rust and frontend tests on virtual devices |
| `cargo xtask smoke` | Playwright against a real `sdrmm` process |
| `cargo xtask perf` | DSP throughput and allocation gates |
| `cargo xtask audit` | `cargo-deny` and RustSec advisories |
| `cargo xtask desktop` | Tauri compile check, no installers |
| `cargo xtask sanitize` | Vendored C decoders under AddressSanitizer and UBSan |
| `cargo xtask fuzz` | libFuzzer on every decoder, channel settings, and the dPMR vocoder |

```sh
cargo install --locked cargo-nextest cargo-deny cargo-fuzz
pnpm --dir web exec playwright install chromium
```

`test` needs cargo-nextest, `audit` cargo-deny, `fuzz` cargo-fuzz, and `sanitize` clang.
Automated tests never need real hardware.

## Generated files

Regenerate and commit these with the change that caused them:

| When you change | Run | Updates |
|---|---|---|
| REST routes or wire types | `cargo xtask codegen` | `openapi.json`, `web/src/generated/schema.d.ts` |
| Dependencies | `cargo xtask licenses` | `THIRD_PARTY_NOTICES.md`, embedded notices |
| `web/pnpm-lock.yaml` or a git dependency's `rev` | `cargo xtask nix-hash` | Hashes in `packaging/nix/package.nix` |
| Decoder reference signals | `cargo xtask fixtures` | SigMF files in `fixtures/` |
| Band-plan sources | `cargo xtask bandplan` | Embedded band plans |
| `assets/icon.svg` | `cargo xtask icons` | Desktop and web icons |
| Demo scenes in `web/e2e/scenes.ts` | `pnpm --dir web demo:record` | `site/public/demo/` |
| README screenshots | `cargo xtask screenshots` | `assets/screenshots/` |

`nix-hash` needs Nix on Linux, or runs in a `nixos/nix` container elsewhere. `cargo xtask check`
catches stale output.

## Desktop app

The Tauri app is outside the default workspace. On Linux it needs WebKitGTK. `cargo xtask desktop`
checks that it compiles; [Releases](releases.md#desktop-bundles) builds installers.

## Hardware capture tests

These ignored tests measure loss on real, idle radios, from USB through DSP to publication:

```sh
SDRMM_CAPTURE_DRIVER=hackrf SDRMM_CAPTURE_RATE=8000000 SDRMM_CAPTURE_SECONDS=30 \
  cargo test -p sdrmm-engine --lib --no-default-features --features rtlsdr,hackrf \
  connected_radio_capture_health -- --ignored --nocapture
```

`SDRMM_CAPTURE_DRIVER` is `hackrf`, `rtlsdr`, or `both`. Default rates are 20 MS/s for HackRF and
2.4 MS/s for RTL-SDR. The test fails on any loss unless told otherwise.

| Variable | Does |
|---|---|
| `SDRMM_CAPTURE_CHANNELS=8` | Channels per radio, default 4 |
| `SDRMM_CAPTURE_MIXED=1` | Cycle NFM, WFM, AM, and SSB |
| `SDRMM_CAPTURE_RETUNE=1` | Retune channels every 5 s |
| `SDRMM_CAPTURE_DEVICE_RETUNE=1` | Retune radios every 5 s |
| `SDRMM_CAPTURE_RTL_RATE=3200000` | Override the RTL-SDR rate only |
| `SDRMM_CAPTURE_CPU_THREADS=4` | Add CPU load threads |
| `SDRMM_CAPTURE_RECORD=1` | Record IQ and verify sample counts |
| `SDRMM_CAPTURE_HISTORY=1` | Capture history, then record live, and verify |
| `SDRMM_CAPTURE_HISTORY_SECONDS=6` | History length, default 1 s |
| `SDRMM_CAPTURE_TRANSPORT_SECONDS=5` | Raw USB test length per radio; 0 skips it |
| `SDRMM_CAPTURE_ALLOW_DROPS=1` | Measure overload instead of failing |

Software counters miss some USB losses. For RTL-SDR, also check the hardware byte counter:

```sh
SDRMM_RTL_TEST_RATE=3200000 SDRMM_RTL_TEST_SECONDS=60 \
  cargo test -p sdrmm-device-rtlsdr --lib connected_rtl_counter_continuity -- --ignored --nocapture
```

The 8-bit counter cannot see losses of exact multiples of 256 bytes.

## Before a pull request

Format, lint, check, and test what you changed. For docs, run `mdbook build docs` and check links.
The full gates are `cargo xtask check` and `cargo xtask test`. See
[Contributing](https://github.com/Newspicel/sdrminusminus/blob/main/CONTRIBUTING.md).
