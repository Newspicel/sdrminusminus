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
window. Desktop installers include a private SoapySDR runtime and the supported hardware modules,
so installing a second system-wide copy of SoapySDR is unnecessary. SDRplay receivers need one
extra step: their vendor API is licensed for genuine SDRplay hardware and is installed from
[SDRplay](https://www.sdrplay.com/downloads/), not by this installer — see
[SDRplay](../hardware.md#sdrplay).

## Portable server

Portable `sdrmm` archives are useful on a Raspberry Pi, home server, or machine you want to access
from another browser. Unpack the archive and run:

```sh
./sdrmm
```

The server listens on every interface at port `8080` by default. Open `http://<server>:8080` from
a browser on the same network.

Portable archives use the host's SoapySDR 0.8 runtime. Install the core library and the module for
your receiver before starting sdr--. Run `sdrmm --doctor` to confirm what the binary can see.

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

The package links to Nixpkgs' SoapySDR core but deliberately bundles no hardware modules. On NixOS,
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

Building is the right choice when you are contributing, need a specialized backend selection, or
want to package sdr-- for another environment. Follow [Build and test](../development/building.md)
for the complete toolchain and commands.

## Next step

Every installation includes a virtual signal source. Continue with
[Your first receiver](first-receiver.md) before connecting hardware.
