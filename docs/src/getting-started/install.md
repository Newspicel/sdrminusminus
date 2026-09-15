# Install sdr--

Choose a desktop app for local use or a server to control from a browser. Both provide the same
receiver and interface.

| Installation | Best for |
|---|---|
| [Desktop application](#desktop-application) | A radio connected to your computer |
| [Portable server](#portable-server) | A Raspberry Pi, home server, or remote receiver |
| [Homebrew](#homebrew) | Package management on macOS or Linux |
| [Nix](#nix) | Linux systems managed with Nix |
| [Container](#container) | A persistent server with Docker |

## Desktop application

Download your platform's installer from
[GitHub Releases](https://github.com/Newspicel/sdrminusminus/releases), install it, and open sdr--.
The app starts its server automatically on a private local port.

| Platform | Package |
|---|---|
| macOS | `.dmg` for Apple silicon or Intel |
| Linux | `.deb` or `.AppImage` |
| Windows | `.msi` or `.exe` |

## Portable server

Download and unpack the `sdrmm` archive for your operating system and processor from
[GitHub Releases](https://github.com/Newspicel/sdrminusminus/releases). Run the binary:

```sh
./sdrmm
```

On Windows, run `sdrmm.exe`. Open <http://localhost:8080> on the server or
`http://<server>:8080` from another computer.

The server listens on all network interfaces without authentication by default. Configure
[a shared token and HTTPS](../server/configuration.md) before allowing untrusted network access.

## Homebrew

Add the tap:

```sh
brew tap newspicel/tap
```

For the macOS desktop app:

```sh
brew install --cask sdrminusminus
```

For the server on macOS or Linux:

```sh
brew install sdrmm
brew services start sdrmm
```

The cask installs into `/Applications`. The service runs the server in the background and starts
it at login. Open <http://localhost:8080>.

## Nix

With flakes enabled, install and launch the desktop app on x86_64 or aarch64 Linux:

```sh
nix --extra-experimental-features 'nix-command flakes' \
  profile install github:Newspicel/sdrminusminus
sdrmm-desktop
```

The flake exposes the desktop package as `sdrmm-desktop`, `sdrmm`, and `default`.
To build it from a checkout:

```sh
nix --extra-experimental-features 'nix-command flakes' build
```

The result is `result/bin/sdrmm-desktop`.

The Nix package uses SoapySDR for local radios. Select their modules with `soapyPlugins`.
This NixOS example assumes the repository is declared as the `sdrminusminus` flake input:

```nix
environment.systemPackages = [
  (inputs.sdrminusminus.packages.${pkgs.stdenv.hostPlatform.system}.sdrmm.override {
    soapyPlugins = with pkgs; [ soapyrtlsdr soapyremote ];
  })
];

hardware.rtl-sdr.enable = true;
users.users.your-user.extraGroups = [ "plugdev" ];
```

Keep only the modules and hardware options you need. The package provides the SoapySDR core;
modules and USB permissions come from your configuration.

## Container

On Linux, start the supplied Docker Compose service:

```sh
git clone https://github.com/Newspicel/sdrminusminus.git
cd sdrminusminus
docker compose pull
docker compose up -d
```

Open <http://localhost:8080>. The service persists its database and recordings in `sdrmm-data`.
For USB radios, set `group_add` to the host group that owns the device. See
[container setup](../server/deployment.md) for permissions, authentication, and HTTPS.

## Connect your radio

Open a **Device** node and select your receiver. Many radios work with the built-in drivers;
others need a vendor library or SoapySDR module. The [hardware guide](../hardware.md) lists the
requirements for each receiver and package.

If a radio is missing, select **Check hardware** on an unbound Device node or run `sdrmm --doctor`.

## Stable and nightly builds

Use a stable release for regular use. Desktop apps check for stable updates at startup.
The rolling [nightly release](https://github.com/Newspicel/sdrminusminus/releases/tag/nightly)
follows `main` and may change saved-data formats without migration support. Stable apps do not
automatically update to nightlies.

## Build from source

Follow [Build and test](../development/building.md) to develop sdr-- or choose custom backends.

## Next step

Follow [Your first receiver](first-receiver.md) to listen to broadcast FM with an RTL-SDR.
