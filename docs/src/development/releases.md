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

Use `deb,appimage` on Linux and `msi,nsis` on Windows. Before bundling, stage the matching locked
Soapy runtime into `apps/desktop/resources/soapy` with the scripts under `packaging/soapy`, then
verify the result:

```sh
cargo xtask soapy-bundle-check
```

Release CI performs this staging from the immutable platform lockfiles. A bundle must include the
core, baseline modules, transitive libraries, and their notices.

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
start the server, and verify that the embedded UI—not the build placeholder—is served.

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

The digests come from the release's own `SHA256SUMS`; an artifact the release does not carry is an
error rather than a formula pointing at a missing download. Publishing needs a `HOMEBREW_TAP_TOKEN`
secret with write access to the tap. Without it the job skips and the release still ships.

Nightlies never reach the tap. Before pushing a change to what the generator writes, check it with
Homebrew itself:

```sh
brew style newspicel/tap
brew audit --strict --online newspicel/tap/sdrmm
brew audit --strict --online --cask newspicel/tap/sdrminusminus
```

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
