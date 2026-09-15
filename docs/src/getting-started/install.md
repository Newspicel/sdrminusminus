# Install sdr--

sdr-- ships five ways: a desktop application, a portable headless server, a Homebrew package, a
Nix flake, and a container. All five run the same receiver engine and serve the same interface,
and all five carry the same built-in drivers.

None of them ship SoapySDR. The core library is opened at runtime from whatever SoapySDR the host
has installed, so a machine without one loses nothing except the radios only a SoapySDR module can
reach. Each section below says what its package brings and what it leaves to the system; see
[SoapySDR modules](../hardware.md#soapysdr-modules) for how to add one.

## Built-in everywhere

Every package below drives these without any extra library:

RTL-SDR, KrakenSDR, HackRF, Airspy R2/Mini, Airspy HF+, AD936x boards (AntSDR, ADALM-Pluto),
rtl_tcp, SpyServer, and the virtual signal sources. SDRplay and Dragon Labs CR-8 are built in too
but need their vendor library installed separately — see [the hardware guide](../hardware.md).
The two Airspy drivers are new and not yet confirmed on air; see
[Airspy](../hardware.md#airspy).

## Desktop application

The simplest option for a radio connected directly to your computer. Download the installer for
your platform from [GitHub Releases](https://github.com/Newspicel/sdrminusminus/releases):

| Platform | Packages |
|---|---|
| macOS | `.dmg` for Apple silicon and Intel |
| Linux | `.deb` and `.AppImage` |
| Windows | `.msi` and `.exe` installers |

The app starts its receiver server on a private loopback port and opens the interface in a native
window.

**SoapySDR:** not included. If one is installed on the system the app finds and uses it; on macOS
that includes the Homebrew prefixes, on Windows a PothosSDR installation on `PATH`.

## Portable server

Portable `sdrmm` archives suit a Raspberry Pi, a home server, or any machine you want to reach
from another browser. Unpack the archive and run:

```sh
./sdrmm
```

The server listens on every interface at port `8080` by default. Open `http://<server>:8080` from
a browser on the same network. The archive depends on nothing beyond the system C library, so it
starts whether or not SoapySDR is present.

**SoapySDR:** not included, and not required to start. Install your distribution's SoapySDR
package to reach hardware that needs a module. Run `sdrmm --doctor` to see what was found.

## Homebrew

On macOS, both packages come from the project's tap:

```sh
brew tap newspicel/tap
brew install --cask sdrminusminus
brew install sdrmm
```

The cask installs the desktop application into `/Applications`. The formula installs the `sdrmm`
server; `brew services start sdrmm` runs it in the background and restarts it at login. The
formula also works on Homebrew for Linux, where it installs the same portable binary published on
the releases page rather than building from source.

**SoapySDR:** the formula depends on Homebrew's `soapysdr`, so the server gets a core library
automatically. Add modules the same way:

```sh
brew install soapybladerf soapyremote
```

## Nix

On NixOS or another Linux system with flakes enabled, install the desktop application straight
from GitHub:

```sh
nix --extra-experimental-features 'nix-command flakes' \
  profile install github:Newspicel/sdrminusminus
sdrmm-desktop
```

The flake supports x86_64 and aarch64 Linux and exposes `sdrmm-desktop`, `sdrmm`, and `default`
packages for each system. From a checkout, this creates `result/bin/sdrmm-desktop`:

```sh
nix --extra-experimental-features 'nix-command flakes' build
```

**SoapySDR:** the wrapper points at Nixpkgs' SoapySDR core, and bundles no modules. Select the
modules and device permissions in your system configuration — with this repository declared as the
`sdrminusminus` flake input:

```nix
environment.systemPackages = [
  (inputs.sdrminusminus.packages.${pkgs.stdenv.hostPlatform.system}.sdrmm.override {
    soapyPlugins = with pkgs; [
      soapybladerf
      soapyremote
    ];
  })
];

hardware.rtl-sdr.enable = true;
hardware.hackrf.enable = true;
users.users.your-user.extraGroups = [ "plugdev" ];
```

Remove whichever module and hardware option you do not need. The selected plugins stay separate
Nix store packages managed by NixOS; the application wrapper only points SoapySDR at them.

## Container

The published container includes the web interface and the built-in drivers:

```sh
docker run --rm \
  -p 8080:8080 \
  -v sdrmm-data:/data \
  --device /dev/bus/usb:/dev/bus/usb \
  --group-add 46 \
  ghcr.io/newspicel/sdrminusminus:latest
```

The image runs as an unprivileged user, so passing the bus is not enough on its own: it also needs
the group that owns the radio's device node. `46` is `plugdev`, which is what the vendor udev
rules grant on Debian and Ubuntu. Where no rule is installed the node stays `root:root` mode
`0664`, so pass `--group-add 0` instead. On the host, `stat -c '%g %G %a' /dev/bus/usb/*/*` names
the group; inside the container, **Check hardware** and `sdrmm --doctor` name the node that could
not be opened and the group that owns it.

**SoapySDR:** a container cannot borrow the host's, so the image installs Debian's SoapySDR core
along with the bladeRF, LimeSDR and SoapyRemote modules. To add another, derive an image:

```dockerfile
FROM ghcr.io/newspicel/sdrminusminus:latest
USER root
RUN apt-get update \
    && apt-get install -y --no-install-recommends soapysdr-module-audio \
    && rm -rf /var/lib/apt/lists/*
USER sdrmm
```

The repository's `docker-compose.yml` also includes a device cgroup rule that keeps replugged USB
devices accessible. See [Containers and remote radios](../server/deployment.md) for a durable
setup.

## Stable and nightly builds

Stable releases use semantic versions and suit persistent installations. The rolling
[`nightly`](https://github.com/Newspicel/sdrminusminus/releases/tag/nightly) release is rebuilt
from `main` when it changes. Nightlies use a date version and should be treated as prereleases.

Desktop builds check stable releases for updates at startup. Nightly releases are not offered as
updates to stable installations.

## Build from source

To contribute, choose a custom set of backends, or package another platform, follow
[Build and test](../development/building.md). Building needs no SoapySDR development package:
nothing links it.

## Next step

Every installation includes a virtual signal source. Continue with
[Your first receiver](first-receiver.md) before connecting hardware.
