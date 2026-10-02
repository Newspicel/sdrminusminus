# Radios

A **Device** node opens one radio: over USB, over the network, or through SoapySDR. Recordings and
generated signals have nodes of their own, see [Other sources](#other-sources).

## Supported radios

Packaged builds include every driver here except CR-8:

| Radio | Connects over | Needs |
|---|---|---|
| [RTL-SDR](#rtl-sdr) | USB | Nothing |
| [KrakenSDR, KerberosSDR](#krakensdr) | USB | Nothing |
| [HackRF](#hackrf) | USB | Nothing |
| [Airspy R2, Mini, HF+, HF+ Discovery](#airspy) | USB | Nothing |
| [ESP32 with ESP-SDR](#esp-sdr) | USB serial | [ESP-SDR firmware](https://github.com/ESPARGOS/esp-sdr) |
| [AntSDR](#antsdr) | Ethernet or USB | Nothing |
| [ADALM-Pluto, other AD936x boards](#plutosdr-and-other-ad936x-boards) | USB or Ethernet | Nothing |
| [SDRplay RSP1, RSP1A, RSP1B, RSP2, RSPduo, RSPdx, RSPdx-R2](#sdrplay) | USB | SDRplay API 3.15+ |
| [SDRplay on another machine](#sdrconnect) | Network | SDRconnect there |
| [KiwiSDR](#kiwisdr) | Network | Nothing |
| [Dragon Labs CR-8](#dragon-labs-cr-8) | USB | Vendor library, a build with `cr8` |
| bladeRF, LimeSDR, USRP, others | USB | A [SoapySDR module](#soapysdr) |

Making a radio? Write to [hi@jhaag.me](mailto:hi@jhaag.me) to get it supported and tested.

## Connect a radio

### USB

Plug it in and it appears in the **Radios** tab of every empty Device node.

On Linux, install your radio's udev rules and add the server's user to the group they name, usually
`plugdev`. Reload udev and replug the radio. SDR-- never needs root. For containers, see
[USB devices](server/deployment.md#usb-devices).

### Network radios

On an empty Device node, open the **Network** tab, pick the protocol under **Via**, and enter
`host:port`. Without a port it uses the default:

| Protocol | Default port |
|---|---:|
| `rtl_tcp` | 1234 |
| SpyServer | 5555 |
| SDRconnect | 5454 |
| KiwiSDR | 8073 |
| AntSDR / Pluto | 30431 |

The address becomes the radio's identity in the workspace. The bookmark button next to **Add**
saves it; saved radios are listed above the form.

Network IQ uses a lot of bandwidth. When the link cannot keep up, the node shows **Lost** with the
share of samples missing: lower the rate.

### Radio missing

Press **Check hardware** on an empty Device node, or run:

```sh
sdrmm --doctor
```

Both list compiled drivers, loaded libraries, SoapySDR modules, found radios, data paths, and
Linux USB permissions. See also [Troubleshooting](troubleshooting.md#a-radio-is-missing).

## Device controls

Controls mean the same thing on every radio:

| Control | Sets |
|---|---|
| Rate | Sample rate |
| BW or Filter | Analog bandwidth before sampling. **BW** has an auto setting. |
| Antenna | Input port, when there is a choice |
| Auto | AGC, on the gain row. The slider shows the gain the radio chose, where it reports it. |
| AGC | AGC mode, on radios that have several |
| LNA, Mixer, VGA, IF, RF, Tuner, Attenuator | One gain stage each, in dB or firmware steps |
| Amp | A switchable preamp |
| Bias tee | Power on the antenna port for an active antenna or LNA |
| PPM | Crystal correction |
| Conv | Local oscillator of an up- or downconverter, in MHz |
| DC block | Removes the radio's own DC spike |

On a radio with several lanes, each lane's gain row is labelled `iq1`, `iq2`, and so on, and the
**All** row sets every lane. On radios that can choose, the −/+ buttons on that row set how many
lanes stream. The footer shows **Queue** delay, **Drops**, **Lost** samples, and **Clipping**.

With **Conv** set, every frequency shown is the one at the antenna. Enter a positive value for a
downconverter, like 9750 for a Ku-band LNB, and a negative one for an upconverter, like -125 for a
Ham It Up.

Settings only one radio has appear as extra chips. Some change the others: RTL-SDR direct
sampling changes the tuning range. There is no transmit.

### Calibration

PPM and the converter offset belong to the radio, not the node: set them once and every Device node
that opens that radio uses them. A USB radio is known by its serial, a network radio by its
address. RTL-SDRs need [serials of their own](#serials).

## RTL-SDR

The built-in driver needs an R820T or R828D tuner; E4000, FC0012, FC0013, and FC2580 dongles are
refused. On Windows, install the WinUSB driver with Zadig.

| Control | Does |
|---|---|
| Tuner | Gain, in the tuner's own steps: 20 dB on an R820T becomes 19.7 dB |
| Auto | Tuner AGC |
| Bias tee | Antenna-port power |
| Direct sampling | `off`, `i`, or `q`. Not on the RTL-SDR Blog V4 or V4 Lite, which upconvert HF. |

Rates: 225 to 300 kHz, or 900 kHz to 3.2 MHz. BW: 290 kHz to 8 MHz.

### Serials

Many dongles ship with the serial `00000001`. Two dongles with one serial are told apart by USB
port instead, shown with `(bus/address)` after the name, and their settings and
[calibration](#calibration) can follow the wrong one after a replug.

SDR-- asks once when it finds such a dongle. **Yes** writes a random serial, or one you type, to
its EEPROM. Replug it afterwards. The dongle must be closed, and one without an EEPROM cannot keep a
serial.

## KrakenSDR

One Device with five lanes; KerberosSDR has four. SDR-- groups the tuners by serial and USB hub,
so the vendor Pi image is not needed. **Make array** wires all lanes to an [Array](user-guide/arrays.md).
There is no direct sampling, and the rate tops out at 2.56 MS/s.

The lanes share a clock, not a phase. The Array switches on the built-in noise source to solve
delay, phase, and gain after every retune and gain change, and checks them every minute. The
antennas are cut off while the source is on; processors pause.

- **Gain:** lane phase moves by up to about 10° across the gain steps. The Array solves at the gain
  you run.
- **Clipping:** the noise source clips at higher gain. Past 75% clipped samples the Array shows
  `Noise clips, lower gain`.
- **Restarts:** a lane error restarts all five lanes in about 80 ms, and the Array searches its
  offsets again from scratch.

If the array shows up as separate dongles, one of its tuners is missing: check `sdrmm --doctor`
or `lsusb`. [Hardware tests](development/hardware-tests.md#krakensdr) has measurements.

Tested on hardware provided by [KrakenRF](https://www.krakenrf.com). Thank you.

## HackRF

| Control | Does |
|---|---|
| LNA | Gain in 8 dB steps |
| VGA | Gain in 2 dB steps |
| Amp | +14 dB RF amplifier |
| BW | Baseband filter, or auto |
| Bias tee | Antenna-port power |

Rates: 2 to 20 MS/s. Tunes 1 MHz to 6 GHz.

## Airspy

Built in, no vendor library needed. To use SoapySDR instead, build without `airspy` and `airspyhf`.

**R2 and Mini:** tunes 24 MHz to 1.8 GHz. LNA, Mixer, and VGA gain use firmware steps, not dB.
AGC can run the LNA, the mixer, or both. Bias tee available.

**HF+ and HF+ Discovery:** tunes up to 31 MHz and 60 to 260 MHz. Controls are Amp (+6 dB),
Attenuator in 6 dB steps down to -48 dB, AGC with a low or high threshold, and PPM, which starts
from the calibration stored on the radio. A centre below 180 kHz (84 kHz at the narrower rates)
tunes to that floor, and the band still shows it. **DC block** appears only at rates that leave a
spike at the centre.

Tested on hardware provided by [Airspy](https://airspy.com). Thank you.

## ESP-SDR

An ESP32 running [ESP-SDR](https://github.com/ESPARGOS/esp-sdr) firmware receives with its Wi-Fi
radio. Flash it with the [browser installer](https://espargos.net/espsdr/app/flash.html).

It captures bursts, not a stream: 4096 samples by default, then a pause while they cross the serial
link, about 0.1 s at 921.6 kBd. Longer bursts update the Scope less often. The timeline marks each
gap.

| Control | Does |
|---|---|
| Frequency | 100 MHz to 6 GHz in 1 MHz steps; only 2.4 GHz (and 5 GHz on C5) is reliable |
| Rate | 80, 40 or 16 MS/s on ESP32, per chip otherwise |
| Tuner | Gain index, not dB. **Auto** is the hardware AGC. |
| BW | Analog bandwidth, or auto for widest |
| Bits | 8 or 10 bits per sample; 10 is slower over serial |
| Burst | Samples per capture |

SDR-- finds the firmware at 2 MBd, 1 MBd or 921.6 kBd. A CP2102 bridge cannot reach 1 MBd, so build
the firmware with `CONFIG_ESP_SDR_UART_BAUD=921600` for those boards. On Linux, add the server's
user to `dialout`.

## AntSDR

SDR-- talks to the iiod server of the AntSDR's Pluto firmware directly, with no libiio. The E310
has two receive lanes on one synthesizer, so they are phase coherent. UHD firmware is not
supported.

**USB:** connect the USB 2.0 port and the board appears with no network setup. Windows needs the
[PlutoSDR drivers](https://wiki.analog.com/university/tools/pluto/drivers/windows).

**Ethernet, direct cable:** the board sits at `192.168.1.10`. Give the computer's Ethernet port a
fixed address in the same range once, and leave the router empty so the internet stays on Wi-Fi:

| System | Where |
|---|---|
| macOS | System Settings, Network, the Ethernet adapter, Details, TCP/IP. Configure IPv4 Manually, IP `192.168.1.100`, subnet mask `255.255.255.0` |
| Windows | Settings, Network & internet, Ethernet, IP assignment, Edit. Manual, IPv4 on, IP `192.168.1.100`, subnet mask `255.255.255.0` |
| Linux | `nmcli connection add type ethernet ifname <port> con-name antsdr ipv4.method manual ipv4.addresses 192.168.1.100/24` |

**Ethernet, through your router:** if your network already uses `192.168.1.x` and nothing else sits
at `.10`, plug the board into the router and it works from every computer on it. Otherwise give the
board a free address in your range: connect it over USB, open the drive it shows, set
`ipaddr_eth` and `netmask_eth` in `config.txt`, and eject. Over SSH the login is `root` /
`analog`.

Empty Device nodes look for a board at `ant.local`, `192.168.1.10`, `pluto.local`, and
`192.168.2.1`; enter any other address in the **Network** tab. If nothing is found,
`ping 192.168.1.10`: no answer means the cable or the computer's address.

Ethernet carries far more than USB 2.0, and two lanes split the link. The E310 locks its antenna
and TX ports in firmware, so those menus are hidden. The other controls are the
[AD936x ones](#plutosdr-and-other-ad936x-boards).

Tested on hardware provided by [MicroPhase](https://www.microphase.cn/). Thank you.

## PlutoSDR and other AD936x boards

Talks to iiod directly over USB or Ethernet, with no libiio or SoapySDR. USB boards appear
automatically, and network boards at `pluto.local`, `192.168.2.1`, `ant.local`, or
`192.168.1.10`.

The board reports its range: typically 70 MHz to 6 GHz on an AD9361, 325 MHz to 3.8 GHz on an
AD9363. Rates run from about 260 kS/s to 61.44 MS/s; below 2.08 MS/s the FPGA decimates. The link
sets the real limit. On a 2×2 board both RX lanes share a clock and are phase coherent.

| Control | Does |
|---|---|
| −/+ | 1 or 2 lanes on a 2×2 board. Starts at 1, which gets the whole link |
| Tuner | Receive gain per lane. The range follows the band |
| TX | Transmit attenuation per lane |
| AGC | Per lane: fast attack, slow attack, or hybrid |
| Filter | Analog bandwidth |
| Quadrature, RF DC, Baseband DC tracking | Hardware corrections |
| Antenna, TX port | Shown only if the board lets the port change |

Linux needs the libiio udev rules. `sdrmm --doctor` checks for them.

## SDRplay

Install the [SDRplay API](https://www.sdrplay.com/downloads/) 3.15 or newer and keep
`sdrplay_apiService` running. No SoapySDR module needed. If an RSP is missing, see the **SDRplay
API** section of `sdrmm --doctor`. For containers, see
[SDRplay receivers](server/deployment.md#sdrplay-receivers). For NixOS, see
[Nix](getting-started/install.md#nix).

Both gain sliders raise gain when moved up:

| Slider | Sets |
|---|---|
| RF | LNA gain. The steps depend on frequency, port, and HDR mode. |
| IF | 0 to 39 dB |

AGC runs the IF gain at 5, 50, or 100 Hz.

Rates run from 62.5 kS/s to 10.66 MS/s on one tuner.

**RSPduo:** each mode is its own entry: Tuner 1, Tuner 2, Dual Tuner, Master (on either tuner), and
Slave. Modes in use by another program are hidden. Dual Tuner gives two independent streams at up to 2 MS/s each.
Slave waits for a master program, which owns the clock.

### SDRconnect

Reach an RSP on another machine through [SDRconnect](https://www.sdrplay.com/sdrconnect/), with no
local SDRplay API. Enable its WebSocket API, or run `SDRconnect_headless --websocket_port=5454`. On
a Device node pick **Network → SDRconnect** and enter `host:5454`, or `host:5454/secondary` for an
RSPduo's second tuner.

The link is unencrypted `ws://`. Use it on a trusted network or through a tunnel.

SDR-- receives raw IQ and does its own demodulation. Settings:

| Setting | Does |
|---|---|
| LNA | RF gain over the LNA states; lower means more gain. There is no IF gain. |
| Device VFO | SDRconnect's VFO inside the sampled window |
| Filter | SDRconnect's channel filter |
| Receiver | Which radio: name, slot, or serial |
| Network mode | Stream quality |
| Device profile | Load a saved SDRconnect profile |
| Recording | Record on the SDRconnect machine |

The driver follows the public [SDRplay API specification](https://www.sdrplay.com/api/). No vendor
code is included.

## KiwiSDR

Pick **Network → KiwiSDR** and paste the receiver's address, `http://` or `https://`. Public
receivers are listed at [rx.kiwisdr.com](http://rx.kiwisdr.com/). A private Kiwi, or one whose time
limits a password lifts, takes `password@host:8073`. The password becomes part of the radio's
address in the workspace.

A Kiwi streams 12 or 20 kHz of IQ anywhere in 0 to 30 MHz: enough for SSB, CW, AM and the
narrowband decoders. Wider channels show out of band. Gain is the Kiwi's AGC or a manual RF gain.

Public Kiwis are shared. When one is full, kicks you, or hits its time limit, SDR-- stops and does
not reconnect. A dropped connection is retried.

## Dragon Labs CR-8

Eight phase-coherent lanes on one Device, `iq1` to `iq8`. All lanes tune together, 24 MHz to
1.766 GHz, at a fixed 12.5 MS/s, with LNA, Mixer, and VGA gain per lane in firmware steps.
**Clock source** is internal or external.

The packaged builds leave CR-8 out. Build the server with `cr8`, install the vendor library, and
check it with `sdrmm --doctor`. Set `SDRMM_DLCR_LIBRARY` if the library is somewhere unusual.

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
| Fedora | `sudo dnf install SoapySDR` | `soapy-uhd` |
| Arch | `sudo pacman -S soapysdr` | `soapybladerf` |
| macOS | `brew install soapysdr` | `pothosware/pothos/soapybladerf` |
| Windows | [PothosSDR](https://github.com/pothosware/PothosSDR/wiki/Tutorial), default install folder | Included |
| NixOS | [`soapyPlugins`](getting-started/install.md#nix) | |

Packaged builds load SoapySDR at runtime and work without it. The container ships the core with
bladeRF, LimeSDR, and SoapyRemote modules. A remote `SoapySDRServer` shows up in the normal radio
list, through SoapyRemote.

SDR-- needs SoapySDR 0.8 and modules built for it. For unusual install locations:

| Variable | Value |
|---|---|
| `SDRMM_SOAPY_LIBRARY` | Full path to the core library |
| `SOAPY_SDR_ROOT` | SoapySDR install prefix |
| `SOAPY_SDR_PLUGIN_PATH` | Extra module folders |

`SoapySDRUtil --find` shows what SoapySDR itself sees. It knows nothing about the built-in drivers,
which SDR-- prefers when both could open a radio.

SDR-- probes SoapySDR in a child process, so a crashing module cannot take it down. Set
`SDRMM_SOAPY_PROBE=in-process` to turn that off while debugging.

## Other sources

| Node | Gives |
|---|---|
| Recording | Plays a [SigMF recording](user-guide/recording.md#play-a-recording) |
| Signal generator | Test signals, from a carrier tone to DVB-T and ATV |

Debug builds also list [synthetic radios](development/building.md#test-without-a-radio).
