# Radios and hardware

sdr-- drives RTL-SDR, HackRF and SDRplay RSP receivers with its own built-in drivers, reaches
everything else through SoapySDR 0.8, and speaks to `rtl_tcp` and SpyServer receivers over the
network. It also includes virtual sources for the signal
generator, multi-stream tests, and SigMF playback.

## Supported local hardware

RTL-SDR, HackRF and SDRplay receivers need no SoapySDR module. Their drivers are part of sdr--
itself, so they work in every build — including the minimal one — and are hidden from SoapySDR's
search so one radio is never listed twice. RTL-SDR and HackRF need no C library at all; SDRplay
needs the vendor API installed, see [SDRplay](#sdrplay).

| Hardware or transport | Driver |
|---|---|
| RTL-SDR | built in |
| HackRF | built in |
| SDRplay RSP | built in, see [SDRplay](#sdrplay) |

Everything else goes through a private SoapySDR 0.8.1 runtime that desktop installers and
containers bundle:

| Hardware or transport | Bundled module |
|---|---|
| Airspy and Airspy HF+ | SoapyAirspy / SoapyAirspyHF |
| bladeRF | SoapyBladeRF |
| LimeSDR | SoapyLMS7 |
| PlutoSDR and libiio devices | SoapyPlutoSDR |
| Remote Soapy server | SoapyRemote |

UHD is not included in the base package because of its size. Other modules may work if they match
the SoapySDR 0.8 module ABI, but are not part of the release test matrix.

Portable headless archives and source builds use the host SoapySDR installation for the modules
above; the built-in drivers are unaffected either way. The release baseline is SoapySDR 0.8.1, and
the complete curated set is listed in
[`packaging/soapy/environment.yml`](https://github.com/Newspicel/sdrminusminus/blob/main/packaging/soapy/environment.yml).

A host-installed SoapyRTLSDR or SoapyHackRF is ignored rather than competing with the built-in
driver, so a dongle is listed once whatever else is on the machine. A bundled installation
otherwise loads only its own modules, so a radio can never be served by two copies of the same
driver. To add a module the bundle does not carry — a vendor module, or one you compiled
yourself — point `SDRMM_SOAPY_MODULE_PATH` at the directory holding it; those directories are
searched before the bundled ones. A module built against a different SoapySDR generation is
refused by the core and logged rather than loaded.

## How radios are detected

Vendor SoapySDR modules open USB devices while searching for radios, and a faulty one can take
down the process it runs in: some abort, some crash, and at least one leaks a USB context on every
search. sdr-- therefore searches for radios in a short-lived child process. A driver that crashes
or hangs costs one search, logged as a warning, instead of the application, and whatever a driver
leaks is returned to the system when that child exits.

The search itself runs when the set of attached USB devices changes, and once a minute so that
network radios are still found. Set `SDRMM_SOAPY_PROBE=in-process` to search in the application
process instead, which is only useful when debugging a driver.

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
- sample rates, as a menu, as continuous windows, or as both;
- analog bandwidths, likewise;
- antennas, gain stages, AGC, clock and time sources;
- driver-specific booleans, enums, ranges, and text settings.

Changing a setting that affects capabilities causes sdr-- to read the device back before
validating the rest. For example, RTL-SDR direct sampling changes the available frequency range.

### RTL-SDR settings

The built-in driver exposes one `TUNER` gain stage, a `ppm` crystal correction, `bias_tee` for
phantom power on the antenna port, `agc` for the R82xx tuner AGC, and `direct_sampling` (`off`,
`i`, `q`) on every board except the RTL-SDR Blog V4, whose HF path is an upconverter in front of
the tuner rather than a tuner bypass — on that board HF is reached by tuning below 28.8 MHz with
no setting changed.

The tuner's gain table is not evenly spaced, so it is published as the table it is rather than as
a step: the gain slider walks the 29 real settings and cannot land between them. A request that
does come from elsewhere — the API, a stored workspace — is snapped to the nearest entry, and the
snapped value is what the driver reports back. Asking for 20 dB on an R820T therefore reads back
as 19.7 dB: that is the gain the hardware was programmed with, not a rounding error.

The RTL2832U resamples across two windows, 225–300 kHz and 900 kHz–3.2 MHz, and aliases between
them. Both are published, so a rate in the gap is refused rather than quietly accepted. The
familiar rate menu is published alongside them and is what the picker offers.

The R82xx IF filter is continuous rather than a menu, so it is published as a 0–8 MHz window;
0 selects the automatic width that tracks the sample rate.

### HackRF settings

Three gain stages — `LNA` in 8 dB steps, `VGA` in 2 dB steps, and `AMP`, the switched +14 dB RF
amplifier. A switched amplifier is still gain, so it is a gain stage with two settings rather
than a boolean hidden somewhere else; the client renders it as a switch, and it shows up in the
gain budget where it belongs. `bias_tee` supplies phantom power on the antenna port.

## SDRplay

RSP1, RSP1A, RSP1B, RSP2, RSPduo, RSPdx and RSPdx-R2 receivers work through a driver built into
sdr--, with no SoapySDR module involved. The driver needs the SDRplay vendor API installed:

1. **The driver** — part of sdr--. Nothing to install.
2. **The vendor API** — the library, service and hardware driver, licensed for use with genuine
   SDRplay hardware and not licensed for redistribution. Install it yourself from
   [SDRplay](https://www.sdrplay.com/downloads/). Version 3.15 or newer is required.

Install the API, plug in the RSP, and the receiver appears. sdr-- resolves the vendor library at
runtime from wherever the installer put it — `/usr/local/lib` on Linux and macOS,
`C:\Program Files\SDRplay\API` on Windows — so it is never linked, never bundled, and its absence
costs nothing: a machine without it simply lists no RSP. `sdrmm --doctor` reports what it found
under **SDRplay API**.

The API also runs a background service (`sdrplay_apiService`). If it is stopped, the doctor check
says the service is not responding even though everything is installed — start the service and
retry.

If a host-installed SoapySDRPlay3 module is also present, sdr-- prefers its own driver for that
receiver, so an RSP is never listed twice.

### Gain

SDRplay hardware is specified in gain *reduction*; sdr-- presents both stages as gain, so higher
is always more signal:

- **RF** — the LNA state, in dB of gain relative to that band's weakest state. The available
  steps change with frequency, the selected port, and HDR mode, so the range is re-read whenever
  tuning moves to another band.
- **IF** — 0 to 39 dB, the inverse of the 20–59 dB IF gain reduction the API takes.

AGC controls the IF stage: with it enabled the IF slider sets the starting point and the setpoint
extra sets the target level in dBFS.

### Sample rates

Single-tuner modes sample the ADC between 2 and 10.66 MHz, and rates below 2 MHz are reached by
decimating a legal ADC rate, so anything from 62.5 kHz to 10.66 MHz works.

### RSPduo

The RSPduo appears once per operating mode it can currently offer — Tuner 1, Tuner 2, Dual Tuner,
Master and Slave — and the chosen mode is part of the stored device identity, so a saved node
comes back in the same mode. Dual Tuner gives one device with two independently tuned streams.

Dual Tuner, Master and Slave run the ADC at a fixed 6 MHz with a 1.62 MHz IF, which puts their
sample rates at 2 MHz and each halving below it down to 62.5 kHz, with the analog bandwidth capped
at 1.536 MHz. A Slave waits for its master application to start; if no master appears, starting
the stream reports that it is still waiting. The master owns the clock, so a slave cannot change
the sample rate beyond its own decimation, and cannot apply a ppm correction.

### Where the driver comes from

The interface to the vendor library is written in Rust from the public
[SDRplay API specification](https://www.sdrplay.com/api/), whose legal notice grants a royalty-free
licence to use the information in it to design software that uses SDRplay receivers. No SDRplay
source, header or binary is copied into this project or shipped with it, and the gain tables above
come from the same document.

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
