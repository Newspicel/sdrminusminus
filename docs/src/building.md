# Building and running

## Prerequisites

- The pinned nightly Rust toolchain. `rust-toolchain.toml` names it and `rustup` installs it on
  the first build — do not substitute a stable toolchain, the workspace builds with
  `-Zpolonius=next`.
- Node 24 and pnpm 11.
- SoapySDR, **only** for the default build: `libsoapysdr-dev` on Debian/Ubuntu,
  `brew install soapysdr` on macOS. Building with
  `--no-default-features --features rtl-native,hackrf-native` skips it entirely, which is the
  shape release artifacts ship in — those need no C library at all.

## From a clone

```sh
git clone https://github.com/Newspicel/sdrminusminus
cd sdrminusminus
pnpm --dir web install --frozen-lockfile
pnpm --dir web build          # the server embeds these assets
cargo run -p sdrmm            # serves on 0.0.0.0:8080
```

Open <http://localhost:8080>, open the **Signal Generator (virtual)** device and add an NFM
channel at +300 kHz — you will hear a 1 kHz tone without owning a radio.

For UI work use `cargo xtask dev` instead: it runs the server plus a Vite dev server with HMR
on <http://localhost:5173>, proxying `/api` and the WebSocket to the server.

## Tasks

`cargo xtask` is the entry point for everything, and CI invokes the same subcommands verbatim —
so every gate that can fail in CI can be run locally first.

| Command | Does |
|---|---|
| `cargo xtask dev` | Server + Vite dev server with HMR |
| `cargo xtask codegen` | Regenerate `openapi.json` + `web/src/generated` |
| `cargo xtask check` | The full gate, ordered cheapest-first |
| `cargo xtask test` | Rust + web test suites, on `device-virtual` |
| `cargo xtask smoke` | The Playwright browser flow against the real binary |
| `cargo xtask audit` | The dependency graph against RustSec |
| `cargo xtask fixtures` | Regenerate the synthesized SigMF decoder fixtures |
| `cargo xtask dist` | The release archive for this host |
| `cargo xtask desktop` | The Tauri shell — compile gate, or `--bundles` for installers |

## Release artifacts

`cargo xtask dist [--target <triple>]` produces exactly what the release pipeline uploads:
`dist/sdrmm-<version>-<triple>.tar.gz` (`.zip` on Windows), holding the binary plus `README.md`
and `LICENSE`. It builds the web UI first and then asserts it is present, because
`crates/server/build.rs` creates an empty `web/dist` when one is missing — so a release built
without the UI would otherwise succeed and silently ship a "not built" placeholder page.

`cargo xtask desktop --bundles dmg` (or `deb,appimage`, `msi,nsis`) builds the desktop
installers through the Tauri CLI, which you need installed:
`cargo install --locked tauri-cli`. Without `--bundles` the command is a compile gate only,
which is what CI runs on every pull request — `apps/desktop` is outside the workspace's
`default-members`, so nothing else builds it.

Versions come from one place: `[workspace.package] version` in the root `Cargo.toml`.
`apps/desktop/tauri.conf.json` deliberately omits `version` so Tauri inherits that one, and the
release pipeline stamps it from the git tag with `cargo xtask set-version`.

A version must be a plain `major.minor.patch` with major and minor ≤ 255 and patch ≤ 65535.
Those are the Windows MSI bundler's limits, and an MSI ProductVersion has no field a `-rc.1`
suffix could occupy — so `set-version` rejects both up front rather than letting the tag build
for twenty minutes and fail in the single job that bundles an installer.

## Nightlies

The release pipeline also runs on a schedule, publishing the full matrix over a rolling
`nightly` prerelease and a `nightly` container tag. Nightlies are versioned `YY.M.D` (a
four-digit year would exceed the MSI major limit above), so `sdrmm --version` names the night a
build came from; the commit is in the release notes.

It only spends runners when there is something to build: the `nightly` tag records the commit
the last nightly was built from, and a run whose `main` matches it stops after the version job.
