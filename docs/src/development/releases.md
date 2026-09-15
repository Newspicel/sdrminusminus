# Release process

Tagged releases publish portable servers, desktop installers, signed update bundles, and container
images. A scheduled workflow updates the rolling nightly when `main` changes.

## Versioning

The root workspace version is the source of truth. Set it with:

```sh
cargo xtask set-version 1.2.3
```

Stable tags use `v<major>.<minor>.<patch>`. Nightlies use the UTC date as `YY.M.D`.
Windows MSI requires major and minor to fit in eight bits and patch in sixteen bits; prerelease
suffixes are unsupported. The task validates these limits.

## Portable archives

```sh
cargo xtask dist
cargo xtask dist --target aarch64-unknown-linux-gnu
```

The task installs a missing Rust target, builds the frontend and release binary, verifies embedded
assets, and writes a `.tar.gz` or `.zip` under `dist/` with README and license files.

Archives load SoapySDR at runtime without linking or bundling it. Verify startup on a clean machine
both with and without a system SoapySDR installation.

## Desktop bundles

Run the compile gate:

```sh
cargo xtask desktop
```

To create installers, install the Tauri CLI:

```sh
cargo install --locked tauri-cli
cargo xtask desktop --bundles dmg
```

Use `deb,appimage` on Linux and `msi,nsis` on Windows. Installers use system SoapySDR at runtime.

The AppImage bundles the GStreamer WebKit plays audio through, and bundles only what the build
machine has: an `appimage` build needs `patchelf` and the GStreamer plugin packages installed, or
the app it produces is silently mute.

## Desktop updates

The app checks the latest stable GitHub release at startup. Update archives use a Tauri updater
signature separate from platform code signing. Preserve the private updater key; installed clients
trust its compiled public key.

Without a local signing key, the bundle task uses `--no-sign`. Those installers cannot serve as
application updates. Release CI requires signatures and builds the update manifest:

```sh
cargo xtask updater-manifest \
  --version 1.2.3 \
  --dir dist/release \
  --base-url https://github.com/Newspicel/sdrminusminus/releases/download/v1.2.3
```

## Containers

Releases publish Linux `amd64` and `arm64` images:

```text
ghcr.io/newspicel/sdrminusminus:<version>
ghcr.io/newspicel/sdrminusminus:latest
```

Nightlies update only `:nightly`. Smoke tests check the binary, SoapySDR modules, server startup,
and embedded frontend.

## Homebrew tap

The release workflow updates the `sdrmm` formula and `sdrminusminus` cask in
`Newspicel/homebrew-tap` after publishing stable downloads:

```sh
cargo xtask homebrew-tap \
  --version 1.2.3 \
  --sums SHA256SUMS \
  --repo Newspicel/sdrminusminus \
  --out ../homebrew-tap
```

The generator checks required artifacts against `SHA256SUMS`. The tap job needs a writable
`HOMEBREW_TAP_TOKEN`; without it, the job is skipped. Other release jobs continue.

Validate generator changes:

```sh
brew style newspicel/tap
brew audit --strict --online newspicel/tap/sdrmm
brew audit --strict --online --cask newspicel/tap/sdrminusminus
```

## Building a pull request

Add `build_nightly` to a same-repository pull request to build the full release matrix. The run
uploads portable archives, installers, update bundles, `latest.json`, and container tarballs.
A pull-request comment links to the artifacts.

This rehearsal publishes no release, tag, or registry image. Containers can be imported with
`docker load`. The build uses the manifest version, currently `0.0.0`.

Each push rebuilds and cancels the older run. Remove the label to stop. Forks cannot use this
workflow because the bundle jobs require signing secrets.

## Release checklist

1. Run `cargo xtask check`, `test`, `smoke`, and `audit`.
2. Run `cargo xtask desktop` and build the container.
3. Check generated API, license, fixture, icon, and band-plan outputs.
4. Validate hardware with the candidate package, including reconnect and recording.
5. Confirm updater and platform signing credentials.
6. Tag the reviewed commit and check every artifact job.
7. Install a published artifact and run `sdrmm --version` and `sdrmm --doctor`.

Manual workflow dispatch rehearses the artifact matrix without publishing a GitHub release.
