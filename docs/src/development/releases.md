# Release process

Tagged releases publish portable servers, desktop installers, signed update bundles, and container
images. The nightly workflow updates the rolling nightly when `main` changes.

## Changesets

Every user-visible change adds a file to `.changeset/`:

```sh
cargo xtask changeset patch "RTL-SDR: keep gain after reconnect"
```

To release, on a clean `main`:

```sh
cargo xtask release --dry-run
cargo xtask release
git push origin v1.2.3
```

`release` picks the next version from the largest bump among changesets added since the last tag
and tags `HEAD`. It never commits. The release workflow turns those changesets into the GitHub
release notes (`cargo xtask release-notes 1.2.3`) and fails on a tag without any. sdrmm.com reads
its changelog from the GitHub releases. Released changesets can be deleted any time.

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
Linux and Windows bundles are built on x86-64 and ARM64, each on a native machine. Windows ARM64
builds `nsis` alone, because WiX 3 emits no arm64 package.

AppImage builds need `patchelf`, `xdg-utils`, and GStreamer plugin packages. The bundle includes
the installed plugins WebKit uses for audio.

## Desktop updates

The app checks `downloads.sdrmm.com` for the latest stable release at startup, then GitHub if the
mirror has none. Update archives use a Tauri updater signature separate from platform code signing. Preserve the private updater key; installed clients
trust its compiled public key.

Without a local signing key, the bundle task uses `--no-sign`. Those installers cannot serve as
application updates. Release CI requires signatures and builds the update manifest:

```sh
cargo xtask updater-manifest \
  --version 1.2.3 \
  --dir dist/release \
  --base-url https://github.com/Newspicel/sdrmm/releases/download/v1.2.3
```

## Containers

Releases publish Linux `amd64` and `arm64` images:

```text
ghcr.io/newspicel/sdrmm:<version>
ghcr.io/newspicel/sdrmm:latest
```

Nightlies update only `:nightly`. Smoke tests check the binary, SoapySDR modules, server startup,
and embedded frontend. CI builds and smoke-tests both architectures.

## Homebrew tap

The `sdrmm` formula lives in homebrew-core. The release workflow updates the `sdrmm-app`
cask in `Newspicel/homebrew-tap` after publishing stable downloads:

```sh
cargo xtask homebrew-tap \
  --version 1.2.3 \
  --sums SHA256SUMS \
  --repo Newspicel/sdrmm \
  --out ../homebrew-tap
```

The generator checks required artifacts against `SHA256SUMS`. The tap job needs a writable
`HOMEBREW_TAP_TOKEN`; without it, the job is skipped. Other release jobs continue.

Validate generator changes:

```sh
brew style newspicel/tap
brew audit --strict --online --cask newspicel/tap/sdrmm-app
```

## downloads.sdrmm.com

The R2 bucket `sdrmm` serves `downloads.sdrmm.com`. Each stable release is mirrored to
`releases/<tag>/`, and `releases/latest` names the newest tag. `releases/latest.json` is the
updater manifest with mirror URLs; the download page uses the mirror once it holds the newest tag.
GitHub Releases stays the primary copy. The mirror job needs `CLOUDFLARE_API_TOKEN` (R2 write) and `CLOUDFLARE_ACCOUNT_ID`; without
them, it is skipped.

Denoise models live under `denoise/v1/`. After `cargo xtask denoise-model`, upload them with:

```sh
scripts/r2-upload.sh denoise/v1 target/denoise-model/*.sdrmmnn
```

Uploads are cached as immutable. Never replace a file; publish a new prefix instead.

## Release checklist

1. Run `cargo xtask check`, `test`, `smoke`, and `audit`.
2. Run `cargo xtask desktop` and build the container.
3. Check generated API, license, fixture, icon, and band-plan outputs.
4. Validate hardware with the candidate package, including reconnect and recording.
5. Confirm updater and platform signing credentials.
6. Run `cargo xtask release`, push the tag and check every artifact job.
7. Install a published artifact and run `sdrmm --version` and `sdrmm --doctor`.

Manual workflow dispatch rehearses the artifact matrix without publishing a GitHub release.
