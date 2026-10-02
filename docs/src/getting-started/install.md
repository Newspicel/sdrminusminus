# Install

Pick the desktop app when the radio is plugged into your computer. Pick the server when the radio
sits somewhere else and you connect from a browser. Both run the same receiver.

| Installation | Best for |
|---|---|
| [Desktop app](#desktop-app) | A radio on your computer |
| [Portable server](#portable-server) | A Raspberry Pi, home server, or remote receiver |
| [Homebrew](#homebrew) | macOS or Linux with Homebrew |
| [APT](#apt) | Debian and Ubuntu |
| [DNF](#dnf) | Fedora |
| [Nix](#nix) | Linux managed with Nix |
| [Container](#container) | Docker |

## Desktop app

Download the installer from the [download page](/download) and open SDR--. The app starts
its own server on a random port only this computer can reach. For a fixed port or access from
other devices, run the [server](#portable-server) instead.

| Platform | Package |
|---|---|
| macOS | `.dmg` for Apple silicon or Intel |
| Linux | `.deb`, `.rpm` or `.AppImage` for x86-64 or ARM64 |
| Windows | `.msi` or `.exe` for x86-64, `.exe` for ARM64 |

## Portable server

Download and unpack the `sdrmm` archive for your system from the [download page](/download),
then run it:

```sh
./sdrmm
```

On Windows, run `sdrmm.exe`. Open <http://localhost:8080> on the server, or
`http://<server>:8080` from another computer.

The server listens on every network interface with no password. Set up
[a token and HTTPS](../server/configuration.md) before untrusted devices can reach it.

## Homebrew

```sh
brew install newspicel/tap/sdrmm-app       # macOS desktop app
brew install sdrmm                         # server, macOS or Linux
brew services start sdrmm
```

The cask installs into `/Applications`. The service starts the server at login. Open
<http://localhost:8080>.

## APT

```sh
curl -fsSL https://downloads.sdrmm.com/packages/key.gpg \
  | sudo tee /usr/share/keyrings/sdrmm.gpg > /dev/null
echo "deb [signed-by=/usr/share/keyrings/sdrmm.gpg] https://downloads.sdrmm.com/packages/deb stable main" \
  | sudo tee /etc/apt/sources.list.d/sdrmm.list
sudo apt update
sudo apt install sdrmm-app
```

APT and DNF install the desktop app. For the `sdrmm` server, use the
[portable server](#portable-server).

## DNF

```sh
sudo dnf config-manager addrepo \
  --from-repofile=https://downloads.sdrmm.com/packages/rpm/sdrmm.repo
sudo dnf install sdrmm-app
```

## Nix

Install and launch the desktop app on x86_64 or aarch64 Linux:

```sh
nix --extra-experimental-features 'nix-command flakes' \
  profile install github:Newspicel/sdrmm
sdrmm-desktop
```

The flake exports the package as `sdrmm-desktop`, `sdrmm`, and `default`. From a checkout,
`nix build` produces `result/bin/sdrmm-desktop`.

Radios without a built-in driver need SoapySDR modules, picked with `soapyPlugins`. For SDRplay,
enable `services.sdrplayApi` and pass the unfree `pkgs.sdrplay` as `sdrplayApi`. This NixOS
example assumes the flake input is named `sdrmm`:

```nix
environment.systemPackages = [
  (inputs.sdrmm.packages.${pkgs.stdenv.hostPlatform.system}.sdrmm.override {
    soapyPlugins = with pkgs; [ soapybladerf soapyremote ];
  })
];

hardware.rtl-sdr.enable = true;
users.users.your-user.extraGroups = [ "plugdev" ];
```

## Container

On Linux:

```sh
git clone https://github.com/Newspicel/sdrmm.git
cd sdrmm
docker compose up -d
```

Open <http://localhost:8080>. Data lives in the `sdrmm-data` volume. For USB radios, tokens, and
HTTPS, see [Deployment](../server/deployment.md#docker-compose).

## Stable or nightly

Use a stable release. The desktop app checks for stable updates at startup and never moves to a
nightly on its own. The [nightly release](https://github.com/Newspicel/sdrmm/releases/tag/nightly)
follows `main` and may change saved data without a migration.

## Next

- Plug in a radio and check [Radios](../hardware.md) if it needs a driver.
- Build [your first receiver](first-receiver.md).
- To build from source, see [Build and test](../development/building.md).
