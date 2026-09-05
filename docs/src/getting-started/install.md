# Install sdr--

sdr-- is distributed as a desktop application, a portable headless server, and a container. All
three run the same receiver engine and serve the same interface.

## Desktop application

The desktop app is the simplest option for a radio connected directly to your computer. Download
the installer for your platform from [GitHub Releases](https://github.com/Newspicel/sdrminusminus/releases):

| Platform | Packages |
|---|---|
| macOS | `.dmg` for Apple silicon and Intel |
| Linux | `.deb` and `.AppImage` |
| Windows | `.msi` and `.exe` installers |

The app starts its receiver server on a private loopback port and opens the interface in a native
window. Desktop installers include SoapySDR and the supported hardware modules. SDRplay receivers also
require the separately installed vendor API; see [SDRplay receivers](../hardware.md#sdrplay).
CR-8 receivers need the [vendor library](../hardware.md#dragon-labs-cr-8).

## Portable server

Portable `sdrmm` archives are useful on a Raspberry Pi, home server, or machine you want to access
from another browser. Unpack the archive and run:

```sh
./sdrmm
```

The server listens on every interface at port `8080` by default. Open `http://<server>:8080` from
a browser on the same network.

Portable archives require the host's SoapySDR 0.8 runtime. Receivers handled through SoapySDR also
need their hardware module; native drivers do not. Run `sdrmm --doctor` to check available drivers
and devices.

## Homebrew

On macOS, both packages come from the project's tap:

```sh
brew tap newspicel/tap
brew install --cask sdrminusminus
brew install sdrmm
```

The cask installs the desktop application into `/Applications`. The formula installs the `sdrmm`
server and Homebrew's SoapySDR alongside it; `brew services start sdrmm` runs the server in the
background and restarts it at login.

The formula also works on Homebrew for Linux. It installs the same portable binary published on
the releases page, not a source build.

## Nix

On NixOS or another Linux system with flakes enabled, install the Tauri desktop application
directly from GitHub:

```sh
nix --extra-experimental-features 'nix-command flakes' \
  profile install github:Newspicel/sdrminusminus
sdrmm-desktop
```

The flake supports x86_64 and aarch64 Linux and exposes `sdrmm-desktop`, `sdrmm`, and `default`
packages for each system. From a checkout, the following creates `result/bin/sdrmm-desktop`:

```sh
nix --extra-experimental-features 'nix-command flakes' build
```

The package links to Nixpkgs' SoapySDR core and bundles no SoapySDR hardware modules. On NixOS,
select the modules and device permissions in your system configuration. For example, with this
repository declared as the `sdrminusminus` flake input:

```nix
environment.systemPackages = [
  (inputs.sdrminusminus.packages.${pkgs.stdenv.hostPlatform.system}.sdrmm.override {
    soapyPlugins = with pkgs; [
      soapyrtlsdr
      soapyhackrf
    ];
  })
];

hardware.rtl-sdr.enable = true;
hardware.hackrf.enable = true;
users.users.your-user.extraGroups = [ "plugdev" ];
```

Remove whichever module and hardware option you do not need. The selected plugins remain separate
Nix store packages managed by NixOS; the application wrapper only points SoapySDR at them.

## Container

The published container includes the web interface, SoapySDR, and the supported open-source
hardware modules:

```sh
docker run --rm \
  -p 8080:8080 \
  -v sdrmm-data:/data \
  --device /dev/bus/usb:/dev/bus/usb \
  ghcr.io/newspicel/sdrminusminus:latest
```

On Linux, USB access still depends on host udev permissions. The repository's
`docker-compose.yml` also includes a device cgroup rule that keeps replugged USB devices
accessible. See [Containers and remote radios](../server/deployment.md) for a durable setup.

## Stable and nightly builds

Stable releases use semantic versions and are suitable for persistent installations. The rolling
[`nightly`](https://github.com/Newspicel/sdrminusminus/releases/tag/nightly) release is rebuilt
from `main` when it changes. Nightlies use a date version and should be treated as prereleases.

Desktop builds check stable releases for updates at startup. Nightly releases are not offered as
updates to stable installations.

## Build from source

To contribute, choose a custom set of backends, or package another platform, follow
[Build and test](../development/building.md).

## Next step

Every installation includes a virtual signal source. Continue with
[Your first receiver](first-receiver.md) before connecting hardware.
