# Build and test

The workspace contains the headless server, desktop shell, frontend, DSP libraries, hardware
backends, protocol decoders, and project tooling. CI calls the same `cargo xtask` commands used
locally.

## Prerequisites

- Rust through `rustup`. The repository pins a nightly toolchain and the `rustfmt`, `clippy`, and
  `rust-src` components in `rust-toolchain.toml`.
- Node 26.
- pnpm 11; the exact package-manager version is declared in `web/package.json`.
- SoapySDR 0.8 development files for the normal local-hardware build.
- A C/C++ toolchain and CMake for native dependencies.

On Debian or Ubuntu:

```sh
sudo apt-get update
sudo apt-get install -y build-essential cmake libsoapysdr-dev
```

On macOS:

```sh
brew install cmake soapysdr
```

The first Cargo command automatically installs the pinned Rust toolchain. Do not substitute stable
Rust: the workspace intentionally uses its pinned compiler and `-Zpolonius=next` configuration.

## Build and run

```sh
git clone https://github.com/Newspicel/sdrminusminus.git
cd sdrminusminus
pnpm --dir web install --frozen-lockfile
pnpm --dir web build
cargo run -p sdrmm
```

The server embeds `web/dist` at compile time and listens on <http://localhost:8080>. Build the web
application before compiling a distributable binary; when the directory is absent, the server
build script creates a placeholder so backend-only development can still compile.

For local development:

```sh
cargo xtask dev
```

This runs `sdrmm` with development CORS on port `8080` and Vite with hot module replacement on
<http://localhost:5173>. Vite proxies API and WebSocket traffic to the Rust server. Pass `--watch`
to restart the Rust server whenever backend inputs change:

```sh
cargo xtask dev --watch
```

## Backend feature flags

Normal builds enable `soapy` and `net-client`. A virtual-only build is useful for backend work on a
machine without SoapySDR:

```sh
cargo run -p sdrmm --no-default-features
```

To retain direct `rtl_tcp` and SpyServer support:

```sh
cargo run -p sdrmm --no-default-features --features net-client
```

The built-in virtual driver and recording playback are always available.

## Local gates

| Command | What it runs |
|---|---|
| `cargo xtask check` | Toolchain checks, generated-data checks, Rust format and Clippy, frontend format/lint/type-check, release-shaped builds, web build, codegen drift |
| `cargo xtask test` | Rust and frontend unit/integration tests using virtual devices |
| `cargo xtask smoke` | Playwright against a real `sdrmm` process and the virtual signal generator |
| `cargo xtask audit` | `cargo-deny` and the RustSec advisory database |
| `cargo xtask desktop` | Tauri desktop compile gate without building installers |

Install the smoke browser once before running the Playwright gate:

```sh
pnpm --dir web exec playwright install chromium
cargo xtask smoke
```

`cargo xtask test` requires `cargo-nextest`, and `cargo xtask audit` requires `cargo-deny`:

```sh
cargo install --locked cargo-nextest cargo-deny
```

Tests never enumerate real hardware in CI. Engine and server tests construct a registry with the
virtual backend, which keeps them deterministic and prevents test runs from claiming an attached
radio.

## Generated files

Run the matching task whenever its source changes:

| Source change | Command | Generated output |
|---|---|---|
| REST routes or wire types | `cargo xtask codegen` | `openapi.json`, `web/src/generated/schema.d.ts` |
| Dependency lockfiles | `cargo xtask licenses` | `THIRD_PARTY_NOTICES.md`, embedded notices JSON |
| `web/pnpm-lock.yaml` | `cargo xtask nix-hash` | The pnpm store hash in `packaging/nix/package.nix` |
| Decoder reference signals | `cargo xtask fixtures` | SigMF pairs under `fixtures/` |
| Band-plan source imports | `cargo xtask bandplan` | Embedded regional tables |
| `assets/icon.svg` | `cargo xtask icons` | Desktop and web icon variants |

Generated outputs are committed. `cargo xtask check` detects drift for the outputs that must match
on every change.

`nix-hash` is the one that cannot run everywhere: the hash covers a store only nix can build, so the
task uses nix on Linux and a `nixos/nix` container elsewhere. `check` does not build anything — it
compares a digest of the lockfile recorded beside the hash, which is enough to catch a lockfile that
moved without the hash, and leaves proving the hash itself to the Nix job in CI.

## Desktop prerequisites

The Tauri app is outside the workspace's default members because Linux builds need WebKit and
desktop integration packages. Build it explicitly through `cargo xtask desktop`. To create local
installers, install the Tauri CLI and the platform prerequisites, then follow
[Release process](releases.md#desktop-bundles).

## Before opening a pull request

Run the checks proportional to the change. Documentation-only work should at minimum build the
mdBook and validate its links. Code changes should normally run:

```sh
cargo xtask check
cargo xtask test
```

Add `cargo xtask smoke`, `cargo xtask desktop`, or a hardware validation when the affected surface
requires it.
