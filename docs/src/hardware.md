# Radios

A **Device** node opens one radio: over USB, through SoapySDR, or over the network. Recordings
and test signals have nodes of their own, see [Other sources](#other-sources).

Radio missing? Press **Check hardware** on an empty Device node, or run `sdrmm --doctor`.

## Supported radios

The desktop and portable builds include these drivers:

| Radio | Needs |
|---|---|
| RTL-SDR | Nothing |
| KrakenSDR, KerberosSDR | Nothing |
| HackRF | Nothing |
| Airspy R2, Mini, HF+, HF+ Discovery | Nothing; [experimental](#airspy) |
| AntSDR, ADALM-Pluto, other AD936x boards | The board serving [iiod](#antsdr-plutosdr-and-other-ad936x-boards) |
| SDRplay RSP1, RSP1A, RSP1B, RSP2, RSPduo, RSPdx, RSPdx-R2 | [SDRplay API](#sdrplay) 3.15+, or [SDRconnect](#sdrconnect) on another machine |
| Dragon Labs CR-8 | [Vendor library](#dragon-labs-cr-8) and a build with `cr8` |
| bladeRF, LimeSDR, USRP, others | A [SoapySDR module](#soapysdr) |

The Nix package uses SoapySDR for all local radios.

## Check the installation

```sh
sdrmm --doctor
```

It lists compiled drivers, loaded libraries, SoapySDR modules, found radios, data paths, and
Linux USB permissions. **Check hardware** runs the same checks from the interface.

## Linux USB permissions

Install your radio's udev rules and add the server's user to the group they name, usually
`plugdev`. Reload udev and replug the radio. SDR-- never needs root.

For containers, see [USB devices](server/deployment.md#usb-devices).

## SoapySDR

SoapySDR covers radios without a built-in driver. Install the core and a module for your radio:

| Radio | Module |
|---|---|
| bladeRF | SoapyBladeRF |
| LimeSDR | SoapyLMS7 |
| USRP | SoapyUHD |
| Remote SoapySDR server | SoapyRemote |

| System | Core | Example module |
|---|---|---|
| Debian, Ubuntu, Raspberry Pi OS | `sudo apt install libsoapysdr0.8` | `soapysdr-module-bladerf` |
| Fedora | `sudo dnf install SoapySDR` | `SoapySDR-bladeRF` |
| Arch | `sudo pacman -S soapysdr` | `soapybladerf` |
| macOS | `brew install soapysdr` | `soapybladerf` |
| Windows | [PothosSDR](https://github.com/pothosware/PothosSDR/wiki/Tutorial), on `PATH` | Included |
| NixOS | [`soapyPlugins`](getting-started/install.md#nix) | |

Desktop and portable builds load SoapySDR at runtime and work without it. The Homebrew formula
installs the core. The container ships the core with bladeRF, LimeSDR, and SoapyRemote modules.

Modules must match SoapySDR 0.8. Others are rejected and logged. For unusual install locations:

| Variable | Value |
|---|---|
| `SDRMM_SOAPY_LIBRARY` | Full path to the core library |
| `SDRMM_SOAPY_MODULE_PATH` | Extra module folders, searched first |

`SoapySDRUtil --find` shows what SoapySDR itself sees. It knows nothing about the built-in
drivers, which SDR-- prefers when both could open a radio.

## Network radios

On an empty Device node, open the **Network** tab and enter `host:port`:

| Protocol | Default port |
|---|---:|
| `rtl_tcp` | 1234 |
| SpyServer | 5555 |
| SDRconnect | 5454 |
| AD936x / iiod | 30431 |

The address becomes the radio's identity in the workspace. A remote `SoapySDRServer` shows up in
the normal radio list instead, through SoapyRemote. Network IQ uses a lot of bandwidth: pick the
lowest rate that works and watch the drop counter.

## Other sources

| Node | Gives |
|---|---|
| Recording | Plays a [SigMF recording](user-guide/recording.md#play-a-recording) |
| Signal generator | Test signals in 44 modes, from a plain tone to DVB-T |

Debug builds also list synthetic radios: a four-lane coherent array and test transceivers.

## Device controls

Controls mean the same thing on every radio:

| Control | Sets |
|---|---|
| Rate | Sample rate |
| Filter | Analog bandwidth before sampling, or Auto |
| Antenna | Input port, when there is a choice |
| AGC | **Auto** on the gain row. The radio sets its own gain; the slider shows what it chose, where the radio reports it. |
| LNA, Mixer, VGA, IF, RF, Tuner, Attenuator | One gain stage each, in dB or firmware steps |
| Amp | A switchable preamp |
| Bias tee | Power on the antenna port for an active antenna or LNA |
| PPM | Crystal correction |
| Converter | Local oscillator of an up- or downconverter, in MHz |
| DC block | Removes the radio's own DC spike |

With a converter set, every frequency shown is the one at the antenna. Enter a positive value for
a downconverter, like 9750 for a Ku-band LNB, and a negative one for an upconverter, like −125 for
a Ham It Up.

Settings only one radio has appear below these rows. Some change the others: RTL-SDR direct
sampling changes the tuning range. Transmit is not available yet.

## RTL-SDR

| Control | Does |
|---|---|
| Tuner | Gain, in the tuner's own steps: 20 dB on an R820T becomes 19.7 dB |
| AGC | Tuner AGC |
| Bias tee | Antenna-port power |
| Direct sampling | `off`, `i`, or `q`. Not on the RTL-SDR Blog V4 or V4 Lite, which upconvert HF. |

Rates: 225 to 300 kHz, or 900 kHz to 3.2 MHz. Filter: 290 kHz to 8 MHz on R82xx tuners.

## KrakenSDR

One Device with five lanes; KerberosSDR has four. SDR-- groups the tuners by serial and USB hub,
so the vendor Pi image is not needed. Each lane has its own dial, gain, and AGC. Lanes wired to a
coherent node tune together. There is no direct sampling. SDR-- runs the noise source during [calibration](user-guide/arrays.md#krakensdr).

If the array shows up as separate dongles, one of its tuners is missing: check `sdrmm --doctor`
or `lsusb`.

## HackRF

| Control | Does |
|---|---|
| LNA | Gain in 8 dB steps |
| VGA | Gain in 2 dB steps |
| Amp | +14 dB RF amplifier |
| Filter | Baseband filter, or Auto |
| Bias tee | Antenna-port power |

## Airspy

Built in, no vendor library needed. Both drivers are **experimental**: tests pass, but live
reception is not yet verified. To use SoapySDR instead, build without `airspy` and `airspyhf`.

**R2 and Mini:** LNA, Mixer, and VGA gain use firmware steps, not dB. AGC can run the LNA, the
mixer, or both. Bias tee available.

**HF+ and HF+ Discovery:** tunes up to 31 MHz and 60 to 260 MHz. Controls are Amp, attenuation in
6 dB steps down to −48 dB, AGC with a low or high threshold, and bias tee. The vendor's adaptive
IQ balance is not implemented, so image rejection can be weaker at zero-IF rates.

## AntSDR, PlutoSDR and other AD936x boards

Talks to iiod directly over Ethernet or USB, with no libiio or SoapySDR. USB boards appear on their
own. **Search** also tries `ant.local`, `192.168.1.10`, `pluto.local`, and `192.168.2.1`. Enter
other addresses in the **Network** tab.

The board reports its range: typically 70 MHz to 6 GHz on an AD9361, 325 MHz to 3.8 GHz on an
AD9363. Rates run from about 2.1 to 61.44 MS/s, limited by the link: USB 2.0 carries a few MS/s,
gigabit Ethernet much more. On a 2×2 board both RX lanes share a clock and are phase coherent.

| Control | Does |
|---|---|
| Tuner | Receive gain per lane |
| TX | Transmit attenuation per lane |
| AGC | Off, slow attack, fast attack, or hybrid |
| Quadrature, RF DC, baseband DC tracking | Hardware corrections |
| FIR filter | Programmable decimating filter |
| Antenna, TX port | Receive and transmit ports |

Linux needs the libiio udev rules. `sdrmm --doctor` checks for them.

## SDRplay

Install the [SDRplay API](https://www.sdrplay.com/downloads/) 3.15 or newer and keep
`sdrplay_apiService` running. No SoapySDR module needed. If an RSP is missing, see the
**SDRplay API** section of `sdrmm --doctor`. For containers, see
[SDRplay receivers](server/deployment.md#sdrplay-receivers).

Both gain sliders raise gain when moved up:

| Slider | Sets |
|---|---|
| RF | LNA gain. The steps depend on frequency, port, and HDR mode. |
| IF | 0 to 39 dB |

AGC runs the IF gain at 5, 50, or 100 Hz. With AGC on, the IF slider sets the starting gain.

Rates run from 62.5 kS/s to 10.66 MS/s on one tuner.

**RSPduo:** each mode is its own entry: Tuner 1, Tuner 2, Dual Tuner, Master, and Slave. Modes in
use by another program are hidden. Dual Tuner gives two independent streams at up to 2 MS/s each.
Slave waits for a master program, which owns the clock.

### SDRconnect

Reach an RSP on another machine through [SDRconnect](https://www.sdrplay.com/sdrconnect/), with no
local SDRplay API. Enable its WebSocket API, or run `SDRconnect_headless --websocket_port=5454`.
On a Device node pick **Network → SDRconnect** and enter `host:5454`, or `host:5454/secondary`
for an RSPduo's second tuner.

The link is unencrypted `ws://`. Use it on a trusted network or through a tunnel.

SDR-- receives raw IQ and does its own demodulation. Extra settings:

| Setting | Does |
|---|---|
| `lna` | RF gain over the LNA states; lower means more gain. There is no IF gain. |
| `device_vfo_frequency` | SDRconnect's VFO inside the sampled window |
| `filter_bandwidth` | SDRconnect's channel filter |
| `receiver` | Which radio: name, slot, or serial |
| `network_mode` | Stream quality |
| `device_profile` | Load a saved SDRconnect profile |
| `recording` | Record on the SDRconnect machine |

The driver follows the public [SDRplay API specification](https://www.sdrplay.com/api/). No vendor
code is included.

## Dragon Labs CR-8

Eight `phase_coherent` lanes on one Device, `iq1` to `iq8`, for calibration, direction finding,
beamforming, and passive radar. All lanes tune together at a fixed 12.5 MS/s, with LNA, mixer,
and VGA gain per lane. The clock is internal or an external 10 MHz reference.

The packaged builds leave CR-8 out. Build the server with `cr8`, install the vendor library, and
check it with `sdrmm --doctor`. Set `SDRMM_DLCR_LIBRARY` if the library is somewhere unusual.

## How radios are found

SDR-- looks for radios when USB devices change, and for network radios once a minute. SoapySDR
probing runs in a child process, so a crashing module cannot take SDR-- down. Set
`SDRMM_SOAPY_PROBE=in-process` to turn that off while debugging.
