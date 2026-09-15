# Radios and hardware

sdr-- opens a receiver in one of four ways: a built-in driver, a SoapySDR module from the host's
own SoapySDR installation, a network protocol, or a virtual source. This page lists what each one
supports and how to get a radio working.

## Built-in drivers

Standard builds include native drivers for RTL-SDR, KrakenSDR, HackRF, Airspy, Airspy HF+, AD936x
boards, SDRplay RSP, and Dragon Labs CR-8. These drivers do not require SoapySDR modules. Custom
builds can omit them through feature flags.

| Receiver | Extra software |
|---|---|
| RTL-SDR | none |
| KrakenSDR and KerberosSDR | none |
| HackRF | none |
| Airspy R2 and Airspy Mini | none, see [Airspy](#airspy) |
| Airspy HF+ and HF+ Discovery | none, see [Airspy](#airspy) |
| AntSDR E200 and E310, ADALM-Pluto, and other AD936x boards serving libiio | none, see [AntSDR, PlutoSDR and other AD936x boards](#antsdr-plutosdr-and-other-ad936x-boards) |
| SDRplay RSP1, RSP1A, RSP1B, RSP2, RSPduo, RSPdx, RSPdx-R2 | SDRplay API 3.15 or newer, see [SDRplay](#sdrplay) |
| Dragon Labs CR-8 | the vendor CR-8 library, see [Dragon Labs CR-8](#dragon-labs-cr-8) |

If a host SoapyRTLSDR, SoapyHackRF, SoapyAirspy, SoapyAirspyHF, SoapyPlutoSDR or SoapySDRPlay3
module is installed, it is skipped for these receivers so that one radio is never listed twice.

## Airspy

Both Airspy drivers speak to the radio over their own USB stack, so neither needs libairspy,
libairspyhf, or a SoapySDR module.

**These two drivers have not yet been confirmed on air.** Their USB encodings and signal
processing are covered by unit tests, and the control protocol was written from the vendor
firmware interface, but no Airspy has been connected to this build. Treat them as experimental
and report what you find. Installing SoapySDR with `soapysdr-module-airspy` or
`soapysdr-module-airspyhf` is the fallback: build without the `airspy` and `airspyhf` features,
and the SoapySDR modules become visible again.

### Airspy R2 and Airspy Mini

The R2 and the Mini sample one real signal centred a quarter of the way up their own ADC rate,
and the complex baseband is formed here rather than in the radio. The rate shown in the interface
is the complex rate you receive; twice that many samples cross the USB bus.

LNA, mixer and VGA appear as separate gain stages numbered in steps rather than decibels, because
that is what the firmware takes and the decibels each step buys change with the band. LNA and
mixer AGC, and the bias tee, are switches on the Device node.

### Airspy HF+ and HF+ Discovery

The HF+ covers up to 31 MHz and 60 to 260 MHz. Nothing tunes between those two windows, so a
frequency in the gap is refused rather than tuned badly.

The preamp appears as a switch and the attenuator as negative gain, from 0 to −48 dB in 6 dB
steps, since taking signal away is what it does. AGC, its threshold, and the bias tee are
switches.

At the rates where the radio works at zero IF, the wanted signal sits on the converter's own DC
term. The engine parks the local oscillator clear of each channel and removes that term, which is
the same treatment every other zero-IF receiver here gets. The vendor library additionally runs an
adaptive IQ balancer that this driver does not, so image rejection may be poorer than libairspyhf
achieves on those rates.

## SoapySDR modules

sdr-- does not ship SoapySDR. No package links it and no installer carries it: the core library
is opened at runtime from whatever SoapySDR the host has, and a machine without one simply reports
no SoapySDR hardware and keeps every built-in driver working.

Install it the way you install anything else on your system, then add the module for your radio:

| System | SoapySDR core | Example module |
|---|---|---|
| Debian, Ubuntu, Raspberry Pi OS | `sudo apt install libsoapysdr0.8` | `sudo apt install soapysdr-module-bladerf` |
| Fedora | `sudo dnf install SoapySDR` | `sudo dnf install SoapySDR-bladeRF` |
| Arch | `sudo pacman -S soapysdr` | `sudo pacman -S soapybladerf` |
| macOS (Homebrew) | `brew install soapysdr` | `brew install soapybladerf` |
| Windows | [PothosSDR](https://github.com/pothosware/PothosSDR/wiki/Tutorial) installs the core and modules together |
| NixOS | see [Nix](getting-started/install.md#nix) — modules are selected in your configuration |

Receivers that need a module, because no built-in driver covers them:

| Receiver | Module |
|---|---|
| bladeRF | SoapyBladeRF |
| LimeSDR | SoapyLMS7 |
| Remote SoapySDR server | SoapyRemote |
| USRP | SoapyUHD |

Airspy receivers need no module; see [Airspy](#airspy) if you would rather use one anyway.

Any module matching the SoapySDR 0.8 ABI works; a module built against a different SoapySDR
generation is refused and logged rather than loaded. Modules for hardware sdr-- drives itself are
ignored, so installing `soapysdr-module-all` does not produce duplicate entries.

`sdrmm --doctor` reports which SoapySDR was found, where it was loaded from, and which modules it
loaded. Two environment variables override the search when a system puts things somewhere
unusual:

| Variable | Effect |
|---|---|
| `SDRMM_SOAPY_LIBRARY` | Full path to the SoapySDR core to open, instead of searching for one |
| `SDRMM_SOAPY_MODULE_PATH` | Extra module directories, searched before the host's own |

## Network receivers

On an unbound Device node, open the **Network** tab and enter a hostname or address. Ports
default to `1234` for `rtl_tcp`, `5555` for SpyServer, and `30431` for an AD936x board. All three
protocols are built in and work without SoapySDR.

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

It lists compiled backends, whether a SoapySDR runtime was found and where, its module search
paths, discovered modules and devices, data paths, and Linux USB permission checks. The same
report is available from **Hardware not showing up?** on an unbound Device node.

A SoapySDR install brings its own utility, which is worth running when a module is involved:

```sh
SoapySDRUtil --info
SoapySDRUtil --find
SoapySDRUtil --probe="driver=bladerf"
```

If `SoapySDRUtil --info` finds a SoapySDR that `sdrmm --doctor` does not, the two are looking in
different places: set `SDRMM_SOAPY_LIBRARY` to the path `SoapySDRUtil` reports. Fix discovery and
permission errors there before starting sdr--. Receivers handled by built-in drivers do not
appear in `SoapySDRUtil` at all; use `sdrmm --doctor` for those.

## How radios are discovered

SoapySDR discovery runs in a short-lived child process because vendor modules can crash or hang
while probing hardware. A failed probe produces a warning without terminating the application.

Discovery runs when attached USB devices change and once a minute to find network radios.
For driver debugging, `SDRMM_SOAPY_PROBE=in-process` runs discovery in the application process.

## Linux USB permissions

sdr-- does not need root as long as the receiver's udev rules are installed. On Debian-derived
systems the driver package usually installs them. After adding or changing a rule, reload udev or
reconnect the device.

The rules grant a group rather than the whole machine, usually `plugdev`, so being in that group is
part of the installation.

In containers, the host rules still decide whether the unprivileged container user can open the USB
node. Passing `/dev/bus/usb` is necessary but does not override its permissions, and the container
user is in no host group until it is given one: run with `--group-add <gid>`, the group that owns
the node, or `0` for a node no udev rule has touched. See
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

## KrakenSDR

A KrakenSDR is five receive chains on one clock in one case, and sdr-- opens it as one receiver
with five lanes. Its dongles are recognised by the serial numbers KrakenRF gives them and by the
hub they hang off, so they are not also offered individually. A KerberosSDR is opened the same way
with four lanes. Nothing from the vendor's Raspberry Pi image is needed or used.

| Setting | Effect |
|---|---|
| `TUNER` | the tuner gain stage, set per lane |
| `ppm` | crystal frequency correction |
| `bias_tee` | phantom power on all five antenna ports |
| `agc` | R82xx tuner AGC |

The lanes are tuned together, and their gain is set for each lane from the Device node. Direct
sampling is not offered: an array's elements are wired to antennas.

The five tuners share a clock but not a synthesizer, so the receiver reports the `time_sync`
coherence tier and comes up at a new set of relative phases after every retune. The built-in
calibration noise source is not a setting: it belongs to the calibration that switches it in and
out, which sdr-- runs for you. See [Coherent arrays](user-guide/arrays.md#krakensdr).

If the unit does not appear, check that all five chains enumerate — `lsusb` on Linux, `sdrmm
--doctor` anywhere. A unit missing a chain is reported as loose dongles rather than as a radio,
which is the fault showing rather than a smaller array being invented.

## HackRF

Three gain stages and one switch:

| Setting | Range |
|---|---|
| `LNA` | 8 dB steps |
| `VGA` | 2 dB steps |
| `AMP` | the switched +14 dB RF amplifier, rendered as a switch |
| `bias_tee` | phantom power on the antenna port |

`AMP` appears as a switch and contributes to the displayed total gain.

## AntSDR, PlutoSDR and other AD936x boards

A board running the libiio firmware is opened by a driver built into sdr--, which speaks the iiod
protocol itself over ethernet or over USB. No libiio, no SoapySDR module and no vendor software is
involved. One driver covers every board built around an AD936x transceiver: an AntSDR E200 or E310,
an ADALM-Pluto, and anything else that serves the same devices through iiod.

Everything the device face offers is read from the board rather than assumed, so an AD9361 reports
70 MHz – 6 GHz and an AD9363 reports 325 MHz – 3.8 GHz, and a 2×2 board is opened with two receive
and two transmit lanes.

| Setting | Effect |
|---|---|
| `RX` | receive gain, set per lane |
| `TX` | transmit attenuation, set per lane |
| `ppm` | crystal correction, counted from the value the board was trimmed to at the factory |
| `gain_mode` | `manual`, `slow_attack`, `fast_attack` or `hybrid` AGC |
| `quadrature_tracking`, `rf_dc_tracking`, `bb_dc_tracking` | the transceiver's own corrections |
| `fir_filter` | the programmable decimating filter |
| `tx_port` | which transmit port is driven |

The antenna control selects the receive port, `A_BALANCED` on a board with one input.

**Finding one.** A board attached over USB appears on its own. A board on the network is found at
the addresses these radios ship on — `ant.local`, `192.168.1.10`, `pluto.local` and `192.168.2.1`
— when you press **Search**; anywhere else, type its address into the **Network** tab.

**Tuning.** Receive and transmit share the dial: setting a centre frequency moves both
synthesizers, so a transmission lands where the receiver is tuned.

**Two lanes.** Both receivers on one AD936x run from the same synthesizer and the same converter
clock, so sdr-- treats a 2×2 board as phase coherent and a bearing taken across its lanes means
something. Their gain and input port are set per lane; the dial is shared. A board wired 1×1, which
is what a stock ADALM-Pluto is, opens with one lane in each direction.

**Sample rate.** The board resamples continuously between roughly 2.084 MS/s and 61.44 MS/s. What
actually arrives is limited by the link: USB 2.0 carries a few megasamples per second, and the
E200's gigabit ethernet carries considerably more.

**Over USB.** The board's IIO interface is claimed directly. On Linux that needs the libiio udev
rules, the same ones the vendor packages install; `sdrmm --doctor` reports whether they are there.
A board that offers only two endpoint couples is opened half duplex, because receiving and
transmitting at once needs a conversation for each alongside the control one.

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
2. For a SoapySDR receiver, probe the device with `SoapySDRUtil`.
3. Stream for at least 30 minutes and watch the Device overrun counter.
4. Unplug and reconnect once, and confirm the workspace binds to the same radio again.
5. Exercise tuning, gain, AGC, bandwidth, antenna, and the advertised advanced settings.
6. Make a short recording and replay it.

sdr-- reports the TX capabilities of transmit-capable radios, but the transmit workflow is not
available yet. Validate driver TX behaviour only in a shielded, attenuated, legally authorized
bench setup.
