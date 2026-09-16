# Radios and hardware

Select a radio on a **Device** node. SDR-- supports built-in drivers, SoapySDR modules, network
receivers, and virtual sources. Use **Check hardware** or `sdrmm --doctor` if a receiver is missing.

## Built-in drivers

Standard desktop and portable builds include the drivers below, except where noted.
The Nix package uses SoapySDR for local hardware. Custom builds can select their own backends.

| Receiver | Extra software |
|---|---|
| RTL-SDR | None |
| KrakenSDR and KerberosSDR | None |
| HackRF | None |
| Airspy R2 and Mini | None; [experimental driver](#airspy) |
| Airspy HF+ and HF+ Discovery | None; [experimental driver](#airspy) |
| AntSDR, ADALM-Pluto, and compatible AD936x boards | None; the board must serve [iiod](#antsdr-plutosdr-and-other-ad936x-boards) |
| SDRplay RSP1, RSP1A, RSP1B, RSP2, RSPduo, RSPdx, RSPdx-R2 | [SDRplay API](#sdrplay) 3.15 or newer, or [SDRconnect](#over-the-network-with-sdrconnect) on another machine |
| Dragon Labs CR-8 | [Vendor CR-8 library](#dragon-labs-cr-8); requires a server build with `cr8` enabled |

SoapySDR modules for receivers handled by enabled built-in drivers are skipped to avoid duplicate
entries. Virtual sources and direct network protocols do not need SoapySDR.

## Check the installation

```sh
sdrmm --doctor
```

The report lists compiled backends, loaded libraries, SoapySDR paths and modules, discovered
receivers, data paths, and Linux USB permissions. **Check hardware** on an unbound Device node
runs the same checks.

For a radio using SoapySDR, also run:

```sh
SoapySDRUtil --info
SoapySDRUtil --find
SoapySDRUtil --probe="driver=bladerf"
```

Replace `bladerf` with your module's driver name. If the utility finds a library that SDR-- misses,
set `SDRMM_SOAPY_LIBRARY` to its full path. Use `sdrmm --doctor` to check built-in drivers;
`SoapySDRUtil` reports only its own modules and devices.

## Linux USB permissions

Install your receiver's udev rules and join the group they grant, usually `plugdev`. Reload udev
and reconnect the radio after changing rules. The server account needs permission to open the
USB device; SDR-- does not require root.

Containers need the USB bus passed through and the owning group's numeric ID in `group_add`.
Check it on the host with `stat -c '%g %G %a' /dev/bus/usb/*/*`. See
[container USB setup](server/deployment.md#usb-devices) for reconnect support and examples.

## SoapySDR modules

Install SoapySDR and a matching module for hardware without a built-in driver.

| Receiver | Module |
|---|---|
| bladeRF | SoapyBladeRF |
| LimeSDR | SoapyLMS7 |
| USRP | SoapyUHD |
| Remote SoapySDR server | SoapyRemote |

### Package contents

| Package | SoapySDR availability |
|---|---|
| Desktop installer or portable archive | Uses a separately installed system library and modules |
| Homebrew server formula | Installs the core as a dependency; add modules separately |
| Nix | Provides the core; select modules with `soapyPlugins` |
| Container | Includes Debian's core and bladeRF, LimeSDR, and SoapyRemote modules |

The core loads at runtime. Built-in drivers work when SoapySDR is absent.

### Install the core and modules

| System | Core | Example module |
|---|---|---|
| Debian, Ubuntu, Raspberry Pi OS | `sudo apt install libsoapysdr0.8` | `sudo apt install soapysdr-module-bladerf` |
| Fedora | `sudo dnf install SoapySDR` | `sudo dnf install SoapySDR-bladeRF` |
| Arch | `sudo pacman -S soapysdr` | `sudo pacman -S soapybladerf` |
| macOS with Homebrew | `brew install soapysdr` | `brew install soapybladerf` |
| Windows | [PothosSDR](https://github.com/pothosware/PothosSDR/wiki/Tutorial) | Included modules |
| NixOS | [Nix configuration](getting-started/install.md#nix) | `soapyPlugins` |

Modules must match the SoapySDR 0.8 ABI. Incompatible modules are rejected and logged.
Use these overrides for nonstandard locations:

| Variable | Value |
|---|---|
| `SDRMM_SOAPY_LIBRARY` | Full path to the core library |
| `SDRMM_SOAPY_MODULE_PATH` | Extra module directories, searched before system paths |

On macOS, discovery includes Homebrew prefixes. On Windows, put the PothosSDR installation on
`PATH`. To add container modules, see [SoapySDR in containers](server/deployment.md#soapysdr-modules).

## Network receivers

Open the **Network** tab on an unbound Device node and enter the receiver's address.

| Protocol | Default port |
|---|---:|
| `rtl_tcp` | 1234 |
| SpyServer | 5555 |
| SDRconnect | 5454 |
| AD936x / iiod | 30431 |

All four protocols are built in. A remote `SoapySDRServer` instead requires SoapyRemote and
appears through the normal device search.

## Virtual sources

Release builds support [SigMF recording playback](user-guide/recording.md#play-a-recording).
Synthetic sources are available only in debug builds: a signal generator, a four-lane coherent
array, and test transceivers. See [Build and test](development/building.md#development-signal-sources).

## Device controls

Controls reflect the selected driver's capabilities: RX/TX lanes, tuning, sample rate, bandwidth,
antennas, gain, AGC, clock sources, and model-specific settings. Changing a setting can change
other available controls. For example, RTL-SDR direct sampling changes the tuning range.

The interface reports transmit capabilities, but the transmit workflow is not yet available.

## RTL-SDR

| Setting | Effect |
|---|---|
| `TUNER` | Tuner gain |
| `ppm` | Crystal frequency correction |
| `bias_tee` | Antenna-port power |
| `agc` | R82xx tuner AGC |
| `direct_sampling` | `off`, `i`, or `q` |

- **Gain:** uses the tuner's supported steps. An R820T request for 20 dB rounds to 19.7 dB.
- **Sample rate:** 225–300 kHz or 900 kHz–3.2 MHz. Rates in the gap are rejected.
- **IF filter:** 0–8 MHz on R82xx tuners; `0` selects bandwidth automatically.
- **Direct sampling:** unavailable on RTL-SDR Blog V4. Its upconverter handles tuning below 28.8 MHz.

## KrakenSDR

KrakenSDR opens as one Device with five lanes; KerberosSDR has four. Discovery groups the tuners
by serial number and USB hub. The vendor Raspberry Pi image is not required.

| Setting | Effect |
|---|---|
| `TUNER` | Gain per lane |
| `ppm` | Crystal correction |
| `bias_tee` | Power on the array's antenna ports |
| `agc` | R82xx tuner AGC |

All lanes tune together. Direct sampling is unavailable. The shared clock provides `time_sync`
coherence; relative phase must be recalibrated after each retune. SDR-- controls the built-in
noise source during [array calibration](user-guide/arrays.md#krakensdr).

If the array is missing, check that every tuner appears in `sdrmm --doctor` or Linux `lsusb`.
Incomplete units appear as individual dongles.

## HackRF

| Setting | Effect |
|---|---|
| `LNA` | Gain in 8 dB steps |
| `VGA` | Gain in 2 dB steps |
| `AMP` | Switchable +14 dB RF amplifier; included in total gain |
| `bias_tee` | Antenna-port power |

## Airspy

The built-in Airspy drivers need no libairspy, libairspyhf, or SoapySDR module. Both are
**experimental**: USB and signal-processing tests pass, but live reception has not been verified.

To use SoapySDR instead, build without the `airspy` and `airspyhf` features and install the
corresponding SoapySDR modules.

### Airspy R2 and Airspy Mini

The displayed sample rate is complex IQ output. The USB stream carries real ADC samples at twice
that rate; SDR-- converts them to IQ.

LNA, mixer, and VGA gain use firmware step numbers rather than dB. The Device also exposes
LNA AGC, mixer AGC, and bias-tee switches.

### Airspy HF+ and HF+ Discovery

Tuning covers up to 31 MHz and 60–260 MHz. Frequencies in the gap are rejected.

Controls include a preamp switch, attenuation from 0 to −48 dB in 6 dB steps, AGC, AGC threshold,
and bias tee.

At zero-IF rates, the engine offsets the local oscillator and removes DC. The driver does not
implement the vendor library's adaptive IQ balancing, so image rejection may be lower at these rates.

## AntSDR, PlutoSDR and other AD936x boards

The built-in driver connects directly to iiod over Ethernet or USB. It supports AntSDR E200/E310,
ADALM-Pluto, and compatible AD936x boards without a host libiio or SoapySDR installation.

Capabilities come from the board. An AD9361 typically reports 70 MHz–6 GHz; an AD9363 reports
325 MHz–3.8 GHz. A 2×2 board exposes two RX and two TX lanes; a stock Pluto exposes one of each.

| Setting | Effect |
|---|---|
| `RX` | Receive gain per lane |
| `TX` | Transmit attenuation per lane |
| `ppm` | Crystal correction relative to factory trim |
| `gain_mode` | `manual`, `slow_attack`, `fast_attack`, or `hybrid` |
| `quadrature_tracking`, `rf_dc_tracking`, `bb_dc_tracking` | Hardware corrections |
| `fir_filter` | Programmable decimating filter |
| `tx_port` | Transmit port |
| Antenna | Receive port; usually `A_BALANCED` on a single-input board |

**Discovery:** USB boards appear automatically. **Search** checks `ant.local`, `192.168.1.10`,
`pluto.local`, and `192.168.2.1`. Enter other addresses in the **Network** tab.

**Tuning and lanes:** the dial tunes RX and TX together. Two RX lanes share a synthesizer and
sample clock and report phase coherence. Gain and input port are set per lane.

**Sample rate:** roughly 2.084–61.44 MS/s, limited in practice by the connection. USB 2.0 carries
a few MS/s; gigabit Ethernet allows higher rates.

**USB:** Linux requires the libiio udev rules. `sdrmm --doctor` checks them. Boards exposing only
two endpoint pairs operate half duplex; simultaneous RX and TX requires another pair.

## SDRplay

Install [SDRplay API](https://www.sdrplay.com/downloads/) 3.15 or newer and keep
`sdrplay_apiService` running. The built-in driver loads the vendor library at runtime, usually
from `/usr/local/lib` or `C:\Program Files\SDRplay\API`. No SoapySDR module is needed.

The API is installed separately. If an RSP is missing, check **SDRplay API** in `sdrmm --doctor`.
Container setup requires [the library and host IPC](server/deployment.md#sdrplay-receivers).

### Gain

Both sliders show gain, so increasing either raises the signal level.

| Stage | Control |
|---|---|
| RF | LNA gain relative to the band's weakest state; steps depend on frequency, port, and HDR mode |
| IF | 0–39 dB, corresponding to the inverse of the API's 20–59 dB gain reduction |

AGC controls IF gain. With AGC enabled, the IF slider sets the starting gain and the setpoint
sets the target level in dBFS.

### Sample rates

Single-tuner modes provide 62.5 kS/s–10.66 MS/s. Rates below 2 MS/s use hardware decimation.

### RSPduo

Available operating modes appear as separate choices: Tuner 1, Tuner 2, Dual Tuner, Master, and
Slave. The workspace saves the chosen mode. Modes held by another application are unavailable.

Dual Tuner exposes two independently tuned streams. Dual Tuner, Master, and Slave use a 6 MHz
ADC rate and 1.62 MHz IF. Output rates are 2 MS/s and successive halvings down to 62.5 kS/s;
analog bandwidth is capped at 1.536 MHz.

Slave mode waits for a master application. The master owns the clock; a slave can change its
own decimation but cannot apply ppm correction.

### Over the network with SDRconnect

An RSP attached to another machine is reachable through [SDRconnect](https://www.sdrplay.com/sdrconnect/)
without the SDRplay API on this one. Enable the WebSocket API in SDRconnect, or start
`SDRconnect_headless --websocket_port=5454`, then enter `host:5454` on the Device node's
**Network** tab.

SDR-- tunes the receiver, sets its sample rate and antenna, and takes the 16-bit IQ stream.
Demodulation happens here, so the SDR-- channels are the ones that decode. A tuner of an RSPduo
is addressed by appending it to the address: `host:5454/secondary`, with `host:5454` meaning the
primary tuner.

The centre frequency, sample rate and antenna are the usual Device controls. What the API adds
beyond them:

| Setting | Effect |
|---|---|
| `lna_state` | RF gain state, between the receiver's own minimum and maximum |
| `device_vfo_frequency` | Where SDRconnect's VFO sits inside the sampled window |
| `filter_bandwidth` | Its channel filter, no wider than the receiver's `demod_max_bandwidth` |
| `receiver` | Which radio on the host: a name from the list, a slot in it, or a serial number |
| `network_mode` | Stream quality for a receiver SDRconnect itself reaches over the network |
| `device_profile` | Applies a device profile saved in SDRconnect |
| `recording` | Starts an IQ, audio, or compressed-audio recording on the SDRconnect host |

Everything after the detector stays on the SDRconnect side: its demodulator mode, squelch, audio
AGC, de-emphasis, noise reduction, RDS decoder and audio chain produce sound that SDR-- never
reads, so changing them here would alter someone's session without changing a sample. This side
demodulates in a channel. For the same reason the demodulated-audio and spectrum streams stay
switched off; if one arrives anyway it is reported rather than quietly carried.

What the receiver reports back and cannot be set — signal power and SNR, RDS text, the stereo
lock, ADC overload — is written to the log, and an overload is a warning.

A session someone was already running is left running when SDR-- stops; one SDR-- started is
stopped again.

### Licensing

The Rust interface follows the public [SDRplay API specification](https://www.sdrplay.com/api/).
The specification grants use of its information for software supporting SDRplay receivers.
No vendor source, headers, or binaries are included. Gain tables come from that specification.

## Dragon Labs CR-8

The CR-8 has eight `phase_coherent` lanes sharing a clock and synthesizer. Use one Device node
with outputs `iq` through `iq8` for calibration, direction finding, beamforming, or passive radar.

Install the vendor library separately and run `sdrmm --doctor` to verify loading. Set
`SDRMM_DLCR_LIBRARY` to its full path if it is outside the normal search locations.
Use a server build with the `cr8` feature enabled. Standard packaged builds exclude this backend.

| Setting | Behaviour |
|---|---|
| Frequency | Tunes all eight lanes together |
| Sample rate | Fixed at 12.5 MS/s |
| Gain | LNA, mixer, and VGA per lane |
| Clock | Onboard oscillator or external 10 MHz reference |

The tuning range follows the hardware documentation because the SDK does not report it.

## How radios are discovered

Discovery runs when USB devices change and once per minute for network radios. SoapySDR probing
uses a child process so a crashing or stalled vendor module does not terminate SDR--.
For debugging, `SDRMM_SOAPY_PROBE=in-process` disables that isolation.

## Before an unattended deployment

Test the packaged build with your radio:

1. Save the `sdrmm --doctor` report.
2. Stream for at least 30 minutes and check overruns.
3. Test tuning, gain, sample rate, and the controls you intend to use.
4. Reconnect the radio and confirm the workspace restores it.
5. Record a short capture and replay it.
