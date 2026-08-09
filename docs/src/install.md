# Install and run

There are four ways to run sdr--: the headless server binary, a container, the desktop app,
or a build from source. Only the last one is available in every case today.

> [!NOTE]
> Packaging is the M5 deliverable and lands alongside this documentation. Releases are
> tag-driven and publish `sdrmm` tarballs for linux-x86_64, linux-aarch64 and macOS-arm64,
> a multi-arch container image, and the desktop bundles. Until a release is published, build
> from source — it is three commands.

## Build from source

### Prerequisites

| Requirement | Notes |
|---|---|
| Rust nightly, pinned | `rust-toolchain.toml` names the exact nightly; `rustup` installs it on the first `cargo` invocation. Do not override it — the build uses the next-gen borrow checker (`-Zpolonius=next`). |
| Node 24 + pnpm 11 | The web client. `corepack enable` or install pnpm directly. |
| A C toolchain | For the vendored libopus build. `build-essential` on Debian/Ubuntu, Xcode command line tools on macOS. |
| SoapySDR (optional) | Only for the default feature set. `apt install libsoapysdr-dev soapysdr-module-all`, or `brew install soapysdr soapyrtlsdr soapyhackrf`. Skip it with `--no-default-features`. |

### Build and run

```sh
git clone https://github.com/Newspicel/sdrminusminus
cd sdrminusminus
pnpm --dir web install --frozen-lockfile
pnpm --dir web build      # the server embeds web/dist at compile time
cargo run -p sdrmm
```

The server binds `0.0.0.0:8080` by default. Open <http://localhost:8080>.

If you start the server without building the frontend first, it still runs — the API and
WebSocket work, and the root page tells you the UI is not built yet.

### Without SoapySDR

```sh
cargo run -p sdrmm --no-default-features --features rtl-native,hackrf-native
```

This is the shape release artifacts ship in: no libSoapySDR dependency at all, and the native
pure-Rust RTL-SDR and HackRF backends compiled in, so real hardware works with nothing
installed. Bare `--no-default-features` drops those backends too and leaves only the virtual
devices (signal generator, recording playback) — useful for checking that every backend really
is optional, which is what CI does, but not what you want on a receiver. Both configurations
are gated in CI on every change, so neither can rot.

## Headless server

`apps/sdrmm` is the Raspberry Pi target: one binary with the UI embedded.

```
sdrmm [--bind ADDR:PORT] [--db PATH] [--recordings-dir PATH] [--dev-cors]
```

| Flag | Default | Purpose |
|---|---|---|
| `--bind` | `0.0.0.0:8080` | Listen address. LAN-trusted by default — see [security](operating/security.md). |
| `--db` | `<platform data dir>/sdrmm/sdrmm.db` | SQLite file for presets, bookmarks, the recordings index and the decoder log. The default is absolute so a systemd unit, an SSH session and a double-click all open the same database. |
| `--recordings-dir` | `<platform data dir>/sdrmm/recordings` | Where SigMF recordings are written and scanned from. |
| `--token` | unset | Require this shared token on every API, WebSocket and MCP request. Also read from `SDRMM_TOKEN`, so it need not appear in the process list. Unset means LAN-trusted and unauthenticated — see [security](operating/security.md). |
| `--doctor` | — | Print environment diagnostics and exit. |
| `--dev-cors` | off | Relax CORS for a separate dev origin. Used by `cargo xtask dev`; do not use it on a shared network. |

The platform data directory is `~/.local/share` on Linux and
`~/Library/Application Support` on macOS.

`sdrmm --doctor` prints an environment report — which backends compiled in, which devices and
Soapy modules were found, USB permissions, and the paths it would use — then exits without
opening anything. See [Troubleshooting](operating/troubleshooting.md).

### Running as a service

sdr-- has no daemon mode; run it under your init system. A minimal systemd unit:

```ini
[Unit]
Description=sdr-- server
After=network-online.target

[Service]
ExecStart=/usr/local/bin/sdrmm --bind 0.0.0.0:8080
User=sdr
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

The service user needs USB access to the SDR — see [Hardware](hardware.md) for the udev
rules.

## Container

The image is multi-arch and published to `ghcr.io/newspicel/sdrminusminus`. It already passes
`--db /data/sdrmm.db --recordings-dir /data/recordings`, so it needs a volume at `/data` and
USB passthrough:

```sh
docker run -d -p 8080:8080 \
  --device /dev/bus/usb \
  -v sdrmm-data:/data \
  ghcr.io/newspicel/sdrminusminus:latest
```

Anything you append replaces the default `--bind 0.0.0.0:8080`, so pass the full flag when
you change it. `docker-compose.yml` in the repository is the maintained deployment: it adds a
device cgroup rule for the whole usbfs major, because a plain `devices:` list whitelists only
the nodes that existed at container start and a replugged SDR comes back as a new minor.

The container runs as an unprivileged user, so whether it may open an SDR is decided by the
**host's** udev rules — see [Hardware](hardware.md).

USB passthrough is Linux only. Docker Desktop on macOS runs in a VM that does not see host
USB devices, so a container there is limited to virtual devices and recordings.

## Desktop app

`apps/desktop` is a Tauri v2 shell. It embeds `crates/server` in-process on an ephemeral
loopback port and points its WebView at it, which means the desktop app and a remote browser
run exactly the same frontend against exactly the same API — there is one client, not two.
It stores its database and recordings in the platform app-data directory.

Desktop bundles are built by the release workflow. To build one yourself:

```sh
pnpm --dir web build
cargo build -p sdrmm-desktop --release
```

The Tauri app is excluded from the default workspace members, because it pulls the platform
webview toolchain (webkit2gtk on Linux) that the rest of the gate does not need.

## Development mode

```sh
cargo xtask dev
```

Starts the server on `:8080` with relaxed CORS and a Vite dev server on
<http://localhost:5173> that proxies `/api` (including the WebSocket upgrade) to it. Use the
Vite origin: HMR is intact and the same-origin model still matches production.

Run `cargo xtask check` before committing — it is the same gate CI runs.
