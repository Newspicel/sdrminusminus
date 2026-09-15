# Build and test

Build the web interface, then the Rust server. Local development and CI use the same `cargo xtask`
commands.

## Prerequisites

| Tool | Requirement |
|---|---|
| Rust | Install through rustup; use `rust-toolchain.toml` |
| Node | 26 |
| pnpm | 11; exact version in `web/package.json` |
| Native build tools | C/C++ compiler and CMake |

On Debian or Ubuntu:

```sh
sudo apt-get update
sudo apt-get install -y build-essential cmake
```

On macOS:

```sh
brew install cmake
```

Cargo installs the pinned nightly compiler and components automatically. The workspace uses
`-Zpolonius=next`, so the pinned toolchain is required. SoapySDR loads at runtime and needs no
build-time development package.

## Build and run

```sh
git clone https://github.com/Newspicel/sdrminusminus.git
cd sdrminusminus
pnpm --dir web install --frozen-lockfile
pnpm --dir web build
cargo run -p sdrmm
```

Open <http://localhost:8080>. Distributable builds embed `web/dist`; build the frontend first.
Backend-only builds can compile with a placeholder interface if that directory is missing.

For frontend hot reload and automatic backend restarts:

```sh
cargo xtask dev --watch
```

Open <http://localhost:5173>. Vite proxies API and WebSocket traffic to port `8080`.
Omit `--watch` to leave backend restarts manual.

## Backend feature flags

The server defaults enable `soapy`, `sdrplay`, `cr8`, `rtlsdr`, `hackrf`, `airspy`, `airspyhf`,
`ad936x`, `net-client`, and `gpu-fft`. Packaged releases use a selected subset; see
[hardware requirements](../hardware.md).

Disable hardware backends:

```sh
cargo run -p sdrmm --no-default-features
```

Keep direct `rtl_tcp` and SpyServer clients:

```sh
cargo run -p sdrmm --no-default-features --features net-client
```

SigMF playback remains available in both builds.

## Development signal sources

Debug builds expose the signal generator and synthetic array/transceiver sources. Release builds
hide them and keep recording playback available.

To test audio in a debug build, select **Signal Generator (virtual)** on Device, connect an NFM
channel at 300 kHz above the Device centre, then connect its audio to Speaker. Starting playback
produces a 1 kHz tone.

## Local gates

| Command | What it runs |
|---|---|
| `cargo xtask check` | Toolchain checks, generated-data checks, Rust format and Clippy, frontend format/lint/type-check, release-shaped builds, web build, codegen drift |
| `cargo xtask test` | Rust and frontend unit/integration tests using virtual devices |
| `cargo xtask smoke` | Playwright against a real `sdrmm` process and the virtual signal generator |
| `cargo xtask audit` | `cargo-deny` and the RustSec advisory database |
| `cargo xtask desktop` | Tauri desktop compile gate without building installers |
| `cargo xtask sanitize` | Decoder tests with the vendored C under AddressSanitizer and UndefinedBehaviorSanitizer |
| `cargo xtask fuzz` | libFuzzer against every decoder, channel settings, and the dPMR vocoder chain |

Install the tools needed for your checks:

```sh
cargo install --locked cargo-nextest cargo-deny cargo-fuzz
pnpm --dir web exec playwright install chromium
```

`test` needs cargo-nextest, `audit` needs cargo-deny, and `fuzz` needs cargo-fuzz. Sanitizer tests
also require clang. Automated tests use virtual devices and never require real hardware.

## Generated files

Regenerate and commit outputs when their sources change:

| Source change | Command | Generated output |
|---|---|---|
| REST routes or wire types | `cargo xtask codegen` | `openapi.json`, `web/src/generated/schema.d.ts` |
| Dependency lockfiles | `cargo xtask licenses` | `THIRD_PARTY_NOTICES.md`, embedded notices JSON |
| `web/pnpm-lock.yaml` | `cargo xtask nix-hash` | The pnpm store hash in `packaging/nix/package.nix` |
| A git dependency's `rev` | `cargo xtask nix-hash` | The cargo git hashes in `packaging/nix/package.nix` |
| Decoder reference signals | `cargo xtask fixtures` | SigMF pairs under `fixtures/` |
| Band-plan source imports | `cargo xtask bandplan` | Embedded regional tables |
| `assets/icon.svg` | `cargo xtask icons` | Desktop and web icon variants |

`cargo xtask check` detects stale contracts and metadata. `nix-hash` uses Nix on Linux or a
`nixos/nix` container elsewhere. It updates the pnpm store hash and Cargo git-dependency hashes.
Local checks compare lockfile digests and commits; Nix CI verifies the hashes by building.

## Desktop prerequisites

The Tauri app is outside the default workspace members. Linux needs WebKitGTK and desktop
integration libraries. Use `cargo xtask desktop` for the compile gate and follow
[Desktop bundles](releases.md#desktop-bundles) to create installers.

## Before opening a pull request

Format, lint, check, and test the affected parts. For documentation, run `mdbook build docs`
and check local links and anchors. For code, the full gates are:

```sh
cargo xtask check
cargo xtask test
```

Add browser, desktop, DSP performance, or hardware validation when the change needs it.
See [Contributing](https://github.com/Newspicel/sdrminusminus/blob/main/CONTRIBUTING.md).
