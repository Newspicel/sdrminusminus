# Building and running

## Prerequisites

- The pinned nightly Rust toolchain. `rust-toolchain.toml` names it and `rustup` installs it on
  the first build — do not substitute a stable toolchain, the workspace builds with
  `-Zpolonius=next`.
- Node 26 and pnpm 11.
- SoapySDR 0.8 development files for normal builds: `libsoapysdr-dev` on Debian/Ubuntu or
  `brew install soapysdr` on macOS. Soapy is the default and release hardware backend. A
  specialized virtual/network-only build is available with
  `--no-default-features --features net-client`.

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
| `cargo xtask icons` | Re-render every icon from `assets/icon.svg` (commit the output) |
| `cargo xtask dist` | The release archive for this host |
| `cargo xtask desktop` | The Tauri shell — compile gate, or `--bundles` for installers |

## Release artifacts

`cargo xtask dist [--target <triple>]` produces exactly what the release pipeline uploads:
`dist/sdrmm-<version>-<triple>.tar.gz` (`.zip` on Windows), holding the binary plus `README.md`
and license/notices files. These portable headless archives compile the Soapy backend but do not
copy a runtime: the target machine must provide SoapySDR 0.8.1 (module ABI 0.8) and its hardware
module. Release baselines are SoapyRTLSDR 0.3.3 and SoapyHackRF 0.3.4; the other curated versions
are declared in `packaging/soapy/environment.yml`. SDRplay uses the separately installed
SoapySDRPlay3 0.5.2 module and SDRplay API 3.15 or newer described in the
[hardware guide](hardware.md#sdrplay). Release and Docker builds install the
matching immutable `packaging/soapy/conda-<platform>.lock`, which pins every transitive package
URL and checksum. Confirm the installation with `sdrmm --doctor`;
its report includes the compiled backend, core version, module search paths, and discovered
modules.

The command builds the web UI first and then asserts it is present, because
`crates/server/build.rs` creates an empty `web/dist` when one is missing — so a release built
without the UI would otherwise succeed and silently ship a "not built" placeholder page.

`cargo xtask desktop --bundles dmg` (or `deb,appimage`, `msi,nsis`) builds the desktop
installers through the Tauri CLI, which you need installed:
`cargo install --locked tauri-cli`. Before bundling, stage the pinned private runtime from
the matching `packaging/soapy/conda-<platform>.lock` into a Conda prefix, then stage it in
`apps/desktop/resources/soapy` with
`packaging/soapy/stage-unix.sh <conda-prefix> apps/desktop/resources/soapy` (or
`powershell -NoProfile -File packaging/soapy/stage-windows.ps1 -Prefix "<conda-prefix>" -Destination apps/desktop/resources/soapy`
on Windows). `cargo xtask soapy-bundle-check`
checks the staged payload. Release CI
performs these steps automatically and verifies the resulting installers. Without `--bundles`
the command is a compile gate only,
which is what CI runs on every pull request — `apps/desktop` is outside the workspace's
`default-members`, so nothing else builds it.

Versions come from one place: `[workspace.package] version` in the root `Cargo.toml`.
`apps/desktop/tauri.conf.json` deliberately omits `version` so Tauri inherits that one, and the
release pipeline stamps it from the git tag with `cargo xtask set-version`.

A version must be a plain `major.minor.patch` with major and minor ≤ 255 and patch ≤ 65535.
Those are the Windows MSI bundler's limits, and an MSI ProductVersion has no field a `-rc.1`
suffix could occupy — so `set-version` rejects both up front rather than letting the tag build
for twenty minutes and fail in the single job that bundles an installer.

## Desktop updates

The desktop app checks for updates once at startup and offers them in a native dialog. The check
runs Rust-side through `UpdaterExt`, so the frontend is not involved and no Tauri capability is
granted — the WebView is pointed at the embedded server's origin, and reaching the updater from
JS would mean opening IPC to a remote URL.

Clients poll `latest.json` on the newest non-prerelease GitHub release. Nightlies are
prereleases, so they are never offered to someone on a stable build; a nightly's own version is
`YY.M.D`, which sorts above any `0.x`, so nightly installs are simply never offered anything.

Bundles carry an updater signature, which is separate from Apple's code signature and comes from
the key in `TAURI_SIGNING_PRIVATE_KEY`. The public half lives in `apps/desktop/tauri.conf.json`,
and a client trusts nothing else — losing the private key means no already-installed client can
ever be updated again. Because a pubkey is configured, the Tauri CLI treats a missing private key
as an error rather than skipping the signature, so `xtask desktop --bundles` passes `--no-sign`
when the variable is unset and says so: a local bundle is installable but cannot be served as an
update. The release workflow is the only place that matters, and it fails if no `.sig` was
produced.

`cargo xtask updater-manifest` builds `latest.json` from the `.sig` files in a release directory.
It is strict about names: the macOS bundler writes a bare `sdr--.app.tar.gz` with no architecture
in it, so both slices would arrive at one release under the same name — the workflow renames them
and the generator fails on anything it cannot place, rather than shipping a manifest that is
missing a platform.

## Nightlies

The release pipeline also runs on a schedule, publishing the full matrix over a rolling
`nightly` prerelease and a `nightly` container tag. Nightlies are versioned `YY.M.D` (a
four-digit year would exceed the MSI major limit above), so `sdrmm --version` names the night a
build came from; the commit is in the release notes.

It only spends runners when there is something to build: the `nightly` tag records the commit
the last nightly was built from, and a run whose `main` matches it stops after the version job.
