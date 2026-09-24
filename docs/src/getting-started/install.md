# Install

Pick the desktop app when the radio is plugged into your computer. Pick the server when the radio
sits somewhere else and you connect from a browser. Both run the same receiver.

| Installation | Best for |
|---|---|
| [Desktop app](#desktop-app) | A radio on your computer |
| [Portable server](#portable-server) | A Raspberry Pi, home server, or remote receiver |
| [Homebrew](#homebrew) | macOS or Linux with Homebrew |
| [WinGet](#winget) | Windows |
| [APT](#apt) | Debian and Ubuntu |
| [DNF](#dnf) | Fedora |
| [AUR](#aur) | Arch Linux |
| [Nix](#nix) | Linux managed with Nix |
| [Container](#container) | Docker |

## Desktop app

Download the installer from the [download page](/download.html) and open SDR--. The app starts
its own server on a random port only this computer can reach. For a fixed port or access from
other devices, run the [server](#portable-server) instead.

| Platform | Package |
|---|---|
| macOS | `.dmg` for Apple silicon or Intel |
| Linux | `.deb`, `.rpm` or `.AppImage` for x86-64 or ARM64 |
| Windows | `.msi` or `.exe` for x86-64, `.exe` for ARM64 |

## Portable server

Download and unpack the `sdrmm` archive for your system from the [download page](/download.html),
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
brew tap newspicel/tap
brew install --cask sdrminusminus   # macOS desktop app
brew install sdrmm                  # server, macOS or Linux
brew services start sdrmm
```

The cask installs into `/Applications`. The service starts the server at login. Open
<http://localhost:8080>.

## WinGet

```powershell
winget install Newspicel.SDRminusminus
```

## APT

```sh
curl -fsSL https://newspicel.github.io/packages/key.gpg \
  | sudo tee /usr/share/keyrings/sdrminusminus.gpg > /dev/null
echo "deb [signed-by=/usr/share/keyrings/sdrminusminus.gpg] https://newspicel.github.io/packages/deb stable main" \
  | sudo tee /etc/apt/sources.list.d/sdrminusminus.list
sudo apt update
sudo apt install sdrminusminus
```

APT and DNF install the desktop app. For the `sdrmm` server, use the
[portable server](#portable-server).

## DNF

```sh
sudo dnf config-manager addrepo \
  --from-repofile=https://newspicel.github.io/packages/rpm/sdrminusminus.repo
sudo dnf install sdrminusminus
```

## AUR

```sh
yay -S sdrminusminus-bin   # desktop app
yay -S sdrmm-bin           # server
```

## Nix

Install and launch the desktop app on x86_64 or aarch64 Linux:

```sh
nix --extra-experimental-features 'nix-command flakes' \
  profile install github:Newspicel/sdrminusminus
sdrmm-desktop
```

The flake exports the package as `sdrmm-desktop`, `sdrmm`, and `default`. From a checkout,
`nix build` produces `result/bin/sdrmm-desktop`.

The Nix package reaches local radios through SoapySDR. Pick the modules with `soapyPlugins`.
This NixOS example assumes the flake input is named `sdrminusminus`:

```nix
environment.systemPackages = [
  (inputs.sdrminusminus.packages.${pkgs.stdenv.hostPlatform.system}.sdrmm.override {
    soapyPlugins = with pkgs; [ soapyrtlsdr soapyremote ];
  })
];

hardware.rtl-sdr.enable = true;
users.users.your-user.extraGroups = [ "plugdev" ];
```

## Container

On Linux:

```sh
git clone https://github.com/Newspicel/sdrminusminus.git
cd sdrminusminus
docker compose up -d
```

Open <http://localhost:8080>. Data lives in the `sdrmm-data` volume. For USB radios, tokens, and
HTTPS, see [Deployment](../server/deployment.md#docker-compose).

## Stable or nightly

Use a stable release. The desktop app checks for stable updates at startup and never moves to a
nightly on its own. The [nightly release](https://github.com/Newspicel/sdrminusminus/releases/tag/nightly)
follows `main` and may change saved data without a migration.

## Next

- Plug in a radio and check [Radios](../hardware.md) if it needs a driver.
- Build [your first receiver](first-receiver.md).
- To build from source, see [Build and test](../development/building.md).
