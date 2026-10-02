# Releases

Tagged releases publish portable servers, a source tarball, desktop installers, signed update
bundles, and container images. The nightly workflow rebuilds the rolling `nightly` release at
03:07 UTC when `main` has changed.

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
assets, and writes a `.tar.gz` or `.zip` under `dist/` with the binary, the FFmpeg libraries,
`README.md`, `LICENSE`, and `THIRD_PARTY_NOTICES.md`. `cargo xtask source-dist` writes the source
tarball with the built frontend. `cargo xtask link-check <dir>` checks that macOS and Windows
binaries link only bundled and system libraries.

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

Use `deb,rpm,appimage` on Linux and `msi,nsis` on Windows. Installers use system SoapySDR at
runtime. Linux and Windows bundles are built on x86-64 and ARM64, each on a native machine. Windows
ARM64 builds `nsis` alone, because WiX 3 emits no arm64 package.

AppImage builds need `patchelf`, `xdg-utils`, and GStreamer plugin packages. The bundle includes
the installed plugins WebKit uses for audio.

## Desktop updates

Release builds check `downloads.sdrmm.com` for the latest stable release at startup, then GitHub.
Local builds never check. On Linux only the AppImage updates itself. Update archives use a Tauri
updater signature separate from platform code signing. Preserve the private updater key;
installed clients trust its compiled public key.

Without `TAURI_SIGNING_PRIVATE_KEY`, the bundle task uses `--no-sign`. Those installers cannot
serve as application updates. Release CI requires signatures and builds the update manifest:

```sh
cargo xtask updater-manifest \
  --version 1.2.3 \
  --dir dist/release \
  --base-url https://github.com/Newspicel/sdrmm/releases/download/v1.2.3
```

## Containers

Releases publish Linux `amd64` and `arm64` images, also mirrored as
`ghcr.io/newspicel/sdrminusminus`:

```text
ghcr.io/newspicel/sdrmm:<version>
ghcr.io/newspicel/sdrmm:<major>.<minor>
ghcr.io/newspicel/sdrmm:latest
```

Nightlies update `:nightly` and add a `sha-<commit>` tag. CI builds both architectures and checks
the binary, SoapySDR modules, server startup, and embedded frontend.

## Homebrew tap

The `sdrmm` formula lives in homebrew-core and builds from the GitHub source tarball. The release
workflow updates the `sdrmm-app` cask in `Newspicel/homebrew-tap`, which downloads from GitHub
Releases:

```sh
cargo xtask homebrew-tap \
  --version 1.2.3 \
  --sums SHA256SUMS \
  --repo Newspicel/sdrmm \
  --out ../homebrew-tap
```

The generator checks required artifacts against `SHA256SUMS`.

Validate generator changes:

```sh
brew style newspicel/tap
brew audit --strict --online --cask newspicel/tap/sdrmm-app
```

## AUR and WinGet

The release workflow pushes `sdrmm-bin` and `sdrmm-app-bin` to the AUR, written by
`cargo xtask aur --version 1.2.3 --sums SHA256SUMS --out <dir>`. It submits
`Newspicel.SDRmm` to WinGet once the package is listed in `winget-pkgs`.

## downloads.sdrmm.com

The R2 bucket `sdrmm` serves `downloads.sdrmm.com`, the primary download location. A stable release
uploads to `releases/<tag>/` first, then to GitHub Releases as the fallback. Once both hold it,
`releases/latest`, `releases/latest.json` (updater manifest) and `releases/release.json` (download
page) move to the new tag. AUR packages and the WinGet manifest point at the mirror; the Homebrew
cask stays on GitHub. Nightlies stay on GitHub only.

After a release, the `linux-repo` workflow publishes the signed APT and RPM repository under
`packages/`. Dispatch it by hand to rebuild the repository from a tag.

`scripts/r2-upload.sh <prefix> <file>...` uploads through `wrangler`. Denoise models live under
`denoise/v1/`. `cargo xtask denoise-model` builds them into `target/denoise-model/` and fails until
the catalog in `crates/server/src/denoise_models.rs` matches. Then upload:

```sh
scripts/r2-upload.sh denoise/v1 target/denoise-model/*.sdrmmnn
```

Uploads are cached as immutable. Never replace a file; publish a new prefix instead.

## Secrets

| Secret | Needed for | Without it |
|---|---|---|
| `CLOUDFLARE_API_TOKEN`, `CLOUDFLARE_ACCOUNT_ID` | R2 uploads | A tagged release fails |
| `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Updater signatures | The release fails |
| `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` | macOS signing and notarization | The release fails |
| `HOMEBREW_TAP_TOKEN` | Writing the tap | The tap job is skipped |
| `AUR_SSH_KEY` | Pushing to the AUR | The AUR job is skipped |
| `WINGET_TOKEN` | WinGet submission | The WinGet job is skipped |
| `PACKAGES_GPG_KEY` | Signing the APT and RPM repository | The repository job is skipped |

A tag release warns for every skipped job. Other release jobs continue.

## Release checklist

1. Run `cargo xtask check`, `test`, `smoke`, and `audit`.
2. Run `cargo xtask desktop` and build the container.
3. Check generated API, license, icon, and band-plan outputs.
4. Validate hardware with the candidate package, including reconnect and recording.
5. Confirm updater and platform signing credentials.
6. Run `cargo xtask release`, push the tag and check every artifact job.
7. Install a published artifact and run `sdrmm --version` and `sdrmm --doctor`.

Manual workflow dispatch rehearses the artifact matrix without publishing a GitHub release.
