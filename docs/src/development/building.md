# Build and test

CI and local development run the same `cargo xtask` commands.

## Prerequisites

| Tool | Version |
|---|---|
| Rust | 1.99.0, pinned in `rust-toolchain.toml`; rustup installs it |
| Node | 26 |
| pnpm | 12, exact version in `web/package.json` |
| FFmpeg | 9, built by `scripts/build-media.py` |
| Native | C/C++ compiler, Clang/libclang, CMake, GNU Make, NASM, Python 3.12+ |

```sh
sudo apt-get install -y build-essential cmake clang libclang-dev python3 nasm   # Debian, Ubuntu
brew install cmake python nasm                                                  # macOS
```

Fuzzing and `cargo xtask sanitize` also need a nightly toolchain. SoapySDR loads at runtime, so no
development package is needed.

## Build and run

```sh
git clone https://github.com/Newspicel/sdrmm.git
cd sdrmm
python3 scripts/build-media.py
export FFMPEG_DIR="$(python3 scripts/build-media.py --print-prefix)"
pnpm --dir web install --frozen-lockfile
pnpm --dir web build
cargo run -p sdrmm
```

Open <http://localhost:8080>.

The media script builds the few FFmpeg 9.0.2 codecs SDR-- needs, from checksummed source, into
`.media/<target>` as shared libraries. Keep `FFMPEG_DIR` set for every Cargo command. To run
binaries and tests, CI also adds `$FFMPEG_DIR/lib` to `LD_LIBRARY_PATH` on Linux and
`DYLD_LIBRARY_PATH` on macOS. A system FFmpeg older than 9 does not compile. For cross builds,
pass `--target <triple>` to the script. Nix uses its own FFmpeg.

The server embeds `web/dist`, so build the frontend first. Without it, the server shows a
placeholder page.

For hot reload of the frontend and automatic backend restarts:

```sh
cargo xtask dev --watch
```

Open <http://localhost:5173>. Vite forwards API and WebSocket traffic to port 8080. Without
`--watch`, the backend runs once.

### Windows

Build from a Visual Studio developer shell with LLVM and MSYS2 Make installed; the CI Rust action
in `.github/actions/rust` shows the setup. Set `MEDIA_SHELL_BIN` to the folders holding `bash`,
`make`, and `clang-cl`, and `LIBCLANG_PATH` to libclang. The script adds those folders only for
the tools it runs, so MSYS2's `link.exe` never hides the MSVC linker.

ARM64 builds the codecs without assembly, because FFmpeg's ARM assembler tools do not ship with
the toolchain. xtask retries Cargo up to three times on Windows, since the
`ffmpeg-sys-the-third` build script sometimes crashes loading libclang
([issue 145](https://github.com/shssoichiro/ffmpeg-the-third/issues/145)).

## Feature flags

The server enables `soapy`, `sdrplay`, `cr8`, `rtlsdr`, `hackrf`, `airspy`, `airspyhf`, `espsdr`,
`ad936x`, `net-client`, and `gpu-fft` by default. Packaged releases leave out `cr8`.

```sh
cargo run -p sdrmm --no-default-features                        # no radio drivers
cargo run -p sdrmm --no-default-features --features net-client  # network radios only
```

Recording playback and the signal generator work in every build.

## Test without a radio

Add a **Signal generator** node, pick a signal, and wire it to a matching channel and a Speaker.

Debug builds also offer synthetic radios on the Device node. The UI shows them when it runs from
`cargo xtask dev` or was built with `VITE_ENABLE_SYNTHETIC_DEVICES=true`.

| Radio | Key | Is |
|---|---|---|
| Kraken bench ×5 | `kraken5` | Five lanes on one clock with a noise source, like a KrakenSDR |
| Coherent Array ×4 | `array4` | Four lanes on one clock and LO, with a pilot tone |
| Dongle 1, Dongle 2 | `dongle1`, `dongle2` | One lane each, two elements of a line array; Dongle 1 holds the noise source |
| Test band | `band` | A band of test signals on one lane |
| Receiver ×4, Transceiver 2×2, Half-duplex 1×1 | `quad`, `transceiver`, `halfduplex` | Lane and transmit shapes |

The bench radios hear one FM emitter at 137° and its echo at 250°.
[Hardware tests](hardware-tests.md) cover real radios.

## Checks

| Command | Runs |
|---|---|
| `cargo xtask check` | Format, Clippy, web and site lint and type-check, crate rules, feature-set builds, generated-file drift |
| `cargo xtask test` | Generated fixtures, then Rust, web and site tests, with SoapySDR hidden |
| `cargo xtask smoke` | Playwright against a real `sdrmm` process |
| `cargo xtask perf` | DSP, channel and engine throughput and allocation gates |
| `cargo xtask audit` | RustSec advisories through cargo-deny |
| `cargo xtask desktop` | Clippy on the Tauri app, no installers |
| `cargo xtask sanitize` | Channel tests, with the vendored C, under AddressSanitizer and UBSan |
| `cargo xtask fuzz` | libFuzzer on every channel type, channel settings, and dPMR voice |

```sh
cargo install --locked cargo-nextest cargo-deny cargo-fuzz
pnpm --dir web exec playwright install chromium
```

`test` needs cargo-nextest, `audit` cargo-deny, `fuzz` cargo-fuzz, and `sanitize` clang.
`fuzz` takes `--target <channel_chain|channel_settings|dv_voice>`, `--seconds` (default 60),
`--jobs`, and `--minimize`. Automated tests never need real hardware.

CI runs these on every push and pull request, plus the mobile, iPhone, Android, container image,
and Nix jobs. The nightly workflow adds sanitizers, four hours of fuzzing per target, iPhone UI and
end-to-end runs, Android device tests, and measurement suites.

## Generated files

Regenerate and commit these with the change that caused them:

| When you change | Run | Updates |
|---|---|---|
| REST routes or wire types | `cargo xtask codegen` | `openapi.json`, `web/src/generated/` |
| Dependencies | `cargo xtask licenses` | `THIRD_PARTY_NOTICES.md`, `crates/server/data/notices.json`, phone notices |
| `web/pnpm-lock.yaml` or a git dependency's `rev` | `cargo xtask nix-hash` | Hashes in `packaging/nix/package.nix` |
| Band-plan sources | `cargo xtask bandplan` | `crates/server/data/bandplan/` |
| `assets/icon.svg` | `cargo xtask icons` | Desktop, iPhone, web, site, and docs icons |
| Demo scenes in `web/e2e/scenes.ts` | `pnpm --dir web demo:record` | `site/public/demo/` |
| README screenshots | `cargo xtask screenshots` | `assets/screenshots/` |

`nix-hash` needs Nix on Linux, or Docker to run `nixos/nix` elsewhere. `cargo xtask check` catches
stale codegen, notices, and Nix hashes.

`cargo xtask fixtures` writes the synthesized decoder SigMF pairs into `fixtures/`. Git ignores
them, and `cargo xtask test` regenerates them.

## Other tasks

| Command | Does |
|---|---|
| `cargo xtask excerpt` | Trims a capture into a fixture; see `fixtures/README.md` |
| `cargo xtask replay` | Runs a capture through one channel |
| `cargo xtask ber <entry>` | Bit error rate curves into `target/ber` |
| `cargo xtask ident-matrix` | Signal identifier against the fixtures |
| `cargo xtask compare <dsp\|decoders\|apps>` | Compares with other SDR software; `--ours` measures SDR-- alone |

## Desktop app

The Tauri app is a workspace member but not a default member. On Linux it needs WebKitGTK:

```sh
sudo apt-get install -y libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev
```

`cargo xtask desktop` checks that it compiles; [Releases](releases.md#desktop-bundles) builds
installers.

## Before a commit

Format, lint, check, and test what you changed. For docs, run `mdbook build docs` and check links.
The full gates are `cargo xtask check` and `cargo xtask test`.
