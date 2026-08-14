# Radios and hardware

sdr-- uses SoapySDR 0.8 for local radios and native clients for `rtl_tcp` and SpyServer receivers.
It also includes virtual sources for the signal generator, multi-stream tests, and SigMF playback.

## Supported local hardware

Desktop installers and containers bundle a private SoapySDR 0.8.1 runtime with these modules:

| Hardware or transport | Bundled module |
|---|---|
| RTL-SDR | SoapyRTLSDR |
| HackRF | SoapyHackRF |
| Airspy and Airspy HF+ | SoapyAirspy / SoapyAirspyHF |
| bladeRF | SoapyBladeRF |
| LimeSDR | SoapyLMS7 |
| PlutoSDR and libiio devices | SoapyPlutoSDR |
| Remote Soapy server | SoapyRemote |

UHD is not included in the base package because of its size. SDRplay is supported through a
separately installed runtime and module. Other modules may work if they match the SoapySDR 0.8
module ABI, but are not part of the release test matrix.

Portable headless archives and source builds use the host SoapySDR installation. The release
baseline is SoapySDR 0.8.1, SoapyRTLSDR 0.3.3, and SoapyHackRF 0.3.4; the complete curated set is
listed in
[`packaging/soapy/environment.yml`](https://github.com/Newspicel/sdrminusminus/blob/main/packaging/soapy/environment.yml).

## Check your installation

Run diagnostics before opening a radio:

```sh
sdrmm --doctor
```

The report shows compiled backends, SoapySDR core version, module search paths, discovered modules
and devices, data paths, and Linux USB permission checks. The same report is available from
**Hardware not showing up?** inside an unbound Device node.

For host installations, SoapySDR's own utility is also useful:

```sh
SoapySDRUtil --info
SoapySDRUtil --find
SoapySDRUtil --probe="driver=rtlsdr"
```

If `SoapySDRUtil` cannot find the receiver, fix the driver or permission problem before debugging
sdr--.

## Linux USB permissions

sdr-- does not need to run as root when the normal udev rules for the receiver are installed. On
Debian-derived systems, the driver package commonly installs them. After adding or changing a
rule, reload udev or unplug and reconnect the device.

For containers, the host rules still decide whether the unprivileged container user can open the
USB node. Passing `/dev/bus/usb` is necessary but does not override its permissions. See
[Containers and remote radios](server/deployment.md#usb-devices) for a Compose example that also
survives reconnects.

## Device controls

The interface is generated from the capabilities and setting metadata reported by the driver. A
radio may expose:

- separate RX and TX streams;
- device-wide or per-stream tuning;
- sample rates and analog bandwidths;
- antennas, gain stages, AGC, clock and time sources;
- driver-specific booleans, enums, ranges, and text settings.

Changing a setting that affects capabilities causes sdr-- to read the device back before
validating the rest. For example, RTL-SDR direct sampling changes the available frequency range.

### RTL-SDR settings

Current SoapyRTLSDR builds commonly expose `direct_samp`, `iq_swap`, `offset_tune`, `digital_agc`,
and, when supported, `biastee`, `testmode`, or `dithering`. `iq_swap` reverses spectral orientation
by exchanging I and Q; it is independent of direct sampling.

## SDRplay

RSP1, RSP1A/B, RSP2, RSPduo, and RSPdx/R2 receivers work through
[SoapySDRPlay3](https://github.com/pothosware/SoapySDRPlay3). The tested baseline is module 0.5.2
with SDRplay API 3.15 or newer.

Install the proprietary API for your platform from [SDRplay](https://www.sdrplay.com/api/) first.
Its service, library, and hardware driver are licensed for genuine SDRplay hardware and are not
redistributed with sdr--.

For a source build or portable server that uses the host SoapySDR runtime, install a matching
SoapySDRPlay3 module into that runtime:

```sh
git clone --branch soapy-sdrplay3-0.5.2 --depth 1 \
  https://github.com/pothosware/SoapySDRPlay3.git
cmake -S SoapySDRPlay3 -B SoapySDRPlay3/build -DCMAKE_BUILD_TYPE=Release
cmake --build SoapySDRPlay3/build --parallel
sudo cmake --install SoapySDRPlay3/build
SoapySDRUtil --find="driver=sdrplay"
```

Desktop installers and the container use their own Soapy tree. A compatible local extension must
place the module in that tree's `lib/SoapySDR/modules0.8` directory and make the vendor API library
and service available in the same environment. The container module directory is
`/opt/conda/lib/SoapySDR/modules0.8`.

RSPduo operating modes appear as distinct devices even when they share a serial number. sdr--
keeps the selected mode in the stored device identity and handles Dual Tuner streams separately.

## Network receivers

An unbound Device node offers **Radio on the network?** for direct `rtl_tcp` and SpyServer
connections. Enter a hostname or address; ports default to `1234` for `rtl_tcp` and `5555` for
SpyServer. These protocols are available in normal builds without SoapySDR.

SoapyRemote is a different transport. A host running `SoapySDRServer` is discovered and operated
through the bundled SoapyRemote module instead of the direct network-device form.

## Validate real hardware

Before relying on an unattended receiver, test the exact packaged build and radio:

1. Run `sdrmm --doctor` and save the module versions.
2. Probe the device with `SoapySDRUtil` when using a host runtime.
3. Stream for at least 30 minutes and watch Device overruns.
4. Unplug and reconnect once; verify the workspace binds to the same radio again.
5. Exercise tuning, gain, AGC, bandwidth, antenna, and advertised advanced settings.
6. Make and replay a short recording.

For transmit-capable radios, sdr-- reports TX capabilities, but the general transmit workflow is
not yet available. Validate driver TX behavior only in a shielded, attenuated, legally authorized
bench setup.
