# Radios and hardware

sdr-- opens a receiver in one of four ways: a built-in driver, a bundled SoapySDR module, a network
protocol, or a virtual source. This page lists what each one supports and how to get a radio
working.

## Built-in drivers

Standard builds include native drivers for RTL-SDR, HackRF, SDRplay RSP, and Dragon Labs CR-8.
These drivers do not require SoapySDR modules. Custom builds can omit them through feature flags.

| Receiver | Extra software |
|---|---|
| RTL-SDR | none |
| HackRF | none |
| SDRplay RSP1, RSP1A, RSP1B, RSP2, RSPduo, RSPdx, RSPdx-R2 | SDRplay API 3.15 or newer, see [SDRplay](#sdrplay) |
| Dragon Labs CR-8 | the vendor CR-8 library, see [Dragon Labs CR-8](#dragon-labs-cr-8) |

If a host SoapyRTLSDR, SoapyHackRF or SoapySDRPlay3 module is installed, it is skipped for these
receivers so that one radio is never listed twice.

## SoapySDR modules

Desktop installers and containers ship a private SoapySDR 0.8.1 runtime with these modules:

| Receiver | Module |
|---|---|
| Airspy and Airspy HF+ | SoapyAirspy / SoapyAirspyHF |
| bladeRF | SoapyBladeRF |
| LimeSDR | SoapyLMS7 |
| PlutoSDR and libiio devices | SoapyPlutoSDR |
| Remote Soapy server | SoapyRemote |

The exact versions are pinned in
[`packaging/soapy/environment.yml`](https://github.com/Newspicel/sdrminusminus/blob/main/packaging/soapy/environment.yml).
UHD is not bundled because of its size. Other modules may work if they match the SoapySDR 0.8
module ABI, but they are not part of the release test matrix; a module built against a different
SoapySDR generation is refused and logged rather than loaded.

Bundled installations use their own modules unless you add an explicit search path. Portable
archives and source builds use the host's SoapySDR installation. Native drivers are independent
of this module selection.

To load a module the bundle does not carry, point `SDRMM_SOAPY_MODULE_PATH` at the directory
holding it. Those directories are searched before the bundled ones.

## Network receivers

On an unbound Device node, open **Radio on the network?** and enter a hostname or address. Ports
default to `1234` for `rtl_tcp` and `5555` for SpyServer. Both protocols are built in and work
without SoapySDR.

SoapyRemote is a separate path: a host running `SoapySDRServer` is discovered automatically and
appears in the normal device list, so use that instead of the network form.

## Virtual sources

Every build includes sources that need no hardware: a signal generator, a four-lane coherent array,
a 2×2 transceiver, a half-duplex 1×1 transceiver, and playback of any SigMF recording in the
library. [Your first receiver](getting-started/first-receiver.md) builds a working receiver on the
signal generator.

## Check the installation

Run the diagnostic report before opening a radio:

```sh
sdrmm --doctor
```

It lists compiled backends, the SoapySDR core version, module search paths, discovered modules and
devices, data paths, and Linux USB permission checks. The same report is available from **Hardware
not showing up?** on an unbound Device node.

On a host SoapySDR installation, its own utility is also worth running:

```sh
SoapySDRUtil --info
SoapySDRUtil --find
SoapySDRUtil --probe="driver=airspy"
```

For a receiver using SoapySDR, fix discovery or permission errors here before starting sdr--.
Use `sdrmm --doctor` for receivers handled by native drivers.

## How radios are discovered

SoapySDR discovery runs in a short-lived child process because vendor modules can crash or hang
while probing hardware. A failed probe produces a warning without terminating the application.

Discovery runs when attached USB devices change and once a minute to find network radios.
For driver debugging, `SDRMM_SOAPY_PROBE=in-process` runs discovery in the application process.

## Linux USB permissions

sdr-- does not need root as long as the receiver's udev rules are installed. On Debian-derived
systems the driver package usually installs them. After adding or changing a rule, reload udev or
reconnect the device.

In containers, the host rules still decide whether the unprivileged container user can open the USB
node. Passing `/dev/bus/usb` is necessary but does not override its permissions. See
[Containers and remote radios](server/deployment.md#usb-devices) for a Compose example that also
survives reconnects.

## Device controls

The device face is generated from the capabilities and setting metadata the driver reports, so it
differs by model. A radio may expose:

- separate RX and TX streams;
- device-wide or per-stream tuning;
- sample rates as a menu, as continuous windows, or as both;
- analog bandwidths, likewise;
- antennas, gain stages, AGC, and clock and time sources;
- driver-specific booleans, enums, ranges, and text settings.

Changing a setting that affects capabilities makes sdr-- re-read the device before validating the
rest. RTL-SDR direct sampling is one example: it changes the available frequency range.

## RTL-SDR

The built-in driver exposes:

| Setting | Effect |
|---|---|
| `TUNER` | the tuner gain stage |
| `ppm` | crystal frequency correction |
| `bias_tee` | phantom power on the antenna port |
| `agc` | R82xx tuner AGC |
| `direct_sampling` | `off`, `i`, or `q` |

**Gain.** The slider uses the tuner's actual gain table. Values from the API or saved settings
are rounded to the nearest supported entry; for example, an R820T request for 20 dB returns 19.7 dB.

**Sample rate.** The RTL2832U supports 225–300 kHz and 900 kHz–3.2 MHz. Rates in the gap are
rejected. The picker also offers a menu of common rates.

**IF filter.** The R82xx driver exposes a 0–8 MHz setting. Use `0` for automatic bandwidth matched
to the sample rate.

**Direct sampling.** This is available except on RTL-SDR Blog V4 boards. The V4 uses an upconverter
for HF; tune below 28.8 MHz without changing the direct-sampling setting.

## HackRF

Three gain stages and one switch:

| Setting | Range |
|---|---|
| `LNA` | 8 dB steps |
| `VGA` | 2 dB steps |
| `AMP` | the switched +14 dB RF amplifier, rendered as a switch |
| `bias_tee` | phantom power on the antenna port |

`AMP` appears as a switch and contributes to the displayed total gain.

## SDRplay

RSP receivers use a driver built into sdr--, with no SoapySDR module involved. That driver needs
SDRplay's own API, which is licensed for use with genuine SDRplay hardware and cannot be
redistributed, so you install it yourself:

1. Download and install the API from [SDRplay](https://www.sdrplay.com/downloads/). Version 3.15 or
   newer is required.
2. Plug in the RSP. It appears in the device list.

sdr-- loads the vendor library at runtime from the install location, usually `/usr/local/lib`
on Linux and macOS or `C:\Program Files\SDRplay\API` on Windows. The library is not bundled.
Without it, no RSP appears in the device list. `sdrmm --doctor` reports the result under
**SDRplay API**.

The API also runs a background service (`sdrplay_apiService`). If the service is stopped, the
doctor check reports that the API is not responding even though it is installed. Start the service
and retry.

### Gain

SDRplay hardware is specified in gain *reduction*. sdr-- presents both stages as gain, so higher is
always more signal:

- **RF** — the LNA state, in dB of gain relative to that band's weakest state. The available steps
  change with frequency, the selected port and HDR mode, so the range is re-read whenever tuning
  moves to another band.
- **IF** — 0 to 39 dB, the inverse of the API's 20–59 dB IF gain reduction.

AGC controls the IF stage. With AGC enabled, the IF slider sets the starting point and the setpoint
extra sets the target level in dBFS.

### Sample rates

Single-tuner modes sample the ADC between 2 and 10.66 MHz. Rates below 2 MHz are reached by
decimating a legal ADC rate, so anything from 62.5 kHz to 10.66 MHz is available.

### RSPduo

The RSPduo appears once per operating mode it can currently offer: Tuner 1, Tuner 2, Dual Tuner,
Master and Slave. The chosen mode is part of the stored device identity, so a saved node comes back
in the same mode. Dual Tuner gives one device with two independently tuned streams.

Dual Tuner, Master and Slave run the ADC at a fixed 6 MHz with a 1.62 MHz IF. Their sample rates
are therefore 2 MHz and each halving below it down to 62.5 kHz, with analog bandwidth capped at
1.536 MHz. A Slave waits for its master application to start; if no master appears, starting the
stream reports that it is still waiting. The master owns the clock, so a slave cannot change the
sample rate beyond its own decimation and cannot apply a ppm correction.

If another application already holds the RSPduo, only the modes still free are listed.

### Licensing

The interface to the vendor library is written in Rust from the public
[SDRplay API specification](https://www.sdrplay.com/api/), whose legal notice grants a royalty-free
licence to use the information in it to design software that uses SDRplay receivers. No SDRplay
source, header or binary is copied into this project or shipped with it, and the gain tables above
come from the same document.

## Dragon Labs CR-8

The CR-8 provides eight `phase_coherent` lanes sharing a clock and synthesizer. Add one Device
node and use `iq` through `iq8`; no Array node is needed. These lanes support calibration,
direction finding, beamforming, and passive radar.

The vendor library is loaded at runtime and must be installed separately. Run `sdrmm --doctor`
to check whether it loaded. Without it, no CR-8 is discovered.

| Setting | Behaviour |
|---|---|
| Frequency | All eight channels are tuned together, in one coherent call |
| Sample rate | Fixed at 12.5 MS/s; any other rate is refused |
| Gain | LNA, mixer and VGA, settable per channel |
| Clock source | The on-board oscillator, or a 10 MHz reference on the external input |

If the library is somewhere the loader will not look, point `SDRMM_DLCR_LIBRARY` at it.

The reported CR-8 tuning range comes from its documentation; the SDK does not expose that range.

## Before an unattended deployment

Test the exact packaged build against the exact radio:

1. Run `sdrmm --doctor` and save the module versions.
2. On a host runtime, probe the device with `SoapySDRUtil`.
3. Stream for at least 30 minutes and watch the Device overrun counter.
4. Unplug and reconnect once, and confirm the workspace binds to the same radio again.
5. Exercise tuning, gain, AGC, bandwidth, antenna, and the advertised advanced settings.
6. Make a short recording and replay it.

sdr-- reports the TX capabilities of transmit-capable radios, but the transmit workflow is not
available yet. Validate driver TX behaviour only in a shielded, attenuated, legally authorized
bench setup.
