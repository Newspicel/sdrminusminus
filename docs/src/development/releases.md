# Release process

Releases are tag-driven and build portable server archives, desktop installers, update bundles,
and multi-architecture container images. A scheduled workflow publishes a rolling nightly only
when `main` has changed since the previous nightly.

## Versioning

The root `[workspace.package] version` is the source of truth. The desktop configuration inherits
it, and the release workflow stamps it from the tag with:

```sh
cargo xtask set-version 1.2.3
```

Release versions must be plain `major.minor.patch`. The major and minor components must fit in
eight bits and patch in sixteen bits because Windows MSI ProductVersion cannot represent larger
values or prerelease suffixes. The task rejects invalid versions before a bundle job starts.

Stable release tags use `v<major>.<minor>.<patch>`. Nightlies use the UTC date as `YY.M.D`, which
also remains within the MSI limits.

## Portable archives

Build the same archive produced in CI:

```sh
cargo xtask dist
cargo xtask dist --target aarch64-unknown-linux-gnu
```

The command installs a missing Rust target, builds the frontend, compiles the release binary with
Soapy and network backends, verifies the embedded UI, and writes a `.tar.gz` or `.zip` under
`dist/` with README and license files.

Portable archives link against SoapySDR but do not bundle its runtime. Test the archive on a clean
machine with the documented SoapySDR 0.8 dependency.

## Desktop bundles

Without `--bundles`, the desktop task is the compile gate used on pull requests:

```sh
cargo xtask desktop
```

Creating installers requires the Tauri CLI:

```sh
cargo install --locked tauri-cli
cargo xtask desktop --bundles dmg
```

Use `deb,appimage` on Linux and `msi,nsis` on Windows. Nothing is staged beforehand: no artifact
carries SoapySDR, and no build step links it.

## Desktop updates

The desktop app checks the newest non-prerelease GitHub release once at startup. Update archives
are signed separately from platform code signing with the Tauri updater key. The public key is
compiled into the application; losing the private key prevents updates to already installed
clients.

When a local signing key is absent, the bundle task passes `--no-sign` and produces installers that
cannot be published as application updates. Release CI requires signatures and creates
`latest.json` from them:

```sh
cargo xtask updater-manifest \
  --version 1.2.3 \
  --dir dist/release \
  --base-url https://github.com/Newspicel/sdrminusminus/releases/download/v1.2.3
```

## Containers

The release workflow builds Linux `amd64` and `arm64` images and publishes a manifest at:

```text
ghcr.io/newspicel/sdrminusminus:<version>
ghcr.io/newspicel/sdrminusminus:latest
```

Nightlies update only the `nightly` tag. Image smoke tests run the binary, inspect Soapy modules,
start the server, and verify that it serves the built frontend.

## Homebrew tap

`Newspicel/homebrew-tap` carries a `sdrmm` formula for the portable server and a `sdrminusminus`
cask for the desktop application. Both describe published downloads rather than a source build, so
the release workflow writes them after the release exists:

```sh
cargo xtask homebrew-tap \
  --version 0.4.0 \
  --sums SHA256SUMS \
  --repo Newspicel/sdrminusminus \
  --out ../homebrew-tap
```

The generator reads digests from the release's `SHA256SUMS` and fails if a required artifact is
missing. Updating the tap requires `HOMEBREW_TAP_TOKEN` with write access. Without that secret,
the tap job is skipped while the release continues.

The tap publishes stable releases only. Validate generator changes with Homebrew:

```sh
brew style newspicel/tap
brew audit --strict --online newspicel/tap/sdrmm
brew audit --strict --online --cask newspicel/tap/sdrminusminus
```

## Building a pull request

Label a pull request `build_nightly` to run the same rehearsal against the branch. The release
workflow builds the full matrix — portable archives, desktop installers and update bundles for
every platform, `latest.json`, and a container image per architecture — and attaches everything to
the run as artifacts. A comment on the pull request links to them and is rewritten on each rebuild.

Nothing is published: no tag, no GitHub release, and no registry push. The container images are
uploaded as `docker load`-able tarballs instead. The version is fixed at the manifest's own `0.0.0`
rather than derived from the branch or the pull request, so two builds of the same commit produce
identical artifacts.

Every push rebuilds while the label is attached, cancelling the superseded run; remove the label to
stop. The build runs only for branches in this repository, because a fork's run receives none of
the signing secrets the desktop bundles require.

## Release checklist

Before tagging:

1. Run `cargo xtask check`, `cargo xtask test`, `cargo xtask smoke`, and `cargo xtask audit`.
2. Run `cargo xtask desktop` and build the container.
3. Confirm generated API, license, fixture, icon, and band-plan outputs are current.
4. Validate supported hardware with the candidate package, including a reconnect and recording.
5. Confirm the updater signing secret and platform signing credentials are available.
6. Tag the exact reviewed commit and watch every artifact matrix job.
7. Install or unpack at least one published artifact and run `sdrmm --version` and
   `sdrmm --doctor`.

Use the release workflow's manual dispatch as a rehearsal. It builds and uploads the full artifact
matrix without publishing a GitHub release.
